#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
mod intel_backend;

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
pub use intel_backend::{IntelDecoderAdapter, IntelEncoderAdapter};
