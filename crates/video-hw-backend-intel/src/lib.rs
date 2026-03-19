pub use video_hw_core::*;

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod intel_backend;

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use intel_backend::{IntelDecoderAdapter, IntelEncoderAdapter};
