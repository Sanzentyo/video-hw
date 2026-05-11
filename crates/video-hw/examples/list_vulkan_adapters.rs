use anyhow::Result;

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
fn main() -> Result<()> {
    for adapter in video_hw::vulkan_adapter_reports()? {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            adapter.index,
            adapter.name,
            adapter.vendor_id,
            adapter.device_id,
            adapter.supports_decoding,
            adapter.supports_encoding
        );
    }
    Ok(())
}

#[cfg(not(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
)))]
fn main() -> Result<()> {
    anyhow::bail!("Vulkan backend is not enabled on this target");
}
