//! Vulkan HEVC capability, device, environment, and readback probing.

use super::*;

pub(super) fn hevc_decode_session_limit_from_env(env_key: &str, capability_limit: u32) -> u32 {
    hevc_decode_session_limit(
        std::env::var(env_key).ok().as_deref(),
        capability_limit,
        false,
    )
}

pub(super) fn hevc_decode_session_limit_from_env_allow_zero(
    env_key: &str,
    capability_limit: u32,
) -> u32 {
    hevc_decode_session_limit(
        std::env::var(env_key).ok().as_deref(),
        capability_limit,
        true,
    )
}

pub(super) fn hevc_decode_session_limit(
    configured: Option<&str>,
    capability_limit: u32,
    allow_zero: bool,
) -> u32 {
    let default_limit = capability_limit.max(1);
    configured
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| allow_zero || *value > 0)
        .map(|value| value.min(default_limit))
        .unwrap_or(default_limit)
}

fn hevc_experimental_dpb_mode() -> HevcExperimentalDpbMode {
    parse_hevc_experimental_dpb_mode(
        std::env::var("VIDEO_HW_VULKAN_HEVC_EXPERIMENTAL_DPB")
            .ok()
            .as_deref(),
    )
}

pub(super) fn parse_hevc_experimental_dpb_mode(value: Option<&str>) -> HevcExperimentalDpbMode {
    match value.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) => match value.as_str() {
            "0" | "off" | "false" | "disable" | "disabled" => HevcExperimentalDpbMode::Off,
            "auto" => HevcExperimentalDpbMode::Auto,
            "1" | "on" | "true" | "full" | "" => HevcExperimentalDpbMode::On,
            _ => HevcExperimentalDpbMode::On,
        },
        None => HevcExperimentalDpbMode::On,
    }
}

fn hevc_experimental_dpb_marker_path() -> PathBuf {
    std::env::temp_dir().join("video-hw-vulkan-hevc-dpb-inflight.flag")
}

impl HevcExperimentalDpbMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::On => "on",
        }
    }
}

impl HevcExperimentalDpbDecision {
    pub(super) fn enabled(self) -> bool {
        matches!(self, Self::EnabledOn | Self::EnabledAuto)
    }
}

pub(super) fn decide_hevc_experimental_dpb(
    mode: HevcExperimentalDpbMode,
    marker_present: bool,
    marker_write_failed: bool,
) -> HevcExperimentalDpbDecision {
    match mode {
        HevcExperimentalDpbMode::Off => HevcExperimentalDpbDecision::DisabledOff,
        HevcExperimentalDpbMode::On => HevcExperimentalDpbDecision::EnabledOn,
        HevcExperimentalDpbMode::Auto => {
            if marker_present {
                HevcExperimentalDpbDecision::DisabledAutoMarkerPresent
            } else if marker_write_failed {
                HevcExperimentalDpbDecision::DisabledAutoMarkerWriteFailed
            } else {
                HevcExperimentalDpbDecision::EnabledAuto
            }
        }
    }
}

pub(super) fn format_hevc_experimental_dpb_status(
    decision: HevcExperimentalDpbDecision,
    marker_path: &std::path::Path,
    marker_write_error: Option<&str>,
) -> String {
    let marker_display = marker_path.display();
    match decision {
        HevcExperimentalDpbDecision::DisabledOff => {
            "mode=off (experimental DPB disabled)".to_string()
        }
        HevcExperimentalDpbDecision::EnabledOn => {
            if let Some(err) = marker_write_error {
                format!("mode=on (experimental DPB enabled; inflight marker write failed: {err})")
            } else {
                format!(
                    "mode=on (experimental DPB enabled; inflight marker armed at {marker_display})"
                )
            }
        }
        HevcExperimentalDpbDecision::EnabledAuto => format!(
            "mode=auto (experimental DPB enabled; no stale marker; inflight marker armed at {marker_display})"
        ),
        HevcExperimentalDpbDecision::DisabledAutoMarkerPresent => format!(
            "mode=auto (experimental DPB disabled; stale inflight marker detected at {marker_display})"
        ),
        HevcExperimentalDpbDecision::DisabledAutoMarkerWriteFailed => {
            let detail = marker_write_error.unwrap_or("unknown error");
            format!("mode=auto (experimental DPB disabled; inflight marker write failed: {detail})")
        }
    }
}

pub(super) fn configure_hevc_experimental_dpb() -> HevcExperimentalDpbConfiguration {
    let mode = hevc_experimental_dpb_mode();
    let marker_path = hevc_experimental_dpb_marker_path();
    let marker_present = marker_path.exists();
    let should_attempt_marker_write = match mode {
        HevcExperimentalDpbMode::Off => false,
        HevcExperimentalDpbMode::On => true,
        HevcExperimentalDpbMode::Auto => !marker_present,
    };

    let mut marker_write_error = None;
    let mut armed_marker_path = None;
    if should_attempt_marker_write {
        match std::fs::write(&marker_path, b"inflight") {
            Ok(()) => {
                armed_marker_path = Some(marker_path.clone());
            }
            Err(err) => {
                marker_write_error = Some(err.to_string());
            }
        }
    }

    let decision = decide_hevc_experimental_dpb(
        mode,
        marker_present,
        should_attempt_marker_write && armed_marker_path.is_none(),
    );
    let status =
        format_hevc_experimental_dpb_status(decision, &marker_path, marker_write_error.as_deref());
    let enabled = decision.enabled();
    if !enabled {
        armed_marker_path = None;
    }

    HevcExperimentalDpbConfiguration {
        enabled,
        mode,
        status,
        marker_path: armed_marker_path,
    }
}

pub(super) fn hevc_slice_segment_offset_mode(vendor_id: u32) -> HevcSliceSegmentOffsetMode {
    let env_value = std::env::var("VIDEO_HW_VULKAN_HEVC_SLICE_OFFSET_MODE").ok();
    hevc_slice_segment_offset_mode_from_override(vendor_id, env_value.as_deref())
}

pub(super) fn hevc_slice_segment_offset_mode_from_override(
    vendor_id: u32,
    override_value: Option<&str>,
) -> HevcSliceSegmentOffsetMode {
    if let Some(raw) = override_value {
        if let Ok(offset) = raw.parse::<u32>() {
            return HevcSliceSegmentOffsetMode::Fixed(offset);
        }
        match raw.to_ascii_lowercase().as_str() {
            "rbsp" | "payload" | "spec" => return HevcSliceSegmentOffsetMode::RbspPayload,
            "nalu" | "nal" | "header" => return HevcSliceSegmentOffsetMode::NaluHeader,
            "annexb" | "startcode" | "start_code" | "zero" | "0" => {
                return HevcSliceSegmentOffsetMode::AnnexBStartCode;
            }
            "global" | "global_rbsp" | "single" | "single_buffer" => {
                return HevcSliceSegmentOffsetMode::GlobalRbspPayload;
            }
            "memory" | "memory_absolute" | "binding" | "bind" => {
                return HevcSliceSegmentOffsetMode::MemoryBindingAbsolute;
            }
            _ => {}
        }
    }
    default_hevc_slice_segment_offset_mode(vendor_id)
}

fn default_hevc_slice_segment_offset_mode(_vendor_id: u32) -> HevcSliceSegmentOffsetMode {
    HevcSliceSegmentOffsetMode::AnnexBStartCode
}

pub(super) fn build_decode_readback_regions(
    output_format: vk::Format,
    coded_width: u32,
    coded_height: u32,
) -> Result<(u64, Vec<vk::BufferImageCopy>), String> {
    if coded_width == 0 || coded_height == 0 {
        return Err(format!(
            "invalid coded extent for decode readback: {}x{}",
            coded_width, coded_height
        ));
    }

    let mut regions = Vec::new();
    let mut next_offset = 0_u64;
    let mut push_region = |aspect_mask: vk::ImageAspectFlags,
                           plane_width: u32,
                           plane_height: u32,
                           bytes_per_texel: u64|
     -> Result<(), String> {
        let row_bytes = u64::from(plane_width)
            .checked_mul(bytes_per_texel)
            .ok_or_else(|| "decode readback row size overflowed u64".to_string())?;
        let plane_bytes = row_bytes
            .checked_mul(u64::from(plane_height))
            .ok_or_else(|| "decode readback plane size overflowed u64".to_string())?;
        let offset = align_up(next_offset, 4);
        regions.push(
            vk::BufferImageCopy::default()
                .buffer_offset(offset)
                .buffer_row_length(plane_width)
                .buffer_image_height(plane_height)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_offset(vk::Offset3D::default())
                .image_extent(vk::Extent3D {
                    width: plane_width,
                    height: plane_height,
                    depth: 1,
                }),
        );
        next_offset = offset
            .checked_add(plane_bytes)
            .ok_or_else(|| "decode readback buffer size overflowed u64".to_string())?;
        Ok(())
    };

    match output_format {
        vk::Format::G8_B8R8_2PLANE_420_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(
                vk::ImageAspectFlags::PLANE_1,
                coded_width.div_ceil(2),
                coded_height.div_ceil(2),
                2,
            )?;
        }
        vk::Format::G8_B8R8_2PLANE_422_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(
                vk::ImageAspectFlags::PLANE_1,
                coded_width.div_ceil(2),
                coded_height,
                2,
            )?;
        }
        vk::Format::G8_B8R8_2PLANE_444_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(vk::ImageAspectFlags::PLANE_1, coded_width, coded_height, 2)?;
        }
        vk::Format::G8_B8_R8_3PLANE_420_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(
                vk::ImageAspectFlags::PLANE_1,
                coded_width.div_ceil(2),
                coded_height.div_ceil(2),
                1,
            )?;
            push_region(
                vk::ImageAspectFlags::PLANE_2,
                coded_width.div_ceil(2),
                coded_height.div_ceil(2),
                1,
            )?;
        }
        vk::Format::G8_B8_R8_3PLANE_422_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(
                vk::ImageAspectFlags::PLANE_1,
                coded_width.div_ceil(2),
                coded_height,
                1,
            )?;
            push_region(
                vk::ImageAspectFlags::PLANE_2,
                coded_width.div_ceil(2),
                coded_height,
                1,
            )?;
        }
        vk::Format::G8_B8_R8_3PLANE_444_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(vk::ImageAspectFlags::PLANE_1, coded_width, coded_height, 1)?;
            push_region(vk::ImageAspectFlags::PLANE_2, coded_width, coded_height, 1)?;
        }
        vk::Format::B8G8R8A8_UNORM | vk::Format::R8G8B8A8_UNORM => {
            push_region(vk::ImageAspectFlags::COLOR, coded_width, coded_height, 4)?;
        }
        _ => {
            return Err(format!(
                "decode readback is not implemented for output format {output_format:?}"
            ));
        }
    }

    Ok((next_offset, regions))
}

pub(super) fn run_hevc_decode_probe() -> HevcDecodePrerequisiteProbe {
    // SAFETY: We only load function pointers from the Vulkan loader and keep the returned
    // handle within this function. No borrowed pointers escape this module.
    let entry = match unsafe { ash::Entry::load() } {
        Ok(entry) => entry,
        Err(err) => {
            return HevcDecodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to load Vulkan entry: {err}"
            ));
        }
    };

    // SAFETY: `vk::InstanceCreateInfo::default()` produces a valid zero-initialized struct
    // for instance creation in ash. We destroy the created instance before returning.
    let instance = match unsafe { entry.create_instance(&vk::InstanceCreateInfo::default(), None) }
    {
        Ok(instance) => instance,
        Err(err) => {
            return HevcDecodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to create Vulkan instance: {err}"
            ));
        }
    };

    let probe_result = (|| -> Result<HevcDecodePrerequisiteProbe, String> {
        // SAFETY: `instance` is valid for the duration of this closure; we only read
        // physical-device handles returned by Vulkan and do not dereference raw pointers.
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;

        if physical_devices.is_empty() {
            return Ok(HevcDecodePrerequisiteProbe::NoCompatibleAdapter);
        }

        let mut observed_extensions = ExtensionFlags::default();
        let mut observed_decode_queue = false;
        let mut device_init_errors = Vec::new();
        for physical_device in physical_devices {
            let support = query_adapter_decode_support(&instance, physical_device)
                .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
            observed_extensions.union_assign(support.extensions);
            observed_decode_queue |= support.decode_queue_family_index.is_some();

            if support.extensions.supports_hevc_decode()
                && let Some(queue_family_index) = support.decode_queue_family_index
            {
                match try_initialize_hevc_decode_device(
                    &instance,
                    physical_device,
                    queue_family_index,
                ) {
                    Ok(()) => return Ok(HevcDecodePrerequisiteProbe::Ready),
                    Err(err) => device_init_errors.push(err),
                }
            }
        }

        let mut missing = Vec::new();
        if !observed_extensions.has_video_queue {
            missing.push("VK_KHR_video_queue");
        }
        if !observed_extensions.has_video_decode_queue {
            missing.push("VK_KHR_video_decode_queue");
        }
        if !observed_extensions.has_video_decode_h265 {
            missing.push("VK_KHR_video_decode_h265");
        }

        if !missing.is_empty() {
            return Ok(HevcDecodePrerequisiteProbe::MissingExtensions { missing });
        }

        if !observed_decode_queue {
            return Ok(HevcDecodePrerequisiteProbe::MissingDecodeQueueFamily);
        }

        if !device_init_errors.is_empty() {
            let joined = device_init_errors.join("; ");
            return Ok(HevcDecodePrerequisiteProbe::DeviceInitializationFailed(
                joined,
            ));
        }

        Ok(HevcDecodePrerequisiteProbe::NoCompatibleAdapter)
    })();

    // SAFETY: `instance` was created in this function and is no longer used after this call.
    unsafe {
        instance.destroy_instance(None);
    }

    probe_result.unwrap_or_else(HevcDecodePrerequisiteProbe::ProbeUnavailable)
}

pub(super) fn split_annexb_nalus(bitstream: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let mut cursor = 0usize;
    while let Some((start, prefix_len)) = find_annexb_start_code(bitstream, cursor) {
        let nalu_start = start.saturating_add(prefix_len);
        if nalu_start >= bitstream.len() {
            break;
        }
        let next_start = find_annexb_start_code(bitstream, nalu_start)
            .map(|(offset, _)| offset)
            .unwrap_or(bitstream.len());
        if next_start > nalu_start {
            nalus.push(&bitstream[nalu_start..next_start]);
        }
        cursor = next_start;
    }
    nalus
}

fn find_annexb_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    if from >= data.len() {
        return None;
    }
    let mut index = from;
    while index + 3 <= data.len() {
        if data[index] == 0 && data[index + 1] == 0 {
            if data.get(index + 2) == Some(&1) {
                return Some((index, 3));
            }
            if index + 3 < data.len() && data[index + 2] == 0 && data[index + 3] == 1 {
                return Some((index, 4));
            }
        }
        index += 1;
    }
    None
}

pub(super) fn hevc_nal_type_raw(nalu: &[u8]) -> Option<u8> {
    let header = *nalu.first()?;
    Some((header >> 1) & 0b0011_1111)
}

pub(super) fn hevc_nal_type(nalu: &[u8]) -> Option<NALUnitType> {
    Some(NALUnitType::from(hevc_nal_type_raw(nalu)?))
}

pub(super) fn query_hevc_decode_output_formats(
    video_queue: &ash::khr::video_queue::Instance,
    physical_device: vk::PhysicalDevice,
    profile: vk::VideoProfileInfoKHR<'_>,
) -> Result<Vec<vk::Format>, String> {
    let profiles = [profile];
    let mut profile_list = vk::VideoProfileListInfoKHR::default().profiles(&profiles);
    let format_info = vk::PhysicalDeviceVideoFormatInfoKHR::default()
        .image_usage(
            vk::ImageUsageFlags::VIDEO_DECODE_DST_KHR
                | vk::ImageUsageFlags::VIDEO_DECODE_DPB_KHR
                | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .push_next(&mut profile_list);

    let mut property_count = 0_u32;
    // SAFETY: We pass valid pointers and request only the count in this first query.
    let count_result = unsafe {
        (video_queue
            .fp()
            .get_physical_device_video_format_properties_khr)(
            physical_device,
            &format_info,
            &mut property_count,
            std::ptr::null_mut(),
        )
    };
    if count_result != vk::Result::SUCCESS {
        return Err(format!("video format count query failed: {count_result:?}"));
    }

    if property_count == 0 {
        return Ok(Vec::new());
    }

    let mut properties = vec![vk::VideoFormatPropertiesKHR::default(); property_count as usize];
    // SAFETY: `properties` points to writable storage sized from the prior count query.
    let properties_result = unsafe {
        (video_queue
            .fp()
            .get_physical_device_video_format_properties_khr)(
            physical_device,
            &format_info,
            &mut property_count,
            properties.as_mut_ptr(),
        )
    };
    if properties_result != vk::Result::SUCCESS {
        return Err(format!(
            "video format properties query failed: {properties_result:?}"
        ));
    }

    Ok(properties
        .into_iter()
        .map(|property| property.format)
        .collect())
}

pub(super) fn query_adapter_decode_support(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Result<AdapterDecodeSupport, vk::Result> {
    // SAFETY: We call Vulkan with a valid `physical_device` handle returned from
    // `enumerate_physical_devices` for this same `instance`.
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical_device) }?;

    let mut flags = ExtensionFlags::default();
    for extension in extensions {
        // SAFETY: Vulkan guarantees `extension_name` is a null-terminated C string.
        let name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        flags.has_video_queue |= name == vk::KHR_VIDEO_QUEUE_NAME;
        flags.has_video_decode_queue |= name == vk::KHR_VIDEO_DECODE_QUEUE_NAME;
        flags.has_video_decode_h265 |= name == vk::KHR_VIDEO_DECODE_H265_NAME;
        flags.has_video_maintenance1 |= name == vk::KHR_VIDEO_MAINTENANCE1_NAME;
    }

    let decode_queue_family_index = query_video_codec_queue_family_index(
        instance,
        physical_device,
        vk::QueueFlags::VIDEO_DECODE_KHR,
        vk::VideoCodecOperationFlagsKHR::DECODE_H265,
    )
    .map(DecodeQueueFamilyIndex);

    Ok(AdapterDecodeSupport {
        extensions: flags,
        decode_queue_family_index,
    })
}

fn query_video_codec_queue_family_index(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    required_queue_flag: vk::QueueFlags,
    required_codec_operation: vk::VideoCodecOperationFlagsKHR,
) -> Option<u32> {
    // SAFETY: We only query immutable queue-family metadata for a valid physical device.
    let queue_count =
        unsafe { instance.get_physical_device_queue_family_properties2_len(physical_device) };
    if queue_count == 0 {
        return None;
    }

    let mut queue_properties2 = vec![vk::QueueFamilyProperties2::default(); queue_count];
    let mut video_properties = vec![vk::QueueFamilyVideoPropertiesKHR::default(); queue_count];
    for (queue_property, video_property) in queue_properties2
        .iter_mut()
        .zip(video_properties.iter_mut())
    {
        *queue_property = queue_property.push_next(video_property);
    }

    // SAFETY: `queue_properties2` and chained `video_properties` live for the duration of the call.
    unsafe {
        instance
            .get_physical_device_queue_family_properties2(physical_device, &mut queue_properties2)
    };

    let queue_family_properties = queue_properties2
        .iter()
        .map(|property| property.queue_family_properties)
        .collect::<Vec<_>>();
    let codec_operations = video_properties
        .iter()
        .map(|property| property.video_codec_operations)
        .collect::<Vec<_>>();
    find_video_codec_queue_family_index(
        &queue_family_properties,
        &codec_operations,
        required_queue_flag,
        required_codec_operation,
    )
}

pub(super) fn find_video_codec_queue_family_index(
    queue_family_properties: &[vk::QueueFamilyProperties],
    codec_operations: &[vk::VideoCodecOperationFlagsKHR],
    required_queue_flag: vk::QueueFlags,
    required_codec_operation: vk::VideoCodecOperationFlagsKHR,
) -> Option<u32> {
    let has_codec_operation_metadata = queue_family_properties
        .iter()
        .zip(codec_operations.iter())
        .any(|(queue_family, codec_operation)| {
            queue_family.queue_count > 0
                && queue_family.queue_flags.contains(required_queue_flag)
                && !codec_operation.is_empty()
        });

    let strict_match = queue_family_properties
        .iter()
        .zip(codec_operations.iter())
        .enumerate()
        .find_map(|(index, (queue_family, codec_operation))| {
            (queue_family.queue_count > 0
                && queue_family.queue_flags.contains(required_queue_flag)
                && codec_operation.contains(required_codec_operation))
            .then(|| u32::try_from(index).ok())
            .flatten()
        });
    if strict_match.is_some() || has_codec_operation_metadata {
        return strict_match;
    }

    queue_family_properties
        .iter()
        .enumerate()
        .find_map(|(index, queue_family)| {
            (queue_family.queue_count > 0 && queue_family.queue_flags.contains(required_queue_flag))
                .then(|| u32::try_from(index).ok())
                .flatten()
        })
}

fn try_initialize_hevc_decode_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: DecodeQueueFamilyIndex,
) -> Result<(), String> {
    let device = create_hevc_decode_device(instance, physical_device, queue_family_index)?;

    // SAFETY: `device` was created above and is no longer used after this point.
    unsafe {
        device.destroy_device(None);
    }
    Ok(())
}

pub(super) fn create_hevc_decode_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: DecodeQueueFamilyIndex,
) -> Result<ash::Device, String> {
    let priorities = [1.0_f32];
    let queue_create_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family_index.0)
        .queue_priorities(&priorities);
    let extension_names = [
        vk::KHR_VIDEO_QUEUE_NAME.as_ptr(),
        vk::KHR_VIDEO_DECODE_QUEUE_NAME.as_ptr(),
        vk::KHR_VIDEO_DECODE_H265_NAME.as_ptr(),
        vk::KHR_SYNCHRONIZATION2_NAME.as_ptr(),
    ];
    let mut synchronization2_features =
        vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true);
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(std::slice::from_ref(&queue_create_info))
        .enabled_extension_names(&extension_names)
        .push_next(&mut synchronization2_features);

    // SAFETY: All pointers referenced by `create_info` live until the call returns.
    unsafe { instance.create_device(physical_device, &create_info, None) }
        .map_err(|err| format!("logical device initialization failed: {err}"))
}
