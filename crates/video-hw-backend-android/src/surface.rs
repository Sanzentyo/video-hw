//! Android MediaCodec Surface backend facade.
//!
//! Decoder, encoder, extraction, lifecycle cleanup, and tests live in focused child modules.
//! This facade owns shared JNI imports, constants, configuration types, and public re-exports.
use jni::{
    JNIEnv, JavaVM,
    objects::{GlobalRef, JByteBuffer, JObject, JValue},
    sys::{JNI_FALSE, JNI_TRUE, jobject},
};
use video_hw_core::{
    BackendError, Codec, EncodedChunk, EncodedContentKind, EncodedLayout, Timestamp90k,
};

use crate::codec::mime_for_codec;

const MEDIACODEC_INFO_TRY_AGAIN_LATER: i32 = -1;
const MEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED: i32 = -2;
const MEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED: i32 = -3;
const MEDIACODEC_BUFFER_FLAG_KEY_FRAME: i32 = 1;
const MEDIACODEC_BUFFER_FLAG_CODEC_CONFIG: i32 = 2;
const MEDIACODEC_BUFFER_FLAG_END_OF_STREAM: i32 = 4;
const MEDIACODEC_CONFIGURE_FLAG_ENCODE: i32 = 1;
const COLOR_FORMAT_SURFACE: i32 = 0x7f00_0789;
const MAX_BLOCKING_DEQUEUE_TIMEOUT_US: i64 = 50_000;
const MAX_EOS_DRAIN_MISSES: u32 = 100;

#[derive(Debug, Clone)]
/// Configuration for a surface-input MediaCodec encoder.
pub struct AndroidSurfaceEncoderConfig {
    /// Codec produced by the encoder.
    pub codec: Codec,
    /// Encoded width in pixels.
    pub width: u32,
    /// Encoded height in pixels.
    pub height: u32,
    /// Nominal frame rate.
    pub fps: u32,
    /// Optional target bitrate in bits per second.
    pub bitrate: Option<u32>,
    /// Optional interval between intra frames, in seconds.
    pub i_frame_interval_sec: Option<i32>,
    /// MediaCodec dequeue timeout in microseconds.
    pub timeout_us: i64,
    /// Optional explicit MediaCodec component name.
    pub codec_name: Option<String>,
    /// Requests parameter sets to be prepended to H.264/HEVC IDR frames.
    pub prepend_sps_pps_to_idr_frames: bool,
}

impl AndroidSurfaceEncoderConfig {
    /// Creates a surface encoder configuration with conservative defaults.
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
#[derive(Debug, Clone)]
/// Configuration for a surface-output MediaCodec decoder.
pub struct AndroidSurfaceDecoderConfig {
    /// Codec accepted by the decoder.
    pub codec: Codec,
    /// Decoded width in pixels.
    pub width: u32,
    /// Decoded height in pixels.
    pub height: u32,
    /// Nominal frame rate used when callers do not provide timestamps.
    pub fps: u32,
    /// MediaCodec dequeue timeout in microseconds.
    pub timeout_us: i64,
    /// Optional explicit MediaCodec component name.
    pub codec_name: Option<String>,
}

impl AndroidSurfaceDecoderConfig {
    /// Creates a surface decoder configuration with conservative defaults.
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

mod decoder;
mod encoder;
mod extraction;
mod lifecycle;
#[cfg(test)]
mod tests;

pub use decoder::AndroidSurfaceDecoder;
pub use encoder::AndroidSurfaceEncoder;

use extraction::{
    OutputBufferInfo, encoded_layout, encoder_config_buffer_payload, media_format_buffer,
    normalize_encoder_pts_90k, set_media_format_i32,
};
use lifecycle::{
    bounded_blocking_timeout_us, call_cleanup_method, checked_i32, cleanup_results,
    input_error_after_reclaim, jni_error, output_after_release, queue_empty_input_buffer,
    release_media_codec, release_output_buffer, release_surface, validate_surface_config,
    validate_timeout,
};
