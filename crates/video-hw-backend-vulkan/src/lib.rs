//! Vulkan Video backend adapters.
//!
//! The implementation is available on Linux and Windows when the `backend-vulkan` feature is
//! enabled. Pixel conversion is owned by `video-hw-core`; this crate exposes only backend APIs.

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
mod vulkan_av1_decode;
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
mod vulkan_hevc_decode;
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
mod vulkan_hevc_encode;

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
mod vulkan_backend;

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
pub use vulkan_backend::{
    VulkanAdapterReport, VulkanDecoderAdapter, VulkanEncoderAdapter, vulkan_adapter_reports,
};
