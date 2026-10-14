//! Codec support policy, capability reports, and actionable blocker diagnostics.

use super::*;

pub(super) fn ensure_vk_codec_supported(codec: Codec, operation: &str) -> Result<(), BackendError> {
    match codec {
        Codec::H264 => Ok(()),
        Codec::Hevc => {
            let message = if operation == "decode" {
                hevc_decode_blocker_message()
            } else {
                hevc_encode_blocker_message()
            };
            Err(BackendError::UnsupportedConfig(message))
        }
        Codec::Av1 => {
            let message = if operation == "decode" {
                av1_decode_blocker_message()
            } else {
                av1_encode_blocker_message()
            };
            Err(BackendError::UnsupportedConfig(message))
        }
    }
}

pub(super) fn av1_decode_blocker_message() -> String {
    let base = "Vulkan AV1 decode initialization failed";
    match probe_av1_decode_prerequisites() {
        Av1DecodePrerequisiteProbe::Ready => format!(
            "{base}; runtime prerequisites are present, but AV1 session bootstrap or decode submission failed for this stream"
        ),
        Av1DecodePrerequisiteProbe::MissingExtensions { missing } => {
            format!("{base}; missing Vulkan extensions: {}", missing.join(", "))
        }
        Av1DecodePrerequisiteProbe::MissingDecodeQueueFamily => {
            format!("{base}; no queue family advertises VIDEO_DECODE_KHR with AV1 decode operation")
        }
        Av1DecodePrerequisiteProbe::SessionBootstrapFailed(details) => {
            format!("{base}; AV1 decode session bootstrap failed: {details}")
        }
        Av1DecodePrerequisiteProbe::NoCompatibleAdapter => format!(
            "{base}; required extensions were observed but not on a single adapter that can run AV1 decode prerequisites"
        ),
        Av1DecodePrerequisiteProbe::ProbeUnavailable(details) => {
            format!("{base}; extension probe failed: {details}")
        }
    }
}

pub(super) fn av1_decode_blocker_message_with_bitstream(bitstream: &[u8]) -> String {
    let mut message = av1_decode_blocker_message();
    match inspect_av1_low_overhead_obus(bitstream) {
        Ok(inspection) => {
            message.push_str(&format!(
                "; parsed AV1 OBUs: obu_count={}, temporal_units={}, sequence_header={}, frame_payload={}, sequence_header_obu_len={}, frame_headers={}, key_frames={}, inter_frames={}, intra_only_frames={}, switch_frames={}, show_existing_frames={}, coded={}x{}, std_sequence_header={}, bitstream_session_parameters={}, submit_skeleton={}, picture_info_skeleton={}, decode_info_skeleton={}, decode_info_count={}, command_skeleton={}",
                inspection.obu_count,
                inspection.temporal_unit_count,
                inspection.has_sequence_header,
                inspection.has_frame_payload,
                inspection
                    .sequence_header_obu_len
                    .map_or_else(|| "none".to_string(), |len| len.to_string()),
                inspection.frame_header_count,
                inspection.key_frame_count,
                inspection.inter_frame_count,
                inspection.intra_only_frame_count,
                inspection.switch_frame_count,
                inspection.show_existing_frame_count,
                inspection
                    .coded_width
                    .map_or_else(|| "unknown".to_string(), |width| width.to_string()),
                inspection
                    .coded_height
                    .map_or_else(|| "unknown".to_string(), |height| height.to_string()),
                extract_av1_std_sequence_header(bitstream)
                    .map(|_| "ready".to_string())
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                probe_av1_decode_session_parameters_for_bitstream(bitstream)
                    .map(|probe| {
                        format!(
                            "ready(coded={}x{}, picture_format={:?}, offset_align={}, size_align={}, upload_bytes={}, decode_image_layers={}, decode_image_barrier_layers={}, readback_bytes={}, readback_regions={}, command_record_decodes={}, command_buffer_recorded={}, command_buffer_submitted={}, session_memory_count={}, session_memory_bound={}, session_memory_total={}, session_memory_max_align={})",
                            probe.coded_width,
                            probe.coded_height,
                            probe.picture_format,
                            probe.min_bitstream_buffer_offset_alignment,
                            probe.min_bitstream_buffer_size_alignment,
                            probe.bitstream_upload_bytes,
                            probe.decode_image_layers,
                            probe.decode_image_barrier_layers,
                            probe.readback_bytes,
                            probe.readback_region_count,
                            probe.command_record_decode_count,
                            probe.command_buffer_recorded,
                            probe.command_buffer_submitted,
                            probe.session_memory_requirement_count,
                            probe.session_memory_bound_count,
                            probe.session_memory_total_size,
                            probe.session_memory_max_alignment
                        )
                    })
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                build_av1_decode_submit_skeleton(bitstream)
                    .map(|skeleton| {
                        format!(
                            "ready(tu={}, frame_header_offset={}, tile_count={})",
                            skeleton.temporal_unit_index,
                            skeleton.frame_header_offset,
                            skeleton.tile_offsets.len()
                        )
                    })
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                build_av1_decode_submit_skeleton(bitstream)
                    .and_then(|skeleton| build_av1_decode_picture_info_skeleton(&skeleton))
                    .map(|picture| {
                        format!(
                            "ready(frame_type={:?}, frame_header_offset={}, vk_frame_header_offset={}, tile_count={}, tile_bytes={}, reference_slots={})",
                            picture.std_picture_info.frame_type,
                            picture.frame_header_offset,
                            picture.vk_picture_info().frame_header_offset,
                            picture.tile_offsets.len(),
                            picture.tile_sizes.iter().copied().sum::<u32>(),
                            picture.reference_name_slot_indices.len()
                        )
                    })
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                build_av1_decode_info_skeleton(bitstream)
                    .map(|decode| {
                        let mut av1_picture_info = decode.picture_info.vk_picture_info();
                        let dst_picture_resource =
                            decode.dst_picture_resource(vk::ImageView::null(), 0);
                        let mut setup_dpb_info = decode.vk_setup_dpb_slot_info();
                        let setup_reference_slot = decode.vk_setup_reference_slot(
                            0,
                            &dst_picture_resource,
                            &mut setup_dpb_info,
                        );
                        let vk_decode_info = decode.vk_decode_info_with_setup_reference_slot(
                            vk::Buffer::null(),
                            dst_picture_resource,
                            &setup_reference_slot,
                            &mut av1_picture_info,
                        );
                        format!(
                            "ready(src_offset={}, src_range={}, vk_src_offset={}, vk_src_range={}, coded={}x{}, dst_extent={}x{}, setup_slot={}, setup_slot_chained={}, frame_header_offset={}, tile_count={})",
                            decode.src_buffer_offset,
                            decode.src_buffer_range,
                            vk_decode_info.src_buffer_offset,
                            vk_decode_info.src_buffer_range,
                            decode.coded_width,
                            decode.coded_height,
                            vk_decode_info.dst_picture_resource.coded_extent.width,
                            vk_decode_info.dst_picture_resource.coded_extent.height,
                            setup_reference_slot.slot_index,
                            !vk_decode_info.p_setup_reference_slot.is_null(),
                            decode.picture_info.frame_header_offset,
                            decode.tile_count()
                        )
                    })
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                build_av1_decode_info_skeletons(bitstream)
                    .map(|decodes| decodes.len().to_string())
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                build_av1_key_frame_decode_command_skeleton(bitstream, 4)
                    .map(|command| {
                        let begin_resources =
                            command.begin_picture_resources(vk::ImageView::null());
                        let frame_resources = command
                            .frame_picture_resources(vk::ImageView::null())
                            .unwrap_or_default();
                        let frame_bundles = command.frame_record_bundles().unwrap_or_default();
                        let image_plan = command
                            .decode_image_plan(vk::Format::G8_B8R8_2PLANE_420_UNORM)
                            .ok();
                        let image_layers = image_plan
                            .as_ref()
                            .map(|plan| plan.array_layers)
                            .unwrap_or_default();
                        let image_usage = image_plan
                            .as_ref()
                            .map(|plan| plan.usage)
                            .unwrap_or_default();
                        let image_create_layers = image_plan
                            .as_ref()
                            .map(|plan| command.vk_decode_image_create_info(plan).array_layers)
                            .unwrap_or_default();
                        let begin_reference_infos = command.begin_std_reference_infos();
                        let mut begin_dpb_infos = command
                            .begin_dpb_slot_infos(&begin_reference_infos)
                            .unwrap_or_default();
                        let begin_reference_slots = command
                            .begin_reference_slots(&begin_resources, &mut begin_dpb_infos)
                            .unwrap_or_default();
                        let begin_coding_info = command
                            .vk_begin_coding_info(
                                vk::VideoSessionKHR::null(),
                                vk::VideoSessionParametersKHR::null(),
                                &begin_reference_slots,
                            )
                            .unwrap_or_default();
                        let reset_control = command.vk_reset_coding_control_info();
                        let end_coding_info = command.vk_end_coding_info();
                        let record_steps = command.record_steps();
                        format!(
                            "ready(frames={}, coded={}x{}, begin_slots={}, begin_resources={}, frame_resources={}, frame_bundles={}, image_layers={}, image_create_layers={}, image_usage={:?}, begin_reference_slots={}, vk_begin_refs={}, reset={}, end_flags_empty={}, record_steps={}, slots={})",
                            command.frames.len(),
                            command.coded_width,
                            command.coded_height,
                            command.begin_slots.len(),
                            begin_resources.len(),
                            frame_resources.len(),
                            frame_bundles.len(),
                            image_layers,
                            image_create_layers,
                            image_usage,
                            begin_reference_slots.len(),
                            begin_coding_info.reference_slot_count,
                            reset_control.flags.contains(vk::VideoCodingControlFlagsKHR::RESET),
                            end_coding_info.flags.is_empty(),
                            record_steps.len(),
                            command
                                .frames
                                .iter()
                                .map(|frame| frame.setup_slot_index.to_string())
                                .collect::<Vec<_>>()
                                .join("/")
                        )
                    })
                    .and_then(|command_status| {
                        let (offset_alignment, size_alignment, alignment_source) =
                            probe_av1_decode_session_parameters_for_bitstream(bitstream)
                                .map(|probe| {
                                    (
                                        probe.min_bitstream_buffer_offset_alignment,
                                        probe.min_bitstream_buffer_size_alignment,
                                        "capability",
                                    )
                                })
                                .unwrap_or((4096, 4096, "fallback"));
                        build_av1_aligned_key_frame_decode_command_skeleton(
                            bitstream,
                            4,
                            offset_alignment,
                            size_alignment,
                        )
                            .map(|(upload_plan, aligned_command)| {
                                let submit_bundles = upload_plan
                                    .frame_submit_bundles(&aligned_command)
                                    .unwrap_or_default();
                                let first_decode_info_ready = upload_plan
                                    .with_frame_decode_info(
                                        &aligned_command,
                                        0,
                                        vk::Buffer::null(),
                                        Av1DecodePictureViews {
                                            dst: vk::ImageView::null(),
                                            reference: vk::ImageView::null(),
                                        },
                                        |decode_info, _bundle| {
                                            !decode_info.p_next.is_null()
                                                && !decode_info.p_setup_reference_slot.is_null()
                                        },
                                    )
                                    .unwrap_or(false);
                                let decode_info_loop_count = upload_plan
                                    .with_frame_decode_infos(
                                        &aligned_command,
                                        vk::Buffer::null(),
                                        Av1DecodePictureViews {
                                            dst: vk::ImageView::null(),
                                            reference: vk::ImageView::null(),
                                        },
                                        |_decode_info, bundle| bundle.frame_index,
                                    )
                                    .map(|indices| indices.len())
                                    .unwrap_or_default();
                                let mut sequence_visits = 0usize;
                                let mut sequence_decodes = 0usize;
                                let mut sequence_begin_refs = 0u32;
                                let mut sequence_reset = false;
                                let mut sequence_first_decode_offset = 0u64;
                                let mut sequence_end_empty = false;
                                let sequence_ready = upload_plan
                                    .visit_decode_command_sequence(
                                        &aligned_command,
                                        vk::VideoSessionKHR::null(),
                                        vk::VideoSessionParametersKHR::null(),
                                        vk::Buffer::null(),
                                        Av1DecodePictureViews {
                                            dst: vk::ImageView::null(),
                                            reference: vk::ImageView::null(),
                                        },
                                        |visit| {
                                            sequence_visits += 1;
                                            match visit {
                                                Av1DecodeCommandVisit::BeginCoding(info) => {
                                                    sequence_begin_refs =
                                                        info.reference_slot_count;
                                                }
                                                Av1DecodeCommandVisit::ResetCoding(info) => {
                                                    sequence_reset = info.flags.contains(
                                                        vk::VideoCodingControlFlagsKHR::RESET,
                                                    );
                                                }
                                                Av1DecodeCommandVisit::DecodeFrame {
                                                    decode_info,
                                                    bundle,
                                                } => {
                                                    sequence_decodes += 1;
                                                    if bundle.frame_index == 0 {
                                                        sequence_first_decode_offset =
                                                            decode_info.src_buffer_offset;
                                                    }
                                                }
                                                Av1DecodeCommandVisit::EndCoding(info) => {
                                                    sequence_end_empty = info.flags.is_empty();
                                                }
                                            }
                                        },
                                    )
                                    .is_ok();
                                let record_summary = upload_plan
                                    .record_decode_command_sequence(
                                        &aligned_command,
                                        vk::VideoSessionKHR::null(),
                                        vk::VideoSessionParametersKHR::null(),
                                        vk::Buffer::null(),
                                        Av1DecodePictureViews {
                                            dst: vk::ImageView::null(),
                                            reference: vk::ImageView::null(),
                                        },
                                        |_visit| Ok(()),
                                    )
                                    .ok();
                                let record_summary_valid = record_summary
                                    .as_ref()
                                    .is_some_and(|summary| {
                                        summary.validate_for_command(&aligned_command).is_ok()
                                    });
                                format!(
                                    "{command_status}; aligned_upload=ready(bytes={}, frames={}, submit_bundles={}, first_decode_info={}, decode_info_loop={}, command_sequence={}, record_summary={}, record_summary_valid={}, sequence_visits={}, sequence_decodes={}, sequence_begin_refs={}, sequence_reset={}, sequence_first_decode_offset={}, sequence_end_empty={}, offset_align={}, size_align={}, align_source={}, first_offset={}, first_range={})",
                                    upload_plan.bytes.len(),
                                    aligned_command.frames.len(),
                                    submit_bundles.len(),
                                    first_decode_info_ready,
                                    decode_info_loop_count,
                                    sequence_ready,
                                    record_summary
                                        .as_ref()
                                        .map(|summary| format!(
                                            "{}/{}/{}/{}",
                                            summary.begin_count,
                                            summary.reset_count,
                                            summary.decode_count,
                                            summary.end_count
                                        ))
                                        .unwrap_or_else(|| "unavailable".to_string()),
                                    record_summary_valid,
                                    sequence_visits,
                                    sequence_decodes,
                                    sequence_begin_refs,
                                    sequence_reset,
                                    sequence_first_decode_offset,
                                    sequence_end_empty,
                                    offset_alignment,
                                    size_alignment,
                                    alignment_source,
                                    aligned_command
                                        .frames
                                        .first()
                                        .map(|frame| frame.src_buffer_offset)
                                        .unwrap_or_default(),
                                    aligned_command
                                        .frames
                                        .first()
                                        .map(|frame| frame.src_buffer_range)
                                        .unwrap_or_default()
                                )
                            })
                    })
                    .unwrap_or_else(|err| format!("unavailable({err})"))
            ));
        }
        Err(err) => {
            message.push_str(&format!("; OBU inspection failed: {err}"));
        }
    }
    message
}

pub(super) fn av1_encode_blocker_message() -> String {
    "Vulkan AV1 encode initialization failed; ash 0.38.0 exposes VK_KHR_video_decode_av1 bindings but not VK_KHR_video_encode_av1, so video-hw cannot implement Vulkan AV1 encode without updating Vulkan bindings"
        .to_string()
}

pub(super) fn hevc_decode_blocker_message() -> String {
    let base = "Vulkan HEVC decode initialization failed";
    match probe_hevc_decode_prerequisites() {
        HevcDecodePrerequisiteProbe::Ready => format!(
            "{base}; runtime prerequisites are present, but HEVC session bootstrap/decode submit failed"
        ),
        HevcDecodePrerequisiteProbe::MissingExtensions { missing } => {
            format!("{base}; missing Vulkan extensions: {}", missing.join(", "))
        }
        HevcDecodePrerequisiteProbe::MissingDecodeQueueFamily => {
            format!(
                "{base}; no queue family advertises VIDEO_DECODE_KHR with H.265 decode operation"
            )
        }
        HevcDecodePrerequisiteProbe::DeviceInitializationFailed(details) => {
            format!("{base}; device bootstrap for HEVC decode failed: {details}")
        }
        HevcDecodePrerequisiteProbe::NoCompatibleAdapter => format!(
            "{base}; required extensions were observed but not on a single adapter that can run HEVC decode end-to-end"
        ),
        HevcDecodePrerequisiteProbe::ProbeUnavailable(details) => {
            format!("{base}; extension probe failed: {details}")
        }
    }
}

pub(super) fn hevc_encode_blocker_message() -> String {
    hevc_encode_blocker_message_with_config(1920, 1080, 30)
}

pub(super) fn hevc_encode_blocker_message_with_config(
    _coded_width: u32,
    _coded_height: u32,
    _fps: i32,
) -> String {
    let base = "Vulkan HEVC encode initialization failed";
    match probe_hevc_encode_prerequisites() {
        HevcEncodePrerequisiteProbe::Ready => {
            format!(
                "{base}; runtime prerequisites are present, but HEVC session bootstrap or encode submission failed for this configuration"
            )
        }
        HevcEncodePrerequisiteProbe::MissingExtensions { missing } => {
            format!("{base}; missing Vulkan extensions: {}", missing.join(", "))
        }
        HevcEncodePrerequisiteProbe::MissingEncodeQueueFamily => {
            format!(
                "{base}; no queue family advertises VIDEO_ENCODE_KHR with H.265 encode operation"
            )
        }
        HevcEncodePrerequisiteProbe::DeviceInitializationFailed(details) => {
            format!("{base}; device bootstrap for HEVC encode failed: {details}")
        }
        HevcEncodePrerequisiteProbe::NoCompatibleAdapter => format!(
            "{base}; required extensions were observed but not on a single adapter that can run HEVC encode end-to-end"
        ),
        HevcEncodePrerequisiteProbe::ProbeUnavailable(details) => {
            format!("{base}; extension probe failed: {details}")
        }
    }
}

pub(super) fn resolve_hevc_decode_physical_device_index() -> Option<usize> {
    std::env::var("VIDEO_HW_VULKAN_HEVC_DECODE_PHYSICAL_DEVICE_INDEX")
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

pub(super) fn hevc_decode_blocker_message_with_bitstream(
    bitstream: &[u8],
    physical_device_index: Option<usize>,
) -> String {
    let mut message = hevc_decode_blocker_message();
    match extract_hevc_parameter_sets_annexb(bitstream) {
        Ok(parameter_sets) => {
            message.push_str(&format!(
                "; parsed HEVC parameter sets: vps={} bytes, sps={} bytes, pps={} bytes, coded={}x{}",
                parameter_sets.vps.len(),
                parameter_sets.sps.len(),
                parameter_sets.pps.len(),
                parameter_sets.coded_width,
                parameter_sets.coded_height
            ));
            match probe_hevc_decode_session_bootstrap_with_access_unit_limit_and_physical_device_index(
                bitstream,
                None,
                physical_device_index,
            ) {
                Ok(bootstrap) => {
                    let formats = bootstrap
                        .decode_output_formats
                        .iter()
                        .map(|format| format!("{format:?}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let formats = if formats.is_empty() {
                        "none".to_string()
                    } else {
                        formats
                    };
                    let session_create = match &bootstrap.video_session_create_probe {
                        HevcVideoSessionCreateProbe::Created => "created".to_string(),
                        HevcVideoSessionCreateProbe::Failed(err) => format!("failed ({err})"),
                    };
                    let session_parameters_create =
                        match &bootstrap.video_session_parameters_create_probe {
                            HevcVideoSessionParametersCreateProbe::Created => "created".to_string(),
                            HevcVideoSessionParametersCreateProbe::Failed(err) => {
                                format!("failed ({err})")
                            }
                            HevcVideoSessionParametersCreateProbe::Skipped(reason) => {
                                format!("skipped ({reason})")
                            }
                        };
                    let decode_submit_skeleton = match &bootstrap.decode_submit_skeleton_probe {
                        HevcDecodeSubmitSkeletonProbe::Ready(skeleton) => format!(
                            "ready(vps_id={}, sps_id={}, pps_id={}, vcl_nalus={}, first_slice_nal_type={:?}, first_slice_pps_id={:?}, first_slice_pic_order_cnt_lsb={:?}, dpb_slots={:?}, ref_slots={:?})",
                            skeleton.vps_id,
                            skeleton.sps_id,
                            skeleton.pps_id,
                            skeleton.vcl_nalu_count,
                            skeleton.first_slice_nal_type,
                            skeleton.first_slice_pps_id,
                            skeleton.first_slice_pic_order_cnt_lsb,
                            skeleton.planned_dpb_slots,
                            skeleton.planned_reference_slots
                        ),
                        HevcDecodeSubmitSkeletonProbe::Failed(err) => format!("failed ({err})"),
                        HevcDecodeSubmitSkeletonProbe::Skipped(reason) => {
                            format!("skipped ({reason})")
                        }
                    };
                    let decode_submit_execution = match &bootstrap.decode_submit_execution_probe {
                        HevcDecodeSubmitExecutionProbe::Ready {
                            queue_family_index,
                            output_format,
                            coded_width,
                            coded_height,
                            readback_non_zero,
                            readback_bytes,
                            readback_planes,
                            readback_sample_stride,
                            readback_sample_count,
                            readback_sample,
                            submitted_access_units,
                            experimental_dpb_enabled,
                            experimental_dpb_mode,
                            experimental_dpb_status,
                        } => format!(
                            "ready(queue_family_index={}, output_format={output_format:?}, coded={}x{}, readback_non_zero={}, readback_bytes={}, readback_planes={}, readback_sample_stride={}, readback_sample_count={}, readback_sample_len={}, submitted_access_units={}, experimental_dpb_enabled={}, experimental_dpb_mode={}, experimental_dpb_status={experimental_dpb_status:?})",
                            queue_family_index,
                            coded_width,
                            coded_height,
                            readback_non_zero,
                            readback_bytes,
                            readback_planes,
                            readback_sample_stride,
                            readback_sample_count,
                            readback_sample.len(),
                            submitted_access_units,
                            experimental_dpb_enabled,
                            experimental_dpb_mode
                        ),
                        HevcDecodeSubmitExecutionProbe::Failed(err) => format!("failed ({err})"),
                        HevcDecodeSubmitExecutionProbe::Skipped(reason) => {
                            format!("skipped ({reason})")
                        }
                    };
                    message.push_str(&format!(
                        "; session bootstrap probe: coded={}x{}, supported={}x{}..{}x{}, max_dpb_slots={}, max_active_refs={}, max_level_idc={}, output_formats=[{}], video_session_create={}, video_session_parameters_create={}, decode_submit_skeleton={}, decode_submit_execution={}",
                        bootstrap.coded_width,
                        bootstrap.coded_height,
                        bootstrap.min_coded_width,
                        bootstrap.min_coded_height,
                        bootstrap.max_coded_width,
                        bootstrap.max_coded_height,
                        bootstrap.max_dpb_slots,
                        bootstrap.max_active_reference_pictures,
                        bootstrap.max_level_idc,
                        formats,
                        session_create,
                        session_parameters_create,
                        decode_submit_skeleton,
                        decode_submit_execution
                    ));
                }
                Err(err) => {
                    message.push_str(&format!("; session bootstrap probe failed: {err}"));
                }
            }
        }
        Err(err) => {
            message.push_str(&format!("; parameter-set extraction failed: {err}"));
        }
    }
    message
}

pub(super) fn vulkan_capability_report(codec: Codec) -> CapabilityReport {
    let (decode_runtime_available, encode_runtime_available) = match codec {
        Codec::H264 => probe_vulkan_support(),
        Codec::Hevc => (
            matches!(
                probe_hevc_decode_prerequisites(),
                HevcDecodePrerequisiteProbe::Ready
            ),
            matches!(
                probe_hevc_encode_prerequisites(),
                HevcEncodePrerequisiteProbe::Ready
            ),
        ),
        Codec::Av1 => (
            matches!(
                probe_av1_decode_prerequisites(),
                Av1DecodePrerequisiteProbe::Ready
            ),
            false,
        ),
    };
    let decode_supported = true;
    let encode_supported = matches!(codec, Codec::H264 | Codec::Hevc);
    CapabilityReport {
        codec,
        contract: CapabilityContract {
            decode: DecodeCapability {
                supported: decode_supported,
                output_modes: if decode_supported {
                    vec![
                        DecodeOutputCapability {
                            mode: DecodeOutputMode::Metadata,
                            origin: DecodeOutputOrigin::MetadataOnly,
                        },
                        DecodeOutputCapability {
                            mode: DecodeOutputMode::Nv12,
                            origin: DecodeOutputOrigin::Native,
                        },
                        DecodeOutputCapability {
                            mode: DecodeOutputMode::Rgb24,
                            origin: DecodeOutputOrigin::ConvertedFromNv12,
                        },
                    ]
                } else {
                    Vec::new()
                },
                streaming_mode: StreamingMode::FlushOnly,
                fallback_policy: FallbackPolicy::NoFallback,
                requires_side_data: false,
                dimension_constraints: DimensionConstraints {
                    min_width: 2,
                    min_height: 2,
                    width_alignment: 2,
                    height_alignment: 2,
                },
            },
            encode: EncodeCapability {
                supported: encode_supported,
                input_formats: if encode_supported {
                    vec![EncodeInputFormat::Argb8888, EncodeInputFormat::Nv12]
                } else {
                    Vec::new()
                },
                encoded_layouts: if encode_supported {
                    vec![EncodedLayout::AnnexB]
                } else {
                    Vec::new()
                },
                streaming_mode: StreamingMode::FlushOnly,
                fallback_policy: FallbackPolicy::NoFallback,
                dimension_constraints: DimensionConstraints {
                    min_width: 2,
                    min_height: 2,
                    width_alignment: 2,
                    height_alignment: 2,
                },
            },
        },
        decode_runtime: RuntimeCapability {
            status: if decode_runtime_available {
                RuntimeStatus::Available
            } else {
                RuntimeStatus::Unavailable
            },
            hardware_acceleration: decode_runtime_available,
            reason: if decode_runtime_available {
                None
            } else {
                Some(format!(
                    "no usable Vulkan {codec} decode adapter was reported"
                ))
            },
        },
        encode_runtime: RuntimeCapability {
            status: if encode_runtime_available {
                RuntimeStatus::Available
            } else {
                RuntimeStatus::Unavailable
            },
            hardware_acceleration: encode_runtime_available,
            reason: if encode_runtime_available {
                None
            } else if encode_supported {
                Some(format!(
                    "no usable Vulkan {codec} encode adapter was reported"
                ))
            } else {
                Some(format!("Vulkan {codec} encode is not implemented"))
            },
        },
    }
}
