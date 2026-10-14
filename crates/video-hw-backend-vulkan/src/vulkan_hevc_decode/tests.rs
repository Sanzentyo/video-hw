//! Unit tests for HEVC syntax, planning, and non-destructive Vulkan probes.

use std::path::PathBuf;

use super::*;

#[test]
fn extension_flags_require_all_three_hevc_decode_extensions() {
    let mut flags = ExtensionFlags::default();
    assert!(!flags.supports_hevc_decode());
    flags.has_video_queue = true;
    assert!(!flags.supports_hevc_decode());
    flags.has_video_decode_queue = true;
    assert!(!flags.supports_hevc_decode());
    flags.has_video_decode_h265 = true;
    assert!(flags.supports_hevc_decode());
}

#[test]
fn find_video_codec_queue_family_index_requires_h265_decode_operation() {
    let mut queue_family_properties = vec![vk::QueueFamilyProperties::default(); 2];
    queue_family_properties[0].queue_count = 1;
    queue_family_properties[0].queue_flags = vk::QueueFlags::VIDEO_DECODE_KHR;
    queue_family_properties[1].queue_count = 1;
    queue_family_properties[1].queue_flags = vk::QueueFlags::VIDEO_DECODE_KHR;

    let codec_operations = vec![
        vk::VideoCodecOperationFlagsKHR::ENCODE_H265,
        vk::VideoCodecOperationFlagsKHR::DECODE_H265,
    ];
    let queue_family_index = find_video_codec_queue_family_index(
        &queue_family_properties,
        &codec_operations,
        vk::QueueFlags::VIDEO_DECODE_KHR,
        vk::VideoCodecOperationFlagsKHR::DECODE_H265,
    );
    assert_eq!(queue_family_index, Some(1));
}

#[test]
fn find_video_codec_queue_family_index_falls_back_when_codec_metadata_absent() {
    let mut queue_family_properties = vec![vk::QueueFamilyProperties::default(); 1];
    queue_family_properties[0].queue_count = 1;
    queue_family_properties[0].queue_flags = vk::QueueFlags::VIDEO_DECODE_KHR;
    let codec_operations = vec![vk::VideoCodecOperationFlagsKHR::empty()];
    let queue_family_index = find_video_codec_queue_family_index(
        &queue_family_properties,
        &codec_operations,
        vk::QueueFlags::VIDEO_DECODE_KHR,
        vk::VideoCodecOperationFlagsKHR::DECODE_H265,
    );
    assert_eq!(queue_family_index, Some(0));
}

#[test]
fn hevc_decode_probe_returns_stable_enum() {
    let status = probe_hevc_decode_prerequisites();
    match status {
        HevcDecodePrerequisiteProbe::Ready
        | HevcDecodePrerequisiteProbe::MissingExtensions { .. }
        | HevcDecodePrerequisiteProbe::MissingDecodeQueueFamily
        | HevcDecodePrerequisiteProbe::NoCompatibleAdapter
        | HevcDecodePrerequisiteProbe::DeviceInitializationFailed(_)
        | HevcDecodePrerequisiteProbe::ProbeUnavailable(_) => {}
    }
}

#[test]
fn extract_hevc_parameter_sets_reports_missing_sets() {
    let err = extract_hevc_parameter_sets_annexb(&[0, 0, 1, 0x26, 0x01])
        .expect_err("non-parameter-set stream should fail");
    assert!(
        err.contains("missing VPS") || err.contains("missing SPS") || err.contains("missing PPS")
    );
}

#[test]
fn split_annexb_nalus_handles_three_and_four_byte_start_codes() {
    let stream = [
        0_u8, 0, 0, 1, 0x40, 0x01, 0x02, 0, 0, 1, 0x42, 0x01, 0x03, 0, 0, 0, 1, 0x44, 0x01, 0x04,
    ];
    let nalus = split_annexb_nalus(&stream);
    assert_eq!(nalus.len(), 3);
    assert_eq!(nalus[0], &[0x40, 0x01, 0x02]);
    assert_eq!(nalus[1], &[0x42, 0x01, 0x03]);
    assert_eq!(nalus[2], &[0x44, 0x01, 0x04]);
}

#[test]
fn extract_hevc_parameter_sets_parses_repository_sample() {
    let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.h265");
    let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
    let parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
        .expect("repository HEVC sample should contain VPS/SPS/PPS");

    assert!(!parameter_sets.vps.is_empty());
    assert!(!parameter_sets.sps.is_empty());
    assert!(!parameter_sets.pps.is_empty());
    assert!(parameter_sets.coded_width > 0);
    assert!(parameter_sets.coded_height > 0);

    let std_parameter_sets = build_hevc_std_parameter_set_storage(&parameter_sets)
        .expect("sample parameter sets should map to StdVideo structs");
    let parsed_vps = parse_hevc_vps(&parameter_sets.vps)
        .expect("repository VPS should expose StdVideo VPS fields");
    assert_eq!(std_parameter_sets.vps.len(), 1);
    assert_eq!(std_parameter_sets.sps.len(), 1);
    assert_eq!(std_parameter_sets.pps.len(), 1);
    assert!(!std_parameter_sets.vps[0].pProfileTierLevel.is_null());
    assert!(!std_parameter_sets.vps[0].pDecPicBufMgr.is_null());
    assert!(!std_parameter_sets.sps[0].pProfileTierLevel.is_null());
    assert_eq!(
        std_parameter_sets.sps[0].flags.scaling_list_enabled_flag(),
        0
    );
    assert_eq!(
        std_parameter_sets.sps[0]
            .flags
            .sps_scaling_list_data_present_flag(),
        0
    );
    assert!(std_parameter_sets.sps[0].pScalingLists.is_null());
    assert_eq!(
        std_parameter_sets.vps[0].vps_max_sub_layers_minus1,
        parsed_vps.vps_max_sub_layers_minus1
    );
    assert_eq!(
        std_parameter_sets.vps[0]
            .flags
            .vps_sub_layer_ordering_info_present_flag(),
        bool_to_u32(parsed_vps.vps_sub_layer_ordering_info_present_flag)
    );
    assert_eq!(
        std_parameter_sets.vps[0]
            .flags
            .vps_timing_info_present_flag(),
        bool_to_u32(parsed_vps.vps_timing_info_present_flag)
    );
    assert_eq!(
        std_parameter_sets.vps[0].vps_num_units_in_tick,
        parsed_vps.vps_num_units_in_tick
    );
    assert_eq!(
        std_parameter_sets.vps[0].vps_time_scale,
        parsed_vps.vps_time_scale
    );
    assert_eq!(
        std_parameter_sets
            .vps_profile_tier_level
            .general_profile_idc,
        parsed_vps.profile_tier_level.general_profile_idc
    );
    assert_eq!(
        std_parameter_sets.vps_profile_tier_level.general_level_idc,
        parsed_vps.profile_tier_level.general_level_idc
    );
    assert_eq!(
        std_parameter_sets.vps[0].pProfileTierLevel,
        std_parameter_sets.vps_profile_tier_level.as_ref() as *const StdVideoH265ProfileTierLevel
    );
    assert_eq!(
        std_parameter_sets.vps[0].pHrdParameters,
        std_parameter_sets.vps_hrd_parameters.as_ref() as *const StdVideoH265HrdParameters
    );
    assert_eq!(
        std_parameter_sets
            .vps_hrd_parameters
            .flags
            .nal_hrd_parameters_present_flag(),
        0
    );
    assert_eq!(
        std_parameter_sets.sps[0].pProfileTierLevel,
        std_parameter_sets.profile_tier_level.as_ref() as *const StdVideoH265ProfileTierLevel
    );
    assert_eq!(
        std_parameter_sets.sps[0].sps_video_parameter_set_id,
        parameter_sets.parsed_sps.sps_video_parameter_set_id
    );
    if !std_parameter_sets.sps[0].pSequenceParameterSetVui.is_null() {
        let vui = std_parameter_sets
            .sequence_parameter_set_vui
            .as_ref()
            .unwrap();
        assert_eq!(vui.flags.vui_hrd_parameters_present_flag(), 0);
        let hrd = std_parameter_sets
            .sequence_parameter_set_vui_hrd
            .as_ref()
            .unwrap();
        assert_eq!(
            vui.pHrdParameters,
            hrd.as_ref() as *const StdVideoH265HrdParameters
        );
        assert_eq!(hrd.flags.nal_hrd_parameters_present_flag(), 0);
    }
}

#[test]
fn estimate_hevc_access_unit_count_matches_repository_sample() {
    let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.h265");
    let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
    let count = estimate_hevc_access_unit_count(&bitstream)
        .expect("HEVC access-unit count should be derivable from repository sample");
    assert_eq!(count, 303);
}

#[test]
fn extract_hevc_access_unit_headers_matches_repository_sample() {
    let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.h265");
    let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
    let headers = extract_hevc_access_unit_headers(&bitstream)
        .expect("HEVC access-unit headers should be derivable from repository sample");
    assert_eq!(headers.len(), 303);
    assert!(headers.iter().all(|header| header.nal_unit_type <= 31));
}

#[test]
fn decode_submit_skeleton_probe_maps_repository_sample() {
    let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.h265");
    let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
    let parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
        .expect("repository HEVC sample should contain VPS/SPS/PPS");
    let capability_snapshot = HevcCapabilitySnapshot {
        min_bitstream_buffer_offset_alignment: 1,
        min_bitstream_buffer_size_alignment: 1,
        picture_access_granularity: vk::Extent2D {
            width: 1,
            height: 1,
        },
        min_coded_extent: vk::Extent2D {
            width: 64,
            height: 64,
        },
        max_coded_extent: vk::Extent2D {
            width: 8192,
            height: 8192,
        },
        max_dpb_slots: 8,
        max_active_reference_pictures: 4,
        max_level_idc: 0,
        std_header_version: vk::ExtensionProperties::default(),
    };

    match build_hevc_decode_submit_skeleton_probe(&parameter_sets, &capability_snapshot) {
        HevcDecodeSubmitSkeletonProbe::Ready(skeleton) => {
            assert_eq!(
                skeleton.sps_id,
                u8::try_from(parameter_sets.parsed_sps.sps_seq_parameter_set_id)
                    .expect("sample sps id must fit u8")
            );
            assert!(skeleton.vcl_nalu_count > 0);
            assert_eq!(skeleton.first_slice_pps_id, Some(skeleton.pps_id));
            assert!(!skeleton.planned_dpb_slots.is_empty());
        }
        other => panic!("expected decode submit skeleton to be ready, got {other:?}"),
    }
}

#[test]
fn build_submit_probe_bitstream_payload_maps_first_slice() {
    let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.h265");
    let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
    let parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
        .expect("repository HEVC sample should contain VPS/SPS/PPS");

    let payload = build_hevc_submit_probe_bitstream_payload(
        &parameter_sets,
        &bitstream,
        MAX_HEVC_SUBMIT_PROBE_ACCESS_UNITS,
        1,
        1,
    )
    .expect("submit probe payload should be built from parsed parameter sets");
    // First AU starts at offset 0 with alignment=1; first bytes are the Annex B start code.
    assert!(payload.bytes.starts_with(&[0, 0, 1]));
    assert_eq!(
        payload.access_units.len(),
        MAX_HEVC_SUBMIT_PROBE_ACCESS_UNITS
    );
    let au0 = &payload.access_units[0];
    // slice_segment_offset stores the normalized RBSP offset from buffer_offset.
    assert_eq!(au0.slice_segment_offset, 5);
    // Absolute position (buffer_offset + 5) must lie within the buffer.
    let abs_offset0 = usize::try_from(au0.buffer_offset)
        .ok()
        .and_then(|o| o.checked_add(usize::try_from(au0.slice_segment_offset).ok()?));
    assert!(abs_offset0.is_some_and(|o| o < payload.bytes.len()));

    // AU1 must also have slice_segment_offset==5 (relative) and a non-zero buffer_offset.
    if payload.access_units.len() > 1 {
        let au1 = &payload.access_units[1];
        assert_eq!(
            au1.slice_segment_offset, 5,
            "slice_segment_offset must point to the RBSP payload for all AUs"
        );
        assert!(
            au1.buffer_offset > 0,
            "AU1 must start at a non-zero buffer offset"
        );
        // The normalized RBSP offset should resolve to a valid byte inside the shared upload.
        let rbsp_abs = au1
            .buffer_offset
            .checked_add(u64::from(au1.slice_segment_offset))
            .and_then(|v| usize::try_from(v).ok());
        assert!(
            rbsp_abs.is_some_and(|o| o < payload.bytes.len()),
            "RBSP offset (buffer_offset+5) for AU1 must be within buffer"
        );
    }

    assert_eq!(
        payload.access_units[0].header.pps_id,
        payload.parsed_pps.pps_pic_parameter_set_id
    );
}

#[test]
fn hevc_slice_segment_offset_mode_defaults_to_annexb_start_code() {
    assert_eq!(
        hevc_slice_segment_offset_mode_from_override(0x10DE, None),
        HevcSliceSegmentOffsetMode::AnnexBStartCode
    );
    assert_eq!(
        hevc_slice_segment_offset_mode_from_override(0x8086, None),
        HevcSliceSegmentOffsetMode::AnnexBStartCode
    );
}

#[test]
fn hevc_slice_segment_offset_mode_override_takes_precedence() {
    assert_eq!(
        hevc_slice_segment_offset_mode_from_override(0x10DE, Some("rbsp")),
        HevcSliceSegmentOffsetMode::RbspPayload
    );
    assert_eq!(
        hevc_slice_segment_offset_mode_from_override(0x8086, Some("nalu")),
        HevcSliceSegmentOffsetMode::NaluHeader
    );
    assert_eq!(
        hevc_slice_segment_offset_mode_from_override(0x8086, Some("startcode")),
        HevcSliceSegmentOffsetMode::AnnexBStartCode
    );
    assert_eq!(
        hevc_slice_segment_offset_mode_from_override(0x8086, Some("global")),
        HevcSliceSegmentOffsetMode::GlobalRbspPayload
    );
    assert_eq!(
        hevc_slice_segment_offset_mode_from_override(0x8086, Some("memory")),
        HevcSliceSegmentOffsetMode::MemoryBindingAbsolute
    );
    assert_eq!(
        hevc_slice_segment_offset_mode_from_override(0x8086, Some("5")),
        HevcSliceSegmentOffsetMode::Fixed(5)
    );
    assert_eq!(
        hevc_slice_segment_offset_mode_from_override(0x8086, Some("unknown")),
        HevcSliceSegmentOffsetMode::AnnexBStartCode
    );
}

#[test]
fn build_submit_probe_bitstream_payload_allows_extended_access_unit_limit() {
    let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.h265");
    let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
    let parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
        .expect("repository HEVC sample should contain VPS/SPS/PPS");
    let access_unit_headers =
        extract_hevc_access_unit_headers(&bitstream).expect("sample must expose access units");
    let payload = build_hevc_submit_probe_bitstream_payload(
        &parameter_sets,
        &bitstream,
        access_unit_headers.len(),
        1,
        1,
    )
    .expect("extended submit payload should be built");
    assert_eq!(payload.access_units.len(), access_unit_headers.len());
    assert!(payload.access_units.len() > MAX_HEVC_SUBMIT_PROBE_ACCESS_UNITS);
}

#[test]
fn build_decode_readback_regions_nv12_handles_odd_dimensions() {
    let (buffer_size, regions) =
        build_decode_readback_regions(vk::Format::G8_B8R8_2PLANE_420_UNORM, 641, 479)
            .expect("NV12 readback regions should be computed");
    assert_eq!(regions.len(), 2);
    assert_eq!(buffer_size, 461_120);

    let y_region = regions[0];
    assert_eq!(y_region.buffer_offset, 0);
    assert_eq!(y_region.buffer_row_length, 641);
    assert_eq!(y_region.buffer_image_height, 479);
    assert_eq!(
        y_region.image_subresource.aspect_mask,
        vk::ImageAspectFlags::PLANE_0
    );
    assert_eq!(y_region.image_extent.width, 641);
    assert_eq!(y_region.image_extent.height, 479);

    let uv_region = regions[1];
    assert_eq!(uv_region.buffer_offset, 307_040);
    assert_eq!(uv_region.buffer_row_length, 321);
    assert_eq!(uv_region.buffer_image_height, 240);
    assert_eq!(
        uv_region.image_subresource.aspect_mask,
        vk::ImageAspectFlags::PLANE_1
    );
    assert_eq!(uv_region.image_extent.width, 321);
    assert_eq!(uv_region.image_extent.height, 240);
}

#[test]
fn build_decode_readback_regions_rejects_unsupported_format() {
    let err = build_decode_readback_regions(vk::Format::D32_SFLOAT, 640, 360)
        .expect_err("unsupported decode output format should be rejected");
    assert!(
        err.contains("not implemented"),
        "expected unsupported format details, got: {err}"
    );
}

#[test]
fn decide_hevc_experimental_dpb_auto_disables_when_marker_exists() {
    let decision = decide_hevc_experimental_dpb(HevcExperimentalDpbMode::Auto, true, false);
    assert_eq!(
        decision,
        HevcExperimentalDpbDecision::DisabledAutoMarkerPresent
    );
}

#[test]
fn decide_hevc_experimental_dpb_auto_disables_when_marker_write_fails() {
    let decision = decide_hevc_experimental_dpb(HevcExperimentalDpbMode::Auto, false, true);
    assert_eq!(
        decision,
        HevcExperimentalDpbDecision::DisabledAutoMarkerWriteFailed
    );
}

#[test]
fn decide_hevc_experimental_dpb_on_keeps_path_enabled_when_marker_write_fails() {
    let decision = decide_hevc_experimental_dpb(HevcExperimentalDpbMode::On, false, true);
    assert_eq!(decision, HevcExperimentalDpbDecision::EnabledOn);
    assert!(decision.enabled());
}

#[test]
fn parse_hevc_experimental_dpb_mode_defaults_to_on() {
    assert_eq!(
        parse_hevc_experimental_dpb_mode(None),
        HevcExperimentalDpbMode::On
    );
    assert_eq!(
        parse_hevc_experimental_dpb_mode(Some("")),
        HevcExperimentalDpbMode::On
    );
    assert_eq!(
        parse_hevc_experimental_dpb_mode(Some("unknown")),
        HevcExperimentalDpbMode::On
    );
}

#[test]
fn parse_hevc_experimental_dpb_mode_accepts_explicit_overrides() {
    assert_eq!(
        parse_hevc_experimental_dpb_mode(Some("off")),
        HevcExperimentalDpbMode::Off
    );
    assert_eq!(
        parse_hevc_experimental_dpb_mode(Some("auto")),
        HevcExperimentalDpbMode::Auto
    );
    assert_eq!(
        parse_hevc_experimental_dpb_mode(Some("on")),
        HevcExperimentalDpbMode::On
    );
}

#[test]
fn hevc_decode_session_limit_from_env_defaults_and_clamps() {
    assert_eq!(hevc_decode_session_limit(None, 15, false), 15);
    assert_eq!(hevc_decode_session_limit(Some("4"), 15, false), 4);
    assert_eq!(hevc_decode_session_limit(Some("99"), 15, false), 15);
    assert_eq!(hevc_decode_session_limit(Some("0"), 15, false), 15);
    assert_eq!(hevc_decode_session_limit(Some("invalid"), 15, false), 15);
}

#[test]
fn hevc_decode_session_limit_allow_zero_accepts_zero_override() {
    assert_eq!(hevc_decode_session_limit(Some("0"), 15, true), 0);
    assert_eq!(hevc_decode_session_limit(Some("99"), 15, true), 15);
}

#[test]
fn format_hevc_experimental_dpb_status_surfaces_auto_marker_reason() {
    let marker_path = PathBuf::from("C:\\temp\\video-hw-vulkan-hevc-dpb-inflight.flag");
    let status = format_hevc_experimental_dpb_status(
        HevcExperimentalDpbDecision::DisabledAutoMarkerPresent,
        &marker_path,
        None,
    );
    assert!(status.contains("mode=auto"));
    assert!(status.contains("disabled"));
    assert!(status.contains("stale inflight marker"));
}

#[test]
fn hevc_reference_nal_type_identifies_reference_and_non_reference_types() {
    assert!(
        is_hevc_reference_nal_type(19),
        "IDR should be reference-capable"
    );
    assert!(
        is_hevc_reference_nal_type(1),
        "TRAIL_R should be reference-capable"
    );
    assert!(
        !is_hevc_reference_nal_type(0),
        "TRAIL_N should be treated as non-reference"
    );
}

#[test]
fn select_hevc_decode_dpb_slot_rotates_reference_slots() {
    let mut next_reference_slot = 0_usize;
    let active = Vec::new();
    let selected = Vec::new();
    let first =
        select_hevc_decode_dpb_slot(true, true, 4, &mut next_reference_slot, &active, &selected);
    let second =
        select_hevc_decode_dpb_slot(true, true, 4, &mut next_reference_slot, &active, &selected);
    let third =
        select_hevc_decode_dpb_slot(true, true, 4, &mut next_reference_slot, &active, &selected);
    assert_eq!(first, 1);
    assert_eq!(second, 2);
    assert_eq!(third, 3);
}

#[test]
fn select_hevc_decode_dpb_slot_uses_free_slot_for_non_reference_when_slot_zero_is_active() {
    let mut next_reference_slot = 0_usize;
    let active = vec![HevcActiveReferenceSlot {
        slot: 0,
        pic_order_cnt_val: 0,
    }];
    let selected = Vec::new();
    let slot =
        select_hevc_decode_dpb_slot(true, false, 4, &mut next_reference_slot, &active, &selected);
    assert_eq!(slot, 1);
}

#[test]
fn select_hevc_decode_dpb_slot_avoids_current_references() {
    let mut next_reference_slot = 0_usize;
    let active = vec![HevcActiveReferenceSlot {
        slot: 1,
        pic_order_cnt_val: 0,
    }];
    let selected = active.clone();
    let slot =
        select_hevc_decode_dpb_slot(true, true, 4, &mut next_reference_slot, &active, &selected);
    assert_eq!(slot, 2);
}

#[test]
fn select_hevc_references_for_ref_pic_set_respects_before_after_limits() {
    let candidates = vec![
        HevcActiveReferenceSlot {
            slot: 1,
            pic_order_cnt_val: 8,
        },
        HevcActiveReferenceSlot {
            slot: 2,
            pic_order_cnt_val: 7,
        },
        HevcActiveReferenceSlot {
            slot: 3,
            pic_order_cnt_val: 12,
        },
        HevcActiveReferenceSlot {
            slot: 4,
            pic_order_cnt_val: 13,
        },
    ];
    let selected = select_hevc_references_for_ref_pic_set(&candidates, 10, 1, 2);
    let selected_pocs = selected
        .iter()
        .map(|reference| reference.pic_order_cnt_val)
        .collect::<Vec<_>>();
    assert_eq!(selected_pocs, vec![8, 12, 13]);
}

#[test]
fn bit_width_for_index_range_matches_expected_values() {
    assert_eq!(bit_width_for_index_range(0), 0);
    assert_eq!(bit_width_for_index_range(1), 1);
    assert_eq!(bit_width_for_index_range(3), 2);
    assert_eq!(bit_width_for_index_range(7), 3);
}

#[test]
fn parse_hevc_inline_short_term_ref_pic_set_usage_reads_used_counts() {
    // Manually encoded non-predicted inline RPS with current_poc=10:
    //   num_negative_pics=1, num_positive_pics=2
    //   negative[0]: delta_poc_s0_minus1=0, used=1  → DeltaPocS0[0]=-1  → abs_poc=9
    //   positive[0]: delta_poc_s1_minus1=0, used=0  (not used by current picture)
    //   positive[1]: delta_poc_s1_minus1=1, used=1  → DeltaPocS1[1]=3   → abs_poc=13
    let rbsp = [0x4f_u8, 0x94_u8];
    let mut reader = RbspBitReader::new(&rbsp);
    let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.h265");
    let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
    let parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
        .expect("repository HEVC sample should contain VPS/SPS/PPS");
    let pocs = parse_hevc_inline_short_term_ref_pic_set_usage(
        &mut reader,
        0,
        &parameter_sets.parsed_sps,
        10,
    )
    .expect("inline short-term RPS usage should parse")
    .expect("result should be Some");
    assert_eq!(pocs.before_count, 1, "one before-reference");
    assert_eq!(pocs.after_count, 1, "one after-reference");
    assert_eq!(pocs.before[0], 9, "before-ref poc = 10 + (-1)");
    assert_eq!(pocs.after[0], 13, "after-ref poc = 10 + 3");
    assert_eq!(pocs.num_delta_pocs_of_ref_rps_idx, 0, "non-predicted → 0");
}

#[test]
fn parse_hevc_inline_short_term_ref_pic_set_usage_parses_predicted_sets() {
    fn push_ue(bits: &mut Vec<bool>, value: u32) {
        let code_num = value.saturating_add(1);
        let bit_len = u32::BITS.saturating_sub(code_num.leading_zeros()) as usize;
        for _ in 0..bit_len.saturating_sub(1) {
            bits.push(false);
        }
        for bit_index in (0..bit_len).rev() {
            bits.push(((code_num >> bit_index) & 1) != 0);
        }
    }
    fn bits_to_bytes(bits: &[bool]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity((bits.len().saturating_add(7)) / 8);
        let mut current = 0_u8;
        for (index, bit) in bits.iter().enumerate() {
            current = (current << 1) | u8::from(*bit);
            if index % 8 == 7 {
                bytes.push(current);
                current = 0;
            }
        }
        let remaining = bits.len() % 8;
        if remaining != 0 {
            current <<= 8 - remaining;
            bytes.push(current);
        }
        bytes
    }

    let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.h265");
    let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
    let parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
        .expect("repository HEVC sample should contain VPS/SPS/PPS");
    let short_term = &parameter_sets.parsed_sps.short_term_ref_pic_sets;
    let short_term_count = short_term.num_delta_pocs.len();
    if short_term_count == 0 {
        return;
    }
    let ref_index = short_term_count - 1;
    let ref_num_delta_pocs = short_term
        .delta_poc_s0
        .get(ref_index)
        .expect("delta_poc_s0 should exist for reference set")
        .len()
        .saturating_add(
            short_term
                .delta_poc_s1
                .get(ref_index)
                .expect("delta_poc_s1 should exist for reference set")
                .len(),
        );
    let mut bits = Vec::new();
    bits.push(true); // inter_ref_pic_set_prediction_flag
    push_ue(&mut bits, 0); // delta_idx_minus1
    bits.push(false); // delta_rps_sign
    push_ue(&mut bits, 0); // abs_delta_rps_minus1
    for _ in 0..ref_num_delta_pocs {
        bits.push(false); // used_by_curr_pic_flag
        bits.push(false); // use_delta_flag
    }
    let rbsp = bits_to_bytes(&bits);
    let mut reader = RbspBitReader::new(&rbsp);
    let pocs = parse_hevc_inline_short_term_ref_pic_set_usage(
        &mut reader,
        short_term_count,
        &parameter_sets.parsed_sps,
        0,
    )
    .expect("predicted short-term RPS usage should parse")
    .expect("result should be Some");
    assert_eq!(pocs.before_count, 0, "all entries unused → no before-refs");
    assert_eq!(pocs.after_count, 0, "all entries unused → no after-refs");
    assert_eq!(pocs.num_delta_pocs_of_ref_rps_idx, ref_num_delta_pocs);
}

#[test]
fn select_hevc_references_by_inline_poc_picks_exact_poc() {
    // Regression: DPB contains poc=0, poc=4, poc=2 in insertion order.
    // Inline RPS requests poc=4 as a before-reference.
    // Must select poc=4, not poc=2 (most recently inserted before-reference).
    let active = vec![
        HevcActiveReferenceSlot {
            slot: 1,
            pic_order_cnt_val: 0,
        },
        HevcActiveReferenceSlot {
            slot: 2,
            pic_order_cnt_val: 4,
        },
        HevcActiveReferenceSlot {
            slot: 3,
            pic_order_cnt_val: 2,
        },
    ];
    let mut inline_pocs = HevcInlineRefPicSetPocs::default();
    inline_pocs.before[0] = 4;
    inline_pocs.before_count = 1;
    let selected = select_hevc_references_by_inline_poc(&active, &inline_pocs);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].pic_order_cnt_val, 4);
    assert_eq!(selected[0].slot, 2);
}

#[test]
fn resolve_hevc_slice_type_reference_usage_limits_maps_slice_types() {
    let parsed_pps = ParsedHevcPps {
        pps_pic_parameter_set_id: 0,
        pps_seq_parameter_set_id: 0,
        num_extra_slice_header_bits: 0,
        num_ref_idx_l0_default_active_minus1: 1,
        num_ref_idx_l1_default_active_minus1: 2,
        init_qp_minus26: 0,
        diff_cu_qp_delta_depth: 0,
        pps_cb_qp_offset: 0,
        pps_cr_qp_offset: 0,
        pps_beta_offset_div2: 0,
        pps_tc_offset_div2: 0,
        log2_parallel_merge_level_minus2: 0,
        num_tile_columns_minus1: 0,
        num_tile_rows_minus1: 0,
        column_width_minus1: [0; 19],
        row_height_minus1: [0; 21],
        dependent_slice_segments_enabled_flag: false,
        output_flag_present_flag: false,
        sign_data_hiding_enabled_flag: false,
        cabac_init_present_flag: false,
        constrained_intra_pred_flag: false,
        transform_skip_enabled_flag: false,
        cu_qp_delta_enabled_flag: false,
        pps_slice_chroma_qp_offsets_present_flag: false,
        weighted_pred_flag: false,
        weighted_bipred_flag: false,
        transquant_bypass_enabled_flag: false,
        tiles_enabled_flag: false,
        entropy_coding_sync_enabled_flag: false,
        uniform_spacing_flag: false,
        loop_filter_across_tiles_enabled_flag: false,
        pps_loop_filter_across_slices_enabled_flag: false,
        deblocking_filter_control_present_flag: false,
        deblocking_filter_override_enabled_flag: false,
        pps_deblocking_filter_disabled_flag: false,
        lists_modification_present_flag: false,
        slice_segment_header_extension_present_flag: false,
    };
    assert_eq!(
        resolve_hevc_slice_type_reference_usage_limits(&parsed_pps, Some(0)),
        Some((2, 3))
    );
    assert_eq!(
        resolve_hevc_slice_type_reference_usage_limits(&parsed_pps, Some(1)),
        Some((2, 0))
    );
    assert_eq!(
        resolve_hevc_slice_type_reference_usage_limits(&parsed_pps, Some(2)),
        Some((0, 0))
    );
}

#[test]
fn build_hevc_ref_pic_set_lists_assigns_before_and_after_ref_pic_set_indices() {
    // `RefPicSetStCurrBefore/After` must hold **DPB slot indices** (the `.slot` field of
    // each HevcActiveReferenceSlot), NOT positions in the `pReferenceSlots` array.
    // Spec (VkVideoDecodeH265PictureInfoKHR): "each element … identifies an active reference
    // picture using its DPB slot index".  Using slot values that differ from their array
    // positions verifies this.
    let references = vec![
        HevcActiveReferenceSlot {
            slot: 5,
            pic_order_cnt_val: 12,
        }, // index=0, after (poc 12 > 10)
        HevcActiveReferenceSlot {
            slot: 3,
            pic_order_cnt_val: 8,
        }, // index=1, before (poc 8 < 10)
        HevcActiveReferenceSlot {
            slot: 7,
            pic_order_cnt_val: 16,
        }, // index=2, after (poc 16 > 10)
    ];
    let (before, after, count) = build_hevc_ref_pic_set_lists(&references, 10, 256);
    assert_eq!(before[0], 3); // DPB slot of poc=8 (the before reference)
    assert_eq!(before[1], HEVC_NO_REFERENCE_PICTURE);
    assert_eq!(after[0], 5); // DPB slot of poc=12 (first after reference)
    assert_eq!(after[1], 7); // DPB slot of poc=16 (second after reference)
    assert_eq!(count, 3);
}

#[test]
fn build_hevc_ref_pic_set_lists_excludes_equal_poc_entries() {
    let references = vec![
        HevcActiveReferenceSlot {
            slot: 2,
            pic_order_cnt_val: 10,
        }, // equal, excluded
        HevcActiveReferenceSlot {
            slot: 3,
            pic_order_cnt_val: 8,
        }, // before
    ];
    let (before, after, count) = build_hevc_ref_pic_set_lists(&references, 10, 256);
    assert_eq!(before[0], 3); // DPB slot 3 (poc=8, the before reference)
    assert_eq!(after[0], HEVC_NO_REFERENCE_PICTURE);
    assert_eq!(count, 1);
}

#[test]
fn build_hevc_ref_pic_set_lists_marks_empty_entries_as_no_reference() {
    let references = Vec::<HevcActiveReferenceSlot>::new();
    let (before, after, count) = build_hevc_ref_pic_set_lists(&references, 0, 256);
    assert_eq!(
        before,
        [HEVC_NO_REFERENCE_PICTURE; HEVC_REF_PIC_SET_LIST_SIZE]
    );
    assert_eq!(
        after,
        [HEVC_NO_REFERENCE_PICTURE; HEVC_REF_PIC_SET_LIST_SIZE]
    );
    assert_eq!(count, 0);
}

#[test]
fn decode_submit_skeleton_probe_surfaces_pps_parse_errors() {
    let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.h265");
    let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
    let mut parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
        .expect("repository HEVC sample should contain VPS/SPS/PPS");
    parameter_sets.pps = vec![0x44, 0x01];
    let capability_snapshot = HevcCapabilitySnapshot {
        min_bitstream_buffer_offset_alignment: 1,
        min_bitstream_buffer_size_alignment: 1,
        picture_access_granularity: vk::Extent2D {
            width: 1,
            height: 1,
        },
        min_coded_extent: vk::Extent2D {
            width: 64,
            height: 64,
        },
        max_coded_extent: vk::Extent2D {
            width: 8192,
            height: 8192,
        },
        max_dpb_slots: 8,
        max_active_reference_pictures: 4,
        max_level_idc: 0,
        std_header_version: vk::ExtensionProperties::default(),
    };

    match build_hevc_decode_submit_skeleton_probe(&parameter_sets, &capability_snapshot) {
        HevcDecodeSubmitSkeletonProbe::Failed(err) => {
            assert!(
                err.contains("parse PPS"),
                "expected parse failure details in error: {err}"
            );
        }
        other => panic!("expected decode submit skeleton failure, got {other:?}"),
    }
}

#[test]
fn hevc_decode_session_bootstrap_rejects_invalid_bitstream() {
    let err = probe_hevc_decode_session_bootstrap(&[1, 2, 3])
        .expect_err("invalid stream should fail before Vulkan bootstrap");
    assert!(
        err.contains("missing VPS") || err.contains("missing SPS") || err.contains("missing PPS")
    );
}

#[test]
fn hevc_decode_bootstrap_cache_keys_on_access_unit_limit() {
    let bitstream = [0_u8, 0, 0, 1, 0x26, 0x01];
    let uncapped_key = hevc_decode_bootstrap_cache_key(&bitstream, None, None);
    let capped_key = hevc_decode_bootstrap_cache_key(&bitstream, Some(303), None);
    let adapter_key = hevc_decode_bootstrap_cache_key(&bitstream, Some(303), Some(1));
    assert_ne!(uncapped_key, capped_key);
    assert_ne!(capped_key, adapter_key);

    let bootstrap = HevcDecodeSessionBootstrap {
        coded_width: 1920,
        coded_height: 1080,
        min_coded_width: 64,
        min_coded_height: 64,
        max_coded_width: 8192,
        max_coded_height: 8192,
        max_dpb_slots: 8,
        max_active_reference_pictures: 4,
        max_level_idc: 120,
        decode_output_formats: vec![vk::Format::G8_B8R8_2PLANE_420_UNORM],
        video_session_create_probe: HevcVideoSessionCreateProbe::Created,
        video_session_parameters_create_probe: HevcVideoSessionParametersCreateProbe::Created,
        decode_submit_skeleton_probe: HevcDecodeSubmitSkeletonProbe::Skipped(
            "test-only".to_string(),
        ),
        decode_submit_execution_probe: HevcDecodeSubmitExecutionProbe::Ready {
            queue_family_index: 0,
            output_format: vk::Format::G8_B8R8_2PLANE_420_UNORM,
            coded_width: 1920,
            coded_height: 1080,
            readback_non_zero: true,
            readback_bytes: 16,
            readback_planes: 2,
            readback_sample_stride: 16,
            readback_sample_count: 1,
            readback_sample: vec![1, 2, 3, 4],
            submitted_access_units: 1,
            experimental_dpb_enabled: false,
            experimental_dpb_mode: "off",
            experimental_dpb_status: "test-only".to_string(),
        },
    };
    store_hevc_decode_bootstrap_cache(capped_key, bootstrap);

    let cached =
        lookup_hevc_decode_bootstrap_cache(capped_key).expect("bootstrap should be cached");
    assert_eq!(cached.coded_width, 1920);
    assert_eq!(cached.coded_height, 1080);
    assert!(lookup_hevc_decode_bootstrap_cache(uncapped_key).is_none());
}

#[test]
fn build_hevc_ref_pic_set_lists_classifies_correctly_after_poc_wraparound() {
    // After POC LSB wraparound (max=256): current poc=4 (true poc=260),
    // references at poc_lsb=253 (true poc=253, one before wraparound) must be
    // classified as "before" using modular circular distance.
    let references = vec![
        HevcActiveReferenceSlot {
            slot: 1,
            pic_order_cnt_val: 253, // d=(253-4).rem_euclid(256)=249 > 128 → before
        },
        HevcActiveReferenceSlot {
            slot: 2,
            pic_order_cnt_val: 3, // d=(3-4).rem_euclid(256)=255 > 128 → before
        },
        HevcActiveReferenceSlot {
            slot: 3,
            pic_order_cnt_val: 6, // d=(6-4).rem_euclid(256)=2 ≤ 128 → after
        },
    ];
    let (before, after, count) = build_hevc_ref_pic_set_lists(&references, 4, 256);
    assert_eq!(
        before[0], 1,
        "DPB slot 1 (poc=253) should be classified as before-reference"
    );
    assert_eq!(
        before[1], 2,
        "DPB slot 2 (poc=3) should be classified as before-reference"
    );
    assert_eq!(
        after[0], 3,
        "DPB slot 3 (poc=6) should be classified as after-reference"
    );
    assert_eq!(count, 3);
}

#[test]
fn inline_rps_abs_poc_wraps_correctly_after_max_poc_lsb() {
    // Regression: foreman stream has CRA at poc_lsb=250 followed by frames crossing
    // the max_poc_lsb=256 boundary. At current_poc=4 (true=260), the RPS delta=-7
    // should yield abs_poc=253 (not -3).
    fn push_ue(bits: &mut Vec<bool>, value: u32) {
        let code_num = value.saturating_add(1);
        let bit_len = u32::BITS.saturating_sub(code_num.leading_zeros()) as usize;
        for _ in 0..bit_len.saturating_sub(1) {
            bits.push(false);
        }
        for bit_index in (0..bit_len).rev() {
            bits.push(((code_num >> bit_index) & 1) != 0);
        }
    }
    fn bits_to_bytes(bits: &[bool]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity((bits.len().saturating_add(7)) / 8);
        let mut current = 0_u8;
        for (index, bit) in bits.iter().enumerate() {
            current = (current << 1) | u8::from(*bit);
            if index % 8 == 7 {
                bytes.push(current);
                current = 0;
            }
        }
        let remaining = bits.len() % 8;
        if remaining != 0 {
            current <<= 8 - remaining;
            bytes.push(current);
        }
        bytes
    }

    // Build a non-predicted inline RPS: 1 negative pic (delta_minus1=6 → delta=-7).
    // inter_ref_pic_set_prediction_flag is not read when st_rps_idx=0 (hardcoded false).
    let mut bits = Vec::new();
    push_ue(&mut bits, 1); // num_negative_pics = 1
    push_ue(&mut bits, 0); // num_positive_pics = 0
    push_ue(&mut bits, 6); // delta_poc_s0_minus1[0] = 6 → cumulative=-7
    bits.push(true); // used_by_curr_pic_s0_flag = true
    let rbsp = bits_to_bytes(&bits);
    let mut reader = RbspBitReader::new(&rbsp);

    let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.h265");
    let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
    let parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
        .expect("repository HEVC sample should contain VPS/SPS/PPS");
    assert_eq!(
        parameter_sets.parsed_sps.max_pic_order_cnt_lsb(),
        256,
        "sample must have max_poc_lsb=256 for this test to be meaningful"
    );

    // current_poc=4, delta=-7 → raw = 4+(-7) = -3 → with rem_euclid(256) = 253
    let pocs = parse_hevc_inline_short_term_ref_pic_set_usage(
        &mut reader,
        0,
        &parameter_sets.parsed_sps,
        4,
    )
    .expect("inline RPS should parse without error")
    .expect("result should be Some");
    assert_eq!(pocs.before_count, 1, "should have one before-reference");
    assert_eq!(
        pocs.before[0], 253,
        "abs_poc should be 253 (not -3) after wraparound: 4 + (-7) rem_euclid 256 = 253"
    );
}
