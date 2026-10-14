//! Video-session memory binding and HEVC session-parameter construction.

use super::*;

pub(super) fn bind_hevc_encode_video_session_memory(
    device: &ash::Device,
    instance: &ash::Instance,
    video_queue_device: &ash::khr::video_queue::Device,
    physical_device: vk::PhysicalDevice,
    video_session: vk::VideoSessionKHR,
    session_memories: &mut Vec<vk::DeviceMemory>,
) -> Result<(), String> {
    let mut requirement_count = 0_u32;
    // SAFETY: first query asks only for requirement count for a valid video session handle.
    let count_result = unsafe {
        (video_queue_device
            .fp()
            .get_video_session_memory_requirements_khr)(
            device.handle(),
            video_session,
            &mut requirement_count,
            std::ptr::null_mut(),
        )
    };
    if count_result != vk::Result::SUCCESS {
        return Err(format!(
            "vkGetVideoSessionMemoryRequirementsKHR count query failed: {count_result:?}"
        ));
    }
    if requirement_count == 0 {
        return Err(
            "vkGetVideoSessionMemoryRequirementsKHR returned no memory requirements".to_string(),
        );
    }

    let mut requirements =
        vec![vk::VideoSessionMemoryRequirementsKHR::default(); requirement_count as usize];
    // SAFETY: `requirements` storage is sized from the count query above.
    let requirements_result = unsafe {
        (video_queue_device
            .fp()
            .get_video_session_memory_requirements_khr)(
            device.handle(),
            video_session,
            &mut requirement_count,
            requirements.as_mut_ptr(),
        )
    };
    if requirements_result != vk::Result::SUCCESS {
        return Err(format!(
            "vkGetVideoSessionMemoryRequirementsKHR query failed: {requirements_result:?}"
        ));
    }

    session_memories.reserve(requirements.len());
    let mut bindings = Vec::with_capacity(requirements.len());
    for requirement in &requirements {
        let memory_requirements = requirement.memory_requirements;
        let memory_type_index = find_memory_type_index(
            physical_device,
            instance,
            memory_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .or_else(|| {
            find_memory_type_index(
                physical_device,
                instance,
                memory_requirements.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
            )
        })
        .ok_or_else(|| {
            format!(
                "no compatible memory type for encode video session bind index {} (bits=0x{:X})",
                requirement.memory_bind_index, memory_requirements.memory_type_bits
            )
        })?;
        let allocation_size = memory_requirements.size.max(1);
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(allocation_size)
            .memory_type_index(memory_type_index);
        // SAFETY: allocation info references only POD values and `device` is valid.
        let memory = unsafe { device.allocate_memory(&allocate_info, None) }.map_err(|err| {
            format!("vkAllocateMemory for encode video session bind failed: {err}")
        })?;
        session_memories.push(memory);
        bindings.push(
            vk::BindVideoSessionMemoryInfoKHR::default()
                .memory_bind_index(requirement.memory_bind_index)
                .memory(memory)
                .memory_offset(0)
                .memory_size(allocation_size),
        );
    }

    // SAFETY: bind infos reference allocations that stay alive for this call.
    let bind_result = unsafe {
        (video_queue_device.fp().bind_video_session_memory_khr)(
            device.handle(),
            video_session,
            u32::try_from(bindings.len())
                .map_err(|_| "encode session binding count exceeds u32 range".to_string())?,
            bindings.as_ptr(),
        )
    };
    if bind_result != vk::Result::SUCCESS {
        return Err(format!(
            "vkBindVideoSessionMemoryKHR failed: {bind_result:?}"
        ));
    }

    Ok(())
}

pub(super) fn create_hevc_encode_video_session_parameters(
    devices: HevcEncodeSessionParameterDevices<'_>,
    config: HevcEncodeSessionParameterCreateConfig,
) -> Result<HevcEncodeSessionParameters, String> {
    let parameter_mode = resolve_hevc_encode_parameter_mode();
    match parameter_mode {
        HevcEncodeParameterMode::Sample
        | HevcEncodeParameterMode::SampleNoAddInfo
        | HevcEncodeParameterMode::SampleVpsOnly
        | HevcEncodeParameterMode::SampleSpsOnly
        | HevcEncodeParameterMode::SampleSpsNoVui
        | HevcEncodeParameterMode::SampleSpsVuiFlagOff
        | HevcEncodeParameterMode::SampleSpsNoVuiFlagOn
        | HevcEncodeParameterMode::SampleSpsNoRps
        | HevcEncodeParameterMode::SampleSpsSafeFlags
        | HevcEncodeParameterMode::SampleSpsLevel
        | HevcEncodeParameterMode::SampleSpsSubLayerOrdering
        | HevcEncodeParameterMode::SampleSpsLevelOrdering
        | HevcEncodeParameterMode::SamplePpsOnly => {
            create_hevc_encode_video_session_parameters_from_sample(devices, config, parameter_mode)
        }
        HevcEncodeParameterMode::EmptyTemplate => {
            let mut encode_h265_session_parameters =
                vk::VideoEncodeH265SessionParametersCreateInfoKHR::default()
                    .max_std_vps_count(1)
                    .max_std_sps_count(1)
                    .max_std_pps_count(1);
            let mut quality_level_info = vk::VideoEncodeQualityLevelInfoKHR::default()
                .quality_level(resolve_hevc_encode_probe_quality_level(
                    config.max_quality_levels,
                ));
            let create_info = vk::VideoSessionParametersCreateInfoKHR::default()
                .video_session(config.video_session)
                .video_session_parameters_template(vk::VideoSessionParametersKHR::null())
                .push_next(&mut encode_h265_session_parameters)
                .push_next(&mut quality_level_info);
            let mut video_session_parameters = vk::VideoSessionParametersKHR::null();
            // SAFETY: `create_info` references stack data alive for this call.
            let result = unsafe {
                (devices
                    .video_queue_device
                    .fp()
                    .create_video_session_parameters_khr)(
                    devices.device.handle(),
                    &create_info,
                    std::ptr::null(),
                    &mut video_session_parameters,
                )
            };
            if result != vk::Result::SUCCESS {
                return Err(format!(
                    "vkCreateVideoSessionParametersKHR failed: {result:?}"
                ));
            }
            Ok(HevcEncodeSessionParameters {
                handle: video_session_parameters,
                parameter_set_ids: HevcEncodeParameterSetIds {
                    vps_id: 0,
                    sps_id: 0,
                    pps_id: 0,
                },
                coded_width: config.coded_width,
                coded_height: config.coded_height,
                pps_init_qp_minus26: 0,
                sample_adaptive_offset_enabled: false,
                sps_temporal_mvp_enabled: false,
                mode: parameter_mode,
                size_mode: HevcEncodeProbeParameterSizeMode::Coded,
                vui_safety_mode: HevcEncodeProbeParameterVuiSafetyMode::Preserve,
                feedback_probe_summary: Some("skipped(mode=empty-template)".to_string()),
            })
        }
    }
}

fn create_hevc_encode_video_session_parameters_from_sample(
    devices: HevcEncodeSessionParameterDevices<'_>,
    config: HevcEncodeSessionParameterCreateConfig,
    parameter_mode: HevcEncodeParameterMode,
) -> Result<HevcEncodeSessionParameters, String> {
    debug_assert!(matches!(
        parameter_mode,
        HevcEncodeParameterMode::Sample
            | HevcEncodeParameterMode::SampleNoAddInfo
            | HevcEncodeParameterMode::SampleVpsOnly
            | HevcEncodeParameterMode::SampleSpsOnly
            | HevcEncodeParameterMode::SampleSpsNoVui
            | HevcEncodeParameterMode::SampleSpsVuiFlagOff
            | HevcEncodeParameterMode::SampleSpsNoVuiFlagOn
            | HevcEncodeParameterMode::SampleSpsNoRps
            | HevcEncodeParameterMode::SampleSpsSafeFlags
            | HevcEncodeParameterMode::SampleSpsLevel
            | HevcEncodeParameterMode::SampleSpsSubLayerOrdering
            | HevcEncodeParameterMode::SampleSpsLevelOrdering
            | HevcEncodeParameterMode::SamplePpsOnly
    ));
    let parameter_sample = load_hevc_encode_probe_parameter_sample()?;
    let mut parameter_sets = extract_hevc_parameter_sets_annexb(&parameter_sample)
        .map_err(|err| format!("failed to extract HEVC parameter sets for encode probe: {err}"))?;
    validate_hevc_encode_probe_parameter_profile(&parameter_sets)?;
    let parameter_vui_safety_mode = resolve_hevc_encode_probe_parameter_vui_safety_mode();
    let effective_parameter_mode = resolve_hevc_encode_parameter_mode_with_vui_safety(
        parameter_mode,
        hevc_encode_probe_parameter_sample_override_path().is_some(),
        parameter_sets.parsed_sps.vui_parameters.is_some(),
        parameter_vui_safety_mode,
    );
    apply_hevc_encode_parameter_mode_sps_overrides(&mut parameter_sets, effective_parameter_mode);
    apply_hevc_encode_capability_sps_overrides(
        &mut parameter_sets,
        config.encode_h265_std_syntax_flags,
    );
    let parameter_size_mode = resolve_hevc_encode_probe_parameter_size_mode();
    let mut std_parameter_storage =
        build_hevc_std_parameter_set_storage(&parameter_sets).map_err(|err| {
            format!("failed to build HEVC std parameter storage for encode probe: {err}")
        })?;
    apply_hevc_encode_parameter_mode_std_overrides(
        &mut std_parameter_storage,
        effective_parameter_mode,
    );
    let (parameter_coded_width, parameter_coded_height) = match parameter_size_mode {
        HevcEncodeProbeParameterSizeMode::Sample => {
            (parameter_sets.coded_width, parameter_sets.coded_height)
        }
        HevcEncodeProbeParameterSizeMode::Coded => {
            std_parameter_storage
                .override_encode_sps_picture_size(config.coded_width, config.coded_height);
            (config.coded_width, config.coded_height)
        }
    };
    let (vps_id, sps_id, pps_id) = std_parameter_storage.encode_parameter_set_ids();
    let pps_init_qp_minus26 = std_parameter_storage.encode_pps_init_qp_minus26();
    let sample_adaptive_offset_enabled = parameter_sets
        .parsed_sps
        .sample_adaptive_offset_enabled_flag;
    let sps_temporal_mvp_enabled = parameter_sets.parsed_sps.sps_temporal_mvp_enabled_flag;
    let encode_h265_session_parameters_add = hevc_encode_parameter_mode_add_info_filter(
        effective_parameter_mode,
    )
    .map(|(include_vps, include_sps, include_pps)| {
        std_parameter_storage.encode_add_info_with_filter(include_vps, include_sps, include_pps)
    });
    let mut encode_h265_session_parameters =
        vk::VideoEncodeH265SessionParametersCreateInfoKHR::default()
            .max_std_vps_count(1)
            .max_std_sps_count(1)
            .max_std_pps_count(1);
    if let Some(add_info) = encode_h265_session_parameters_add.as_ref() {
        encode_h265_session_parameters =
            encode_h265_session_parameters.parameters_add_info(add_info);
    }
    let mut quality_level_info = vk::VideoEncodeQualityLevelInfoKHR::default().quality_level(
        resolve_hevc_encode_probe_quality_level(config.max_quality_levels),
    );
    let create_info = vk::VideoSessionParametersCreateInfoKHR::default()
        .video_session(config.video_session)
        .video_session_parameters_template(vk::VideoSessionParametersKHR::null())
        .push_next(&mut encode_h265_session_parameters)
        .push_next(&mut quality_level_info);
    let mut video_session_parameters = vk::VideoSessionParametersKHR::null();
    // SAFETY: `create_info` references stack data alive for this call.
    let result = unsafe {
        (devices
            .video_queue_device
            .fp()
            .create_video_session_parameters_khr)(
            devices.device.handle(),
            &create_info,
            std::ptr::null(),
            &mut video_session_parameters,
        )
    };
    if result != vk::Result::SUCCESS {
        return Err(format!(
            "vkCreateVideoSessionParametersKHR failed: {result:?}"
        ));
    }
    let feedback_probe_summary = match probe_hevc_encode_parameter_feedback(
        devices.device,
        devices.video_encode_device,
        video_session_parameters,
        HevcEncodeParameterSetIds {
            vps_id,
            sps_id,
            pps_id,
        },
    ) {
        Ok(summary) => Some(summary),
        Err(err) => Some(format!("failed({err})")),
    };
    Ok(HevcEncodeSessionParameters {
        handle: video_session_parameters,
        parameter_set_ids: HevcEncodeParameterSetIds {
            vps_id,
            sps_id,
            pps_id,
        },
        coded_width: parameter_coded_width,
        coded_height: parameter_coded_height,
        pps_init_qp_minus26,
        sample_adaptive_offset_enabled,
        sps_temporal_mvp_enabled,
        mode: effective_parameter_mode,
        size_mode: parameter_size_mode,
        vui_safety_mode: parameter_vui_safety_mode,
        feedback_probe_summary,
    })
}

fn hevc_encode_probe_parameter_sample_override_path() -> Option<String> {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SAMPLE_PATH";
    std::env::var(ENV_VAR)
        .ok()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
}

pub(super) fn resolve_hevc_encode_probe_dst_prefix_bytes() -> u64 {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_DST_PREFIX_BYTES";
    let value = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_dst_prefix_bytes(value.as_deref())
}

pub(super) fn hevc_encode_probe_output_path() -> Option<PathBuf> {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_OUTPUT_PATH";
    std::env::var(ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(super) fn hevc_encode_parameter_header_prefix() -> Result<Vec<u8>, String> {
    let sample = load_hevc_encode_probe_parameter_sample()?;
    let end = leading_hevc_non_vcl_annexb_prefix_len(&sample)?;
    Ok(sample[..end].to_vec())
}

fn leading_hevc_non_vcl_annexb_prefix_len(bytes: &[u8]) -> Result<usize, String> {
    let mut cursor = 0_usize;
    let mut end = 0_usize;
    while let Some((start, prefix_len)) = find_annexb_start_code_with_prefix(bytes, cursor) {
        let nalu_start = start
            .checked_add(prefix_len)
            .ok_or_else(|| "HEVC Annex-B NAL start overflow".to_string())?;
        if nalu_start >= bytes.len() {
            break;
        }
        let next_start = find_annexb_start_code_with_prefix(bytes, nalu_start)
            .map(|(next, _)| next)
            .unwrap_or(bytes.len());
        let nal_type = (bytes[nalu_start] & 0x7e) >> 1;
        if nal_type <= 31 {
            break;
        }
        end = next_start;
        cursor = next_start;
    }
    if end == 0 {
        return Err("HEVC parameter sample has no leading non-VCL Annex-B NAL units".to_string());
    }
    Ok(end)
}

fn find_annexb_start_code_with_prefix(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut index = from;
    while index + 3 <= bytes.len() {
        if bytes[index..].starts_with(&[0, 0, 1]) {
            return Some((index, 3));
        }
        if index + 4 <= bytes.len() && bytes[index..].starts_with(&[0, 0, 0, 1]) {
            return Some((index, 4));
        }
        index += 1;
    }
    None
}

pub(super) fn parse_hevc_encode_probe_dst_prefix_bytes(value: Option<&str>) -> u64 {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

pub(super) fn resolve_hevc_encode_probe_dst_prefix_mode() -> HevcEncodeProbeDstPrefixMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_DST_PREFIX_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_dst_prefix_mode(mode.as_deref())
}

pub(super) fn parse_hevc_encode_probe_dst_prefix_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeDstPrefixMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("zero") | Some("zeros") | Some("0") => HevcEncodeProbeDstPrefixMode::Zero,
        Some("parameter-sample")
        | Some("parameter_sample")
        | Some("sample")
        | Some("ffmpeg-sample")
        | Some("ffmpeg_sample")
        | Some("ffmpeg") => HevcEncodeProbeDstPrefixMode::ParameterSample,
        _ => HevcEncodeProbeDstPrefixMode::None,
    }
}

pub(super) fn hevc_encode_probe_dst_prefix_mode_label(
    mode: HevcEncodeProbeDstPrefixMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeDstPrefixMode::None => "none",
        HevcEncodeProbeDstPrefixMode::Zero => "zero",
        HevcEncodeProbeDstPrefixMode::ParameterSample => "parameter-sample",
    }
}

pub(super) fn resolve_hevc_encode_probe_dst_range_mode() -> HevcEncodeProbeDstRangeMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_DST_RANGE_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_dst_range_mode(mode.as_deref())
}

pub(super) fn parse_hevc_encode_probe_dst_range_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeDstRangeMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("ffmpeg")
        | Some("ffmpeg-reserve-align")
        | Some("ffmpeg_reserve_align")
        | Some("reserve-align")
        | Some("reserve_align") => HevcEncodeProbeDstRangeMode::FfmpegReserveAlign,
        _ => HevcEncodeProbeDstRangeMode::Full,
    }
}

pub(super) fn hevc_encode_probe_dst_range_mode_label(
    mode: HevcEncodeProbeDstRangeMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeDstRangeMode::Full => "full",
        HevcEncodeProbeDstRangeMode::FfmpegReserveAlign => "ffmpeg-reserve-align",
    }
}

pub(super) fn hevc_encode_probe_dst_buffer_range(
    aligned_buffer_range: u64,
    size_alignment: u64,
    mode: HevcEncodeProbeDstRangeMode,
) -> u64 {
    match mode {
        HevcEncodeProbeDstRangeMode::Full => aligned_buffer_range,
        HevcEncodeProbeDstRangeMode::FfmpegReserveAlign => align_up(
            aligned_buffer_range.saturating_sub(size_alignment),
            size_alignment,
        ),
    }
}

pub(super) fn load_hevc_encode_probe_parameter_sample() -> Result<Vec<u8>, String> {
    let override_path = hevc_encode_probe_parameter_sample_override_path();
    load_hevc_encode_probe_parameter_sample_from_path(override_path.as_deref())
}

pub(super) fn load_hevc_encode_probe_parameter_sample_from_path(
    override_path: Option<&str>,
) -> Result<Vec<u8>, String> {
    if let Some(path) = override_path.map(str::trim).filter(|path| !path.is_empty()) {
        return std::fs::read(path).map_err(|err| {
            format!("failed to read HEVC encode probe parameter sample at '{path}': {err}")
        });
    }
    Ok(include_bytes!("../../../../sample-videos/sample-10s.h265").to_vec())
}

fn probe_hevc_encode_parameter_feedback(
    device: &ash::Device,
    video_encode_device: &ash::khr::video_encode_queue::Device,
    video_session_parameters: vk::VideoSessionParametersKHR,
    parameter_set_ids: HevcEncodeParameterSetIds,
) -> Result<String, String> {
    let mut h265_get_info = vk::VideoEncodeH265SessionParametersGetInfoKHR::default()
        .write_std_vps(true)
        .write_std_sps(true)
        .write_std_pps(true)
        .std_vps_id(u32::from(parameter_set_ids.vps_id))
        .std_sps_id(u32::from(parameter_set_ids.sps_id))
        .std_pps_id(u32::from(parameter_set_ids.pps_id));
    let get_info = vk::VideoEncodeSessionParametersGetInfoKHR::default()
        .video_session_parameters(video_session_parameters)
        .push_next(&mut h265_get_info);

    let mut h265_feedback_info = vk::VideoEncodeH265SessionParametersFeedbackInfoKHR::default();
    let mut feedback_info = vk::VideoEncodeSessionParametersFeedbackInfoKHR::default()
        .push_next(&mut h265_feedback_info);
    let mut data_size: usize = 0;
    // SAFETY: all structs are initialized and point to stack memory valid for this call.
    let first_result = unsafe {
        (video_encode_device
            .fp()
            .get_encoded_video_session_parameters_khr)(
            device.handle(),
            &get_info,
            &mut feedback_info,
            &mut data_size,
            std::ptr::null_mut(),
        )
    };
    if first_result != vk::Result::SUCCESS && first_result != vk::Result::INCOMPLETE {
        return Err(format!(
            "vkGetEncodedVideoSessionParametersKHR(size-query) failed: {first_result:?}"
        ));
    }

    let mut data = vec![0_u8; data_size];
    // SAFETY: output buffer is valid for `data_size` bytes and pointers live for the call.
    let second_result = unsafe {
        (video_encode_device
            .fp()
            .get_encoded_video_session_parameters_khr)(
            device.handle(),
            &get_info,
            &mut feedback_info,
            &mut data_size,
            data.as_mut_ptr().cast(),
        )
    };
    if second_result != vk::Result::SUCCESS && second_result != vk::Result::INCOMPLETE {
        return Err(format!(
            "vkGetEncodedVideoSessionParametersKHR(data) failed: {second_result:?}"
        ));
    }
    let used_size = data_size.min(data.len());
    let preview_bytes = data
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("");
    Ok(format!(
        "result={second_result:?}, size={used_size}, has_overrides={}, vps_overrides={}, sps_overrides={}, pps_overrides={}, head16={preview_bytes}",
        feedback_info.has_overrides != 0,
        h265_feedback_info.has_std_vps_overrides != 0,
        h265_feedback_info.has_std_sps_overrides != 0,
        h265_feedback_info.has_std_pps_overrides != 0,
    ))
}

pub(super) fn validate_hevc_encode_probe_parameter_profile(
    parameter_sets: &HevcParameterSets,
) -> Result<(), String> {
    const HEVC_MAIN_PROFILE_IDC: u8 = 1;
    let profile_idc = parameter_sets
        .parsed_sps
        .profile_tier_level
        .general_profile
        .profile_idc;
    if profile_idc == HEVC_MAIN_PROFILE_IDC {
        return Ok(());
    }
    Err(format!(
        "unsupported HEVC encode probe parameter sample profile_idc={profile_idc}; only Main profile (profile_idc=1) is currently supported for Vulkan HEVC encode probe session parameters"
    ))
}
