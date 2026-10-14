#[cfg(all(target_os = "macos", feature = "backend-vt"))]
mod vt_bitstream;

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
mod vt_backend;

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
pub use vt_backend::{VtDecoderAdapter, VtEncoderAdapter};
