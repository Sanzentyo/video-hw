#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
mod nv_bitstream;

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
mod nv_meta_decoder;

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
mod nv_backend;

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
pub use nv_backend::{NvDecoderAdapter, NvEncoderAdapter};

#[cfg(all(windows, feature = "backend-nvidia-d3d12"))]
pub mod d3d12;
