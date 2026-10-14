//! HEVC decode capability/session bootstrap orchestration and cached probe entry points.

use super::*;

/// Safe wrapper around Vulkan extension probing used by higher-level backend code.
///
/// All Vulkan FFI (`unsafe`) is confined to this module so callers in `video-hw`
/// only interact with a plain Rust enum and do not need to reason about raw Vulkan
/// object lifetimes, pointers, or destruction ordering.
pub(crate) fn probe_hevc_decode_prerequisites() -> HevcDecodePrerequisiteProbe {
    static CACHE: OnceLock<HevcDecodePrerequisiteProbe> = OnceLock::new();
    CACHE.get_or_init(run_hevc_decode_probe).clone()
}

pub(crate) fn extract_hevc_parameter_sets_annexb(
    bitstream: &[u8],
) -> Result<HevcParameterSets, String> {
    let nalus = split_annexb_nalus(bitstream);
    let mut vps = None;
    let mut sps = None;
    let mut pps = None;
    let mut first_vcl_nalus = Vec::new();

    for nalu in nalus {
        if hevc_nal_type_raw(nalu).is_some_and(|nal_type| nal_type <= 31)
            && first_vcl_nalus.len() < 8
        {
            first_vcl_nalus.push(nalu.to_vec());
        }
        let Some(nal_type) = hevc_nal_type(nalu) else {
            continue;
        };
        match nal_type {
            NALUnitType::VpsNut if vps.is_none() => vps = Some(nalu.to_vec()),
            NALUnitType::SpsNut if sps.is_none() => sps = Some(nalu.to_vec()),
            NALUnitType::PpsNut if pps.is_none() => pps = Some(nalu.to_vec()),
            _ => {}
        }
    }

    let vps = vps.ok_or_else(|| "missing VPS in HEVC Annex-B bitstream".to_string())?;
    let sps = sps.ok_or_else(|| "missing SPS in HEVC Annex-B bitstream".to_string())?;
    let pps = pps.ok_or_else(|| "missing PPS in HEVC Annex-B bitstream".to_string())?;

    let parsed_sps = SpsNALUnit::parse(Cursor::new(&sps))
        .map_err(|err| format!("failed to parse SPS: {err}"))?;
    let parsed_sps_rbsp = parsed_sps.rbsp;
    let coded_width = u32::try_from(parsed_sps_rbsp.cropped_width())
        .map_err(|_| "SPS cropped_width does not fit into u32".to_string())?;
    let coded_height = u32::try_from(parsed_sps_rbsp.cropped_height())
        .map_err(|_| "SPS cropped_height does not fit into u32".to_string())?;

    Ok(HevcParameterSets {
        vps,
        sps,
        pps,
        first_vcl_nalus,
        parsed_sps: parsed_sps_rbsp,
        coded_width,
        coded_height,
    })
}

pub(crate) fn extract_hevc_access_unit_headers(
    bitstream: &[u8],
) -> Result<Vec<HevcAccessUnitHeader>, String> {
    let parameter_sets = extract_hevc_parameter_sets_annexb(bitstream)?;
    let parsed_pps = parse_hevc_pps(&parameter_sets.pps)?;
    let mut unwrapper =
        HevcPocUnwrapper::new(parameter_sets.parsed_sps.log2_max_pic_order_cnt_lsb_minus4);
    let mut headers = Vec::new();

    for nalu in split_annexb_nalus(bitstream) {
        let Some(nal_unit_type) = hevc_nal_type_raw(nalu) else {
            continue;
        };
        if nal_unit_type > 31 {
            continue;
        }
        let slice_header = parse_hevc_slice_header(nalu, &parsed_pps, &parameter_sets.parsed_sps)
            .map_err(|err| {
            format!("failed to parse HEVC slice header while extracting access units: {err}")
        })?;
        if slice_header.is_first_slice_segment {
            let temporal_id = hevc_nalu_temporal_id(nalu);
            let poc_full =
                unwrapper.advance(slice_header.pic_order_cnt_lsb, nal_unit_type, temporal_id);
            headers.push(HevcAccessUnitHeader {
                nal_unit_type: slice_header.nal_unit_type,
                pps_id: slice_header.pps_id,
                pic_order_cnt_lsb: slice_header.pic_order_cnt_lsb,
                temporal_id,
                poc_full,
            });
        }
    }

    if headers.is_empty() {
        return Err(
            "HEVC bitstream does not contain any access-unit-leading VCL NAL units".to_string(),
        );
    }

    Ok(headers)
}

#[cfg(test)]
pub(crate) fn estimate_hevc_access_unit_count(bitstream: &[u8]) -> Result<usize, String> {
    Ok(extract_hevc_access_unit_headers(bitstream)?.len())
}

#[cfg(test)]
pub(crate) fn probe_hevc_decode_session_bootstrap(
    bitstream: &[u8],
) -> Result<HevcDecodeSessionBootstrap, String> {
    probe_hevc_decode_session_bootstrap_with_access_unit_limit_and_physical_device_index(
        bitstream, None, None,
    )
}

pub(crate) fn probe_hevc_decode_session_bootstrap_with_access_unit_limit_and_physical_device_index(
    bitstream: &[u8],
    submit_probe_access_unit_limit: Option<usize>,
    physical_device_index: Option<usize>,
) -> Result<HevcDecodeSessionBootstrap, String> {
    let cache_key = hevc_decode_bootstrap_cache_key(
        bitstream,
        submit_probe_access_unit_limit,
        physical_device_index,
    );
    if let Some(cached) = lookup_hevc_decode_bootstrap_cache(cache_key) {
        return Ok(cached);
    }

    let machine = HevcSessionBootstrapMachine::<AwaitingCapabilityProbe>::parse(bitstream)?;
    let submit_probe_access_unit_limit = submit_probe_access_unit_limit
        .unwrap_or(MAX_HEVC_SUBMIT_PROBE_ACCESS_UNITS)
        .max(1);

    // SAFETY: We only load Vulkan entry points and keep the handle local to this function.
    let entry = unsafe { ash::Entry::load() }
        .map_err(|err| format!("failed to load Vulkan entry: {err}"))?;

    // SAFETY: Default create info is valid for instance creation. We destroy the instance
    // before returning and no raw handle escapes this function.
    let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default(), None) }
        .map_err(|err| format!("failed to create Vulkan instance: {err}"))?;

    let bootstrap_result = (|| -> Result<HevcDecodeSessionBootstrap, String> {
        let machine = machine.probe_capabilities(&entry, &instance, physical_device_index)?;
        let (
            session_probe,
            session_parameters_probe,
            submit_skeleton_probe,
            submit_execution_probe,
        ) = machine.probe_video_session_and_parameters_creation(
            &instance,
            bitstream,
            submit_probe_access_unit_limit,
        );
        Ok(machine.into_bootstrap(
            session_probe,
            session_parameters_probe,
            submit_skeleton_probe,
            submit_execution_probe,
        ))
    })();

    // SAFETY: `instance` was created in this function and is not used after this point.
    unsafe {
        instance.destroy_instance(None);
    }

    if let Ok(bootstrap) = bootstrap_result.as_ref() {
        store_hevc_decode_bootstrap_cache(cache_key, bootstrap.clone());
    }

    bootstrap_result
}

pub(super) fn hevc_decode_bootstrap_cache_key(
    bitstream: &[u8],
    submit_probe_access_unit_limit: Option<usize>,
    physical_device_index: Option<usize>,
) -> HevcDecodeBootstrapCacheKey {
    let mut hasher = DefaultHasher::new();
    bitstream.hash(&mut hasher);
    HevcDecodeBootstrapCacheKey {
        bitstream_hash: hasher.finish(),
        bitstream_len: bitstream.len(),
        submit_probe_access_unit_limit,
        physical_device_index,
    }
}

pub(super) fn hevc_decode_bootstrap_cache()
-> &'static Mutex<HashMap<HevcDecodeBootstrapCacheKey, HevcDecodeSessionBootstrap>> {
    static CACHE: OnceLock<
        Mutex<HashMap<HevcDecodeBootstrapCacheKey, HevcDecodeSessionBootstrap>>,
    > = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn lookup_hevc_decode_bootstrap_cache(
    key: HevcDecodeBootstrapCacheKey,
) -> Option<HevcDecodeSessionBootstrap> {
    hevc_decode_bootstrap_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&key).cloned())
}

pub(super) fn store_hevc_decode_bootstrap_cache(
    key: HevcDecodeBootstrapCacheKey,
    bootstrap: HevcDecodeSessionBootstrap,
) {
    if let Ok(mut cache) = hevc_decode_bootstrap_cache().lock() {
        if cache.len() >= HEVC_DECODE_BOOTSTRAP_CACHE_CAPACITY && !cache.contains_key(&key) {
            cache.clear();
        }
        cache.insert(key, bootstrap);
    }
}

impl HevcSessionBootstrapMachine<AwaitingCapabilityProbe> {
    fn parse(bitstream: &[u8]) -> Result<Self, String> {
        let parsed = ParsedHevcParameterSets(extract_hevc_parameter_sets_annexb(bitstream)?);
        Ok(Self {
            parsed,
            state: AwaitingCapabilityProbe,
            _marker: PhantomData,
        })
    }

    fn probe_capabilities(
        self,
        entry: &ash::Entry,
        instance: &ash::Instance,
        requested_physical_device_index: Option<usize>,
    ) -> Result<HevcSessionBootstrapMachine<CapabilityProbeComplete>, String> {
        let video_queue = ash::khr::video_queue::Instance::new(entry, instance);

        // SAFETY: `instance` is valid here; we only enumerate physical device handles.
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;
        if physical_devices.is_empty() {
            return Err("no Vulkan physical devices available for HEVC bootstrap".to_string());
        }

        let mut probe_errors = Vec::new();
        let mut selected_candidate: Option<(u32, CapabilityProbeComplete)> = None;
        for (physical_device_index, physical_device) in physical_devices.into_iter().enumerate() {
            if let Some(requested_index) = requested_physical_device_index
                && physical_device_index != requested_index
            {
                continue;
            }
            let support = query_adapter_decode_support(instance, physical_device)
                .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
            if !support.extensions.supports_hevc_decode() {
                continue;
            }
            let Some(queue_family_index) = support.decode_queue_family_index else {
                continue;
            };

            let mut decode_h265_profile = vk::VideoDecodeH265ProfileInfoKHR::default()
                .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
            let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
                .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
            let profile = vk::VideoProfileInfoKHR::default()
                .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_H265)
                .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
                .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
                .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
                .push_next(&mut decode_h265_profile)
                .push_next(&mut decode_usage);

            let mut decode_capabilities = vk::VideoDecodeCapabilitiesKHR::default();
            let mut decode_h265_capabilities = vk::VideoDecodeH265CapabilitiesKHR::default();
            let mut capabilities = vk::VideoCapabilitiesKHR::default()
                .push_next(&mut decode_h265_capabilities)
                .push_next(&mut decode_capabilities);

            // SAFETY: All pointers passed into the Vulkan call refer to stack-allocated
            // structs that live for the duration of the call.
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

            let coded_width = self.parsed.0.coded_width;
            let coded_height = self.parsed.0.coded_height;
            if coded_width < capabilities.min_coded_extent.width
                || coded_width > capabilities.max_coded_extent.width
                || coded_height < capabilities.min_coded_extent.height
                || coded_height > capabilities.max_coded_extent.height
            {
                probe_errors.push(format!(
                    "SPS coded extent {}x{} is outside device-supported range {}x{}..{}x{}",
                    coded_width,
                    coded_height,
                    capabilities.min_coded_extent.width,
                    capabilities.min_coded_extent.height,
                    capabilities.max_coded_extent.width,
                    capabilities.max_coded_extent.height
                ));
                continue;
            }

            let decode_output_formats =
                query_hevc_decode_output_formats(&video_queue, physical_device, profile)?;
            let min_coded_extent = capabilities.min_coded_extent;
            let max_coded_extent = capabilities.max_coded_extent;
            let max_dpb_slots = capabilities.max_dpb_slots;
            let max_active_reference_pictures = capabilities.max_active_reference_pictures;
            let std_header_version = capabilities.std_header_version;
            let min_bitstream_buffer_offset_alignment =
                capabilities.min_bitstream_buffer_offset_alignment;
            let min_bitstream_buffer_size_alignment =
                capabilities.min_bitstream_buffer_size_alignment;
            let picture_access_granularity = capabilities.picture_access_granularity;
            // Prefer discrete adapters and those exposing VK_KHR_video_maintenance1.
            // On mixed Intel/NVIDIA systems this avoids picking an iGPU path that can
            // satisfy capability probes but fails later in session execution.
            // SAFETY: `physical_device` was enumerated from `instance`; this is a read-only query.
            let properties = unsafe { instance.get_physical_device_properties(physical_device) };
            let is_discrete = properties.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
            let capability_snapshot = HevcCapabilitySnapshot {
                min_bitstream_buffer_offset_alignment,
                min_bitstream_buffer_size_alignment,
                picture_access_granularity,
                min_coded_extent,
                max_coded_extent,
                max_dpb_slots,
                max_active_reference_pictures,
                max_level_idc: decode_h265_capabilities.max_level_idc,
                std_header_version,
            };
            let selection_score =
                u32::from(is_discrete) * 2 + u32::from(support.extensions.has_video_maintenance1);
            let candidate = CapabilityProbeComplete {
                physical_device,
                queue_family_index,
                capability_snapshot,
                decode_output_formats,
            };
            match &selected_candidate {
                Some((best_score, _)) if *best_score >= selection_score => {}
                _ => selected_candidate = Some((selection_score, candidate)),
            }
        }

        if let Some((_, state)) = selected_candidate {
            return Ok(HevcSessionBootstrapMachine {
                parsed: self.parsed,
                state,
                _marker: PhantomData,
            });
        }

        if probe_errors.is_empty() {
            match requested_physical_device_index {
                Some(index) => Err(format!(
                    "Vulkan physical device index {index} did not pass HEVC decode bootstrap checks"
                )),
                None => Err("no Vulkan adapter passed HEVC decode bootstrap checks".to_string()),
            }
        } else {
            Err(format!(
                "HEVC decode bootstrap checks failed on all candidate adapters: {}",
                probe_errors.join("; ")
            ))
        }
    }
}

impl HevcSessionBootstrapMachine<CapabilityProbeComplete> {
    fn into_bootstrap(
        self,
        video_session_create_probe: HevcVideoSessionCreateProbe,
        video_session_parameters_create_probe: HevcVideoSessionParametersCreateProbe,
        decode_submit_skeleton_probe: HevcDecodeSubmitSkeletonProbe,
        decode_submit_execution_probe: HevcDecodeSubmitExecutionProbe,
    ) -> HevcDecodeSessionBootstrap {
        HevcDecodeSessionBootstrap {
            coded_width: self.parsed.0.coded_width,
            coded_height: self.parsed.0.coded_height,
            min_coded_width: self.state.capability_snapshot.min_coded_extent.width,
            min_coded_height: self.state.capability_snapshot.min_coded_extent.height,
            max_coded_width: self.state.capability_snapshot.max_coded_extent.width,
            max_coded_height: self.state.capability_snapshot.max_coded_extent.height,
            max_dpb_slots: self.state.capability_snapshot.max_dpb_slots,
            max_active_reference_pictures: self
                .state
                .capability_snapshot
                .max_active_reference_pictures,
            max_level_idc: self.state.capability_snapshot.max_level_idc,
            decode_output_formats: self.state.decode_output_formats,
            video_session_create_probe,
            video_session_parameters_create_probe,
            decode_submit_skeleton_probe,
            decode_submit_execution_probe,
        }
    }

    fn probe_video_session_and_parameters_creation(
        &self,
        instance: &ash::Instance,
        bitstream: &[u8],
        submit_probe_access_unit_limit: usize,
    ) -> (
        HevcVideoSessionCreateProbe,
        HevcVideoSessionParametersCreateProbe,
        HevcDecodeSubmitSkeletonProbe,
        HevcDecodeSubmitExecutionProbe,
    ) {
        if self.state.decode_output_formats.is_empty() {
            return (
                HevcVideoSessionCreateProbe::Failed(
                    "video session create probe skipped: no decode output format reported"
                        .to_string(),
                ),
                HevcVideoSessionParametersCreateProbe::Skipped(
                    "video session parameters create probe skipped: no video session handle"
                        .to_string(),
                ),
                HevcDecodeSubmitSkeletonProbe::Skipped(
                    "decode submit skeleton skipped: no video session handle".to_string(),
                ),
                HevcDecodeSubmitExecutionProbe::Skipped(
                    "decode submit execution probe skipped: no video session handle".to_string(),
                ),
            );
        }

        let device = match create_hevc_decode_device(
            instance,
            self.state.physical_device,
            self.state.queue_family_index,
        ) {
            Ok(device) => device,
            Err(err) => {
                return (
                    HevcVideoSessionCreateProbe::Failed(err),
                    HevcVideoSessionParametersCreateProbe::Skipped(
                        "video session parameters create probe skipped: no video session handle"
                            .to_string(),
                    ),
                    HevcDecodeSubmitSkeletonProbe::Skipped(
                        "decode submit skeleton skipped: no video session handle".to_string(),
                    ),
                    HevcDecodeSubmitExecutionProbe::Skipped(
                        "decode submit execution probe skipped: no video session handle"
                            .to_string(),
                    ),
                );
            }
        };

        let probe_result = (|| {
            let mut session_create_errors = Vec::new();
            let mut session_parameters_errors = Vec::new();

            for picture_format in self.state.decode_output_formats.iter().copied() {
                match self.probe_single_video_session_format(&device, instance, picture_format) {
                    Ok(HevcVideoSessionParametersCreateProbe::Created) => {
                        let submit_skeleton_probe = build_hevc_decode_submit_skeleton_probe(
                            &self.parsed.0,
                            &self.state.capability_snapshot,
                        );
                        let submit_execution_probe = probe_hevc_decode_submit_execution(
                            &device,
                            HevcDecodeSubmitExecutionContext {
                                instance,
                                physical_device: self.state.physical_device,
                                queue_family_index: self.state.queue_family_index,
                                output_format: picture_format,
                                capability_snapshot: &self.state.capability_snapshot,
                                parameter_sets: &self.parsed.0,
                                bitstream,
                                submit_probe_access_unit_limit,
                            },
                        );
                        return (
                            HevcVideoSessionCreateProbe::Created,
                            HevcVideoSessionParametersCreateProbe::Created,
                            submit_skeleton_probe,
                            submit_execution_probe,
                        );
                    }
                    Ok(HevcVideoSessionParametersCreateProbe::Failed(err)) => {
                        session_parameters_errors.push(format!("{picture_format:?}: {err}"));
                    }
                    Ok(HevcVideoSessionParametersCreateProbe::Skipped(reason)) => {
                        session_parameters_errors.push(format!("{picture_format:?}: {reason}"));
                    }
                    Err(err) => {
                        session_create_errors.push(format!("{picture_format:?}: {err}"));
                    }
                }
            }

            if !session_parameters_errors.is_empty() {
                return (
                    HevcVideoSessionCreateProbe::Created,
                    HevcVideoSessionParametersCreateProbe::Failed(format!(
                        "vkCreateVideoSessionParametersKHR failed on all candidate output formats: {}",
                        session_parameters_errors.join("; ")
                    )),
                    HevcDecodeSubmitSkeletonProbe::Skipped(
                        "decode submit skeleton skipped: video session parameters creation failed"
                            .to_string(),
                    ),
                    HevcDecodeSubmitExecutionProbe::Skipped(
                        "decode submit execution probe skipped: video session parameters creation failed"
                            .to_string(),
                    ),
                );
            }

            let create_details = if session_create_errors.is_empty() {
                "no decode output formats were reported".to_string()
            } else {
                session_create_errors.join("; ")
            };
            (
                HevcVideoSessionCreateProbe::Failed(format!(
                    "vkCreateVideoSessionKHR failed on all candidate output formats: {create_details}"
                )),
                HevcVideoSessionParametersCreateProbe::Skipped(
                    "video session parameters create probe skipped: video session creation failed"
                        .to_string(),
                ),
                HevcDecodeSubmitSkeletonProbe::Skipped(
                    "decode submit skeleton skipped: video session creation failed".to_string(),
                ),
                HevcDecodeSubmitExecutionProbe::Skipped(
                    "decode submit execution probe skipped: video session creation failed"
                        .to_string(),
                ),
            )
        })();

        // SAFETY: `device` is no longer used after this point.
        unsafe {
            device.destroy_device(None);
        }

        probe_result
    }

    fn probe_single_video_session_format(
        &self,
        device: &ash::Device,
        instance: &ash::Instance,
        picture_format: vk::Format,
    ) -> Result<HevcVideoSessionParametersCreateProbe, String> {
        let mut decode_h265_profile = vk::VideoDecodeH265ProfileInfoKHR::default()
            .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
        let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
            .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
        let profile = vk::VideoProfileInfoKHR::default()
            .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_H265)
            .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
            .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
            .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
            .push_next(&mut decode_h265_profile)
            .push_next(&mut decode_usage);

        let create_info = vk::VideoSessionCreateInfoKHR::default()
            .queue_family_index(self.state.queue_family_index.0)
            .video_profile(&profile)
            .picture_format(picture_format)
            .max_coded_extent(vk::Extent2D {
                width: self.parsed.0.coded_width,
                height: self.parsed.0.coded_height,
            })
            .reference_picture_format(picture_format)
            .max_dpb_slots(self.state.capability_snapshot.max_dpb_slots.max(1))
            .max_active_reference_pictures(
                self.state
                    .capability_snapshot
                    .max_active_reference_pictures
                    .max(1),
            )
            .std_header_version(&self.state.capability_snapshot.std_header_version);
        let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
        let mut video_session = vk::VideoSessionKHR::null();

        // SAFETY: All pointers in `create_info` reference stack values that remain valid
        // during the call, and `device` is a valid logical device created above.
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

        let session_parameters_probe = probe_video_session_parameters_creation(
            device,
            &video_queue_device,
            video_session,
            &self.parsed.0,
        );

        // SAFETY: `video_session` was created by the same `device` and is not reused.
        unsafe {
            (video_queue_device.fp().destroy_video_session_khr)(
                device.handle(),
                video_session,
                std::ptr::null(),
            );
        }
        Ok(session_parameters_probe)
    }
}

pub(super) fn create_video_session_parameters(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_session: vk::VideoSessionKHR,
    parameter_sets: &HevcParameterSets,
) -> Result<vk::VideoSessionParametersKHR, String> {
    let std_parameter_sets = match build_hevc_std_parameter_set_storage(parameter_sets) {
        Ok(std_parameter_sets) => std_parameter_sets,
        Err(err) => {
            return Err(format!(
                "failed to map HEVC VPS/SPS/PPS into StdVideo structs: {err}"
            ));
        }
    };
    let parameters_add_info = std_parameter_sets.add_info();
    let mut decode_h265_session_parameters =
        vk::VideoDecodeH265SessionParametersCreateInfoKHR::default()
            .max_std_vps_count(1)
            .max_std_sps_count(1)
            .max_std_pps_count(1)
            .parameters_add_info(&parameters_add_info);
    let create_info = vk::VideoSessionParametersCreateInfoKHR::default()
        .video_session(video_session)
        .video_session_parameters_template(vk::VideoSessionParametersKHR::null())
        .push_next(&mut decode_h265_session_parameters);
    let mut video_session_parameters = vk::VideoSessionParametersKHR::null();

    // SAFETY: `create_info` references stack data alive for the call and `device` is valid.
    let result = unsafe {
        (video_queue_device.fp().create_video_session_parameters_khr)(
            device.handle(),
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
    Ok(video_session_parameters)
}

pub(super) fn probe_video_session_parameters_creation(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_session: vk::VideoSessionKHR,
    parameter_sets: &HevcParameterSets,
) -> HevcVideoSessionParametersCreateProbe {
    let video_session_parameters = match create_video_session_parameters(
        device,
        video_queue_device,
        video_session,
        parameter_sets,
    ) {
        Ok(video_session_parameters) => video_session_parameters,
        Err(err) => return HevcVideoSessionParametersCreateProbe::Failed(err),
    };

    // SAFETY: `video_session_parameters` was created from this device and is no longer used.
    unsafe {
        (video_queue_device.fp().destroy_video_session_parameters_khr)(
            device.handle(),
            video_session_parameters,
            std::ptr::null(),
        );
    }
    HevcVideoSessionParametersCreateProbe::Created
}

pub(super) fn build_hevc_decode_submit_skeleton_probe(
    parameter_sets: &HevcParameterSets,
    capability_snapshot: &HevcCapabilitySnapshot,
) -> HevcDecodeSubmitSkeletonProbe {
    let parsed_vps = match parse_hevc_vps(&parameter_sets.vps) {
        Ok(parsed_vps) => parsed_vps,
        Err(err) => {
            return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                "failed to parse VPS for decode submit skeleton: {err}"
            ));
        }
    };
    let parsed_pps = match parse_hevc_pps(&parameter_sets.pps) {
        Ok(parsed_pps) => parsed_pps,
        Err(err) => {
            return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                "failed to parse PPS for decode submit skeleton: {err}"
            ));
        }
    };
    let sps_id = match narrow_u64_to_u8(
        parameter_sets.parsed_sps.sps_seq_parameter_set_id,
        "sps_seq_parameter_set_id",
    ) {
        Ok(sps_id) => sps_id,
        Err(err) => {
            return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                "failed to map SPS id for decode submit skeleton: {err}"
            ));
        }
    };

    let dpb_slot_count = capability_snapshot.max_dpb_slots.clamp(1, 16);
    let mut planned_dpb_slots = Vec::new();
    for slot in 0..dpb_slot_count {
        match u8::try_from(slot) {
            Ok(slot_index) => planned_dpb_slots.push(slot_index),
            Err(_) => {
                return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                    "DPB slot index {slot} does not fit in u8"
                ));
            }
        }
    }

    let requested_references = parameter_sets
        .parsed_sps
        .short_term_ref_pic_sets
        .num_delta_pocs
        .first()
        .copied()
        .unwrap_or(0);
    let reference_capacity = u64::from(capability_snapshot.max_active_reference_pictures.max(1));
    let max_plannable_references = u64::from(dpb_slot_count.saturating_sub(1));
    let planned_reference_count = requested_references
        .min(reference_capacity)
        .min(max_plannable_references);
    let mut planned_reference_slots = Vec::new();
    for slot in 1..=planned_reference_count {
        match u8::try_from(slot) {
            Ok(slot_index) => planned_reference_slots.push(slot_index),
            Err(_) => {
                return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                    "reference slot index {slot} does not fit in u8"
                ));
            }
        }
    }

    let (first_slice_nal_type, first_slice_pps_id, first_slice_pic_order_cnt_lsb) =
        if let Some(first_vcl_nalu) = parameter_sets.first_vcl_nalus.first() {
            match parse_hevc_slice_header(first_vcl_nalu, &parsed_pps, &parameter_sets.parsed_sps) {
                Ok(slice_header) => (
                    Some(slice_header.nal_unit_type),
                    Some(slice_header.pps_id),
                    slice_header.pic_order_cnt_lsb,
                ),
                Err(err) => {
                    return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                        "failed to parse first VCL slice header for decode submit skeleton: {err}"
                    ));
                }
            }
        } else {
            (None, None, None)
        };

    HevcDecodeSubmitSkeletonProbe::Ready(HevcDecodeSubmitSkeleton {
        vps_id: parsed_vps.vps_video_parameter_set_id,
        sps_id,
        pps_id: parsed_pps.pps_pic_parameter_set_id,
        vcl_nalu_count: parameter_sets.first_vcl_nalus.len(),
        first_slice_nal_type,
        first_slice_pps_id,
        first_slice_pic_order_cnt_lsb,
        planned_dpb_slots,
        planned_reference_slots,
    })
}
