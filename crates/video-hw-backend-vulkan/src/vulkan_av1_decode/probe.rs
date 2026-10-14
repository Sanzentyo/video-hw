//! AV1 decode capability probing and bitstream-facing decode entry points.

use super::*;

impl Av1DecodeExtensionFlags {
    pub(super) fn supports_av1_decode(self) -> bool {
        self.has_video_queue && self.has_video_decode_queue && self.has_video_decode_av1
    }

    fn union_assign(&mut self, other: Self) {
        self.has_video_queue |= other.has_video_queue;
        self.has_video_decode_queue |= other.has_video_decode_queue;
        self.has_video_decode_av1 |= other.has_video_decode_av1;
    }
}

pub(crate) fn probe_av1_decode_prerequisites() -> Av1DecodePrerequisiteProbe {
    static CACHE: OnceLock<Av1DecodePrerequisiteProbe> = OnceLock::new();
    CACHE.get_or_init(run_av1_decode_probe).clone()
}

pub(crate) fn inspect_av1_low_overhead_obus(
    bitstream: &[u8],
) -> Result<Av1BitstreamInspection, String> {
    let records = parse_av1_low_overhead_obus(bitstream)?;
    let has_sequence_header = records
        .iter()
        .any(|record| record.obu_type == Av1ObuType::SequenceHeader);
    let has_frame_payload = records.iter().any(|record| {
        matches!(
            record.obu_type,
            Av1ObuType::Frame | Av1ObuType::FrameHeader | Av1ObuType::TileGroup
        )
    });
    let sequence_header_obu_len = records
        .iter()
        .find(|record| record.obu_type == Av1ObuType::SequenceHeader)
        .map(|record| record.obu_range.len());
    let parsed_sequence_header = records
        .iter()
        .find(|record| record.obu_type == Av1ObuType::SequenceHeader)
        .map(|record| parse_av1_sequence_header_payload(&bitstream[record.payload_range.clone()]))
        .transpose()?;
    let _std_sequence_header = parsed_sequence_header
        .as_ref()
        .map(build_av1_std_sequence_header)
        .transpose()?;
    let temporal_unit_count = records
        .iter()
        .map(|record| record.temporal_unit_index)
        .max()
        .map_or(0, |index| index + 1);
    let frame_type_counts = inspect_av1_frame_type_counts(bitstream, &records)?;

    Ok(Av1BitstreamInspection {
        obu_count: records.len(),
        temporal_unit_count,
        has_sequence_header,
        has_frame_payload,
        sequence_header_obu_len,
        frame_header_count: frame_type_counts.frame_header_count,
        key_frame_count: frame_type_counts.key_frame_count,
        inter_frame_count: frame_type_counts.inter_frame_count,
        intra_only_frame_count: frame_type_counts.intra_only_frame_count,
        switch_frame_count: frame_type_counts.switch_frame_count,
        show_existing_frame_count: frame_type_counts.show_existing_frame_count,
        coded_width: parsed_sequence_header
            .as_ref()
            .map(ParsedAv1SequenceHeader::coded_width),
        coded_height: parsed_sequence_header
            .as_ref()
            .map(ParsedAv1SequenceHeader::coded_height),
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Av1FrameTypeCounts {
    pub(super) frame_header_count: usize,
    pub(super) key_frame_count: usize,
    pub(super) inter_frame_count: usize,
    pub(super) intra_only_frame_count: usize,
    pub(super) switch_frame_count: usize,
    pub(super) show_existing_frame_count: usize,
}

pub(super) fn inspect_av1_frame_type_counts(
    bitstream: &[u8],
    records: &[Av1ObuRecord],
) -> Result<Av1FrameTypeCounts, String> {
    records
        .iter()
        .filter(|record| matches!(record.obu_type, Av1ObuType::Frame | Av1ObuType::FrameHeader))
        .try_fold(Av1FrameTypeCounts::default(), |mut counts, record| {
            let payload = bitstream
                .get(record.payload_range.clone())
                .ok_or_else(|| "AV1 frame OBU payload range exceeds bitstream".to_string())?;
            let mut bits = BitReader::new(payload);
            counts.frame_header_count = counts.frame_header_count.saturating_add(1);
            if bits.read_bool("show_existing_frame")? {
                counts.show_existing_frame_count =
                    counts.show_existing_frame_count.saturating_add(1);
                return Ok(counts);
            }
            match bits.read_bits_u8(2, "frame_type")? {
                0 => counts.key_frame_count = counts.key_frame_count.saturating_add(1),
                1 => counts.inter_frame_count = counts.inter_frame_count.saturating_add(1),
                2 => {
                    counts.intra_only_frame_count = counts.intra_only_frame_count.saturating_add(1)
                }
                3 => counts.switch_frame_count = counts.switch_frame_count.saturating_add(1),
                _ => unreachable!("2-bit AV1 frame_type cannot exceed 3"),
            }
            Ok(counts)
        })
}

pub(crate) fn extract_av1_std_sequence_header(
    bitstream: &[u8],
) -> Result<StdVideoAV1SequenceHeader, String> {
    let records = parse_av1_low_overhead_obus(bitstream)?;
    let sequence_header = records
        .iter()
        .find(|record| record.obu_type == Av1ObuType::SequenceHeader)
        .ok_or_else(|| "missing AV1 sequence header OBU".to_string())?;
    let parsed =
        parse_av1_sequence_header_payload(&bitstream[sequence_header.payload_range.clone()])?;
    build_av1_std_sequence_header(&parsed)
}

pub(crate) fn probe_av1_decode_session_parameters_for_bitstream(
    bitstream: &[u8],
) -> Result<Av1DecodeBitstreamSessionProbe, String> {
    let options = Av1DecodeBitstreamSessionProbeOptions::from_env();
    probe_av1_decode_session_parameters_for_bitstream_with_options(bitstream, options)
}

pub(crate) fn decode_av1_bitstream_to_nv12_frames(
    bitstream: &[u8],
) -> Result<Vec<Av1DecodeReadbackFrame>, String> {
    let probe = probe_av1_decode_session_parameters_for_bitstream_with_options(
        bitstream,
        Av1DecodeBitstreamSessionProbeOptions {
            record_command_buffer: true,
            submit_command_buffer: true,
            readback: true,
        },
    )?;
    if probe.picture_format != vk::Format::G8_B8R8_2PLANE_420_UNORM {
        return Err(format!(
            "Vulkan AV1 decode output currently requires G8_B8R8_2PLANE_420_UNORM readback (got {:?})",
            probe.picture_format
        ));
    }
    if probe.readback_sample.len() != probe.readback_mapped_bytes {
        return Err(format!(
            "Vulkan AV1 decode readback did not retain full mapped payload: mapped={}, retained={}",
            probe.readback_mapped_bytes,
            probe.readback_sample.len()
        ));
    }
    let frame_count = usize::try_from(probe.readback_sample_count)
        .map_err(|_| "Vulkan AV1 readback sample count does not fit in usize".to_string())?;
    let sample_stride = usize::try_from(probe.readback_sample_stride)
        .map_err(|_| "Vulkan AV1 readback sample stride does not fit in usize".to_string())?;
    if frame_count == 0
        || sample_stride == 0
        || sample_stride.saturating_mul(frame_count) != probe.readback_mapped_bytes
    {
        return Err(format!(
            "Vulkan AV1 readback payload cannot be split into {frame_count} frames: mapped={}",
            probe.readback_mapped_bytes
        ));
    }
    probe
        .readback_sample
        .chunks_exact(sample_stride)
        .map(|sample| {
            Ok(Av1DecodeReadbackFrame {
                coded_width: probe.coded_width,
                coded_height: probe.coded_height,
                data: sample.to_vec(),
                readback_non_zero: sample.iter().any(|byte| *byte != 0),
            })
        })
        .collect()
}

pub(crate) fn count_av1_display_frames(bitstream: &[u8]) -> Result<usize, String> {
    Ok(av1_display_readback_indices(bitstream)?.len())
}

pub(crate) fn av1_display_readback_indices(bitstream: &[u8]) -> Result<Vec<u32>, String> {
    let records = parse_av1_low_overhead_obus(bitstream)?;
    let sequence_header = records
        .iter()
        .find(|record| record.obu_type == Av1ObuType::SequenceHeader)
        .map(|record| {
            oxideav_av1::parse_sequence_header(&bitstream[record.payload_range.clone()])
                .map_err(|err| format!("oxideav AV1 sequence-header parse failed: {err}"))
        })
        .transpose()?
        .ok_or_else(|| "missing AV1 sequence header OBU".to_string())?;
    let mut dpb = OxideAv1Dpb::new();
    let mut reference_base_array_layers: [Option<u32>; 8] = [None; 8];
    let mut decode_index = 0_u32;
    let mut display_indices = Vec::new();

    for record in records
        .iter()
        .filter(|record| matches!(record.obu_type, Av1ObuType::Frame | Av1ObuType::FrameHeader))
    {
        let payload = bitstream
            .get(record.payload_range.clone())
            .ok_or_else(|| "AV1 frame OBU payload range exceeds bitstream".to_string())?;
        let frame_header = match record.obu_type {
            Av1ObuType::Frame => {
                let (frame_header, _) =
                    parse_frame_obu_with_dpb(&sequence_header, payload, &dpb)
                        .map_err(|err| format!("oxideav AV1 frame OBU parse failed: {err}"))?;
                frame_header
            }
            Av1ObuType::FrameHeader => parse_frame_header_with_dpb(&sequence_header, payload, &dpb)
                .map_err(|err| format!("oxideav AV1 frame-header parse failed: {err}"))?,
            _ => unreachable!("filtered to AV1 frame-bearing OBUs"),
        };

        if frame_header.show_existing_frame {
            let map_idx = usize::from(frame_header.frame_to_show_map_idx);
            let base_array_layer = reference_base_array_layers
                .get(map_idx)
                .copied()
                .flatten()
                .ok_or_else(|| {
                    format!(
                        "AV1 show-existing frame references unavailable frame map index {map_idx}"
                    )
                })?;
            display_indices.push(base_array_layer);
        } else {
            if frame_header.show_frame {
                display_indices.push(decode_index);
            }
            for (ref_index, base_array_layer) in reference_base_array_layers.iter_mut().enumerate()
            {
                if (frame_header.refresh_frame_flags & (1_u8 << ref_index)) != 0 {
                    *base_array_layer = Some(decode_index);
                }
            }
            decode_index = decode_index.checked_add(1).ok_or_else(|| {
                "AV1 decode index overflow while mapping display frames".to_string()
            })?;
        }
        update_oxideav_dpb_from_frame_header(&mut dpb, &frame_header);
    }

    Ok(display_indices)
}

pub(crate) fn submit_av1_bitstream_without_readback(
    bitstream: &[u8],
) -> Result<Av1DecodeBitstreamSessionProbe, String> {
    probe_av1_decode_session_parameters_for_bitstream_with_options(
        bitstream,
        Av1DecodeBitstreamSessionProbeOptions {
            record_command_buffer: true,
            submit_command_buffer: true,
            readback: false,
        },
    )
}

pub(super) fn probe_av1_decode_session_parameters_for_bitstream_with_options(
    bitstream: &[u8],
    options: Av1DecodeBitstreamSessionProbeOptions,
) -> Result<Av1DecodeBitstreamSessionProbe, String> {
    let std_sequence_header = extract_av1_std_sequence_header(bitstream)?;
    let coded_width = u32::from(std_sequence_header.max_frame_width_minus_1) + 1;
    let coded_height = u32::from(std_sequence_header.max_frame_height_minus_1) + 1;

    // SAFETY: Loading the Vulkan loader only initializes function pointers. No raw
    // handles escape this function.
    let entry = unsafe { ash::Entry::load() }
        .map_err(|err| format!("failed to load Vulkan entry: {err}"))?;
    // SAFETY: Ash's default instance create info is valid for probing. The instance
    // is destroyed before returning.
    let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default(), None) }
        .map_err(|err| format!("failed to create Vulkan instance: {err}"))?;

    let result = probe_av1_decode_session_parameters_for_bitstream_with_instance(
        &entry,
        &instance,
        bitstream,
        &std_sequence_header,
        coded_width,
        coded_height,
        options,
    );

    // SAFETY: The instance was created in this function and is not used afterwards.
    unsafe {
        instance.destroy_instance(None);
    }
    result
}

impl Av1DecodeBitstreamSessionProbeOptions {
    fn from_env() -> Self {
        let record_command_buffer =
            std::env::var("VIDEO_HW_VULKAN_AV1_RECORD_COMMAND_BUFFER").as_deref() == Ok("1");
        let submit_command_buffer = record_command_buffer
            && std::env::var("VIDEO_HW_VULKAN_AV1_SUBMIT_COMMAND_BUFFER").as_deref() == Ok("1");
        let readback = std::env::var("VIDEO_HW_VULKAN_AV1_READBACK").as_deref() == Ok("1");
        Self {
            record_command_buffer,
            submit_command_buffer,
            readback,
        }
    }
}

pub(super) fn run_av1_decode_probe() -> Av1DecodePrerequisiteProbe {
    // SAFETY: Loading the Vulkan loader only initializes function pointers. No raw
    // handles escape this function.
    let entry = match unsafe { ash::Entry::load() } {
        Ok(entry) => entry,
        Err(err) => {
            return Av1DecodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to load Vulkan entry: {err}"
            ));
        }
    };

    // SAFETY: Ash's default instance create info is valid for probing. The instance
    // is destroyed before returning.
    let instance = match unsafe { entry.create_instance(&vk::InstanceCreateInfo::default(), None) }
    {
        Ok(instance) => instance,
        Err(err) => {
            return Av1DecodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to create Vulkan instance: {err}"
            ));
        }
    };

    let probe_result = (|| -> Result<Av1DecodePrerequisiteProbe, String> {
        // SAFETY: We only query physical-device handles owned by this instance.
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;
        if physical_devices.is_empty() {
            return Ok(Av1DecodePrerequisiteProbe::NoCompatibleAdapter);
        }

        let mut observed_extensions = Av1DecodeExtensionFlags::default();
        let mut observed_decode_queue = false;
        let mut device_init_errors = Vec::new();

        for physical_device in physical_devices {
            let support = query_av1_adapter_decode_support(&instance, physical_device)
                .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
            observed_extensions.union_assign(support.extensions);
            observed_decode_queue |= support.decode_queue_family_index.is_some();

            if support.extensions.supports_av1_decode()
                && let Some(queue_family_index) = support.decode_queue_family_index
            {
                let snapshot = match query_av1_decode_capability_snapshot(
                    &entry,
                    &instance,
                    physical_device,
                ) {
                    Ok(snapshot) => {
                        if let Err(err) = validate_av1_decode_capability_snapshot(&snapshot) {
                            device_init_errors.push(err);
                            continue;
                        }
                        snapshot
                    }
                    Err(err) => {
                        device_init_errors.push(err);
                        continue;
                    }
                };
                match probe_av1_decode_session_parameters(
                    &instance,
                    physical_device,
                    queue_family_index,
                    &snapshot,
                ) {
                    Ok(()) => return Ok(Av1DecodePrerequisiteProbe::Ready),
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
        if !observed_extensions.has_video_decode_av1 {
            missing.push("VK_KHR_video_decode_av1");
        }
        if !missing.is_empty() {
            return Ok(Av1DecodePrerequisiteProbe::MissingExtensions { missing });
        }
        if !observed_decode_queue {
            return Ok(Av1DecodePrerequisiteProbe::MissingDecodeQueueFamily);
        }
        if !device_init_errors.is_empty() {
            return Ok(Av1DecodePrerequisiteProbe::SessionBootstrapFailed(
                device_init_errors.join("; "),
            ));
        }

        Ok(Av1DecodePrerequisiteProbe::NoCompatibleAdapter)
    })();

    // SAFETY: The instance was created in this function and is not used afterwards.
    unsafe {
        instance.destroy_instance(None);
    }

    probe_result.unwrap_or_else(Av1DecodePrerequisiteProbe::ProbeUnavailable)
}

pub(super) fn query_av1_adapter_decode_support(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Result<Av1AdapterDecodeSupport, vk::Result> {
    // SAFETY: `physical_device` came from this instance.
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical_device) }?;
    let mut flags = Av1DecodeExtensionFlags::default();
    for extension in extensions {
        // SAFETY: Vulkan extension names are fixed-size, null-terminated C strings.
        let name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        flags.has_video_queue |= name == vk::KHR_VIDEO_QUEUE_NAME;
        flags.has_video_decode_queue |= name == vk::KHR_VIDEO_DECODE_QUEUE_NAME;
        flags.has_video_decode_av1 |= name == vk::KHR_VIDEO_DECODE_AV1_NAME;
    }

    let decode_queue_family_index = query_video_codec_queue_family_index(
        instance,
        physical_device,
        vk::QueueFlags::VIDEO_DECODE_KHR,
        vk::VideoCodecOperationFlagsKHR::DECODE_AV1,
    );
    let decode_transfer_queue_family_index = query_video_codec_queue_family_index(
        instance,
        physical_device,
        vk::QueueFlags::VIDEO_DECODE_KHR | vk::QueueFlags::TRANSFER,
        vk::VideoCodecOperationFlagsKHR::DECODE_AV1,
    );

    Ok(Av1AdapterDecodeSupport {
        extensions: flags,
        decode_queue_family_index,
        decode_transfer_queue_family_index,
    })
}

pub(super) fn query_av1_decode_capability_snapshot(
    entry: &ash::Entry,
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Result<Av1DecodeCapabilitySnapshot, String> {
    let video_queue = ash::khr::video_queue::Instance::new(entry, instance);
    let mut decode_av1_profile = vk::VideoDecodeAV1ProfileInfoKHR::default()
        .std_profile(StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN)
        .film_grain_support(false);
    let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
    let profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_AV1)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut decode_av1_profile)
        .push_next(&mut decode_usage);

    let mut decode_capabilities = vk::VideoDecodeCapabilitiesKHR::default();
    let mut decode_av1_capabilities = vk::VideoDecodeAV1CapabilitiesKHR::default();
    let mut capabilities = vk::VideoCapabilitiesKHR::default()
        .push_next(&mut decode_av1_capabilities)
        .push_next(&mut decode_capabilities);

    // SAFETY: All chained structs are stack-allocated and live for the call.
    let result = unsafe {
        (video_queue.fp().get_physical_device_video_capabilities_khr)(
            physical_device,
            &profile,
            &mut capabilities,
        )
    };
    if result != vk::Result::SUCCESS {
        return Err(format!(
            "AV1 decode video capabilities query failed: {result:?}"
        ));
    }

    let decode_output_formats =
        query_av1_decode_output_formats(&video_queue, physical_device, profile)?;
    let min_coded_width = capabilities.min_coded_extent.width;
    let min_coded_height = capabilities.min_coded_extent.height;
    let max_coded_width = capabilities.max_coded_extent.width;
    let max_coded_height = capabilities.max_coded_extent.height;
    let min_bitstream_buffer_offset_alignment = capabilities.min_bitstream_buffer_offset_alignment;
    let min_bitstream_buffer_size_alignment = capabilities.min_bitstream_buffer_size_alignment;
    let max_dpb_slots = capabilities.max_dpb_slots;
    let max_active_reference_pictures = capabilities.max_active_reference_pictures;
    let std_header_version = capabilities.std_header_version;
    let max_level = decode_av1_capabilities.max_level;

    Ok(Av1DecodeCapabilitySnapshot {
        min_coded_width,
        min_coded_height,
        max_coded_width,
        max_coded_height,
        min_bitstream_buffer_offset_alignment,
        min_bitstream_buffer_size_alignment,
        max_dpb_slots,
        max_active_reference_pictures,
        max_level,
        std_header_version,
        decode_output_formats,
    })
}

pub(super) fn validate_av1_decode_capability_snapshot(
    snapshot: &Av1DecodeCapabilitySnapshot,
) -> Result<(), String> {
    if snapshot.decode_output_formats.is_empty() {
        return Err("AV1 decode capability query returned no output formats".to_string());
    }
    if snapshot.max_coded_width < snapshot.min_coded_width
        || snapshot.max_coded_height < snapshot.min_coded_height
    {
        return Err(format!(
            "AV1 decode capability query returned invalid coded extent range: min={}x{}, max={}x{}",
            snapshot.min_coded_width,
            snapshot.min_coded_height,
            snapshot.max_coded_width,
            snapshot.max_coded_height
        ));
    }
    if snapshot.min_coded_width == 0 || snapshot.min_coded_height == 0 {
        return Err(format!(
            "AV1 decode capability query returned zero minimum coded extent: {}x{}",
            snapshot.min_coded_width, snapshot.min_coded_height
        ));
    }
    if snapshot.max_dpb_slots == 0 {
        return Err("AV1 decode capability query returned max_dpb_slots=0".to_string());
    }
    if snapshot.max_active_reference_pictures > snapshot.max_dpb_slots {
        return Err(format!(
            "AV1 decode capability query returned max_active_reference_pictures={} greater than max_dpb_slots={}",
            snapshot.max_active_reference_pictures, snapshot.max_dpb_slots
        ));
    }
    let _max_level = snapshot.max_level;
    let _std_header_version = snapshot.std_header_version;
    Ok(())
}

pub(super) fn query_av1_decode_output_formats(
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
    // SAFETY: Count query only writes the property count.
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
        return Err(format!(
            "AV1 decode video format count query failed: {count_result:?}"
        ));
    }
    if property_count == 0 {
        return Ok(Vec::new());
    }

    let mut properties = vec![vk::VideoFormatPropertiesKHR::default(); property_count as usize];
    // SAFETY: `properties` has capacity for the count returned above.
    let query_result = unsafe {
        (video_queue
            .fp()
            .get_physical_device_video_format_properties_khr)(
            physical_device,
            &format_info,
            &mut property_count,
            properties.as_mut_ptr(),
        )
    };
    if query_result != vk::Result::SUCCESS {
        return Err(format!(
            "AV1 decode video format query failed: {query_result:?}"
        ));
    }

    properties.truncate(property_count as usize);
    Ok(properties
        .into_iter()
        .map(|property| property.format)
        .collect())
}

pub(super) fn query_video_codec_queue_family_index(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    required_queue_flag: vk::QueueFlags,
    required_codec_operation: vk::VideoCodecOperationFlagsKHR,
) -> Option<u32> {
    // SAFETY: This reads immutable queue-family metadata for a valid physical device.
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

    // SAFETY: The pNext chains above live for the duration of this call.
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
