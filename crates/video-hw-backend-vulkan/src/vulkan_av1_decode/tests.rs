use super::*;
use ash::vk::Handle;

#[test]
fn extension_flags_require_all_av1_decode_extensions() {
    let mut flags = Av1DecodeExtensionFlags::default();
    assert!(!flags.supports_av1_decode());
    flags.has_video_queue = true;
    assert!(!flags.supports_av1_decode());
    flags.has_video_decode_queue = true;
    assert!(!flags.supports_av1_decode());
    flags.has_video_decode_av1 = true;
    assert!(flags.supports_av1_decode());
}

#[test]
fn find_video_codec_queue_family_index_requires_av1_decode_operation() {
    let mut queue_family_properties = vec![vk::QueueFamilyProperties::default(); 2];
    queue_family_properties[0].queue_count = 1;
    queue_family_properties[0].queue_flags = vk::QueueFlags::VIDEO_DECODE_KHR;
    queue_family_properties[1].queue_count = 1;
    queue_family_properties[1].queue_flags = vk::QueueFlags::VIDEO_DECODE_KHR;

    let codec_operations = vec![
        vk::VideoCodecOperationFlagsKHR::DECODE_H265,
        vk::VideoCodecOperationFlagsKHR::DECODE_AV1,
    ];
    let queue_family_index = find_video_codec_queue_family_index(
        &queue_family_properties,
        &codec_operations,
        vk::QueueFlags::VIDEO_DECODE_KHR,
        vk::VideoCodecOperationFlagsKHR::DECODE_AV1,
    );
    assert_eq!(queue_family_index, Some(1));
}

#[test]
fn capability_snapshot_validation_requires_output_format_and_dpb_slots() {
    let mut snapshot = Av1DecodeCapabilitySnapshot {
        min_coded_width: 16,
        min_coded_height: 16,
        max_coded_width: 4096,
        max_coded_height: 2160,
        min_bitstream_buffer_offset_alignment: 4096,
        min_bitstream_buffer_size_alignment: 4096,
        max_dpb_slots: 4,
        max_active_reference_pictures: 3,
        max_level: ash::vk::native::StdVideoAV1Level_STD_VIDEO_AV1_LEVEL_4_0,
        std_header_version: vk::ExtensionProperties::default(),
        decode_output_formats: Vec::new(),
    };
    let err = validate_av1_decode_capability_snapshot(&snapshot)
        .expect_err("empty output format list should be rejected");
    assert!(err.contains("no output formats"));

    snapshot
        .decode_output_formats
        .push(vk::Format::G8_B8R8_2PLANE_420_UNORM);
    validate_av1_decode_capability_snapshot(&snapshot)
        .expect("valid AV1 decode capability snapshot should pass");

    snapshot.max_dpb_slots = 0;
    let err = validate_av1_decode_capability_snapshot(&snapshot)
        .expect_err("zero DPB slots should be rejected");
    assert!(err.contains("max_dpb_slots=0"));
}

#[test]
fn preferred_probe_extent_stays_inside_capability_range() {
    assert_eq!(preferred_av1_probe_extent(1, 64), 16);
    assert_eq!(preferred_av1_probe_extent(32, 64), 32);
    assert_eq!(preferred_av1_probe_extent(1, 8), 8);
}

#[test]
fn probe_sequence_header_builder_uses_requested_extent() {
    let std_header = build_probe_av1_std_sequence_header(32, 24)
        .expect("probe sequence header dimensions should be valid");
    assert_eq!(std_header.max_frame_width_minus_1, 31);
    assert_eq!(std_header.max_frame_height_minus_1, 23);
    assert_eq!(std_header.flags.still_picture(), 1);
    assert_eq!(std_header.flags.reduced_still_picture_header(), 1);
}

#[test]
fn low_overhead_obu_parser_extracts_sequence_header_and_frames() {
    let bitstream = [
        make_obu(2, &[]),
        make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180)),
        make_obu(6, &[0x04, 0x05]),
    ]
    .concat();

    let inspection =
        inspect_av1_low_overhead_obus(&bitstream).expect("synthetic AV1 OBUs should parse");
    assert_eq!(inspection.obu_count, 3);
    assert_eq!(inspection.temporal_unit_count, 1);
    assert!(inspection.has_sequence_header);
    assert!(inspection.has_frame_payload);
    assert_eq!(inspection.frame_header_count, 1);
    assert_eq!(inspection.key_frame_count, 1);
    assert_eq!(inspection.inter_frame_count, 0);
    assert_eq!(inspection.show_existing_frame_count, 0);
    assert_eq!(inspection.coded_width, Some(320));
    assert_eq!(inspection.coded_height, Some(180));
}

#[test]
fn low_overhead_obu_inspection_counts_inter_and_show_existing_frames() {
    let bitstream = [
        make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180)),
        make_obu(6, &[0x20, 0x00]),
        make_obu(2, &[]),
        make_obu(6, &[0x80]),
    ]
    .concat();

    let inspection =
        inspect_av1_low_overhead_obus(&bitstream).expect("synthetic AV1 OBUs should parse");
    assert_eq!(inspection.frame_header_count, 2);
    assert_eq!(inspection.key_frame_count, 0);
    assert_eq!(inspection.inter_frame_count, 1);
    assert_eq!(inspection.show_existing_frame_count, 1);
}

#[test]
fn low_overhead_obu_parser_splits_temporal_units() {
    let bitstream = [
        make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180)),
        make_obu(6, &[0x02]),
        make_obu(2, &[]),
        make_obu(6, &[0x03]),
    ]
    .concat();

    let inspection =
        inspect_av1_low_overhead_obus(&bitstream).expect("synthetic AV1 OBUs should parse");
    assert_eq!(inspection.obu_count, 4);
    assert_eq!(inspection.temporal_unit_count, 2);
}

#[test]
fn low_overhead_obu_parser_rejects_missing_size_field() {
    let err =
        inspect_av1_low_overhead_obus(&[1 << 3]).expect_err("size-less OBU should be rejected");
    assert!(err.contains("lacks the low-overhead size field"));
}

#[test]
fn low_overhead_obu_parser_rejects_ivf_container_bytes() {
    let err = inspect_av1_low_overhead_obus(b"DKIF\0\0\0\0")
        .expect_err("IVF container bytes should be rejected explicitly");
    assert!(err.contains("IVF container input"));
}

#[test]
fn low_overhead_obu_parser_rejects_truncated_leb128() {
    let err = inspect_av1_low_overhead_obus(&[(1 << 3) | 0x02, 0x80])
        .expect_err("truncated LEB128 should be rejected");
    assert!(err.contains("truncated LEB128"));
}

#[test]
fn low_overhead_obu_parser_rejects_truncated_payload() {
    let err = inspect_av1_low_overhead_obus(&[(1 << 3) | 0x02, 3, 0xab])
        .expect_err("truncated payload should be rejected");
    assert!(err.contains("is truncated"));
}

#[test]
fn sequence_header_parser_reads_reduced_still_picture_dimensions() {
    let payload = av1_reduced_still_sequence_header_payload(640, 360);
    let parsed = parse_av1_sequence_header_payload(&payload)
        .expect("synthetic reduced-still sequence header should parse");

    assert_eq!(parsed.seq_profile, 0);
    assert!(parsed.still_picture);
    assert!(parsed.reduced_still_picture_header);
    assert_eq!(parsed.coded_width(), 640);
    assert_eq!(parsed.coded_height(), 360);
    assert_eq!(parsed.frame_width_bits_minus_1, 9);
    assert_eq!(parsed.frame_height_bits_minus_1, 8);
}

#[test]
fn sequence_header_parser_reads_ffmpeg_libaom_superblock_size() {
    let payload = [
        0x00, 0x00, 0x00, 0x04, 0x3c, 0xfe, 0xcc, 0xda, 0xf9, 0x00, 0x40,
    ];
    let parsed = parse_av1_sequence_header_payload(&payload)
        .expect("generated FFmpeg libaom sequence header should parse");

    assert_eq!(parsed.coded_width(), 320);
    assert_eq!(parsed.coded_height(), 180);
    assert!(!parsed.use_128x128_superblock);
    assert_eq!(parsed.order_hint_bits_minus_1, 6);
}

#[test]
fn std_picture_scope_uses_sequence_superblock_size_for_tile_layout() {
    let scope_64 = Av1DecodeStdPictureInfoScope::new(
        key_frame_std_picture_info(),
        320,
        180,
        false,
        None,
        None,
    );
    assert_eq!(scope_64.width_in_sbs_minus1[0], 4);
    assert_eq!(scope_64.height_in_sbs_minus1[0], 2);
    assert_eq!(scope_64.mi_col_starts[1], 80);
    assert_eq!(scope_64.mi_row_starts[1], 48);

    let scope_128 =
        Av1DecodeStdPictureInfoScope::new(key_frame_std_picture_info(), 320, 180, true, None, None);
    assert_eq!(scope_128.width_in_sbs_minus1[0], 2);
    assert_eq!(scope_128.height_in_sbs_minus1[0], 1);
    assert_eq!(scope_128.mi_col_starts[1], 96);
    assert_eq!(scope_128.mi_row_starts[1], 64);
}

#[test]
fn global_motion_defaults_to_identity_params() {
    let params = av1_identity_global_motion_params();
    assert!(
        params
            .iter()
            .all(|ref_params| *ref_params == [0, 0, 65_536, 0, 0, 65_536])
    );
}

#[test]
fn sequence_header_builder_populates_vulkan_std_header() {
    let payload = av1_reduced_still_sequence_header_payload(640, 360);
    let parsed = parse_av1_sequence_header_payload(&payload)
        .expect("synthetic reduced-still sequence header should parse");
    let std_header = build_av1_std_sequence_header(&parsed)
        .expect("synthetic reduced-still sequence header should map to Vulkan std header");

    assert_eq!(
        std_header.seq_profile,
        StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN
    );
    assert_eq!(std_header.flags.still_picture(), 1);
    assert_eq!(std_header.flags.reduced_still_picture_header(), 1);
    assert_eq!(std_header.max_frame_width_minus_1, 639);
    assert_eq!(std_header.max_frame_height_minus_1, 359);
    assert!(std_header.pColorConfig.is_null());
    assert!(std_header.pTimingInfo.is_null());
}

#[test]
fn bitstream_sequence_header_extractor_returns_vulkan_std_header() {
    let sequence_header = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    let mut bitstream = make_obu(2, &[]);
    bitstream.extend_from_slice(&sequence_header);
    bitstream.extend_from_slice(&make_obu(6, &[0x80]));

    let std_header = extract_av1_std_sequence_header(&bitstream)
        .expect("synthetic AV1 bitstream should yield a Vulkan std sequence header");

    assert_eq!(std_header.max_frame_width_minus_1, 319);
    assert_eq!(std_header.max_frame_height_minus_1, 179);
}

#[test]
fn bitstream_sequence_header_extractor_rejects_missing_sequence_header() {
    let bitstream = make_obu(6, &[0x80]);
    let err = extract_av1_std_sequence_header(&bitstream)
        .expect_err("missing AV1 sequence header should be rejected");
    assert!(err.contains("missing AV1 sequence header"));
}

#[test]
fn bitstream_session_probe_rejects_missing_sequence_header_before_vulkan() {
    let bitstream = make_obu(6, &[0x80]);
    let err = probe_av1_decode_session_parameters_for_bitstream(&bitstream)
        .expect_err("missing AV1 sequence header should be rejected before Vulkan probing");
    assert!(err.contains("missing AV1 sequence header"));
}

#[test]
fn decode_info_skeleton_rejects_sequence_header_after_first_frame() {
    let mut bitstream = make_obu(6, &[0xaa, 0xbb]);
    bitstream.extend_from_slice(&make_obu(
        1,
        &av1_reduced_still_sequence_header_payload(320, 180),
    ));

    let err = build_av1_decode_info_skeleton(&bitstream)
        .expect_err("sequence header after first frame should be rejected");

    assert!(err.contains("sequence header OBU appears after the first frame"));
}

#[test]
fn decode_submit_skeleton_maps_frame_obu_payload() {
    let mut bitstream = make_obu(2, &[]);
    bitstream.extend_from_slice(&make_obu(
        1,
        &av1_reduced_still_sequence_header_payload(320, 180),
    ));
    let frame_obu_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(6, &[0xaa, 0xbb, 0xcc]));

    let skeleton = build_av1_decode_submit_skeleton(&bitstream)
        .expect("frame OBU should produce a submit skeleton");

    assert_eq!(skeleton.temporal_unit_index, 0);
    assert_eq!(skeleton.frame_header_offset as usize, frame_obu_start);
    assert_eq!(skeleton.tile_offsets, vec![(frame_obu_start + 2) as u32]);
    assert_eq!(skeleton.tile_sizes, vec![3]);
}

#[test]
fn key_frame_obu_tile_payload_offset_skips_libaom_header() {
    let sequence_payload = [
        0x00, 0x00, 0x00, 0x04, 0x3c, 0xfe, 0xcc, 0xda, 0xf9, 0x00, 0x40,
    ];
    let sequence_header = parse_av1_sequence_header_payload(&sequence_payload)
        .expect("generated FFmpeg libaom sequence header should parse");
    let payload = [
        0x14, 0x00, 0x24, 0x00, 0x03, 0x8e, 0x69, 0xa2, 0x90, 0xae, 0xb0, 0x28, 0xdb, 0x5c,
    ];
    let header = parse_av1_key_frame_obu_header(&payload, Some(&sequence_header))
        .expect("libaom key-frame header should be parsed");
    assert_eq!(header.tile_payload_offset, 12);
    assert_eq!(header.base_q_idx, 128);
    assert_eq!(header.loop_filter_level, [7, 7, 13, 13]);
    assert_eq!(header.cdef_bits, 1);
    assert!(!header.segmentation_enabled);
}

#[test]
fn loop_filter_delta_updates_skip_ref_and_mode_deltas() {
    let payload = [
        // ref0 update=1, magnitude=0b100101, sign=1, refs1..7 update=0.
        0xcb, 0x00, // mode0 update=1, magnitude=0b000011, sign=0, mode1 update=0.
        0x86, 0x00,
    ];
    let mut bits = BitReader::new(&payload);

    skip_av1_loop_filter_delta_updates(&mut bits)
        .expect("loop-filter delta updates should be skipped");

    assert_eq!(bits.bit_offset, 24);
}

#[test]
fn quantization_parser_reads_signed_deltas_and_qmatrix() {
    let mut writer = BitWriter::default();
    writer.write_bits(128, 8);
    writer.write_bits(1, 1);
    writer.write_bits(0b111_1110, 7);
    writer.write_bits(1, 1);
    writer.write_bits(1, 1);
    writer.write_bits(0b000_0101, 7);
    writer.write_bits(0, 1);
    writer.write_bits(1, 1);
    writer.write_bits(0b111_1111, 7);
    writer.write_bits(1, 1);
    writer.write_bits(0b000_0011, 7);
    writer.write_bits(1, 1);
    writer.write_bits(2, 4);
    writer.write_bits(3, 4);
    writer.write_bits(4, 4);
    let payload = writer.finish();
    let mut bits = BitReader::new(&payload);

    let quantization =
        parse_av1_quantization_params(&mut bits, true).expect("quantization params should parse");

    assert_eq!(quantization.base_q_idx, 128);
    assert_eq!(quantization.delta_q_y_dc, -2);
    assert!(quantization.diff_uv_delta);
    assert_eq!(quantization.delta_q_u_dc, 5);
    assert_eq!(quantization.delta_q_u_ac, 0);
    assert_eq!(quantization.delta_q_v_dc, -1);
    assert_eq!(quantization.delta_q_v_ac, 3);
    assert!(quantization.using_qmatrix);
    assert_eq!(quantization.qm_y, 2);
    assert_eq!(quantization.qm_u, 3);
    assert_eq!(quantization.qm_v, 4);
}

#[test]
fn key_frame_segmentation_parser_reads_feature_table() {
    let mut writer = BitWriter::default();
    writer.write_bits(1, 1);
    writer.write_bits(1, 1);
    writer.write_bits(0b1_1111_1110, 9);
    for _ in 1..(AV1_MAX_SEGMENTS * AV1_SEG_LVL_MAX) {
        writer.write_bits(0, 1);
    }
    let payload = writer.finish();
    let mut bits = BitReader::new(&payload);

    let segmentation = parse_av1_key_frame_segmentation_params(&mut bits)
        .expect("key-frame segmentation params should parse");

    assert!(segmentation.enabled);
    assert!(segmentation.update_map);
    assert!(!segmentation.temporal_update);
    assert!(segmentation.update_data);
    assert_eq!(segmentation.std_segmentation.FeatureEnabled[0], 1);
    assert_eq!(segmentation.std_segmentation.FeatureData[0][0], -2);
    assert_eq!(segmentation.std_segmentation.FeatureEnabled[1], 0);
}

#[test]
fn decode_submit_skeleton_maps_frame_header_and_tile_group_obus() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    let frame_header_obu_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(3, &[0x10, 0x11]));
    let tile_group_obu_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(4, &[0x20, 0x21, 0x22]));

    let skeleton = build_av1_decode_submit_skeleton(&bitstream)
        .expect("frame-header + tile-group OBUs should produce a submit skeleton");

    assert_eq!(skeleton.temporal_unit_index, 0);
    assert_eq!(
        skeleton.frame_header_offset as usize,
        frame_header_obu_start
    );
    assert_eq!(
        skeleton.tile_offsets,
        vec![(tile_group_obu_start + 2) as u32]
    );
    assert_eq!(skeleton.tile_sizes, vec![3]);
}

#[test]
fn decode_submit_skeleton_rejects_inter_frame_obu() {
    let mut bitstream = make_obu(
        1,
        &[
            0x00, 0x00, 0x00, 0x04, 0x3c, 0xfe, 0xcc, 0xda, 0xf9, 0x00, 0x40,
        ],
    );
    // show_existing_frame=0, frame_type=1 (inter frame), followed by padding.
    bitstream.extend_from_slice(&make_obu(6, &[0x20, 0x00]));

    let err = build_av1_decode_submit_skeleton(&bitstream)
        .expect_err("inter-frame AV1 decode must not be treated as key-frame decode");

    assert!(err.contains("AV1 inter-frame decode is not implemented"));
    assert!(err.contains("frame_type=1"));
}

#[test]
fn decode_submit_skeleton_rejects_inter_frame_header_obu() {
    let mut bitstream = make_obu(
        1,
        &[
            0x00, 0x00, 0x00, 0x04, 0x3c, 0xfe, 0xcc, 0xda, 0xf9, 0x00, 0x40,
        ],
    );
    // show_existing_frame=0, frame_type=1 (inter frame), followed by padding.
    bitstream.extend_from_slice(&make_obu(3, &[0x20, 0x00]));
    bitstream.extend_from_slice(&make_obu(4, &[0x20, 0x21, 0x22]));

    let err = build_av1_decode_submit_skeleton(&bitstream)
        .expect_err("inter-frame AV1 frame-header OBU must be rejected");

    assert!(err.contains("AV1 inter-frame decode is not implemented"));
    assert!(err.contains("frame_type=1"));
}

#[test]
fn decode_submit_skeleton_rejects_show_existing_frame_obu() {
    let mut bitstream = make_obu(
        1,
        &[
            0x00, 0x00, 0x00, 0x04, 0x3c, 0xfe, 0xcc, 0xda, 0xf9, 0x00, 0x40,
        ],
    );
    // show_existing_frame=1 has no tile payload and must not be treated as tile data.
    bitstream.extend_from_slice(&make_obu(6, &[0x80]));

    let err = build_av1_decode_submit_skeleton(&bitstream)
        .expect_err("show-existing AV1 frame OBU must be rejected");

    assert!(err.contains("AV1 frame-header parsing is not implemented"));
    assert!(err.contains("show_existing_frame"));
}

#[test]
fn decode_submit_skeleton_groups_multiple_tile_group_obus() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    let frame_header_obu_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(3, &[0x10, 0x11]));
    let first_tile_group_obu_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(4, &[0x20, 0x21]));
    let second_tile_group_obu_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(4, &[0x30, 0x31, 0x32]));

    let skeletons = build_av1_decode_submit_skeletons(&bitstream)
        .expect("multiple tile groups should produce one submit skeleton");

    assert_eq!(skeletons.len(), 1);
    assert_eq!(
        skeletons[0].frame_header_offset as usize,
        frame_header_obu_start
    );
    assert_eq!(
        skeletons[0].tile_offsets,
        vec![
            (first_tile_group_obu_start + 2) as u32,
            (second_tile_group_obu_start + 2) as u32
        ]
    );
    assert_eq!(skeletons[0].tile_sizes, vec![2, 3]);
}

#[test]
fn decode_submit_skeletons_preserve_multiple_temporal_units() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    let first_frame_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(6, &[0x11, 0x12]));
    bitstream.extend_from_slice(&make_obu(2, &[]));
    let second_frame_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(6, &[0x21, 0x22, 0x23]));

    let submits = build_av1_decode_submit_skeletons(&bitstream)
        .expect("multiple frame OBUs should produce ordered submit skeletons");

    assert_eq!(submits.len(), 2);
    assert_eq!(submits[0].temporal_unit_index, 0);
    assert_eq!(submits[0].frame_header_offset as usize, first_frame_start);
    assert_eq!(submits[0].tile_sizes, vec![2]);
    assert_eq!(submits[1].temporal_unit_index, 1);
    assert_eq!(submits[1].frame_header_offset as usize, second_frame_start);
    assert_eq!(submits[1].tile_sizes, vec![3]);
}

#[test]
fn decode_submit_skeleton_rejects_tile_group_without_frame_header() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    bitstream.extend_from_slice(&make_obu(4, &[0x20, 0x21, 0x22]));

    let err = build_av1_decode_submit_skeleton(&bitstream)
        .expect_err("tile group without frame header should be rejected");
    assert!(err.contains("missing a preceding frame-header"));
}

#[test]
fn picture_info_skeleton_uses_key_frame_defaults_and_tiles() {
    let submit = Av1DecodeSubmitSkeleton {
        temporal_unit_index: 0,
        frame_header_offset: 12,
        tile_offsets: vec![12],
        tile_sizes: vec![5],
        use_128x128_superblock: false,
        key_frame_header: None,
        frame_header_summary: None,
        std_overrides: None,
    };

    let picture = build_av1_decode_picture_info_skeleton(&submit)
        .expect("valid submit skeleton should map to picture info skeleton");

    assert_eq!(picture.frame_header_offset, 12);
    assert_eq!(picture.tile_offsets, vec![12]);
    assert_eq!(picture.tile_sizes, vec![5]);
    assert_eq!(
        picture.std_picture_info.frame_type,
        StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY
    );
    assert_eq!(picture.std_picture_info.flags.error_resilient_mode(), 1);
    assert!(
        picture
            .reference_name_slot_indices
            .iter()
            .all(|slot| *slot == -1)
    );

    let vk_picture = picture.vk_picture_info();
    assert_eq!(vk_picture.frame_header_offset, 12);
    assert_eq!(vk_picture.tile_count, 1);
    assert_eq!(vk_picture.reference_name_slot_indices, [-1; 7]);
    assert!(!vk_picture.p_std_picture_info.is_null());
    assert!(!vk_picture.p_tile_offsets.is_null());
    assert!(!vk_picture.p_tile_sizes.is_null());
}

#[test]
fn picture_info_skeleton_maps_inter_frame_header_summary_into_reference_command() {
    let summary = Av1ParsedFrameHeaderSummary {
        frame_type: 1,
        show_existing_frame: false,
        error_resilient_mode: false,
        order_hint: 1,
        enable_order_hint: true,
        order_hint_bits: 8,
        primary_ref_frame: 6,
        refresh_frame_flags: 0x02,
        ref_frame_idx: [0, 1, 2, 3, 4, 5, 6],
        tile_payload_offset: 15,
        std_overrides: None,
    };
    let submit = Av1DecodeSubmitSkeleton {
        temporal_unit_index: 1,
        frame_header_offset: 20,
        tile_offsets: vec![35],
        tile_sizes: vec![8],
        use_128x128_superblock: false,
        key_frame_header: None,
        frame_header_summary: Some(summary),
        std_overrides: None,
    };

    let picture = build_av1_decode_picture_info_skeleton(&submit)
        .expect("inter-frame header summary should map to picture info");

    assert_eq!(
        picture.std_picture_info.frame_type,
        StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_INTER
    );
    assert_eq!(picture.std_picture_info.OrderHint, 1);
    assert_eq!(picture.std_picture_info.primary_ref_frame, 6);
    assert_eq!(picture.std_picture_info.refresh_frame_flags, 0x02);
    assert_eq!(picture.reference_name_slot_indices, [0, 1, 2, 3, 4, 5, 6]);

    let decode = Av1DecodeInfoSkeleton {
        temporal_unit_index: 1,
        src_buffer_offset: 20,
        src_buffer_range: 23,
        coded_width: 320,
        coded_height: 180,
        setup_reference_info: av1_std_reference_info(&picture),
        reference_slot_infos: Vec::new(),
        reference_base_array_layers: Vec::new(),
        picture_info: picture,
    };
    let command = build_av1_key_frame_decode_command_skeleton_from_decodes(&[decode], 8)
        .expect("inter-frame references should build a command skeleton");
    assert_eq!(command.begin_slots.len(), 7);
    assert_eq!(command.frames.len(), 1);
    assert_eq!(
        command.frames[0].reference_slot_indices,
        vec![0, 1, 2, 3, 4, 5, 6]
    );
}

#[test]
fn reference_name_slot_replay_rejects_conflicting_slot_aliases() {
    let mut decodes = (0..18)
        .map(|frame_index| {
            let is_key = frame_index == 0;
            let mut std_picture_info = key_frame_std_picture_info();
            std_picture_info.frame_type = if is_key {
                StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY
            } else {
                StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_INTER
            };
            std_picture_info.OrderHint = frame_index as u8;
            let summary = Av1ParsedFrameHeaderSummary {
                frame_type: if is_key { 0 } else { 1 },
                show_existing_frame: false,
                error_resilient_mode: is_key,
                order_hint: frame_index as u8,
                enable_order_hint: true,
                order_hint_bits: 8,
                primary_ref_frame: if is_key { 7 } else { 0 },
                refresh_frame_flags: if is_key { 0xff } else { 1 << (frame_index % 8) },
                ref_frame_idx: [0, 1, 2, 3, 4, 5, 6],
                tile_payload_offset: 0,
                std_overrides: None,
            };
            let picture_info = Av1DecodePictureInfoSkeleton {
                std_picture_info,
                reference_name_slot_indices: [-1; vk::MAX_VIDEO_AV1_REFERENCES_PER_FRAME_KHR],
                frame_header_offset: 0,
                tile_offsets: vec![0],
                tile_sizes: vec![1],
                use_128x128_superblock: false,
                key_frame_header: None,
                frame_header_summary: Some(summary),
                std_overrides: None,
            };
            Av1DecodeInfoSkeleton {
                temporal_unit_index: frame_index,
                src_buffer_offset: frame_index as u64,
                src_buffer_range: 1,
                coded_width: 320,
                coded_height: 180,
                setup_reference_info: av1_std_reference_info(&picture_info),
                reference_slot_infos: Vec::new(),
                reference_base_array_layers: Vec::new(),
                picture_info,
            }
        })
        .collect::<Vec<_>>();

    let err = apply_av1_reference_name_slot_replay(&mut decodes, 4)
        .expect_err("conflicting aliases must be rejected before returning wrong pixels");

    assert!(err.contains("aliases Vulkan DPB slot"));
    assert!(err.contains("max_dpb_slots=4"));
}

#[test]
fn picture_info_skeleton_rejects_empty_tiles() {
    let submit = Av1DecodeSubmitSkeleton {
        temporal_unit_index: 0,
        frame_header_offset: 12,
        tile_offsets: Vec::new(),
        tile_sizes: Vec::new(),
        use_128x128_superblock: false,
        key_frame_header: None,
        frame_header_summary: None,
        std_overrides: None,
    };

    let err = build_av1_decode_picture_info_skeleton(&submit)
        .expect_err("empty tile list should be rejected");
    assert!(err.contains("at least one tile offset"));
}

#[test]
fn decode_info_skeleton_rebases_frame_obu_offsets_to_source_range() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    let frame_obu_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(6, &[0xaa, 0xbb, 0xcc]));

    let decode = build_av1_decode_info_skeleton(&bitstream)
        .expect("frame OBU should produce a decode info skeleton");

    assert_eq!(decode.src_buffer_offset as usize, frame_obu_start);
    assert_eq!(decode.src_buffer_range, 5);
    assert_eq!(decode.coded_width, 320);
    assert_eq!(decode.coded_height, 180);
    assert_eq!(
        u32::from(decode.setup_reference_info.frame_type),
        StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY
    );
    assert_eq!(decode.setup_reference_info.OrderHint, 0);
    assert_eq!(decode.setup_reference_info.SavedOrderHints, [0; 8]);
    assert_eq!(decode.picture_info.frame_header_offset, 0);
    assert_eq!(decode.picture_info.tile_offsets, vec![2]);
    assert_eq!(decode.picture_info.tile_sizes, vec![3]);
    assert_eq!(decode.tile_count(), 1);
}

#[test]
fn decode_info_skeleton_covers_frame_header_and_tile_group_range() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    let frame_header_obu_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(3, &[0x10, 0x11]));
    let tile_group_obu_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(4, &[0x20, 0x21, 0x22]));

    let decode = build_av1_decode_info_skeleton(&bitstream)
        .expect("frame-header + tile-group OBUs should produce a decode info skeleton");
    let source_start = frame_header_obu_start;
    let tile_payload_start = tile_group_obu_start + 2;
    let source_end = tile_payload_start + 3;

    assert_eq!(decode.src_buffer_offset as usize, source_start);
    assert_eq!(decode.src_buffer_range as usize, source_end - source_start);
    assert_eq!(decode.picture_info.frame_header_offset, 0);
    assert_eq!(
        decode.picture_info.tile_offsets,
        vec![(tile_payload_start - source_start) as u32]
    );
    assert_eq!(decode.picture_info.tile_sizes, vec![3]);
}

#[test]
fn decode_info_skeletons_preserve_multiple_temporal_units() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    let first_frame_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(6, &[0x11, 0x12]));
    bitstream.extend_from_slice(&make_obu(2, &[]));
    let second_frame_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(6, &[0x21, 0x22, 0x23]));

    let decodes = build_av1_decode_info_skeletons(&bitstream)
        .expect("multiple frame OBUs should produce ordered decode info skeletons");

    assert_eq!(decodes.len(), 2);
    assert_eq!(decodes[0].src_buffer_offset as usize, first_frame_start);
    assert_eq!(decodes[0].src_buffer_range, 4);
    assert_eq!(decodes[0].temporal_unit_index, 0);
    assert_eq!(decodes[0].coded_width, 320);
    assert_eq!(decodes[0].coded_height, 180);
    assert_eq!(decodes[0].picture_info.frame_header_offset, 0);
    assert_eq!(decodes[1].src_buffer_offset as usize, second_frame_start);
    assert_eq!(decodes[1].src_buffer_range, 5);
    assert_eq!(decodes[1].temporal_unit_index, 1);
    assert_eq!(decodes[1].picture_info.frame_header_offset, 0);
}

#[test]
fn key_frame_decode_command_skeleton_rotates_setup_slots() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    bitstream.extend_from_slice(&make_obu(6, &[0x11]));
    bitstream.extend_from_slice(&make_obu(2, &[]));
    bitstream.extend_from_slice(&make_obu(6, &[0x21, 0x22]));
    bitstream.extend_from_slice(&make_obu(2, &[]));
    bitstream.extend_from_slice(&make_obu(6, &[0x31, 0x32, 0x33]));

    let command = build_av1_key_frame_decode_command_skeleton(&bitstream, 2)
        .expect("multi-frame key-frame stream should produce a command skeleton");

    assert_eq!(command.coded_width, 320);
    assert_eq!(command.coded_height, 180);
    assert_eq!(
        command.begin_slots,
        vec![
            Av1BeginCodingSlotSkeleton {
                slot_index: 0,
                base_array_layer: 0,
            },
            Av1BeginCodingSlotSkeleton {
                slot_index: 1,
                base_array_layer: 1,
            }
        ]
    );
    assert_eq!(command.frames.len(), 3);
    assert_eq!(
        command
            .frames
            .iter()
            .map(|frame| frame.setup_slot_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 0]
    );
    assert_eq!(
        command
            .frames
            .iter()
            .map(|frame| frame.temporal_unit_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        command
            .frames
            .iter()
            .map(|frame| frame.tile_count)
            .collect::<Vec<_>>(),
        vec![1, 1, 1]
    );

    let image_plan = command
        .decode_image_plan(vk::Format::G8_B8R8_2PLANE_420_UNORM)
        .expect("command should produce a decode image plan");
    assert_eq!(image_plan.format, vk::Format::G8_B8R8_2PLANE_420_UNORM);
    assert_eq!(image_plan.extent.width, 320);
    assert_eq!(image_plan.extent.height, 180);
    assert_eq!(image_plan.array_layers, 3);
    assert!(
        image_plan
            .usage
            .contains(vk::ImageUsageFlags::VIDEO_DECODE_DST_KHR)
    );
    assert!(
        image_plan
            .usage
            .contains(vk::ImageUsageFlags::VIDEO_DECODE_DPB_KHR)
    );
    assert!(image_plan.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC));
    let image_create_info = command.vk_decode_image_create_info(&image_plan);
    assert_eq!(image_create_info.image_type, vk::ImageType::TYPE_2D);
    assert_eq!(image_create_info.format, image_plan.format);
    assert_eq!(image_create_info.extent.width, 320);
    assert_eq!(image_create_info.extent.height, 180);
    assert_eq!(image_create_info.array_layers, 3);
    assert_eq!(image_create_info.usage, image_plan.usage);
    let source_barrier = Av1DecodeCommandSkeleton::vk_decode_source_memory_barrier();
    assert_eq!(source_barrier.src_stage_mask, vk::PipelineStageFlags2::HOST);
    assert_eq!(
        source_barrier.dst_stage_mask,
        vk::PipelineStageFlags2::VIDEO_DECODE_KHR
    );
    assert_eq!(
        source_barrier.dst_access_mask,
        vk::AccessFlags2::VIDEO_DECODE_READ_KHR
    );
    let image_barrier = command
        .vk_decode_image_init_barrier(vk::Image::null())
        .expect("command should produce an image initialization barrier");
    assert_eq!(image_barrier.old_layout, vk::ImageLayout::UNDEFINED);
    assert_eq!(
        image_barrier.new_layout,
        vk::ImageLayout::VIDEO_DECODE_DPB_KHR
    );
    assert_eq!(
        image_barrier.dst_stage_mask,
        vk::PipelineStageFlags2::VIDEO_DECODE_KHR
    );
    assert_eq!(
        image_barrier.dst_access_mask,
        vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR
    );
    assert_eq!(image_barrier.subresource_range.layer_count, 3);
    let readback_plan = command
        .decode_readback_plan(vk::Format::G8_B8R8_2PLANE_420_UNORM)
        .expect("NV12 readback plan should be built");
    assert_eq!(readback_plan.sample_count, 3);
    assert_eq!(readback_plan.sample_stride, 320 * 180 + 320 * 90);
    assert_eq!(readback_plan.buffer_size, (320 * 180 + 320 * 90) * 3);
    assert_eq!(readback_plan.regions.len(), 6);
    assert_eq!(
        readback_plan.regions[0].image_subresource.aspect_mask,
        vk::ImageAspectFlags::PLANE_0
    );
    assert_eq!(
        readback_plan.regions[1].image_subresource.aspect_mask,
        vk::ImageAspectFlags::PLANE_1
    );
    assert_eq!(readback_plan.regions[1].buffer_offset, 320 * 180);
    assert_eq!(readback_plan.regions[1].image_extent.width, 160);
    assert_eq!(readback_plan.regions[1].image_extent.height, 90);

    let begin_resources = command.begin_picture_resources(vk::ImageView::null());
    assert_eq!(begin_resources.len(), 2);
    assert_eq!(begin_resources[0].coded_extent.width, 320);
    assert_eq!(begin_resources[0].coded_extent.height, 180);
    assert_eq!(begin_resources[0].base_array_layer, 0);
    assert_eq!(begin_resources[1].base_array_layer, 1);
    assert_eq!(begin_resources[1].image_view_binding, vk::ImageView::null());

    let frame_resources = command
        .frame_picture_resources(vk::ImageView::null())
        .expect("frame setup slots should map to picture resources");
    assert_eq!(frame_resources.len(), 3);
    assert_eq!(frame_resources[0].base_array_layer, 0);
    assert_eq!(frame_resources[1].base_array_layer, 1);
    assert_eq!(frame_resources[2].base_array_layer, 2);
    assert_eq!(frame_resources[2].coded_extent.width, 320);
    assert_eq!(frame_resources[2].coded_extent.height, 180);

    let frame_bundles = command
        .frame_record_bundles()
        .expect("frame bundles should align with decode info indices");
    assert_eq!(
        frame_bundles,
        vec![
            Av1DecodeFrameRecordBundle {
                frame_index: 0,
                temporal_unit_index: 0,
                decode_info_index: 0,
                setup_slot_index: 0,
                reference_slot_indices: vec![],
                reference_base_array_layers: vec![],
                dst_base_array_layer: 0,
                src_buffer_offset: command.frames[0].src_buffer_offset,
                src_buffer_range: 3,
                tile_count: 1,
            },
            Av1DecodeFrameRecordBundle {
                frame_index: 1,
                temporal_unit_index: 1,
                decode_info_index: 1,
                setup_slot_index: 1,
                reference_slot_indices: vec![],
                reference_base_array_layers: vec![],
                dst_base_array_layer: 1,
                src_buffer_offset: command.frames[1].src_buffer_offset,
                src_buffer_range: 4,
                tile_count: 1,
            },
            Av1DecodeFrameRecordBundle {
                frame_index: 2,
                temporal_unit_index: 2,
                decode_info_index: 2,
                setup_slot_index: 0,
                reference_slot_indices: vec![],
                reference_base_array_layers: vec![],
                dst_base_array_layer: 2,
                src_buffer_offset: command.frames[2].src_buffer_offset,
                src_buffer_range: 5,
                tile_count: 1,
            },
        ]
    );

    let begin_reference_infos = command.begin_std_reference_infos();
    assert_eq!(begin_reference_infos.len(), 2);
    assert_eq!(
        u32::from(begin_reference_infos[0].frame_type),
        StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY
    );
    assert_eq!(begin_reference_infos[0].OrderHint, 0);
    let mut begin_dpb_infos = command
        .begin_dpb_slot_infos(&begin_reference_infos)
        .expect("matching begin reference infos should produce DPB slot infos");
    let begin_reference_slots = command
        .begin_reference_slots(&begin_resources, &mut begin_dpb_infos)
        .expect("matching begin resources and DPB infos should produce reference slots");
    assert!(begin_reference_slots.is_empty());

    let begin_coding_info = command
        .vk_begin_coding_info(
            vk::VideoSessionKHR::null(),
            vk::VideoSessionParametersKHR::null(),
            &begin_reference_slots,
        )
        .expect("matching begin reference slots should build begin coding info");
    assert_eq!(begin_coding_info.video_session, vk::VideoSessionKHR::null());
    assert_eq!(
        begin_coding_info.video_session_parameters,
        vk::VideoSessionParametersKHR::null()
    );
    assert_eq!(begin_coding_info.reference_slot_count, 0);

    let reset_control = command.vk_reset_coding_control_info();
    assert!(
        reset_control
            .flags
            .contains(vk::VideoCodingControlFlagsKHR::RESET)
    );

    let end_coding_info = command.vk_end_coding_info();
    assert!(end_coding_info.flags.is_empty());

    assert_eq!(
        command.record_steps(),
        vec![
            Av1DecodeRecordStep::BeginCoding {
                reference_slot_count: 0,
            },
            Av1DecodeRecordStep::ResetCoding,
            Av1DecodeRecordStep::DecodeFrame {
                frame_index: 0,
                temporal_unit_index: 0,
                setup_slot_index: 0,
                reference_slot_count: 0,
                src_buffer_offset: command.frames[0].src_buffer_offset,
                src_buffer_range: 3,
                tile_count: 1,
            },
            Av1DecodeRecordStep::DecodeFrame {
                frame_index: 1,
                temporal_unit_index: 1,
                setup_slot_index: 1,
                reference_slot_count: 0,
                src_buffer_offset: command.frames[1].src_buffer_offset,
                src_buffer_range: 4,
                tile_count: 1,
            },
            Av1DecodeRecordStep::DecodeFrame {
                frame_index: 2,
                temporal_unit_index: 2,
                setup_slot_index: 0,
                reference_slot_count: 0,
                src_buffer_offset: command.frames[2].src_buffer_offset,
                src_buffer_range: 5,
                tile_count: 1,
            },
            Av1DecodeRecordStep::EndCoding,
        ]
    );
}

#[test]
fn aligned_decode_upload_plan_pads_offsets_and_ranges() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    bitstream.extend_from_slice(&make_obu(6, &[0x11, 0x12]));
    bitstream.extend_from_slice(&make_obu(2, &[]));
    bitstream.extend_from_slice(&make_obu(6, &[0x21, 0x22, 0x23]));

    let plan = build_av1_aligned_decode_bitstream_upload_plan(&bitstream, 8, 4, 4)
        .expect("aligned upload plan should be built for multi-frame AV1");

    assert_eq!(plan.decodes.len(), 2);
    assert_eq!(plan.decodes[0].src_buffer_offset, 0);
    assert_eq!(plan.decodes[0].src_buffer_range, 4);
    assert_eq!(plan.decodes[1].src_buffer_offset, 8);
    assert_eq!(plan.decodes[1].src_buffer_range, 8);
    assert_eq!(&plan.bytes[0..4], &[0x32, 0x02, 0x11, 0x12]);
    assert_eq!(&plan.bytes[4..8], &[0; 4]);
    assert_eq!(&plan.bytes[8..13], &[0x32, 0x03, 0x21, 0x22, 0x23]);
    assert_eq!(&plan.bytes[13..16], &[0; 3]);
    assert_eq!(plan.decodes[0].picture_info.tile_offsets, vec![2]);
    assert_eq!(plan.decodes[1].picture_info.tile_offsets, vec![2]);
}

#[test]
fn aligned_key_frame_decode_command_uses_upload_offsets() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    bitstream.extend_from_slice(&make_obu(6, &[0x11, 0x12]));
    bitstream.extend_from_slice(&make_obu(2, &[]));
    bitstream.extend_from_slice(&make_obu(6, &[0x21, 0x22, 0x23]));

    let (plan, command) = build_av1_aligned_key_frame_decode_command_skeleton(&bitstream, 2, 16, 8)
        .expect("aligned command skeleton should use aligned upload offsets");

    assert_eq!(plan.decodes.len(), 2);
    assert_eq!(command.frames.len(), 2);
    assert_eq!(command.frames[0].src_buffer_offset, 0);
    assert_eq!(command.frames[0].src_buffer_range, 8);
    assert_eq!(command.frames[1].src_buffer_offset, 16);
    assert_eq!(command.frames[1].src_buffer_range, 8);
    assert_eq!(command.frames[1].setup_slot_index, 1);
    assert_eq!(
        plan.frame_upload_ranges(&command)
            .expect("command frames should map into upload byte ranges"),
        vec![0..8, 16..24]
    );
    assert_eq!(
        plan.frame_submit_bundles(&command)
            .expect("aligned plan and command should produce submit bundles"),
        vec![
            Av1DecodeFrameSubmitBundle {
                frame_index: 0,
                temporal_unit_index: 0,
                decode_info_index: 0,
                setup_slot_index: 0,
                reference_slot_indices: vec![],
                reference_base_array_layers: vec![],
                dst_base_array_layer: 0,
                src_buffer_offset: 0,
                src_buffer_range: 8,
                tile_count: 1,
                upload_range: 0..8,
            },
            Av1DecodeFrameSubmitBundle {
                frame_index: 1,
                temporal_unit_index: 1,
                decode_info_index: 1,
                setup_slot_index: 1,
                reference_slot_indices: vec![],
                reference_base_array_layers: vec![],
                dst_base_array_layer: 1,
                src_buffer_offset: 16,
                src_buffer_range: 8,
                tile_count: 1,
                upload_range: 16..24,
            },
        ]
    );

    let dst_view = vk::ImageView::from_raw(0xD57);
    let reference_view = vk::ImageView::from_raw(0xD9B);
    let summary = plan
        .with_frame_decode_info(
            &command,
            1,
            vk::Buffer::null(),
            Av1DecodePictureViews {
                dst: dst_view,
                reference: reference_view,
            },
            |decode_info, bundle| {
                let av1_info = decode_info
                    .p_next
                    .cast::<vk::VideoDecodeAV1PictureInfoKHR<'_>>();
                // SAFETY: `with_frame_decode_info` materializes the AV1 pNext chain for the
                // duration of this callback.
                let std_info = unsafe { &*((*av1_info).p_std_picture_info) };
                let std_pointer_ready = [
                    !std_info.pTileInfo.is_null(),
                    !std_info.pQuantization.is_null(),
                    !std_info.pSegmentation.is_null(),
                    !std_info.pLoopFilter.is_null(),
                    !std_info.pCDEF.is_null(),
                    !std_info.pLoopRestoration.is_null(),
                    !std_info.pGlobalMotion.is_null(),
                ];
                (
                    bundle.frame_index,
                    decode_info.src_buffer_offset,
                    decode_info.src_buffer_range,
                    decode_info.dst_picture_resource.base_array_layer,
                    decode_info.dst_picture_resource.image_view_binding,
                    if decode_info.p_setup_reference_slot.is_null() {
                        vk::ImageView::null()
                    } else {
                        // SAFETY: `with_frame_decode_info` keeps the setup reference slot and
                        // its picture resource alive for this callback.
                        unsafe {
                            (*(*decode_info.p_setup_reference_slot).p_picture_resource)
                                .image_view_binding
                        }
                    },
                    !decode_info.p_next.is_null(),
                    !decode_info.p_setup_reference_slot.is_null(),
                    std_pointer_ready,
                )
            },
        )
        .expect("frame decode info should be materialized inside the callback");
    assert_eq!(
        summary,
        (
            1,
            16,
            8,
            1,
            dst_view,
            reference_view,
            true,
            true,
            [true, true, true, true, true, true, true]
        )
    );

    let loop_summaries = plan
        .with_frame_decode_infos(
            &command,
            vk::Buffer::null(),
            Av1DecodePictureViews {
                dst: vk::ImageView::null(),
                reference: vk::ImageView::null(),
            },
            |decode_info, bundle| {
                (
                    bundle.frame_index,
                    bundle.setup_slot_index,
                    decode_info.src_buffer_offset,
                    decode_info.dst_picture_resource.base_array_layer,
                )
            },
        )
        .expect("all frame decode infos should materialize in command order");
    assert_eq!(loop_summaries, vec![(0, 0, 0, 0), (1, 1, 16, 1)]);

    let mut visits = Vec::new();
    plan.visit_decode_command_sequence(
        &command,
        vk::VideoSessionKHR::null(),
        vk::VideoSessionParametersKHR::null(),
        vk::Buffer::null(),
        Av1DecodePictureViews {
            dst: vk::ImageView::null(),
            reference: vk::ImageView::null(),
        },
        |visit| match visit {
            Av1DecodeCommandVisit::BeginCoding(info) => {
                visits.push(format!("begin:{}", info.reference_slot_count));
            }
            Av1DecodeCommandVisit::ResetCoding(info) => {
                visits.push(format!(
                    "reset:{}",
                    info.flags.contains(vk::VideoCodingControlFlagsKHR::RESET)
                ));
            }
            Av1DecodeCommandVisit::DecodeFrame {
                decode_info,
                bundle,
            } => {
                visits.push(format!(
                    "decode:{}:{}:{}",
                    bundle.frame_index,
                    decode_info.src_buffer_offset,
                    decode_info.dst_picture_resource.base_array_layer
                ));
            }
            Av1DecodeCommandVisit::EndCoding(info) => {
                visits.push(format!("end:{}", info.flags.is_empty()));
            }
        },
    )
    .expect("command sequence visitor should materialize command info in order");
    assert_eq!(
        visits,
        vec![
            "begin:0".to_string(),
            "reset:true".to_string(),
            "decode:0:0:0".to_string(),
            "decode:1:16:1".to_string(),
            "end:true".to_string(),
        ]
    );

    let mut recorded = Vec::new();
    let record_summary = plan
        .record_decode_command_sequence(
            &command,
            vk::VideoSessionKHR::null(),
            vk::VideoSessionParametersKHR::null(),
            vk::Buffer::null(),
            Av1DecodePictureViews {
                dst: vk::ImageView::null(),
                reference: vk::ImageView::null(),
            },
            |visit| {
                recorded.push(match visit {
                    Av1DecodeCommandVisit::BeginCoding(_) => "begin",
                    Av1DecodeCommandVisit::ResetCoding(_) => "reset",
                    Av1DecodeCommandVisit::DecodeFrame { .. } => "decode",
                    Av1DecodeCommandVisit::EndCoding(_) => "end",
                });
                Ok(())
            },
        )
        .expect("record callback should receive all command steps");
    assert_eq!(
        record_summary,
        Av1DecodeCommandRecordSummary {
            begin_count: 1,
            reset_count: 1,
            decode_count: 2,
            end_count: 1,
            decode_query_status_raw: None,
            first_error: None,
        }
    );
    record_summary
        .validate_for_command(&command)
        .expect("record summary should match the planned command sequence");
    assert_eq!(recorded, vec!["begin", "reset", "decode", "decode", "end"]);

    let err = Av1DecodeCommandRecordSummary {
        begin_count: 1,
        reset_count: 1,
        decode_count: 1,
        end_count: 1,
        decode_query_status_raw: None,
        first_error: None,
    }
    .validate_for_command(&command)
    .expect_err("decode count mismatch should be rejected");
    assert!(err.contains("expected 2 decode commands"));
}

#[test]
fn aligned_upload_ranges_reject_command_decode_mismatch() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    bitstream.extend_from_slice(&make_obu(6, &[0x11, 0x12]));
    let (plan, mut command) =
        build_av1_aligned_key_frame_decode_command_skeleton(&bitstream, 1, 16, 8)
            .expect("single frame should produce an aligned command skeleton");
    command.frames[0].src_buffer_range = 4;

    let err = plan
        .frame_upload_ranges(&command)
        .expect_err("command/decode source range mismatch should be rejected");

    assert!(err.contains("source range mismatch"));
}

#[test]
fn key_frame_decode_command_skeleton_rejects_zero_dpb_slots() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    bitstream.extend_from_slice(&make_obu(6, &[0x11]));

    let err = build_av1_key_frame_decode_command_skeleton(&bitstream, 0)
        .expect_err("zero DPB slots should be rejected");

    assert!(err.contains("requires at least one DPB slot"));
}

#[test]
fn begin_reference_slots_reject_mismatched_resource_counts() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    bitstream.extend_from_slice(&make_obu(6, &[0x11]));
    let command = build_av1_key_frame_decode_command_skeleton(&bitstream, 2)
        .expect("single frame should produce a command skeleton");
    let begin_reference_infos = command.begin_std_reference_infos();
    let mut begin_dpb_infos = command
        .begin_dpb_slot_infos(&begin_reference_infos)
        .expect("matching begin reference infos should produce DPB slot infos");

    let err = command
        .begin_reference_slots(&[], &mut begin_dpb_infos)
        .expect_err("resource count mismatch should be rejected");

    assert!(err.contains("picture resource count mismatch"));
}

#[test]
fn frame_record_bundles_reject_unavailable_setup_slots() {
    let command = Av1DecodeCommandSkeleton {
        coded_width: 320,
        coded_height: 180,
        begin_slots: vec![Av1BeginCodingSlotSkeleton {
            slot_index: 0,
            base_array_layer: 0,
        }],
        frames: vec![Av1DecodeFrameCommandSkeleton {
            frame_index: 0,
            temporal_unit_index: 0,
            setup_slot_index: 1,
            dst_base_array_layer: 0,
            reference_slot_indices: vec![],
            reference_base_array_layers: vec![],
            src_buffer_offset: 12,
            src_buffer_range: 3,
            tile_count: 1,
        }],
    };

    let err = command
        .frame_record_bundles()
        .expect_err("unavailable setup slot should be rejected");

    assert!(err.contains("unavailable setup slot"));
}

#[test]
fn frame_record_bundles_allow_slot_base_layer_decoupling() {
    let command = Av1DecodeCommandSkeleton {
        coded_width: 320,
        coded_height: 180,
        begin_slots: vec![Av1BeginCodingSlotSkeleton {
            slot_index: 1,
            base_array_layer: 0,
        }],
        frames: vec![Av1DecodeFrameCommandSkeleton {
            frame_index: 0,
            temporal_unit_index: 0,
            setup_slot_index: 1,
            dst_base_array_layer: 7,
            reference_slot_indices: vec![],
            reference_base_array_layers: vec![],
            src_buffer_offset: 12,
            src_buffer_range: 3,
            tile_count: 1,
        }],
    };

    let resources = command
        .frame_picture_resources(vk::ImageView::null())
        .expect("logical DPB slot and output layer may differ");

    assert_eq!(resources[0].base_array_layer, 7);
}

#[test]
fn begin_coding_info_accepts_empty_reference_slots_after_reset() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    bitstream.extend_from_slice(&make_obu(6, &[0x11]));
    let command = build_av1_key_frame_decode_command_skeleton(&bitstream, 2)
        .expect("single frame should produce a command skeleton");

    let begin = command
        .vk_begin_coding_info(
            vk::VideoSessionKHR::null(),
            vk::VideoSessionParametersKHR::null(),
            &[],
        )
        .expect("reset decode scope should start without active reference slots");

    assert_eq!(begin.reference_slot_count, 0);
    assert_eq!(begin.reference_slot_count, 0);
}

#[test]
fn decode_info_skeleton_builds_vk_decode_info_chain() {
    let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
    let frame_obu_start = bitstream.len();
    bitstream.extend_from_slice(&make_obu(6, &[0xaa, 0xbb, 0xcc]));
    let decode = build_av1_decode_info_skeleton(&bitstream)
        .expect("frame OBU should produce a decode info skeleton");
    let mut av1_picture_info = decode.picture_info.vk_picture_info();
    let av1_picture_info_ptr = &raw const av1_picture_info;
    let dst_picture_resource = decode.dst_picture_resource(vk::ImageView::null(), 2);
    let mut setup_dpb_info = decode.vk_setup_dpb_slot_info();
    let setup_dpb_info_ptr = &raw const setup_dpb_info;
    assert_eq!(
        setup_dpb_info.p_std_reference_info,
        &raw const decode.setup_reference_info
    );
    let setup_reference_slot =
        decode.vk_setup_reference_slot(2, &dst_picture_resource, &mut setup_dpb_info);

    let vk_decode = decode.vk_decode_info_with_setup_reference_slot(
        vk::Buffer::null(),
        dst_picture_resource,
        &setup_reference_slot,
        &mut av1_picture_info,
    );

    assert_eq!(vk_decode.src_buffer, vk::Buffer::null());
    assert_eq!(vk_decode.src_buffer_offset as usize, frame_obu_start);
    assert_eq!(vk_decode.src_buffer_range, 5);
    assert_eq!(vk_decode.dst_picture_resource.coded_offset.x, 0);
    assert_eq!(vk_decode.dst_picture_resource.coded_offset.y, 0);
    assert_eq!(vk_decode.dst_picture_resource.coded_extent.width, 320);
    assert_eq!(vk_decode.dst_picture_resource.coded_extent.height, 180);
    assert_eq!(vk_decode.dst_picture_resource.base_array_layer, 2);
    assert_eq!(
        vk_decode.dst_picture_resource.image_view_binding,
        vk::ImageView::null()
    );
    assert_eq!(
        vk_decode
            .p_next
            .cast::<vk::VideoDecodeAV1PictureInfoKHR<'_>>(),
        av1_picture_info_ptr
    );
    assert_eq!(setup_reference_slot.slot_index, 2);
    assert_eq!(
        setup_reference_slot.p_picture_resource,
        &raw const dst_picture_resource
    );
    assert_eq!(
        setup_reference_slot
            .p_next
            .cast::<vk::VideoDecodeAV1DpbSlotInfoKHR<'_>>(),
        setup_dpb_info_ptr
    );
    assert_eq!(
        vk_decode.p_setup_reference_slot,
        &raw const setup_reference_slot
    );
}

#[test]
fn decode_info_source_range_rejects_tiles_beyond_bitstream() {
    let picture = Av1DecodePictureInfoSkeleton {
        std_picture_info: key_frame_std_picture_info(),
        reference_name_slot_indices: [-1; vk::MAX_VIDEO_AV1_REFERENCES_PER_FRAME_KHR],
        frame_header_offset: 4,
        tile_offsets: vec![8],
        tile_sizes: vec![5],
        use_128x128_superblock: false,
        key_frame_header: None,
        frame_header_summary: None,
        std_overrides: None,
    };

    let err = av1_decode_source_range(12, &picture)
        .expect_err("tile payload beyond the bitstream should be rejected");
    assert!(err.contains("exceeds bitstream length"));
}

#[test]
fn av1_decode_probe_returns_known_status_variant() {
    let status = probe_av1_decode_prerequisites();
    match status {
        Av1DecodePrerequisiteProbe::Ready
        | Av1DecodePrerequisiteProbe::MissingExtensions { .. }
        | Av1DecodePrerequisiteProbe::MissingDecodeQueueFamily
        | Av1DecodePrerequisiteProbe::NoCompatibleAdapter
        | Av1DecodePrerequisiteProbe::SessionBootstrapFailed(_)
        | Av1DecodePrerequisiteProbe::ProbeUnavailable(_) => {}
    }
}

#[test]
fn av1_decode_readback_plan_handles_odd_nv12_dimensions() {
    let plan = build_av1_decode_readback_plan(vk::Format::G8_B8R8_2PLANE_420_UNORM, 641, 479, 1)
        .expect("odd NV12 dimensions should produce a readback plan");

    assert_eq!(plan.regions.len(), 2);
    assert_eq!(plan.sample_count, 1);
    assert_eq!(plan.sample_stride, plan.buffer_size);
    assert_eq!(plan.regions[0].buffer_offset, 0);
    assert_eq!(plan.regions[0].buffer_row_length, 641);
    assert_eq!(plan.regions[0].buffer_image_height, 479);
    assert_eq!(plan.regions[1].buffer_row_length, 321);
    assert_eq!(plan.regions[1].buffer_image_height, 240);
    assert_eq!(plan.regions[1].image_extent.width, 321);
    assert_eq!(plan.regions[1].image_extent.height, 240);
    assert_eq!(plan.regions[1].buffer_offset % 4, 0);
    assert_eq!(
        plan.buffer_size,
        plan.regions[1].buffer_offset + 321 * 240 * 2
    );
}

#[test]
fn av1_decode_readback_plan_copies_each_image_layer() {
    let plan = build_av1_decode_readback_plan(vk::Format::G8_B8R8_2PLANE_420_UNORM, 320, 180, 3)
        .expect("multi-layer NV12 readback should produce a readback plan");

    assert_eq!(plan.sample_count, 3);
    assert_eq!(plan.sample_stride, 320 * 180 + 320 * 90);
    assert_eq!(plan.buffer_size, plan.sample_stride * 3);
    assert_eq!(plan.regions.len(), 6);
    assert_eq!(plan.regions[0].image_subresource.base_array_layer, 0);
    assert_eq!(plan.regions[1].image_subresource.base_array_layer, 0);
    assert_eq!(plan.regions[2].image_subresource.base_array_layer, 1);
    assert_eq!(plan.regions[3].image_subresource.base_array_layer, 1);
    assert_eq!(plan.regions[4].image_subresource.base_array_layer, 2);
    assert_eq!(plan.regions[5].image_subresource.base_array_layer, 2);
    assert_eq!(plan.regions[2].buffer_offset, plan.sample_stride);
    assert_eq!(plan.regions[4].buffer_offset, plan.sample_stride * 2);
}

#[test]
fn av1_decode_readback_plan_rejects_unsupported_format() {
    let err = build_av1_decode_readback_plan(vk::Format::D32_SFLOAT, 320, 180, 1)
        .expect_err("unsupported readback format should be rejected");
    assert!(err.contains("not implemented"));
}

#[test]
#[ignore = "live Vulkan AV1 command-buffer record probe; opt in explicitly"]
fn live_av1_decode_command_record_probe_reports_status() {
    let bitstream = if let Ok(path) = std::env::var("VIDEO_HW_VULKAN_AV1_PROBE_BITSTREAM_PATH") {
        std::fs::read(&path)
            .unwrap_or_else(|err| panic!("failed to read AV1 probe bitstream {path}: {err}"))
    } else {
        let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
        bitstream.extend_from_slice(&make_obu(6, &[0x11, 0x12, 0x13, 0x14]));
        bitstream
    };

    match probe_av1_decode_session_parameters_for_bitstream(&bitstream) {
        Ok(probe) => {
            eprintln!(
                "AV1 Vulkan command record probe: coded={}x{}, format={:?}, upload_bytes={}, image_layers={}, barrier_layers={}, readback_bytes={}, readback_regions={}, readback_mapped_bytes={}, readback_non_zero={}, readback_sample_len={}, record_decodes={}, command_buffer_recorded={}, command_buffer_submitted={}, decode_query_status_raw={:?}",
                probe.coded_width,
                probe.coded_height,
                probe.picture_format,
                probe.bitstream_upload_bytes,
                probe.decode_image_layers,
                probe.decode_image_barrier_layers,
                probe.readback_bytes,
                probe.readback_region_count,
                probe.readback_mapped_bytes,
                probe.readback_non_zero,
                probe.readback_sample.len(),
                probe.command_record_decode_count,
                probe.command_buffer_recorded,
                probe.command_buffer_submitted,
                probe.decode_query_status_raw
            );
            if std::env::var("VIDEO_HW_VULKAN_AV1_RECORD_COMMAND_BUFFER").as_deref() == Ok("1") {
                assert!(
                    probe.command_buffer_recorded,
                    "record probe env was set but command_buffer_recorded=false"
                );
            }
            if std::env::var("VIDEO_HW_VULKAN_AV1_SUBMIT_COMMAND_BUFFER").as_deref() == Ok("1") {
                assert!(
                    probe.command_buffer_submitted,
                    "submit probe env was set but command_buffer_submitted=false"
                );
            }
            if std::env::var("VIDEO_HW_VULKAN_AV1_READBACK").as_deref() == Ok("1") {
                assert_eq!(
                    probe.readback_mapped_bytes as u64, probe.readback_bytes,
                    "readback probe did not map the planned readback byte range"
                );
                assert!(
                    !probe.readback_sample.is_empty(),
                    "readback probe mapped no sample bytes"
                );
            }
        }
        Err(err) => {
            eprintln!("AV1 Vulkan command record probe unavailable: {err}");
        }
    }
}

fn make_obu(obu_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![(obu_type << 3) | 0x02];
    out.extend(write_leb128(payload.len()));
    out.extend_from_slice(payload);
    out
}

fn av1_reduced_still_sequence_header_payload(width: u32, height: u32) -> Vec<u8> {
    let width_minus_1 = width.checked_sub(1).expect("width must be positive");
    let height_minus_1 = height.checked_sub(1).expect("height must be positive");
    let width_bits = 32 - width_minus_1.leading_zeros();
    let height_bits = 32 - height_minus_1.leading_zeros();
    let mut writer = BitWriter::default();
    writer.write_bits(0, 3);
    writer.write_bits(1, 1);
    writer.write_bits(1, 1);
    writer.write_bits(0, 5);
    writer.write_bits(u64::from(width_bits - 1), 4);
    writer.write_bits(u64::from(height_bits - 1), 4);
    writer.write_bits(u64::from(width_minus_1), width_bits as usize);
    writer.write_bits(u64::from(height_minus_1), height_bits as usize);
    writer.write_bits(0, 1);
    writer.write_bits(0, 1);
    writer.write_bits(0, 1);
    writer.finish()
}

fn write_leb128(mut value: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
    out
}

#[derive(Default)]
struct BitWriter {
    data: Vec<u8>,
    bit_offset: usize,
}

impl BitWriter {
    fn write_bits(&mut self, value: u64, count: usize) {
        for shift in (0..count).rev() {
            if self.bit_offset.is_multiple_of(8) {
                self.data.push(0);
            }
            let bit = ((value >> shift) & 1) as u8;
            let byte = self.data.last_mut().expect("byte exists after push");
            *byte |= bit << (7 - (self.bit_offset % 8));
            self.bit_offset += 1;
        }
    }

    fn finish(self) -> Vec<u8> {
        self.data
    }
}
