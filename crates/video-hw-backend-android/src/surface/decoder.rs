//! MediaCodec surface-output decoder and rendered-frame draining.

use super::*;

/// MediaCodec decoder that renders output into a caller-provided Android `Surface`.
pub struct AndroidSurfaceDecoder {
    config: AndroidSurfaceDecoderConfig,
    vm: JavaVM,
    codec: Option<GlobalRef>,
    pts_us: i64,
}

impl AndroidSurfaceDecoder {
    /// Creates, configures, and starts a decoder targeting `output_surface`.
    pub fn new(
        env: &mut JNIEnv<'_>,
        config: AndroidSurfaceDecoderConfig,
        output_surface: &JObject<'_>,
    ) -> Result<Self, BackendError> {
        validate_surface_config(config.width, config.height, config.fps, "decoder")?;
        validate_timeout(config.timeout_us, "decoder")?;
        let vm = env.get_java_vm().map_err(jni_error)?;
        let mime = mime_for_codec(config.codec);
        let mime_string = env.new_string(mime).map_err(jni_error)?;
        let format = env
            .call_static_method(
                "android/media/MediaFormat",
                "createVideoFormat",
                "(Ljava/lang/String;II)Landroid/media/MediaFormat;",
                &[
                    JValue::Object(&JObject::from(mime_string)),
                    JValue::Int(checked_i32(config.width, "width")?),
                    JValue::Int(checked_i32(config.height, "height")?),
                ],
            )
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        set_media_format_i32(env, &format, "frame-rate", checked_i32(config.fps, "fps")?)?;

        let codec_local = match &config.codec_name {
            Some(name) => {
                let name = env.new_string(name).map_err(jni_error)?;
                env.call_static_method(
                    "android/media/MediaCodec",
                    "createByCodecName",
                    "(Ljava/lang/String;)Landroid/media/MediaCodec;",
                    &[JValue::Object(&JObject::from(name))],
                )
                .map_err(jni_error)?
                .l()
                .map_err(jni_error)?
            }
            None => {
                let mime = env.new_string(mime).map_err(jni_error)?;
                env.call_static_method(
                    "android/media/MediaCodec",
                    "createDecoderByType",
                    "(Ljava/lang/String;)Landroid/media/MediaCodec;",
                    &[JValue::Object(&JObject::from(mime))],
                )
                .map_err(jni_error)?
                .l()
                .map_err(jni_error)?
            }
        };

        let codec = match env.new_global_ref(&codec_local) {
            Ok(codec) => codec,
            Err(error) => {
                let _ = call_cleanup_method(env, &codec_local, "release");
                return Err(jni_error(error));
            }
        };
        let setup_result = (|| -> Result<(), BackendError> {
            let null = JObject::null();
            env.call_method(
                codec.as_obj(),
                "configure",
                "(Landroid/media/MediaFormat;Landroid/view/Surface;Landroid/media/MediaCrypto;I)V",
                &[
                    JValue::Object(&format),
                    JValue::Object(output_surface),
                    JValue::Object(&null),
                    JValue::Int(0),
                ],
            )
            .map_err(jni_error)?;
            env.call_method(codec.as_obj(), "start", "()V", &[])
                .map_err(jni_error)?;
            Ok(())
        })();
        if let Err(error) = setup_result {
            let _ = release_media_codec(env, &codec);
            return Err(error);
        }

        Ok(Self {
            config,
            vm,
            codec: Some(codec),
            pts_us: 0,
        })
    }

    /// Queues an access unit using the decoder's generated monotonic timestamp sequence.
    pub fn queue(
        &mut self,
        env: &mut JNIEnv<'_>,
        access_unit: &[u8],
        is_keyframe: bool,
    ) -> Result<bool, BackendError> {
        let queued =
            self.queue_with_presentation_time_us(env, access_unit, is_keyframe, self.pts_us)?;
        if queued {
            self.pts_us = self
                .pts_us
                .saturating_add(1_000_000 / i64::from(self.config.fps.max(1)));
        }
        Ok(queued)
    }

    /// Queues an access unit with an explicit presentation timestamp in microseconds.
    pub fn queue_with_presentation_time_us(
        &mut self,
        env: &mut JNIEnv<'_>,
        access_unit: &[u8],
        is_keyframe: bool,
        presentation_time_us: i64,
    ) -> Result<bool, BackendError> {
        let access_unit_size = checked_i32(access_unit.len(), "access_unit_size")?;
        let codec = self.codec()?;
        let index = env
            .call_method(
                codec,
                "dequeueInputBuffer",
                "(J)I",
                &[JValue::Long(bounded_blocking_timeout_us(
                    self.config.timeout_us,
                ))],
            )
            .map_err(jni_error)?
            .i()
            .map_err(jni_error)?;
        if index < 0 {
            return Ok(false);
        }

        let copy_result = (|| -> Result<(), BackendError> {
            let buffer = env
                .call_method(
                    codec,
                    "getInputBuffer",
                    "(I)Ljava/nio/ByteBuffer;",
                    &[JValue::Int(index)],
                )
                .map_err(jni_error)?
                .l()
                .map_err(jni_error)?;
            if buffer.is_null() {
                return Err(BackendError::Backend(
                    "MediaCodec.getInputBuffer returned null".to_string(),
                ));
            }
            let buffer = JByteBuffer::from(buffer);
            let buffer_ptr = env.get_direct_buffer_address(&buffer).map_err(jni_error)?;
            let buffer_size = env.get_direct_buffer_capacity(&buffer).map_err(jni_error)?;
            if access_unit.len() > buffer_size {
                return Err(BackendError::InvalidInput(format!(
                    "decoder input buffer too small: buffer={}, data={}",
                    buffer_size,
                    access_unit.len()
                )));
            }
            // SAFETY: `get_direct_buffer_address` returned the base of a direct ByteBuffer with
            // `buffer_size` writable bytes, and the length check above proves the source fits. The
            // source slice cannot overlap Java-owned direct-buffer storage.
            unsafe {
                std::ptr::copy_nonoverlapping(access_unit.as_ptr(), buffer_ptr, access_unit.len());
            }
            Ok(())
        })();
        if let Err(error) = copy_result {
            let _ = env.exception_clear();
            let reclaim = queue_empty_input_buffer(env, codec, index, presentation_time_us);
            return Err(input_error_after_reclaim(error, reclaim));
        }
        let flags = if is_keyframe {
            MEDIACODEC_BUFFER_FLAG_KEY_FRAME
        } else {
            0
        };
        env.call_method(
            codec,
            "queueInputBuffer",
            "(IIIJI)V",
            &[
                JValue::Int(index),
                JValue::Int(0),
                JValue::Int(access_unit_size),
                JValue::Long(presentation_time_us),
                JValue::Int(flags),
            ],
        )
        .map_err(jni_error)?;
        Ok(true)
    }

    /// Releases available output buffers for rendering and returns the number rendered.
    pub fn drain(&mut self, env: &mut JNIEnv<'_>, wait: bool) -> Result<u32, BackendError> {
        let mut rendered = 0_u32;
        let buffer_info = env
            .new_object("android/media/MediaCodec$BufferInfo", "()V", &[])
            .map_err(jni_error)?;
        loop {
            let timeout_us = if wait {
                bounded_blocking_timeout_us(self.config.timeout_us)
            } else {
                0
            };
            let codec = self.codec()?;
            let event = env
                .call_method(
                    codec,
                    "dequeueOutputBuffer",
                    "(Landroid/media/MediaCodec$BufferInfo;J)I",
                    &[JValue::Object(&buffer_info), JValue::Long(timeout_us)],
                )
                .map_err(jni_error)?
                .i()
                .map_err(jni_error)?;
            match event {
                index if index >= 0 => {
                    let info_result = OutputBufferInfo::read(env, &buffer_info);
                    if info_result.is_err() {
                        let _ = env.exception_clear();
                    }
                    let release_result = release_output_buffer(env, codec, index, true);
                    let info = output_after_release(info_result, release_result)?;
                    if info.size > 0 {
                        rendered = rendered.saturating_add(1);
                    }
                    if (info.flags & MEDIACODEC_BUFFER_FLAG_END_OF_STREAM) != 0 {
                        break;
                    }
                }
                MEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED | MEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED => {}
                MEDIACODEC_INFO_TRY_AGAIN_LATER => {
                    break;
                }
                other => {
                    return Err(BackendError::Backend(format!(
                        "MediaCodec.dequeueOutputBuffer returned {other}"
                    )));
                }
            }
        }
        Ok(rendered)
    }

    /// Flushes queued decoder work and resets generated presentation timestamps.
    pub fn flush(&mut self, env: &mut JNIEnv<'_>) -> Result<(), BackendError> {
        let codec = self.codec()?;
        env.call_method(codec, "flush", "()V", &[])
            .map_err(jni_error)?;
        self.pts_us = 0;
        Ok(())
    }

    /// Stops and releases MediaCodec resources before this value is dropped.
    pub fn stop_and_release(&mut self, env: &mut JNIEnv<'_>) -> Result<(), BackendError> {
        if let Some(codec) = self.codec.take() {
            release_media_codec(env, &codec)?;
        }
        Ok(())
    }

    fn codec(&self) -> Result<&JObject<'static>, BackendError> {
        self.codec.as_ref().map(GlobalRef::as_obj).ok_or_else(|| {
            BackendError::Backend("MediaCodec has already been released".to_string())
        })
    }
}

impl Drop for AndroidSurfaceDecoder {
    fn drop(&mut self) {
        let Some(codec) = self.codec.take() else {
            return;
        };
        if let Ok(mut env) = self.vm.attach_current_thread() {
            let _ = release_media_codec(&mut env, &codec);
        }
    }
}
