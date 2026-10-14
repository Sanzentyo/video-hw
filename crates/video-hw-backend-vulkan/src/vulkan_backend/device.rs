//! Vulkan adapter probing and decoder/encoder device construction.

use super::*;

pub(super) fn probe_vulkan_support() -> (bool, bool) {
    let Ok(instance) = VulkanInstance::new() else {
        return (false, false);
    };
    let Ok(adapters) = instance.iter_adapters(None) else {
        return (false, false);
    };
    let mut decode_supported = false;
    let mut encode_supported = false;
    for adapter in adapters {
        decode_supported |= adapter.supports_decoding();
        encode_supported |= adapter.supports_encoding();
    }
    (decode_supported, encode_supported)
}

pub(super) fn create_vk_bytes_decoder(
    adapter_index: Option<usize>,
) -> Result<BytesDecoder, BackendError> {
    let device = create_vk_device(true, false, adapter_index)?;
    device
        .create_bytes_decoder(DecoderParameters::default())
        .map_err(|err| {
            BackendError::UnsupportedConfig(format!("failed to create Vulkan decoder: {err}"))
        })
}

pub(super) fn create_vk_wgpu_textures_decoder(
    adapter_index: Option<usize>,
) -> Result<WgpuTexturesDecoder, BackendError> {
    let device = create_vk_device(true, false, adapter_index)?;
    device
        .create_wgpu_textures_decoder(DecoderParameters::default())
        .map_err(|err| {
            BackendError::UnsupportedConfig(format!(
                "failed to create Vulkan texture decoder: {err}"
            ))
        })
}

pub(super) fn create_vk_bytes_encoder(
    width: usize,
    height: usize,
    fps: i32,
    adapter_index: Option<usize>,
) -> Result<BytesEncoder, BackendError> {
    let device = create_vk_device(false, true, adapter_index)?;
    let width_u32 = u32::try_from(width)
        .map_err(|_| BackendError::InvalidInput("frame width does not fit in u32".to_string()))?;
    let height_u32 = u32::try_from(height)
        .map_err(|_| BackendError::InvalidInput("frame height does not fit in u32".to_string()))?;
    let width = NonZeroU32::new(width_u32)
        .ok_or_else(|| BackendError::InvalidInput("frame width must be > 0".to_string()))?;
    let height = NonZeroU32::new(height_u32)
        .ok_or_else(|| BackendError::InvalidInput("frame height must be > 0".to_string()))?;
    let target_fps = u32::try_from(fps.max(1)).unwrap_or(30);
    let video_parameters = VideoParameters {
        width,
        height,
        target_framerate: target_fps.into(),
    };
    let parameters = device
        .encoder_parameters_high_quality(
            video_parameters,
            RateControl::VariableBitrate {
                average_bitrate: 4_000_000,
                max_bitrate: 8_000_000,
                virtual_buffer_size: Duration::from_secs(2),
            },
        )
        .map_err(|err| {
            BackendError::UnsupportedConfig(format!(
                "failed to build Vulkan encoder parameters: {err}"
            ))
        })?;
    device.create_bytes_encoder(parameters).map_err(|err| {
        BackendError::UnsupportedConfig(format!("failed to create Vulkan encoder: {err}"))
    })
}

pub(super) fn create_vk_device(
    require_decode: bool,
    require_encode: bool,
    adapter_index: Option<usize>,
) -> Result<Arc<VulkanDevice>, BackendError> {
    let instance = VulkanInstance::new().map_err(|err| {
        BackendError::UnsupportedConfig(format!("failed to initialize Vulkan: {err}"))
    })?;
    let adapters = instance.iter_adapters(None).map_err(|err| {
        BackendError::UnsupportedConfig(format!("failed to enumerate Vulkan adapters: {err}"))
    })?;
    let adapter = if let Some(adapter_index) = adapter_index {
        adapters
            .into_iter()
            .enumerate()
            .find_map(|(index, adapter)| (index == adapter_index).then_some(adapter))
            .ok_or_else(|| {
                BackendError::UnsupportedConfig(format!(
                    "Vulkan adapter index {adapter_index} is not available"
                ))
            })
            .and_then(|adapter| {
                let info = adapter.info();
                if require_decode && !adapter.supports_decoding() {
                    return Err(BackendError::UnsupportedConfig(format!(
                        "Vulkan adapter index {adapter_index} ({}) does not support decode",
                        info.name
                    )));
                }
                if require_encode && !adapter.supports_encoding() {
                    return Err(BackendError::UnsupportedConfig(format!(
                        "Vulkan adapter index {adapter_index} ({}) does not support encode",
                        info.name
                    )));
                }
                Ok(adapter)
            })?
    } else {
        adapters
            .into_iter()
            .find(|adapter| {
                (!require_decode || adapter.supports_decoding())
                    && (!require_encode || adapter.supports_encoding())
            })
            .ok_or_else(|| {
                let message = match (require_decode, require_encode) {
                    (true, true) => {
                        "no Vulkan adapter supports both decode and encode for direct backend"
                    }
                    (true, false) => "no Vulkan adapter supports decode for direct backend",
                    (false, true) => "no Vulkan adapter supports encode for direct backend",
                    (false, false) => "no Vulkan adapter is available for direct backend",
                };
                BackendError::UnsupportedConfig(message.to_string())
            })?
    };
    adapter
        .create_device(Default::default(), Default::default(), Default::default())
        .map_err(|err| {
            BackendError::UnsupportedConfig(format!("failed to create Vulkan device: {err}"))
        })
}
