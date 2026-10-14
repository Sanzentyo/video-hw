//! MediaCodec surface-input encoder, encoded output draining, and surface ownership.

use super::*;

/// MediaCodec encoder whose input is supplied through an Android `Surface`.
pub struct AndroidSurfaceEncoder {
    config: AndroidSurfaceEncoderConfig,
    vm: JavaVM,
    codec: Option<GlobalRef>,
    input_surface: Option<GlobalRef>,
    eos_signaled: bool,
    eos_seen: bool,
    pts_origin_us: Option<i64>,
    last_pts_90k: Option<i64>,
}
impl AndroidSurfaceEncoder {
    /// Creates, configures, and starts a surface-input encoder.
    pub fn new(
        env: &mut JNIEnv<'_>,
        config: AndroidSurfaceEncoderConfig,
    ) -> Result<Self, BackendError> {
        validate_surface_config(config.width, config.height, config.fps, "encoder")?;
        validate_timeout(config.timeout_us, "encoder")?;
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

        if let Some(bitrate) = config.bitrate {
            set_media_format_i32(env, &format, "bitrate", checked_i32(bitrate, "bitrate")?)?;
        }
        set_media_format_i32(env, &format, "frame-rate", checked_i32(config.fps, "fps")?)?;
        if let Some(interval) = config.i_frame_interval_sec {
            set_media_format_i32(env, &format, "i-frame-interval", interval)?;
        }
        set_media_format_i32(env, &format, "color-format", COLOR_FORMAT_SURFACE)?;
        if config.prepend_sps_pps_to_idr_frames {
            set_media_format_i32(env, &format, "prepend-sps-pps-to-idr-frames", 1)?;
        }

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
                    "createEncoderByType",
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
        let input_surface_result = (|| -> Result<GlobalRef, BackendError> {
            let null = JObject::null();
            env.call_method(
                codec.as_obj(),
                "configure",
                "(Landroid/media/MediaFormat;Landroid/view/Surface;Landroid/media/MediaCrypto;I)V",
                &[
                    JValue::Object(&format),
                    JValue::Object(&null),
                    JValue::Object(&null),
                    JValue::Int(MEDIACODEC_CONFIGURE_FLAG_ENCODE),
                ],
            )
            .map_err(jni_error)?;
            let input_surface_local = env
                .call_method(
                    codec.as_obj(),
                    "createInputSurface",
                    "()Landroid/view/Surface;",
                    &[],
                )
                .map_err(jni_error)?
                .l()
                .map_err(jni_error)?;
            let input_surface = match env.new_global_ref(&input_surface_local) {
                Ok(input_surface) => input_surface,
                Err(error) => {
                    let _ = call_cleanup_method(env, &input_surface_local, "release");
                    return Err(jni_error(error));
                }
            };
            if let Err(error) = env.call_method(codec.as_obj(), "start", "()V", &[]) {
                let _ = env.exception_clear();
                let _ = release_surface(env, &input_surface);
                return Err(jni_error(error));
            }
            Ok(input_surface)
        })();
        let input_surface = match input_surface_result {
            Ok(input_surface) => input_surface,
            Err(error) => {
                let _ = release_media_codec(env, &codec);
                return Err(error);
            }
        };

        Ok(Self {
            config,
            vm,
            codec: Some(codec),
            input_surface: Some(input_surface),
            eos_signaled: false,
            eos_seen: false,
            pts_origin_us: None,
            last_pts_90k: None,
        })
    }

    /// Returns a local JNI reference to the encoder input `Surface`.
    ///
    /// The returned reference belongs to the current JNI local-reference frame. It must not be
    /// retained beyond that frame without first promoting it to a global reference.
    pub fn input_surface(&self, env: &mut JNIEnv<'_>) -> Result<jobject, BackendError> {
        let input_surface = self.input_surface.as_ref().ok_or_else(|| {
            BackendError::Backend("input Surface has already been released".to_string())
        })?;
        Ok(env
            .new_local_ref(input_surface.as_obj())
            .map_err(jni_error)?
            .into_raw())
    }

    #[must_use]
    /// Returns whether the encoder has emitted an end-of-stream output buffer.
    pub fn eos_seen(&self) -> bool {
        self.eos_seen
    }

    /// Signals that no further frames will be submitted through the input surface.
    pub fn signal_end_of_input_stream(&mut self, env: &mut JNIEnv<'_>) -> Result<(), BackendError> {
        if self.eos_signaled {
            return Ok(());
        }
        let codec = self.codec()?;
        env.call_method(codec, "signalEndOfInputStream", "()V", &[])
            .map_err(jni_error)?;
        self.eos_signaled = true;
        Ok(())
    }

    /// Drains encoded chunks, optionally waiting for the end-of-stream marker.
    ///
    /// `wait_for_eos` requires [`Self::signal_end_of_input_stream`] to have been called first.
    pub fn drain(
        &mut self,
        env: &mut JNIEnv<'_>,
        wait_for_eos: bool,
    ) -> Result<Vec<EncodedChunk>, BackendError> {
        if wait_for_eos && !self.eos_signaled {
            return Err(BackendError::InvalidInput(
                "cannot wait for Android surface encoder end of stream before signaling it"
                    .to_string(),
            ));
        }
        if self.eos_seen {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut misses = 0_u32;
        let buffer_info = env
            .new_object("android/media/MediaCodec$BufferInfo", "()V", &[])
            .map_err(jni_error)?;
        loop {
            let timeout_us = if wait_for_eos {
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
                    misses = 0;
                    let output_result =
                        OutputBufferInfo::read(env, &buffer_info).and_then(|info| {
                            let chunk = (info.size > 0)
                                .then(|| self.output_chunk(env, index, &info))
                                .transpose()?;
                            Ok((chunk, info.flags))
                        });
                    if output_result.is_err() {
                        let _ = env.exception_clear();
                    }
                    let release_result = release_output_buffer(env, self.codec()?, index, false);
                    let (chunk, flags) = output_after_release(output_result, release_result)?;
                    if let Some(chunk) = chunk {
                        out.push(chunk);
                    }
                    if (flags & MEDIACODEC_BUFFER_FLAG_END_OF_STREAM) != 0 {
                        self.eos_seen = true;
                        break;
                    }
                }
                MEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED => {
                    misses = 0;
                    out.extend(self.format_config_chunks(env)?);
                }
                MEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED => {}
                MEDIACODEC_INFO_TRY_AGAIN_LATER => {
                    if !wait_for_eos {
                        break;
                    }
                    misses = misses.saturating_add(1);
                    if misses >= MAX_EOS_DRAIN_MISSES {
                        return Err(BackendError::TemporaryBackpressure(
                            "timed out waiting for Android surface encoder end of stream"
                                .to_string(),
                        ));
                    }
                }
                other => {
                    return Err(BackendError::Backend(format!(
                        "MediaCodec.dequeueOutputBuffer returned {other}"
                    )));
                }
            }
        }
        Ok(out)
    }

    /// Stops and releases MediaCodec and input-surface resources before drop.
    pub fn stop_and_release(&mut self, env: &mut JNIEnv<'_>) -> Result<(), BackendError> {
        let codec_result = self
            .codec
            .take()
            .map_or(Ok(()), |codec| release_media_codec(env, &codec));
        let surface_result = self
            .input_surface
            .take()
            .map_or(Ok(()), |surface| release_surface(env, &surface));
        cleanup_results(codec_result, surface_result)
    }

    fn output_chunk(
        &mut self,
        env: &mut JNIEnv<'_>,
        index: i32,
        info: &OutputBufferInfo,
    ) -> Result<EncodedChunk, BackendError> {
        let codec = self.codec()?;
        let buffer = env
            .call_method(
                codec,
                "getOutputBuffer",
                "(I)Ljava/nio/ByteBuffer;",
                &[JValue::Int(index)],
            )
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        if buffer.is_null() {
            return Err(BackendError::Backend(
                "MediaCodec.getOutputBuffer returned null".to_string(),
            ));
        }
        let buffer = JByteBuffer::from(buffer);
        let buffer_ptr = env.get_direct_buffer_address(&buffer).map_err(jni_error)?;
        let buffer_size = env.get_direct_buffer_capacity(&buffer).map_err(jni_error)?;
        let offset = usize::try_from(info.offset)
            .map_err(|_| BackendError::Backend("negative encoder output offset".to_string()))?;
        let size = usize::try_from(info.size)
            .map_err(|_| BackendError::Backend("negative encoder output size".to_string()))?;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| BackendError::Backend("encoder output range overflow".to_string()))?;
        if end > buffer_size {
            return Err(BackendError::Backend(format!(
                "encoder output range exceeds buffer: end={end}, len={buffer_size}"
            )));
        }
        let is_codec_config = (info.flags & MEDIACODEC_BUFFER_FLAG_CODEC_CONFIG) != 0;
        // SAFETY: the direct ByteBuffer base is valid for `buffer_size` bytes, and the checked
        // range above proves `[offset, offset + size)` lies within that allocation.
        let data = unsafe { std::slice::from_raw_parts(buffer_ptr.add(offset), size) }.to_vec();
        let data = if is_codec_config {
            encoder_config_buffer_payload(self.config.codec, data)
        } else {
            data
        };
        let pts_90k = if !is_codec_config {
            Some(Timestamp90k(normalize_encoder_pts_90k(
                &mut self.pts_origin_us,
                &mut self.last_pts_90k,
                info.presentation_time_us,
            )))
        } else {
            None
        };
        Ok(EncodedChunk {
            codec: self.config.codec,
            layout: encoded_layout(self.config.codec),
            kind: if is_codec_config {
                EncodedContentKind::CodecConfig
            } else {
                EncodedContentKind::Frame
            },
            data,
            pts_90k,
            is_keyframe: !is_codec_config && (info.flags & MEDIACODEC_BUFFER_FLAG_KEY_FRAME) != 0,
        })
    }

    fn format_config_chunks(
        &self,
        env: &mut JNIEnv<'_>,
    ) -> Result<Vec<EncodedChunk>, BackendError> {
        let codec = self.codec()?;
        let format = env
            .call_method(
                codec,
                "getOutputFormat",
                "()Landroid/media/MediaFormat;",
                &[],
            )
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        if format.is_null() {
            return Ok(Vec::new());
        }
        let keys: &[&str] = match self.config.codec {
            Codec::H264 => &["csd-0", "csd-1"],
            Codec::Hevc | Codec::Av1 => &["csd-0", "csd-1", "csd-2"],
        };
        keys.iter()
            .filter_map(|key| media_format_buffer(env, &format, key).transpose())
            .map(|buffer| {
                buffer.map(|data| EncodedChunk {
                    codec: self.config.codec,
                    layout: encoded_layout(self.config.codec),
                    kind: EncodedContentKind::CodecConfig,
                    data: encoder_config_buffer_payload(self.config.codec, data),
                    pts_90k: None,
                    is_keyframe: false,
                })
            })
            .collect()
    }

    fn codec(&self) -> Result<&JObject<'static>, BackendError> {
        self.codec.as_ref().map(GlobalRef::as_obj).ok_or_else(|| {
            BackendError::Backend("MediaCodec has already been released".to_string())
        })
    }
}

impl Drop for AndroidSurfaceEncoder {
    fn drop(&mut self) {
        let codec = self.codec.take();
        let input_surface = self.input_surface.take();
        if codec.is_none() && input_surface.is_none() {
            return;
        }
        if let Ok(mut env) = self.vm.attach_current_thread() {
            if let Some(codec) = codec {
                let _ = release_media_codec(&mut env, &codec);
            }
            if let Some(input_surface) = input_surface {
                let _ = release_surface(&mut env, &input_surface);
            }
        }
    }
}
