//! Android MediaCodec backend skeleton for video-hw.
//!
//! This is a design skeleton, not a complete implementation.
//! The intended backend is NDK AMediaCodec for Android targets.

pub use video_hw_core::*;

#[cfg(target_os = "android")]
mod capability;
#[cfg(target_os = "android")]
mod codec;
#[cfg(target_os = "android")]
mod color;
#[cfg(target_os = "android")]
mod ffi;

#[cfg(target_os = "android")]
pub use capability::{android_codec_reports, AndroidCodecReport};
#[cfg(target_os = "android")]
pub use codec::{AndroidDecoderAdapter, AndroidEncoderAdapter};

#[cfg(not(target_os = "android"))]
compile_error!("video-hw-backend-android is only supported on target_os = \"android\"");
