pub use video_hw_core::*;

mod capability;
mod codec;

#[cfg(target_os = "android")]
mod color;
#[cfg(target_os = "android")]
mod ffi;

pub use codec::{AndroidDecoderAdapter, AndroidEncoderAdapter};
