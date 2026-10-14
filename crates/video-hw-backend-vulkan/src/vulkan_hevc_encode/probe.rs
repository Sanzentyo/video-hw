//! Adapter probing and HEVC encode-session bootstrap orchestration.

use super::*;

pub(super) fn run_hevc_encode_session_bootstrap(
    coded_width: u32,
    coded_height: u32,
    _fps: u32,
    source_nv12_frames: Vec<Vec<u8>>,
) -> Result<HevcEncodeSessionBootstrap, String> {
    if coded_width == 0 || coded_height == 0 {
        return Err(format!(
            "HEVC encode bootstrap dimensions must be > 0, got {}x{}",
            coded_width, coded_height
        ));
    }

    // SAFETY: We only load Vulkan entry points and keep the handle local to this function.
    let entry = unsafe { ash::Entry::load() }
        .map_err(|err| format!("failed to load Vulkan entry: {err}"))?;
    let (instance, validation_debug_state) = create_hevc_encode_probe_instance(&entry)?;
    let maintenance1_mode = resolve_hevc_encode_probe_maintenance1_mode();
    let session_dpb_mode = resolve_hevc_encode_probe_session_dpb_mode();
    let session_h265_create_info_mode = resolve_hevc_encode_probe_session_h265_create_info_mode();

    let bootstrap_result = (|| -> Result<HevcEncodeSessionBootstrap, String> {
        let video_queue = ash::khr::video_queue::Instance::new(&entry, &instance);

        // SAFETY: `instance` is valid in this scope and we only consume opaque handles.
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;
        if physical_devices.is_empty() {
            return Err(
                "no Vulkan physical devices available for HEVC encode bootstrap".to_string(),
            );
        }

        let mut probe_errors = Vec::new();
        let mut selected_candidate: Option<(u32, HevcEncodeCapabilityCandidate)> = None;

        for physical_device in physical_devices {
            let support = query_adapter_encode_support(&instance, physical_device)
                .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
            if !support.extensions.supports_hevc_encode() {
                continue;
            }
            let Some(queue_family_index) = support.encode_queue_family_index else {
                continue;
            };

            let mut encode_h265_profile = vk::VideoEncodeH265ProfileInfoKHR::default()
                .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
            let mut encode_usage = vk::VideoEncodeUsageInfoKHR::default()
                .video_usage_hints(vk::VideoEncodeUsageFlagsKHR::DEFAULT);
            let profile = vk::VideoProfileInfoKHR::default()
                .video_codec_operation(vk::VideoCodecOperationFlagsKHR::ENCODE_H265)
                .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
                .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
                .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
                .push_next(&mut encode_usage)
                .push_next(&mut encode_h265_profile);

            let mut encode_capabilities = vk::VideoEncodeCapabilitiesKHR::default();
            let mut encode_h265_capabilities = vk::VideoEncodeH265CapabilitiesKHR::default();
            let mut capabilities = vk::VideoCapabilitiesKHR::default()
                .push_next(&mut encode_h265_capabilities)
                .push_next(&mut encode_capabilities);

            // SAFETY: All pointers reference stack values alive for the duration of the call.
            let result = unsafe {
                (video_queue.fp().get_physical_device_video_capabilities_khr)(
                    physical_device,
                    &profile,
                    &mut capabilities,
                )
            };
            if result != vk::Result::SUCCESS {
                probe_errors.push(format!("video capabilities query failed: {result:?}"));
                continue;
            }

            if coded_width < capabilities.min_coded_extent.width
                || coded_width > capabilities.max_coded_extent.width
                || coded_height < capabilities.min_coded_extent.height
                || coded_height > capabilities.max_coded_extent.height
            {
                probe_errors.push(format!(
                    "configured coded extent {}x{} is outside device-supported range {}x{}..{}x{}",
                    coded_width,
                    coded_height,
                    capabilities.min_coded_extent.width,
                    capabilities.min_coded_extent.height,
                    capabilities.max_coded_extent.width,
                    capabilities.max_coded_extent.height
                ));
                continue;
            }

            let encode_input_formats = query_hevc_encode_formats(
                &video_queue,
                physical_device,
                profile,
                vk::ImageUsageFlags::VIDEO_ENCODE_SRC_KHR,
            )?;
            if encode_input_formats.is_empty() {
                probe_errors.push(
                    "video format query returned no VIDEO_ENCODE_SRC_KHR candidates".to_string(),
                );
                continue;
            }

            let encode_dpb_formats = query_hevc_encode_formats(
                &video_queue,
                physical_device,
                profile,
                vk::ImageUsageFlags::VIDEO_ENCODE_DPB_KHR,
            )?;
            let min_coded_extent = capabilities.min_coded_extent;
            let max_coded_extent = capabilities.max_coded_extent;
            let picture_access_granularity = capabilities.picture_access_granularity;
            let max_dpb_slots = capabilities.max_dpb_slots;
            let max_active_reference_pictures = capabilities.max_active_reference_pictures;
            let min_bitstream_buffer_offset_alignment =
                capabilities.min_bitstream_buffer_offset_alignment;
            let min_bitstream_buffer_size_alignment =
                capabilities.min_bitstream_buffer_size_alignment;
            let std_header_version = capabilities.std_header_version;
            let rate_control_modes = encode_capabilities.rate_control_modes;
            let max_rate_control_layers = encode_capabilities.max_rate_control_layers;
            let max_bitrate = encode_capabilities.max_bitrate;
            let max_quality_levels = encode_capabilities.max_quality_levels;
            let encode_capability_flags = encode_capabilities.flags;
            let encode_h265_capability_flags = encode_h265_capabilities.flags;
            let encode_h265_std_syntax_flags = encode_h265_capabilities.std_syntax_flags;
            let supported_encode_feedback_flags =
                encode_capabilities.supported_encode_feedback_flags;
            let max_level_idc = encode_h265_capabilities.max_level_idc;
            let encode_input_picture_granularity =
                encode_capabilities.encode_input_picture_granularity;
            let video_maintenance1_extension_available = support.extensions.has_video_maintenance1;
            let video_maintenance1_feature_supported = video_maintenance1_extension_available
                && query_video_maintenance1_feature_support(&instance, physical_device);

            let capability_snapshot = HevcEncodeCapabilitySnapshot {
                min_coded_extent,
                max_coded_extent,
                picture_access_granularity,
                encode_input_picture_granularity,
                max_dpb_slots,
                max_active_reference_pictures,
                encode_capability_flags,
                encode_h265_capability_flags,
                encode_h265_std_syntax_flags,
                rate_control_modes,
                max_rate_control_layers,
                max_bitrate,
                max_quality_levels,
                supported_encode_feedback_flags,
                min_bitstream_buffer_offset_alignment,
                min_bitstream_buffer_size_alignment,
                std_header_version,
                max_level_idc,
                video_maintenance1_extension_available,
                video_maintenance1_feature_supported,
            };

            // SAFETY: `physical_device` was enumerated from `instance`; this is a read-only query.
            let properties = unsafe { instance.get_physical_device_properties(physical_device) };
            let is_discrete = properties.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
            let selection_score = u32::from(is_discrete) * 4
                + u32::from(video_maintenance1_feature_supported) * 2
                + u32::from(video_maintenance1_extension_available);
            let candidate = HevcEncodeCapabilityCandidate {
                physical_device,
                extensions: support.extensions,
                queue_family_index,
                capability_snapshot,
                encode_input_formats,
                encode_dpb_formats,
            };
            match &selected_candidate {
                Some((best_score, _)) if *best_score >= selection_score => {}
                _ => selected_candidate = Some((selection_score, candidate)),
            }
        }

        let candidate = if let Some((_, candidate)) = selected_candidate {
            candidate
        } else if probe_errors.is_empty() {
            return Err("no Vulkan adapter passed HEVC encode bootstrap checks".to_string());
        } else {
            return Err(format!(
                "HEVC encode bootstrap checks failed on all candidate adapters: {}",
                probe_errors.join("; ")
            ));
        };

        let (
            video_session_create_probe,
            video_session_parameters_create_probe,
            encode_submit_execution_probe,
        ) = probe_hevc_encode_video_session_and_parameters_creation(
            &instance,
            &candidate,
            HevcEncodeVideoSessionProbeConfig {
                coded_extent: vk::Extent2D {
                    width: coded_width,
                    height: coded_height,
                },
                maintenance1_mode,
                session_dpb_mode,
                session_h265_create_info_mode,
                source_nv12_frames: &source_nv12_frames,
            },
        );
        // SAFETY: the selected device was enumerated from this live instance.
        let selected_properties =
            unsafe { instance.get_physical_device_properties(candidate.physical_device) };
        let video_maintenance1_feature_enabled =
            resolve_hevc_encode_probe_maintenance1_feature_enabled(
                maintenance1_mode,
                candidate
                    .capability_snapshot
                    .video_maintenance1_extension_available,
                candidate
                    .capability_snapshot
                    .video_maintenance1_feature_supported,
            );

        Ok(HevcEncodeSessionBootstrap {
            coded_width,
            coded_height,
            // SAFETY: Vulkan guarantees `device_name` is a NUL-terminated fixed-size C string.
            adapter_name: unsafe { CStr::from_ptr(selected_properties.device_name.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
            adapter_vendor_id: selected_properties.vendor_id,
            adapter_device_id: selected_properties.device_id,
            adapter_driver_version: selected_properties.driver_version,
            adapter_api_version: selected_properties.api_version,
            min_coded_width: candidate.capability_snapshot.min_coded_extent.width,
            min_coded_height: candidate.capability_snapshot.min_coded_extent.height,
            max_coded_width: candidate.capability_snapshot.max_coded_extent.width,
            max_coded_height: candidate.capability_snapshot.max_coded_extent.height,
            picture_access_granularity_width: candidate
                .capability_snapshot
                .picture_access_granularity
                .width,
            picture_access_granularity_height: candidate
                .capability_snapshot
                .picture_access_granularity
                .height,
            encode_input_granularity_width: candidate
                .capability_snapshot
                .encode_input_picture_granularity
                .width,
            encode_input_granularity_height: candidate
                .capability_snapshot
                .encode_input_picture_granularity
                .height,
            coded_extent_input_granularity_aligned: coded_width.is_multiple_of(
                candidate
                    .capability_snapshot
                    .encode_input_picture_granularity
                    .width
                    .max(1),
            ) && coded_height.is_multiple_of(
                candidate
                    .capability_snapshot
                    .encode_input_picture_granularity
                    .height
                    .max(1),
            ),
            max_dpb_slots: candidate.capability_snapshot.max_dpb_slots,
            max_active_reference_pictures: candidate
                .capability_snapshot
                .max_active_reference_pictures,
            rate_control_modes: candidate.capability_snapshot.rate_control_modes,
            max_rate_control_layers: candidate.capability_snapshot.max_rate_control_layers,
            max_bitrate: candidate.capability_snapshot.max_bitrate,
            max_quality_levels: candidate.capability_snapshot.max_quality_levels,
            encode_capability_flags: candidate.capability_snapshot.encode_capability_flags,
            encode_h265_capability_flags: candidate
                .capability_snapshot
                .encode_h265_capability_flags,
            encode_h265_std_syntax_flags: candidate
                .capability_snapshot
                .encode_h265_std_syntax_flags,
            supported_encode_feedback_flags: candidate
                .capability_snapshot
                .supported_encode_feedback_flags,
            min_bitstream_buffer_offset_alignment: candidate
                .capability_snapshot
                .min_bitstream_buffer_offset_alignment,
            min_bitstream_buffer_size_alignment: candidate
                .capability_snapshot
                .min_bitstream_buffer_size_alignment,
            max_level_idc: candidate.capability_snapshot.max_level_idc as u32,
            video_maintenance1_mode: hevc_encode_probe_maintenance1_mode_label(maintenance1_mode),
            video_maintenance1_extension_available: candidate
                .capability_snapshot
                .video_maintenance1_extension_available,
            video_maintenance1_feature_supported: candidate
                .capability_snapshot
                .video_maintenance1_feature_supported,
            video_maintenance1_feature_enabled,
            encode_input_formats: candidate.encode_input_formats,
            encode_dpb_formats: candidate.encode_dpb_formats,
            video_session_create_probe,
            video_session_parameters_create_probe,
            encode_submit_execution_probe,
        })
    })();

    // SAFETY: `instance` is no longer used after this point.
    unsafe {
        if let Some(debug_state) = validation_debug_state.as_ref() {
            debug_state
                .debug_utils
                .destroy_debug_utils_messenger(debug_state.messenger, None);
        }
        instance.destroy_instance(None);
    }

    bootstrap_result
}

pub(super) fn run_hevc_encode_probe() -> HevcEncodePrerequisiteProbe {
    // SAFETY: We only load function pointers from the Vulkan loader and keep the returned
    // handle within this function.
    let entry = match unsafe { ash::Entry::load() } {
        Ok(entry) => entry,
        Err(err) => {
            return HevcEncodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to load Vulkan entry: {err}"
            ));
        }
    };

    // SAFETY: `vk::InstanceCreateInfo::default()` provides a valid zero-initialized
    // instance create descriptor for ash. We destroy the instance before returning.
    let instance = match unsafe { entry.create_instance(&vk::InstanceCreateInfo::default(), None) }
    {
        Ok(instance) => instance,
        Err(err) => {
            return HevcEncodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to create Vulkan instance: {err}"
            ));
        }
    };

    let probe_result = (|| -> Result<HevcEncodePrerequisiteProbe, String> {
        let maintenance1_mode = resolve_hevc_encode_probe_maintenance1_mode();
        // SAFETY: `instance` is valid for this closure and we only consume opaque handles.
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;
        if physical_devices.is_empty() {
            return Ok(HevcEncodePrerequisiteProbe::NoCompatibleAdapter);
        }

        let mut observed_extensions = ExtensionFlags::default();
        let mut observed_encode_queue = false;
        let mut candidates = Vec::new();

        for physical_device in physical_devices {
            let support = query_adapter_encode_support(&instance, physical_device)
                .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
            observed_extensions.union_assign(support.extensions);
            observed_encode_queue |= support.encode_queue_family_index.is_some();

            if support.extensions.supports_hevc_encode()
                && let Some(queue_family_index) = support.encode_queue_family_index
            {
                // Prefer discrete adapters and those exposing maintenance1.
                // SAFETY: `physical_device` was enumerated from `instance`; query is read-only.
                let properties =
                    unsafe { instance.get_physical_device_properties(physical_device) };
                let is_discrete = properties.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
                let maintenance1_feature_supported = support.extensions.has_video_maintenance1
                    && query_video_maintenance1_feature_support(&instance, physical_device);
                let selection_score = u32::from(is_discrete) * 4
                    + u32::from(maintenance1_feature_supported) * 2
                    + u32::from(support.extensions.has_video_maintenance1);
                candidates.push((
                    selection_score,
                    physical_device,
                    queue_family_index,
                    support.extensions,
                ));
            }
        }

        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
        let mut device_init_errors = Vec::new();
        for (_, physical_device, queue_family_index, extensions) in candidates {
            match try_initialize_hevc_encode_device(
                &instance,
                physical_device,
                queue_family_index,
                extensions,
                maintenance1_mode,
            ) {
                Ok(()) => return Ok(HevcEncodePrerequisiteProbe::Ready),
                Err(err) => device_init_errors.push(err),
            }
        }

        let mut missing = Vec::new();
        if !observed_extensions.has_video_queue {
            missing.push("VK_KHR_video_queue");
        }
        if !observed_extensions.has_video_encode_queue {
            missing.push("VK_KHR_video_encode_queue");
        }
        if !observed_extensions.has_video_encode_h265 {
            missing.push("VK_KHR_video_encode_h265");
        }
        if !missing.is_empty() {
            return Ok(HevcEncodePrerequisiteProbe::MissingExtensions { missing });
        }

        if !observed_encode_queue {
            return Ok(HevcEncodePrerequisiteProbe::MissingEncodeQueueFamily);
        }

        if !device_init_errors.is_empty() {
            return Ok(HevcEncodePrerequisiteProbe::DeviceInitializationFailed(
                device_init_errors.join("; "),
            ));
        }

        Ok(HevcEncodePrerequisiteProbe::NoCompatibleAdapter)
    })();

    // SAFETY: `instance` was created in this function and is no longer used afterwards.
    unsafe {
        instance.destroy_instance(None);
    }

    probe_result.unwrap_or_else(HevcEncodePrerequisiteProbe::ProbeUnavailable)
}

fn query_hevc_encode_formats(
    video_queue: &ash::khr::video_queue::Instance,
    physical_device: vk::PhysicalDevice,
    profile: vk::VideoProfileInfoKHR<'_>,
    image_usage: vk::ImageUsageFlags,
) -> Result<Vec<vk::Format>, String> {
    let profiles = [profile];
    let mut profile_list = vk::VideoProfileListInfoKHR::default().profiles(&profiles);
    let format_info = vk::PhysicalDeviceVideoFormatInfoKHR::default()
        .image_usage(image_usage)
        .push_next(&mut profile_list);

    let mut property_count = 0_u32;
    // SAFETY: First query asks only for the number of format properties.
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
            "video format count query failed for usage {image_usage:?}: {count_result:?}"
        ));
    }
    if property_count == 0 {
        return Ok(Vec::new());
    }

    let mut properties = vec![vk::VideoFormatPropertiesKHR::default(); property_count as usize];
    // SAFETY: `properties` points to writable storage sized by the first query.
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
            "video format properties query failed for usage {image_usage:?}: {properties_result:?}"
        ));
    }

    Ok(properties
        .into_iter()
        .map(|property| property.format)
        .collect())
}

fn probe_hevc_encode_video_session_and_parameters_creation(
    instance: &ash::Instance,
    candidate: &HevcEncodeCapabilityCandidate,
    config: HevcEncodeVideoSessionProbeConfig<'_>,
) -> (
    HevcEncodeVideoSessionCreateProbe,
    HevcEncodeVideoSessionParametersCreateProbe,
    HevcEncodeSubmitExecutionProbe,
) {
    let device = match create_hevc_encode_device(
        instance,
        candidate.physical_device,
        candidate.queue_family_index,
        candidate.extensions,
        config.maintenance1_mode,
        candidate
            .capability_snapshot
            .video_maintenance1_feature_supported,
    ) {
        Ok(device) => device,
        Err(err) => {
            return (
                HevcEncodeVideoSessionCreateProbe::Failed(err),
                HevcEncodeVideoSessionParametersCreateProbe::Skipped(
                    "video session parameters create probe skipped: no video session handle"
                        .to_string(),
                ),
                HevcEncodeSubmitExecutionProbe::Skipped(
                    "encode submit execution probe skipped: no video session handle".to_string(),
                ),
            );
        }
    };

    let probe_result = (|| {
        let mut session_create_errors = Vec::new();
        let mut session_parameters_errors = Vec::new();

        let reference_formats = if candidate.encode_dpb_formats.is_empty() {
            candidate.encode_input_formats.clone()
        } else {
            candidate.encode_dpb_formats.clone()
        };

        for picture_format in candidate.encode_input_formats.iter().copied() {
            for reference_picture_format in reference_formats.iter().copied() {
                match probe_single_hevc_encode_session_format(
                    &device,
                    instance,
                    candidate,
                    HevcEncodeSessionFormatProbeConfig {
                        coded_extent: config.coded_extent,
                        picture_format,
                        reference_picture_format,
                        maintenance1_mode: config.maintenance1_mode,
                        session_dpb_mode: config.session_dpb_mode,
                        session_h265_create_info_mode: config.session_h265_create_info_mode,
                    },
                    config.source_nv12_frames,
                ) {
                    Ok((
                        HevcEncodeVideoSessionParametersCreateProbe::Created,
                        submit_execution_probe,
                    )) => {
                        return (
                            HevcEncodeVideoSessionCreateProbe::Created,
                            HevcEncodeVideoSessionParametersCreateProbe::Created,
                            submit_execution_probe,
                        );
                    }
                    Ok((HevcEncodeVideoSessionParametersCreateProbe::Failed(err), _)) => {
                        session_parameters_errors.push(format!(
                            "picture={picture_format:?}, reference={reference_picture_format:?}: {err}"
                        ));
                    }
                    Ok((HevcEncodeVideoSessionParametersCreateProbe::Skipped(reason), _)) => {
                        session_parameters_errors.push(format!(
                            "picture={picture_format:?}, reference={reference_picture_format:?}: {reason}"
                        ));
                    }
                    Err(err) => {
                        session_create_errors.push(format!(
                            "picture={picture_format:?}, reference={reference_picture_format:?}: {err}"
                        ));
                    }
                }
            }
        }

        if !session_parameters_errors.is_empty() {
            return (
                HevcEncodeVideoSessionCreateProbe::Created,
                HevcEncodeVideoSessionParametersCreateProbe::Failed(format!(
                    "vkCreateVideoSessionParametersKHR failed on all candidate format pairs: {}",
                    session_parameters_errors.join("; ")
                )),
                HevcEncodeSubmitExecutionProbe::Skipped(
                    "encode submit execution probe skipped: session parameters creation failed"
                        .to_string(),
                ),
            );
        }

        let create_details = if session_create_errors.is_empty() {
            "no encode input formats were reported".to_string()
        } else {
            session_create_errors.join("; ")
        };
        (
            HevcEncodeVideoSessionCreateProbe::Failed(format!(
                "vkCreateVideoSessionKHR failed on all candidate format pairs: {create_details}"
            )),
            HevcEncodeVideoSessionParametersCreateProbe::Skipped(
                "video session parameters create probe skipped: video session creation failed"
                    .to_string(),
            ),
            HevcEncodeSubmitExecutionProbe::Skipped(
                "encode submit execution probe skipped: video session creation failed".to_string(),
            ),
        )
    })();

    // SAFETY: `device` is no longer used after this point.
    unsafe {
        device.destroy_device(None);
    }

    probe_result
}

fn probe_single_hevc_encode_session_format(
    device: &ash::Device,
    instance: &ash::Instance,
    candidate: &HevcEncodeCapabilityCandidate,
    config: HevcEncodeSessionFormatProbeConfig,
    source_nv12_frames: &[Vec<u8>],
) -> Result<
    (
        HevcEncodeVideoSessionParametersCreateProbe,
        HevcEncodeSubmitExecutionProbe,
    ),
    String,
> {
    let mut encode_h265_profile = vk::VideoEncodeH265ProfileInfoKHR::default()
        .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
    let mut encode_usage = vk::VideoEncodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoEncodeUsageFlagsKHR::DEFAULT);
    let profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::ENCODE_H265)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut encode_usage)
        .push_next(&mut encode_h265_profile);

    let (session_max_dpb_slots, session_max_active_reference_pictures) =
        resolve_hevc_encode_probe_session_dpb_limits(
            config.session_dpb_mode,
            candidate.capability_snapshot.max_dpb_slots,
            candidate.capability_snapshot.max_active_reference_pictures,
        );
    let create_info = vk::VideoSessionCreateInfoKHR::default()
        .queue_family_index(candidate.queue_family_index.0)
        .video_profile(&profile)
        .picture_format(config.picture_format)
        .max_coded_extent(candidate.capability_snapshot.max_coded_extent)
        .reference_picture_format(config.reference_picture_format)
        .max_dpb_slots(session_max_dpb_slots)
        .max_active_reference_pictures(session_max_active_reference_pictures)
        .std_header_version(&candidate.capability_snapshot.std_header_version);
    let mut encode_h265_session_create = vk::VideoEncodeH265SessionCreateInfoKHR::default()
        .use_max_level_idc(true)
        .max_level_idc(candidate.capability_snapshot.max_level_idc);
    let create_info = match config.session_h265_create_info_mode {
        HevcEncodeProbeSessionH265CreateInfoMode::WithMaxLevel => {
            create_info.push_next(&mut encode_h265_session_create)
        }
        HevcEncodeProbeSessionH265CreateInfoMode::Without => create_info,
    };
    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    let video_encode_device = ash::khr::video_encode_queue::Device::new(instance, device);
    let mut video_session = vk::VideoSessionKHR::null();

    // SAFETY: `create_info` references stack data that stays valid for the call.
    let result = unsafe {
        (video_queue_device.fp().create_video_session_khr)(
            device.handle(),
            &create_info,
            std::ptr::null(),
            &mut video_session,
        )
    };
    if result != vk::Result::SUCCESS {
        return Err(format!("vkCreateVideoSessionKHR failed: {result:?}"));
    }
    let mut session_memories = Vec::new();
    let probe_result = (|| -> Result<
        (
            HevcEncodeVideoSessionParametersCreateProbe,
            HevcEncodeSubmitExecutionProbe,
        ),
        String,
    > {
        bind_hevc_encode_video_session_memory(
            device,
            instance,
            &video_queue_device,
            candidate.physical_device,
            video_session,
            &mut session_memories,
        )?;

        let (session_parameters_probe, submit_execution_probe) =
        match create_hevc_encode_video_session_parameters(
            HevcEncodeSessionParameterDevices {
                device,
                video_queue_device: &video_queue_device,
                video_encode_device: &video_encode_device,
            },
            HevcEncodeSessionParameterCreateConfig {
                video_session,
                coded_width: config.coded_extent.width,
                coded_height: config.coded_extent.height,
                encode_h265_std_syntax_flags: candidate
                    .capability_snapshot
                    .encode_h265_std_syntax_flags,
                max_quality_levels: candidate.capability_snapshot.max_quality_levels,
            },
        ) {
            Ok(session_parameters) => {
                let source_frames = if source_nv12_frames.is_empty() {
                    vec![None]
                } else {
                    source_nv12_frames
                        .iter()
                        .map(|source| Some(source.as_slice()))
                        .collect::<Vec<_>>()
                };
                let mut first_ready_probe = None;
                let mut bitstreams = Vec::new();
                for source_nv12 in source_frames {
                    let submit_execution_probe = probe_hevc_encode_submit_execution(
                        device,
                        instance,
                        HevcEncodeSubmitExecutionConfig {
                            physical_device: candidate.physical_device,
                            queue_family_index: candidate.queue_family_index,
                            video_session,
                            video_session_parameters: session_parameters.handle,
                            parameter_set_ids: session_parameters.parameter_set_ids,
                            parameter_set_coded_width: session_parameters.coded_width,
                            parameter_set_coded_height: session_parameters.coded_height,
                            parameter_set_pps_init_qp_minus26: session_parameters
                                .pps_init_qp_minus26,
                            parameter_set_sample_adaptive_offset_enabled: session_parameters
                                .sample_adaptive_offset_enabled,
                            parameter_set_sps_temporal_mvp_enabled: session_parameters
                                .sps_temporal_mvp_enabled,
                            parameter_mode: session_parameters.mode,
                            parameter_feedback_probe_summary: session_parameters
                                .feedback_probe_summary
                                .clone(),
                            parameter_size_mode: session_parameters.size_mode,
                            parameter_vui_safety_mode: session_parameters.vui_safety_mode,
                            encode_h265_std_syntax_flags: candidate
                                .capability_snapshot
                                .encode_h265_std_syntax_flags,
                            coded_width: config.coded_extent.width,
                            coded_height: config.coded_extent.height,
                            picture_format: config.picture_format,
                            reference_picture_format: config.reference_picture_format,
                            picture_access_granularity: candidate
                                .capability_snapshot
                                .picture_access_granularity,
                            rate_control_modes: candidate.capability_snapshot.rate_control_modes,
                            max_rate_control_layers: candidate
                                .capability_snapshot
                                .max_rate_control_layers,
                            max_quality_levels: candidate.capability_snapshot.max_quality_levels,
                            supported_encode_feedback_flags: candidate
                                .capability_snapshot
                                .supported_encode_feedback_flags,
                            min_bitstream_buffer_offset_alignment: candidate
                                .capability_snapshot
                                .min_bitstream_buffer_offset_alignment,
                            min_bitstream_buffer_size_alignment: candidate
                                .capability_snapshot
                                .min_bitstream_buffer_size_alignment,
                            maintenance1_mode: config.maintenance1_mode,
                            maintenance1_feature_enabled:
                                resolve_hevc_encode_probe_maintenance1_feature_enabled(
                                    config.maintenance1_mode,
                                    candidate
                                        .capability_snapshot
                                        .video_maintenance1_extension_available,
                                    candidate
                                        .capability_snapshot
                                        .video_maintenance1_feature_supported,
                                ),
                            session_dpb_mode: config.session_dpb_mode,
                            session_max_dpb_slots,
                            session_max_active_reference_pictures,
                            session_h265_create_info_mode: config.session_h265_create_info_mode,
                            source_nv12: source_nv12.map(<[u8]>::to_vec),
                        },
                    );
                    if let HevcEncodeSubmitExecutionProbe::Ready { bitstream, .. } =
                        &submit_execution_probe
                    {
                        bitstreams.push(bitstream.clone());
                        if first_ready_probe.is_none() {
                            first_ready_probe = Some(submit_execution_probe);
                        }
                    } else {
                        first_ready_probe = Some(submit_execution_probe);
                        break;
                    }
                }
                let mut submit_execution_probe = first_ready_probe.unwrap_or_else(|| {
                    HevcEncodeSubmitExecutionProbe::Skipped(
                        "encode submit execution probe skipped: no source frames".to_string(),
                    )
                });
                if let HevcEncodeSubmitExecutionProbe::Ready {
                    bitstreams: ready_bitstreams,
                    ..
                } = &mut submit_execution_probe
                {
                    *ready_bitstreams = bitstreams;
                }
                // SAFETY: handle was created from this device and is not reused afterwards.
                unsafe {
                    (video_queue_device.fp().destroy_video_session_parameters_khr)(
                        device.handle(),
                        session_parameters.handle,
                        std::ptr::null(),
                    );
                }
                (
                    HevcEncodeVideoSessionParametersCreateProbe::Created,
                    submit_execution_probe,
                )
            }
            Err(err) => (
                HevcEncodeVideoSessionParametersCreateProbe::Failed(err),
                HevcEncodeSubmitExecutionProbe::Skipped(
                    "encode submit execution probe skipped: session parameters creation failed"
                        .to_string(),
                ),
            ),
        };
        Ok((session_parameters_probe, submit_execution_probe))
    })();

    // SAFETY: session bind allocations were created by this device and are no longer used.
    unsafe {
        for memory in session_memories {
            device.free_memory(memory, None);
        }
    }

    // SAFETY: `video_session` was created from this `device` and is not reused.
    unsafe {
        (video_queue_device.fp().destroy_video_session_khr)(
            device.handle(),
            video_session,
            std::ptr::null(),
        );
    }
    probe_result
}
