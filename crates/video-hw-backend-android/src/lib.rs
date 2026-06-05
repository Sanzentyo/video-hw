pub use video_hw_core::*;

mod capability;
mod codec;

#[cfg(target_os = "android")]
mod color;
#[cfg(target_os = "android")]
mod ffi;
#[cfg(target_os = "android")]
mod surface;

pub use codec::{AndroidDecoderAdapter, AndroidEncoderAdapter};
#[cfg(target_os = "android")]
pub use surface::{AndroidSurfaceEncoder, AndroidSurfaceEncoderConfig};
