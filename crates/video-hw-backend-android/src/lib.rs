//! Android MediaCodec backend adapters.
//!
//! Byte-buffer adapters implement the common `video-hw-core` traits. Android-only surface
//! adapters expose JNI-facing APIs for rendering decoded frames and supplying encoder input.

#[cfg(feature = "backend-android")]
mod capability;
#[cfg(feature = "backend-android")]
mod codec;

#[cfg(all(feature = "backend-android", any(target_os = "android", test)))]
mod color;
#[cfg(all(feature = "backend-android", target_os = "android"))]
mod ffi;
#[cfg(all(feature = "backend-android", target_os = "android"))]
mod surface;

#[cfg(feature = "backend-android")]
pub use codec::{AndroidDecoderAdapter, AndroidEncoderAdapter};
#[cfg(all(feature = "backend-android", target_os = "android"))]
pub use surface::{
    AndroidSurfaceDecoder, AndroidSurfaceDecoderConfig, AndroidSurfaceEncoder,
    AndroidSurfaceEncoderConfig,
};

#[cfg(feature = "backend-android-imagereader")]
pub mod image_reader;
