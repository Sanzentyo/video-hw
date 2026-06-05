use jni::{
    JNIEnv,
    objects::{GlobalRef, JByteBuffer, JObject, JValue},
    sys::{JNI_FALSE, JNI_TRUE, jobject},
};
use video_hw_core::{BackendError, Codec, EncodedChunk, EncodedLayout, Timestamp90k};

use crate::codec::mime_for_codec;

const MEDIACODEC_INFO_TRY_AGAIN_LATER: i32 = -1;
const MEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED: i32 = -2;
const MEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED: i32 = -3;
const MEDIACODEC_BUFFER_FLAG_KEY_FRAME: i32 = 1;
const MEDIACODEC_BUFFER_FLAG_CODEC_CONFIG: i32 = 2;
const MEDIACODEC_BUFFER_FLAG_END_OF_STREAM: i32 = 4;
const MEDIACODEC_CONFIGURE_FLAG_ENCODE: i32 = 1;
const COLOR_FORMAT_SURFACE: i32 = 0x7f00_0789;

#[derive(Debug, Clone)]
pub struct AndroidSurfaceEncoderConfig {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: Option<u32>,
    pub i_frame_interval_sec: Option<i32>,
    pub timeout_us: i64,
    pub codec_name: Option<String>,
    pub prepend_sps_pps_to_idr_frames: bool,
}

impl AndroidSurfaceEncoderConfig {
    #[must_use]
    pub fn new(codec: Codec, width: u32, height: u32, fps: u32) -> Self {
        Self {
            codec,
            width,
            height,
            fps,
            bitrate: None,
            i_frame_interval_sec: Some(1),
            timeout_us: 10_000,
            codec_name: None,
            prepend_sps_pps_to_idr_frames: true,
        }
    }
}

pub struct AndroidSurfaceEncoder {
    config: AndroidSurfaceEncoderConfig,
    codec: Option<GlobalRef>,
    input_surface: GlobalRef,
    eos_signaled: bool,
    eos_seen: bool,
}

#[derive(Debug, Clone)]
pub struct AndroidSurfaceDecoderConfig {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub timeout_us: i64,
    pub codec_name: Option<String>,
}

impl AndroidSurfaceDecoderConfig {
    #[must_use]
    pub fn new(codec: Codec, width: u32, height: u32, fps: u32) -> Self {
        Self {
            codec,
            width,
            height,
            fps,
            timeout_us: 10_000,
            codec_name: None,
        }
    }
}

pub struct AndroidSurfaceDecoder {
    config: AndroidSurfaceDecoderConfig,
    codec: Option<GlobalRef>,
    pts_us: i64,
}

impl AndroidSurfaceDecoder {
    pub fn new(
        env: &mut JNIEnv<'_>,
        config: AndroidSurfaceDecoderConfig,
        output_surface: &JObject<'_>,
    ) -> Result<Self, BackendError> {
        if config.width == 0 || config.height == 0 {
            return Err(BackendError::InvalidInput(
                "Android surface decoder dimensions must be non-zero".to_string(),
            ));
        }
        if config.fps == 0 {
            return Err(BackendError::InvalidInput(
                "Android surface decoder fps must be non-zero".to_string(),
            ));
        }
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

        let codec = match &config.codec_name {
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

        let null = JObject::null();
        env.call_method(
            &codec,
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
        env.call_method(&codec, "start", "()V", &[])
            .map_err(jni_error)?;

        Ok(Self {
            config,
            codec: Some(env.new_global_ref(codec).map_err(jni_error)?),
            pts_us: 0,
        })
    }

    pub fn queue(
        &mut self,
        env: &mut JNIEnv<'_>,
        access_unit: &[u8],
        is_keyframe: bool,
    ) -> Result<bool, BackendError> {
        let codec = self.codec()?;
        let index = env
            .call_method(
                codec,
                "dequeueInputBuffer",
                "(J)I",
                &[JValue::Long(self.config.timeout_us)],
            )
            .map_err(jni_error)?
            .i()
            .map_err(jni_error)?;
        if index < 0 {
            return Ok(false);
        }

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
        unsafe {
            std::ptr::copy_nonoverlapping(access_unit.as_ptr(), buffer_ptr, access_unit.len());
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
                JValue::Int(checked_i32(access_unit.len(), "access_unit_size")?),
                JValue::Long(self.pts_us),
                JValue::Int(flags),
            ],
        )
        .map_err(jni_error)?;
        self.pts_us = self
            .pts_us
            .saturating_add(1_000_000 / i64::from(self.config.fps.max(1)));
        Ok(true)
    }

    pub fn drain(&mut self, env: &mut JNIEnv<'_>, wait: bool) -> Result<u32, BackendError> {
        let mut rendered = 0_u32;
        let mut spins = 0_u32;
        let buffer_info = env
            .new_object("android/media/MediaCodec$BufferInfo", "()V", &[])
            .map_err(jni_error)?;
        loop {
            spins = spins.saturating_add(1);
            let timeout_us = if wait { self.config.timeout_us } else { 0 };
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
                    let info = OutputBufferInfo::read(env, &buffer_info)?;
                    env.call_method(
                        codec,
                        "releaseOutputBuffer",
                        "(IZ)V",
                        &[JValue::Int(index), JValue::Bool(JNI_TRUE)],
                    )
                    .map_err(jni_error)?;
                    if info.size > 0 {
                        rendered = rendered.saturating_add(1);
                    }
                    if (info.flags & MEDIACODEC_BUFFER_FLAG_END_OF_STREAM) != 0 {
                        break;
                    }
                }
                MEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED | MEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED => {}
                MEDIACODEC_INFO_TRY_AGAIN_LATER => {
                    if !wait || spins > 10_000 {
                        break;
                    }
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

    pub fn flush(&mut self, env: &mut JNIEnv<'_>) -> Result<(), BackendError> {
        let codec = self.codec()?;
        env.call_method(codec, "flush", "()V", &[])
            .map_err(jni_error)?;
        self.pts_us = 0;
        Ok(())
    }

    pub fn stop_and_release(&mut self, env: &mut JNIEnv<'_>) -> Result<(), BackendError> {
        if let Some(codec) = self.codec.take() {
            let _ = env.call_method(codec.as_obj(), "stop", "()V", &[]);
            env.call_method(codec.as_obj(), "release", "()V", &[])
                .map_err(jni_error)?;
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
        // MediaCodec.stop/release need JNIEnv; callers should use stop_and_release during teardown.
    }
}

impl AndroidSurfaceEncoder {
    pub fn new(
        env: &mut JNIEnv<'_>,
        config: AndroidSurfaceEncoderConfig,
    ) -> Result<Self, BackendError> {
        if config.width == 0 || config.height == 0 {
            return Err(BackendError::InvalidInput(
                "Android surface encoder dimensions must be non-zero".to_string(),
            ));
        }
        if config.fps == 0 {
            return Err(BackendError::InvalidInput(
                "Android surface encoder fps must be non-zero".to_string(),
            ));
        }
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

        let codec = match &config.codec_name {
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

        let null = JObject::null();
        env.call_method(
            &codec,
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
        let input_surface = env
            .call_method(
                &codec,
                "createInputSurface",
                "()Landroid/view/Surface;",
                &[],
            )
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        env.call_method(&codec, "start", "()V", &[])
            .map_err(jni_error)?;

        Ok(Self {
            config,
            codec: Some(env.new_global_ref(codec).map_err(jni_error)?),
            input_surface: env.new_global_ref(input_surface).map_err(jni_error)?,
            eos_signaled: false,
            eos_seen: false,
        })
    }

    pub fn input_surface(&self, env: &mut JNIEnv<'_>) -> Result<jobject, BackendError> {
        Ok(env
            .new_local_ref(self.input_surface.as_obj())
            .map_err(jni_error)?
            .into_raw())
    }

    #[must_use]
    pub fn eos_seen(&self) -> bool {
        self.eos_seen
    }

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

    pub fn drain(
        &mut self,
        env: &mut JNIEnv<'_>,
        wait_for_eos: bool,
    ) -> Result<Vec<EncodedChunk>, BackendError> {
        let mut out = Vec::new();
        let mut spins = 0_u32;
        let buffer_info = env
            .new_object("android/media/MediaCodec$BufferInfo", "()V", &[])
            .map_err(jni_error)?;
        loop {
            spins = spins.saturating_add(1);
            let timeout_us = if wait_for_eos {
                self.config.timeout_us
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
                    let info = OutputBufferInfo::read(env, &buffer_info)?;
                    if info.size > 0 {
                        out.push(self.output_chunk(env, index, &info)?);
                    }
                    let codec = self.codec()?;
                    env.call_method(
                        codec,
                        "releaseOutputBuffer",
                        "(IZ)V",
                        &[JValue::Int(index), JValue::Bool(JNI_FALSE)],
                    )
                    .map_err(jni_error)?;
                    if (info.flags & MEDIACODEC_BUFFER_FLAG_END_OF_STREAM) != 0 {
                        self.eos_seen = true;
                        break;
                    }
                }
                MEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED => {
                    out.extend(self.format_config_chunks(env)?);
                }
                MEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED => {}
                MEDIACODEC_INFO_TRY_AGAIN_LATER => {
                    if !wait_for_eos || spins > 10_000 {
                        break;
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

    pub fn stop_and_release(&mut self, env: &mut JNIEnv<'_>) -> Result<(), BackendError> {
        if let Some(codec) = self.codec.take() {
            let _ = env.call_method(codec.as_obj(), "stop", "()V", &[]);
            env.call_method(codec.as_obj(), "release", "()V", &[])
                .map_err(jni_error)?;
        }
        Ok(())
    }

    fn output_chunk(
        &self,
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
        let offset = usize::try_from(info.offset.max(0))
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
        let data = unsafe { std::slice::from_raw_parts(buffer_ptr.add(offset), size) }.to_vec();
        Ok(EncodedChunk {
            codec: self.config.codec,
            layout: encoded_layout(self.config.codec),
            data,
            pts_90k: ((info.flags & MEDIACODEC_BUFFER_FLAG_CODEC_CONFIG) == 0)
                .then_some(Timestamp90k(us_to_pts_90k(info.presentation_time_us))),
            is_keyframe: (info.flags & MEDIACODEC_BUFFER_FLAG_KEY_FRAME) != 0
                || (info.flags & MEDIACODEC_BUFFER_FLAG_CODEC_CONFIG) != 0,
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
                    data: encoder_config_buffer_payload(self.config.codec, data),
                    pts_90k: None,
                    is_keyframe: true,
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
        // MediaCodec.stop/release need JNIEnv; callers should use stop_and_release during teardown.
    }
}

#[derive(Debug)]
struct OutputBufferInfo {
    offset: i32,
    size: i32,
    presentation_time_us: i64,
    flags: i32,
}

impl OutputBufferInfo {
    fn read(env: &mut JNIEnv<'_>, buffer_info: &JObject<'_>) -> Result<Self, BackendError> {
        Ok(Self {
            offset: env
                .get_field(buffer_info, "offset", "I")
                .map_err(jni_error)?
                .i()
                .map_err(jni_error)?,
            size: env
                .get_field(buffer_info, "size", "I")
                .map_err(jni_error)?
                .i()
                .map_err(jni_error)?,
            presentation_time_us: env
                .get_field(buffer_info, "presentationTimeUs", "J")
                .map_err(jni_error)?
                .j()
                .map_err(jni_error)?,
            flags: env
                .get_field(buffer_info, "flags", "I")
                .map_err(jni_error)?
                .i()
                .map_err(jni_error)?,
        })
    }
}

fn set_media_format_i32(
    env: &mut JNIEnv<'_>,
    format: &JObject<'_>,
    key: &str,
    value: i32,
) -> Result<(), BackendError> {
    let key = env.new_string(key).map_err(jni_error)?;
    env.call_method(
        format,
        "setInteger",
        "(Ljava/lang/String;I)V",
        &[JValue::Object(&JObject::from(key)), JValue::Int(value)],
    )
    .map_err(jni_error)?;
    Ok(())
}

fn media_format_buffer(
    env: &mut JNIEnv<'_>,
    format: &JObject<'_>,
    key: &str,
) -> Result<Option<Vec<u8>>, BackendError> {
    let key_string = env.new_string(key).map_err(jni_error)?;
    let has_key = env
        .call_method(
            format,
            "containsKey",
            "(Ljava/lang/String;)Z",
            &[JValue::Object(&JObject::from(key_string))],
        )
        .map_err(jni_error)?
        .z()
        .map_err(jni_error)?;
    if !has_key {
        return Ok(None);
    }

    let key_string = env.new_string(key).map_err(jni_error)?;
    let buffer = env
        .call_method(
            format,
            "getByteBuffer",
            "(Ljava/lang/String;)Ljava/nio/ByteBuffer;",
            &[JValue::Object(&JObject::from(key_string))],
        )
        .map_err(jni_error)?
        .l()
        .map_err(jni_error)?;
    if buffer.is_null() {
        return Ok(None);
    }
    let data = byte_buffer_remaining(env, &buffer)?;
    Ok((!data.is_empty()).then_some(data))
}

fn byte_buffer_remaining(
    env: &mut JNIEnv<'_>,
    buffer: &JObject<'_>,
) -> Result<Vec<u8>, BackendError> {
    let duplicate = env
        .call_method(buffer, "duplicate", "()Ljava/nio/ByteBuffer;", &[])
        .map_err(jni_error)?
        .l()
        .map_err(jni_error)?;
    let len = env
        .call_method(&duplicate, "remaining", "()I", &[])
        .map_err(jni_error)?
        .i()
        .map_err(jni_error)?;
    if len <= 0 {
        return Ok(Vec::new());
    }
    let array = env.new_byte_array(len).map_err(jni_error)?;
    let array_object: &JObject<'_> = array.as_ref();
    env.call_method(
        &duplicate,
        "get",
        "([B)Ljava/nio/ByteBuffer;",
        &[JValue::Object(array_object)],
    )
    .map_err(jni_error)?;
    env.convert_byte_array(&array).map_err(jni_error)
}

fn encoder_config_buffer_payload(codec: Codec, buffer: Vec<u8>) -> Vec<u8> {
    match codec {
        Codec::H264 | Codec::Hevc => annexb_prefixed(buffer),
        Codec::Av1 => buffer,
    }
}

fn encoded_layout(codec: Codec) -> EncodedLayout {
    match codec {
        Codec::H264 | Codec::Hevc => EncodedLayout::AnnexB,
        Codec::Av1 => EncodedLayout::Av1,
    }
}

fn annexb_prefixed(buffer: Vec<u8>) -> Vec<u8> {
    if buffer.starts_with(&[0, 0, 1]) || buffer.starts_with(&[0, 0, 0, 1]) {
        buffer
    } else {
        let mut out = Vec::with_capacity(buffer.len() + 4);
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&buffer);
        out
    }
}

fn us_to_pts_90k(value: i64) -> i64 {
    value.saturating_mul(90_000) / 1_000_000
}

fn checked_i32<T>(value: T, name: &str) -> Result<i32, BackendError>
where
    T: TryInto<i32> + Copy + std::fmt::Display,
{
    value
        .try_into()
        .map_err(|_| BackendError::InvalidInput(format!("{name} out of range: {value}")))
}

fn jni_error(error: jni::errors::Error) -> BackendError {
    BackendError::Backend(format!("JNI error: {error}"))
}
