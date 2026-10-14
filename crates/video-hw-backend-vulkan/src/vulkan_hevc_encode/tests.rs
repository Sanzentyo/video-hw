use super::*;

#[test]
fn extension_flags_require_all_three_hevc_encode_extensions() {
    let mut flags = ExtensionFlags::default();
    assert!(!flags.supports_hevc_encode());
    flags.has_video_queue = true;
    assert!(!flags.supports_hevc_encode());
    flags.has_video_encode_queue = true;
    assert!(!flags.supports_hevc_encode());
    flags.has_video_encode_h265 = true;
    assert!(flags.supports_hevc_encode());
}

#[test]
fn find_video_codec_queue_family_index_requires_h265_encode_operation() {
    let mut queue_family_properties = vec![vk::QueueFamilyProperties::default(); 2];
    queue_family_properties[0].queue_count = 1;
    queue_family_properties[0].queue_flags = vk::QueueFlags::VIDEO_ENCODE_KHR;
    queue_family_properties[1].queue_count = 1;
    queue_family_properties[1].queue_flags = vk::QueueFlags::VIDEO_ENCODE_KHR;

    let codec_operations = vec![
        vk::VideoCodecOperationFlagsKHR::DECODE_H265,
        vk::VideoCodecOperationFlagsKHR::ENCODE_H265,
    ];
    let queue_family_index = find_video_codec_queue_family_index(
        &queue_family_properties,
        &codec_operations,
        vk::QueueFlags::VIDEO_ENCODE_KHR,
        vk::VideoCodecOperationFlagsKHR::ENCODE_H265,
    );
    assert_eq!(queue_family_index, Some(1));
}

#[test]
fn find_video_codec_queue_family_index_falls_back_when_codec_metadata_absent() {
    let mut queue_family_properties = vec![vk::QueueFamilyProperties::default(); 1];
    queue_family_properties[0].queue_count = 1;
    queue_family_properties[0].queue_flags = vk::QueueFlags::VIDEO_ENCODE_KHR;
    let codec_operations = vec![vk::VideoCodecOperationFlagsKHR::empty()];
    let queue_family_index = find_video_codec_queue_family_index(
        &queue_family_properties,
        &codec_operations,
        vk::QueueFlags::VIDEO_ENCODE_KHR,
        vk::VideoCodecOperationFlagsKHR::ENCODE_H265,
    );
    assert_eq!(queue_family_index, Some(0));
}

#[test]
fn hevc_encode_probe_returns_stable_enum() {
    let status = probe_hevc_encode_prerequisites();
    match status {
        HevcEncodePrerequisiteProbe::Ready
        | HevcEncodePrerequisiteProbe::MissingExtensions { .. }
        | HevcEncodePrerequisiteProbe::MissingEncodeQueueFamily
        | HevcEncodePrerequisiteProbe::NoCompatibleAdapter
        | HevcEncodePrerequisiteProbe::DeviceInitializationFailed(_)
        | HevcEncodePrerequisiteProbe::ProbeUnavailable(_) => {}
    }
}

#[test]
fn hevc_encode_session_bootstrap_rejects_zero_dimensions() {
    let err = probe_hevc_encode_session_bootstrap(0, 720, 30)
        .expect_err("zero width must be rejected before Vulkan probing");
    assert!(err.contains("must be > 0"));
}

#[test]
#[ignore = "live Vulkan HEVC encode probe; may crash buggy drivers"]
fn live_hevc_encode_session_bootstrap_reports_submit_feedback() {
    let width = live_hevc_encode_probe_u32_env("VIDEO_HW_VULKAN_HEVC_ENCODE_LIVE_WIDTH", 320);
    let height = live_hevc_encode_probe_u32_env("VIDEO_HW_VULKAN_HEVC_ENCODE_LIVE_HEIGHT", 180);
    let fps = live_hevc_encode_probe_u32_env("VIDEO_HW_VULKAN_HEVC_ENCODE_LIVE_FPS", 30);
    let bootstrap = probe_hevc_encode_session_bootstrap(width, height, fps)
        .expect("live HEVC encode session bootstrap should complete on supported drivers");
    eprintln!("{bootstrap:#?}");
    match bootstrap.encode_submit_execution_probe {
        HevcEncodeSubmitExecutionProbe::Ready { bytes_written, .. } => {
            assert!(bytes_written > 0, "HEVC encode produced no bytes");
        }
        HevcEncodeSubmitExecutionProbe::Failed(err) => {
            panic!("HEVC encode submit probe failed: {err}");
        }
        HevcEncodeSubmitExecutionProbe::Skipped(reason) => {
            panic!("HEVC encode submit probe skipped: {reason}");
        }
    }
}

fn live_hevc_encode_probe_u32_env(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

#[test]
fn load_hevc_encode_probe_parameter_sample_defaults_to_repository_sample() {
    let sample = load_hevc_encode_probe_parameter_sample_from_path(None)
        .expect("default parameter sample should load from repository bytes");
    assert!(!sample.is_empty());
}

#[test]
fn load_hevc_encode_probe_parameter_sample_surfaces_missing_file_error() {
    let missing_path = std::env::temp_dir()
        .join("video-hw-missing-hevc-encode-parameter-sample-do-not-create.h265");
    let missing_path_str = missing_path.to_string_lossy().into_owned();
    let err = load_hevc_encode_probe_parameter_sample_from_path(Some(&missing_path_str))
        .expect_err("missing override path must surface an explicit read error");
    assert!(err.contains("failed to read HEVC encode probe parameter sample"));
}

#[test]
fn parse_hevc_encode_probe_parameter_size_mode_defaults_to_sample() {
    assert_eq!(
        parse_hevc_encode_probe_parameter_size_mode(None),
        HevcEncodeProbeParameterSizeMode::Sample
    );
    assert_eq!(
        parse_hevc_encode_probe_parameter_size_mode(Some("")),
        HevcEncodeProbeParameterSizeMode::Sample
    );
}

#[test]
fn parse_hevc_encode_probe_parameter_size_mode_accepts_coded_aliases() {
    for alias in ["coded", "probe", "probe-coded", "ffmpeg", "ffmpeg-like"] {
        assert_eq!(
            parse_hevc_encode_probe_parameter_size_mode(Some(alias)),
            HevcEncodeProbeParameterSizeMode::Coded
        );
    }
}

#[test]
fn parse_hevc_encode_probe_parameter_vui_safety_mode_defaults_to_auto() {
    assert_eq!(
        parse_hevc_encode_probe_parameter_vui_safety_mode(None),
        HevcEncodeProbeParameterVuiSafetyMode::Auto
    );
    assert_eq!(
        parse_hevc_encode_probe_parameter_vui_safety_mode(Some("")),
        HevcEncodeProbeParameterVuiSafetyMode::Auto
    );
}

#[test]
fn parse_hevc_encode_probe_parameter_vui_safety_mode_accepts_aliases() {
    for alias in ["preserve", "keep-vui", "ffmpeg"] {
        assert_eq!(
            parse_hevc_encode_probe_parameter_vui_safety_mode(Some(alias)),
            HevcEncodeProbeParameterVuiSafetyMode::Preserve
        );
    }
    for alias in ["force-off", "off", "disabled"] {
        assert_eq!(
            parse_hevc_encode_probe_parameter_vui_safety_mode(Some(alias)),
            HevcEncodeProbeParameterVuiSafetyMode::ForceOff
        );
    }
}

#[test]
fn parse_hevc_encode_probe_image_view_mode_defaults_to_ycbcr_conversion() {
    assert_eq!(
        parse_hevc_encode_probe_image_view_mode(None),
        HevcEncodeProbeImageViewMode::YcbcrConversion
    );
    assert_eq!(
        parse_hevc_encode_probe_image_view_mode(Some("")),
        HevcEncodeProbeImageViewMode::YcbcrConversion
    );
}

#[test]
fn parse_hevc_encode_probe_image_view_mode_accepts_no_ycbcr_aliases() {
    for alias in ["no-ycbcr", "no_ycbcr", "without-ycbcr", "plain"] {
        assert_eq!(
            parse_hevc_encode_probe_image_view_mode(Some(alias)),
            HevcEncodeProbeImageViewMode::NoYcbcrConversion
        );
    }
}

#[test]
fn hevc_encode_parameter_storage_can_override_probe_picture_size() {
    let sample = load_hevc_encode_probe_parameter_sample_from_path(None)
        .expect("default parameter sample should load from repository bytes");
    let parameter_sets = extract_hevc_parameter_sets_annexb(&sample)
        .expect("default parameter sample should parse parameter sets");
    let mut storage = build_hevc_std_parameter_set_storage(&parameter_sets)
        .expect("default parameter sample should build std parameter sets");
    assert_eq!(storage.encode_sps_picture_size(), (1920, 1080));
    storage.override_encode_sps_picture_size(320, 180);
    assert_eq!(storage.encode_sps_picture_size(), (320, 180));
}

#[test]
fn parse_hevc_encode_probe_dst_prefix_bytes_defaults_to_zero() {
    assert_eq!(parse_hevc_encode_probe_dst_prefix_bytes(None), 0);
    assert_eq!(parse_hevc_encode_probe_dst_prefix_bytes(Some("")), 0);
    assert_eq!(
        parse_hevc_encode_probe_dst_prefix_bytes(Some("not-a-number")),
        0
    );
}

#[test]
fn parse_hevc_encode_probe_dst_prefix_bytes_accepts_decimal_bytes() {
    assert_eq!(parse_hevc_encode_probe_dst_prefix_bytes(Some(" 256 ")), 256);
}

#[test]
fn parse_hevc_encode_probe_dst_prefix_mode_defaults_to_none() {
    assert_eq!(
        parse_hevc_encode_probe_dst_prefix_mode(None),
        HevcEncodeProbeDstPrefixMode::None
    );
    assert_eq!(
        parse_hevc_encode_probe_dst_prefix_mode(Some("")),
        HevcEncodeProbeDstPrefixMode::None
    );
}

#[test]
fn parse_hevc_encode_probe_dst_prefix_mode_accepts_fill_aliases() {
    assert_eq!(
        parse_hevc_encode_probe_dst_prefix_mode(Some("zero")),
        HevcEncodeProbeDstPrefixMode::Zero
    );
    for alias in ["parameter-sample", "sample", "ffmpeg"] {
        assert_eq!(
            parse_hevc_encode_probe_dst_prefix_mode(Some(alias)),
            HevcEncodeProbeDstPrefixMode::ParameterSample
        );
    }
}

#[test]
fn parse_hevc_encode_probe_dst_range_mode_defaults_to_full() {
    assert_eq!(
        parse_hevc_encode_probe_dst_range_mode(None),
        HevcEncodeProbeDstRangeMode::Full
    );
    assert_eq!(
        parse_hevc_encode_probe_dst_range_mode(Some("")),
        HevcEncodeProbeDstRangeMode::Full
    );
}

#[test]
fn parse_hevc_encode_probe_dst_range_mode_accepts_ffmpeg_aliases() {
    for alias in [
        "ffmpeg",
        "ffmpeg-reserve-align",
        "ffmpeg_reserve_align",
        "reserve-align",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_dst_range_mode(Some(alias)),
            HevcEncodeProbeDstRangeMode::FfmpegReserveAlign
        );
    }
}

#[test]
fn hevc_encode_probe_dst_buffer_range_can_reserve_ffmpeg_alignment() {
    assert_eq!(
        hevc_encode_probe_dst_buffer_range(1_048_576, 256, HevcEncodeProbeDstRangeMode::Full),
        1_048_576
    );
    assert_eq!(
        hevc_encode_probe_dst_buffer_range(
            1_048_576,
            256,
            HevcEncodeProbeDstRangeMode::FfmpegReserveAlign
        ),
        1_048_320
    );
}

#[test]
fn hevc_encode_probe_bitstream_memory_offset_is_relative_to_dst_buffer_offset() {
    let query_result = HevcEncodeFeedbackQueryResult {
        offset: 13,
        bytes_written: 47,
        status: vk::QueryResultStatusKHR::COMPLETE,
    };
    assert_eq!(
        hevc_encode_probe_bitstream_memory_offset(256, query_result).unwrap(),
        269
    );
}

#[test]
fn hevc_encode_head16_limits_preview_to_first_16_bytes() {
    let bytes = (0_u8..20).collect::<Vec<_>>();
    assert_eq!(
        hevc_encode_head16(&bytes),
        "000102030405060708090a0b0c0d0e0f"
    );
}

#[test]
fn hevc_encode_slice_header_flags_match_ffmpeg_baseline_shape() {
    let flags = hevc_encode_std_slice_segment_header_flags(true);
    assert_eq!(flags.first_slice_segment_in_pic_flag(), 1);
    assert_eq!(flags.dependent_slice_segment_flag(), 0);
    assert_eq!(flags.slice_sao_luma_flag(), 1);
    assert_eq!(flags.slice_sao_chroma_flag(), 1);
    assert_eq!(flags.num_ref_idx_active_override_flag(), 0);
    assert_eq!(flags.cabac_init_flag(), 0);
    assert_eq!(flags.collocated_from_l0_flag(), 1);
    assert_eq!(flags.slice_loop_filter_across_slices_enabled_flag(), 0);

    let no_sao_flags = hevc_encode_std_slice_segment_header_flags(false);
    assert_eq!(no_sao_flags.slice_sao_luma_flag(), 0);
    assert_eq!(no_sao_flags.slice_sao_chroma_flag(), 0);
    assert_eq!(no_sao_flags.collocated_from_l0_flag(), 1);
}

#[test]
fn hevc_encode_rate_control_info_matches_ffmpeg_baseline_shape() {
    let info = hevc_encode_h265_rate_control_info();
    assert!(
        info.flags
            .contains(vk::VideoEncodeH265RateControlFlagsKHR::REFERENCE_PATTERN_FLAT)
    );
    assert!(
        info.flags
            .contains(vk::VideoEncodeH265RateControlFlagsKHR::REGULAR_GOP)
    );
    assert_eq!(info.gop_frame_count, 30);
    assert_eq!(info.idr_period, 30);
    assert_eq!(info.consecutive_b_frame_count, 0);
    assert_eq!(info.sub_layer_count, 0);
}

#[test]
fn hevc_encode_constant_qp_matches_ffmpeg_rate_control_shape() {
    assert_eq!(hevc_encode_probe_constant_qp(None, None), 18);
    assert_eq!(hevc_encode_probe_constant_qp(None, Some("22")), 22);
    assert_eq!(hevc_encode_probe_constant_qp(None, Some("52")), 18);
    assert_eq!(hevc_encode_probe_constant_qp(None, Some("bad")), 18);
    assert_eq!(
        hevc_encode_probe_constant_qp(Some(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED), None,),
        18
    );
    assert_eq!(
        hevc_encode_probe_constant_qp(
            Some(vk::VideoEncodeRateControlModeFlagsKHR::CBR),
            Some("22"),
        ),
        0
    );
    assert_eq!(
        hevc_encode_probe_constant_qp(Some(vk::VideoEncodeRateControlModeFlagsKHR::VBR), None,),
        0
    );
}

#[test]
fn hevc_encode_quality_level_defaults_to_ffmpeg_zero() {
    assert_eq!(parse_hevc_encode_probe_quality_level(None, 7), 0);
    assert_eq!(parse_hevc_encode_probe_quality_level(Some(""), 7), 0);
    assert_eq!(parse_hevc_encode_probe_quality_level(Some("3"), 7), 3);
    assert_eq!(parse_hevc_encode_probe_quality_level(Some("99"), 7), 6);
    assert_eq!(parse_hevc_encode_probe_quality_level(Some("bad"), 7), 0);
    assert_eq!(parse_hevc_encode_probe_quality_level(Some("3"), 0), 0);
}

#[test]
fn hevc_encode_slice_qp_delta_matches_ffmpeg_formula() {
    assert_eq!(hevc_encode_probe_slice_qp_delta(18, 0).unwrap(), -8);
    assert_eq!(hevc_encode_probe_slice_qp_delta(0, 0).unwrap(), -26);
    assert_eq!(hevc_encode_probe_slice_qp_delta(26, -2).unwrap(), 2);
}

#[test]
fn build_hevc_encode_source_upload_regions_maps_nv12_planes() {
    let (buffer_size, regions) =
        build_hevc_encode_source_upload_regions(vk::Format::G8_B8R8_2PLANE_420_UNORM, 321, 181)
            .expect("NV12 upload regions should be supported");
    assert_eq!(regions.len(), 2);
    assert_eq!(
        regions[0].image_subresource.aspect_mask,
        vk::ImageAspectFlags::PLANE_0
    );
    assert_eq!(regions[0].image_extent.width, 321);
    assert_eq!(regions[0].image_extent.height, 181);
    assert_eq!(
        regions[1].image_subresource.aspect_mask,
        vk::ImageAspectFlags::PLANE_1
    );
    assert_eq!(regions[1].image_extent.width, 161);
    assert_eq!(regions[1].image_extent.height, 91);
    assert!(buffer_size >= regions[1].buffer_offset + 161_u64 * 91 * 2);
}

#[test]
fn parse_hevc_encode_parameter_mode_defaults_to_sample() {
    assert_eq!(
        parse_hevc_encode_parameter_mode(None),
        HevcEncodeParameterMode::Sample
    );
    assert_eq!(
        parse_hevc_encode_parameter_mode(Some("")),
        HevcEncodeParameterMode::Sample
    );
    assert_eq!(
        parse_hevc_encode_parameter_mode(Some("sample")),
        HevcEncodeParameterMode::Sample
    );
}

#[test]
fn resolve_hevc_encode_parameter_mode_with_vui_safety_applies_only_for_sample_override_vui() {
    assert_eq!(
        resolve_hevc_encode_parameter_mode_with_vui_safety(
            HevcEncodeParameterMode::Sample,
            true,
            true,
            HevcEncodeProbeParameterVuiSafetyMode::Auto
        ),
        HevcEncodeParameterMode::SampleSpsVuiFlagOff
    );
    assert_eq!(
        resolve_hevc_encode_parameter_mode_with_vui_safety(
            HevcEncodeParameterMode::Sample,
            false,
            true,
            HevcEncodeProbeParameterVuiSafetyMode::Auto
        ),
        HevcEncodeParameterMode::Sample
    );
    assert_eq!(
        resolve_hevc_encode_parameter_mode_with_vui_safety(
            HevcEncodeParameterMode::Sample,
            true,
            false,
            HevcEncodeProbeParameterVuiSafetyMode::Auto
        ),
        HevcEncodeParameterMode::Sample
    );
    assert_eq!(
        resolve_hevc_encode_parameter_mode_with_vui_safety(
            HevcEncodeParameterMode::SampleSpsNoVuiFlagOn,
            true,
            true,
            HevcEncodeProbeParameterVuiSafetyMode::Auto
        ),
        HevcEncodeParameterMode::SampleSpsNoVuiFlagOn
    );
    assert_eq!(
        resolve_hevc_encode_parameter_mode_with_vui_safety(
            HevcEncodeParameterMode::Sample,
            true,
            true,
            HevcEncodeProbeParameterVuiSafetyMode::Preserve
        ),
        HevcEncodeParameterMode::Sample
    );
    assert_eq!(
        resolve_hevc_encode_parameter_mode_with_vui_safety(
            HevcEncodeParameterMode::Sample,
            false,
            true,
            HevcEncodeProbeParameterVuiSafetyMode::ForceOff
        ),
        HevcEncodeParameterMode::SampleSpsVuiFlagOff
    );
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_empty_template_aliases() {
    for alias in [
        "empty",
        "template",
        "minimal",
        "empty-template",
        "empty_template",
        "  EmPtY  ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::EmptyTemplate
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_no_add_aliases() {
    for alias in [
        "sample-no-add",
        "sample_no_add",
        "sample-no-add-info",
        "sample_no_add_info",
        "sample-minimal",
        " Sample-No-Add ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SampleNoAddInfo
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_vps_only_aliases() {
    for alias in [
        "sample-vps-only",
        "sample_vps_only",
        "sample-vps",
        "vps-only",
        "vps_only",
        " Sample-Vps-Only ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SampleVpsOnly
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_sps_only_aliases() {
    for alias in [
        "sample-sps-only",
        "sample_sps_only",
        "sample-sps",
        "sps-only",
        "sps_only",
        " Sample-Sps-Only ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SampleSpsOnly
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_sps_no_vui_aliases() {
    for alias in [
        "sample-sps-no-vui",
        "sample_sps_no_vui",
        "sample-sps-no-vui-info",
        "sps-no-vui",
        "sps_no_vui",
        " Sample-Sps-No-Vui ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SampleSpsNoVui
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_sps_vui_flag_off_aliases() {
    for alias in [
        "sample-sps-vui-flag-off",
        "sample_sps_vui_flag_off",
        "sample-sps-flag-vui-off",
        "sps-vui-flag-off",
        "sps_vui_flag_off",
        " Sample-Sps-Vui-Flag-Off ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SampleSpsVuiFlagOff
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_sps_no_vui_flag_on_aliases() {
    for alias in [
        "sample-sps-no-vui-flag-on",
        "sample_sps_no_vui_flag_on",
        "sample-sps-vui-flag-on",
        "sps-no-vui-flag-on",
        "sps_no_vui_flag_on",
        " Sample-Sps-No-Vui-Flag-On ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SampleSpsNoVuiFlagOn
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_sps_no_rps_aliases() {
    for alias in [
        "sample-sps-no-rps",
        "sample_sps_no_rps",
        "sample-sps-no-ref-sets",
        "sps-no-rps",
        "sps_no_rps",
        " Sample-Sps-No-Rps ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SampleSpsNoRps
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_sps_safe_flags_aliases() {
    for alias in [
        "sample-sps-safe-flags",
        "sample_sps_safe_flags",
        "sample-sps-libx265-flags",
        "sps-safe-flags",
        "sps_safe_flags",
        " Sample-Sps-Safe-Flags ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SampleSpsSafeFlags
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_sps_level_aliases() {
    for alias in [
        "sample-sps-level",
        "sample_sps_level",
        "sample-sps-level-idc",
        "sps-level",
        "sps_level",
        " Sample-Sps-Level ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SampleSpsLevel
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_sps_sub_layer_ordering_aliases() {
    for alias in [
        "sample-sps-sub-layer-ordering",
        "sample_sps_sub_layer_ordering",
        "sample-sps-ordering",
        "sps-ordering",
        "sps_ordering",
        " Sample-Sps-Sub-Layer-Ordering ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SampleSpsSubLayerOrdering
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_sps_level_ordering_aliases() {
    for alias in [
        "sample-sps-level-ordering",
        "sample_sps_level_ordering",
        "sample-sps-libx265-shape",
        "sps-level-ordering",
        "sps_level_ordering",
        " Sample-Sps-Level-Ordering ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SampleSpsLevelOrdering
        );
    }
}

#[test]
fn parse_hevc_encode_parameter_mode_accepts_sample_pps_only_aliases() {
    for alias in [
        "sample-pps-only",
        "sample_pps_only",
        "sample-pps",
        "pps-only",
        "pps_only",
        " Sample-Pps-Only ",
    ] {
        assert_eq!(
            parse_hevc_encode_parameter_mode(Some(alias)),
            HevcEncodeParameterMode::SamplePpsOnly
        );
    }
}

#[test]
fn parse_hevc_encode_probe_reference_index_mode_defaults_to_minus_one() {
    assert_eq!(
        parse_hevc_encode_probe_reference_index_mode(None),
        HevcEncodeProbeReferenceIndexMode::MinusOne
    );
    assert_eq!(
        parse_hevc_encode_probe_reference_index_mode(Some("")),
        HevcEncodeProbeReferenceIndexMode::MinusOne
    );
    assert_eq!(
        parse_hevc_encode_probe_reference_index_mode(Some("minus-one")),
        HevcEncodeProbeReferenceIndexMode::MinusOne
    );
}

#[test]
fn parse_hevc_encode_probe_reference_index_mode_accepts_zero_aliases() {
    for alias in ["zero", "0", " Zero "] {
        assert_eq!(
            parse_hevc_encode_probe_reference_index_mode(Some(alias)),
            HevcEncodeProbeReferenceIndexMode::Zero
        );
    }
}

#[test]
fn parse_hevc_encode_probe_control_mode_defaults_to_default() {
    assert_eq!(
        parse_hevc_encode_probe_control_mode(None),
        HevcEncodeProbeControlMode::Default
    );
    assert_eq!(
        parse_hevc_encode_probe_control_mode(Some("")),
        HevcEncodeProbeControlMode::Default
    );
}

#[test]
fn parse_hevc_encode_probe_control_mode_accepts_ffmpeg_aliases() {
    for alias in ["ffmpeg", "ffmpeg-like", "ffmpeg_like", "compat", " FfMpEg "] {
        assert_eq!(
            parse_hevc_encode_probe_control_mode(Some(alias)),
            HevcEncodeProbeControlMode::Ffmpeg
        );
    }
}

#[test]
fn parse_hevc_encode_probe_control_mode_accepts_none_aliases() {
    for alias in [
        "none",
        "off",
        "disable",
        "disabled",
        "no-control",
        "no_control",
        " NoNe ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_control_mode(Some(alias)),
            HevcEncodeProbeControlMode::None
        );
    }
}

#[test]
fn parse_hevc_encode_probe_nalu_mode_defaults_to_single_slice() {
    assert_eq!(
        parse_hevc_encode_probe_nalu_mode(None),
        HevcEncodeProbeNaluMode::SingleSlice
    );
    assert_eq!(
        parse_hevc_encode_probe_nalu_mode(Some("")),
        HevcEncodeProbeNaluMode::SingleSlice
    );
    assert_eq!(
        parse_hevc_encode_probe_nalu_mode(Some("single-slice")),
        HevcEncodeProbeNaluMode::SingleSlice
    );
}

#[test]
fn parse_hevc_encode_probe_nalu_mode_accepts_empty_aliases() {
    for alias in ["empty", "none", "off", "no-slices", "no_slices", " Empty "] {
        assert_eq!(
            parse_hevc_encode_probe_nalu_mode(Some(alias)),
            HevcEncodeProbeNaluMode::Empty
        );
    }
}

#[test]
fn parse_hevc_encode_probe_codec_info_mode_defaults_to_with_h265_info() {
    assert_eq!(
        parse_hevc_encode_probe_codec_info_mode(None),
        HevcEncodeProbeCodecInfoMode::WithH265Info
    );
    assert_eq!(
        parse_hevc_encode_probe_codec_info_mode(Some("")),
        HevcEncodeProbeCodecInfoMode::WithH265Info
    );
    assert_eq!(
        parse_hevc_encode_probe_codec_info_mode(Some("with-h265-info")),
        HevcEncodeProbeCodecInfoMode::WithH265Info
    );
}

#[test]
fn parse_hevc_encode_probe_codec_info_mode_accepts_minimal_aliases() {
    for alias in [
        "with-h265-info-minimal",
        "with_h265_info_minimal",
        "with-h265-minimal",
        "with_h265_minimal",
        "minimal-h265-info",
        "minimal_h265_info",
        "minimal",
        " Minimal ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_codec_info_mode(Some(alias)),
            HevcEncodeProbeCodecInfoMode::WithH265InfoMinimal
        );
    }
}

#[test]
fn parse_hevc_encode_probe_codec_info_mode_accepts_std_picture_only_aliases() {
    for alias in [
        "with-h265-info-std-picture-only",
        "with_h265_info_std_picture_only",
        "with-h265-info-std-only",
        "with_h265_info_std_only",
        "std-picture-only",
        "std_picture_only",
        "std-only",
        "std_only",
        "std",
        " Std ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_codec_info_mode(Some(alias)),
            HevcEncodeProbeCodecInfoMode::WithH265InfoStdPictureOnly
        );
    }
}

#[test]
fn parse_hevc_encode_probe_codec_info_mode_accepts_empty_std_picture_aliases() {
    for alias in [
        "with-h265-info-empty-std-picture",
        "with_h265_info_empty_std_picture",
        "with-h265-info-empty-std",
        "with_h265_info_empty_std",
        "with-h265-empty-std",
        "with_h265_empty_std",
        "empty-std-picture",
        "empty_std_picture",
        "empty-std",
        "empty_std",
        " Empty-Std ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_codec_info_mode(Some(alias)),
            HevcEncodeProbeCodecInfoMode::WithH265InfoEmptyStdPicture
        );
    }
}

#[test]
fn parse_hevc_encode_probe_codec_info_mode_accepts_no_std_picture_aliases() {
    for alias in [
        "with-h265-info-no-std-picture",
        "with_h265_info_no_std_picture",
        "with-h265-info-no-std",
        "with_h265_info_no_std",
        "with-h265-no-std",
        "with_h265_no_std",
        "no-std-picture",
        "no_std_picture",
        "nostd",
        " NoStd ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_codec_info_mode(Some(alias)),
            HevcEncodeProbeCodecInfoMode::WithH265InfoNoStdPicture
        );
    }
}

#[test]
fn parse_hevc_encode_probe_codec_info_mode_accepts_without_aliases() {
    for alias in [
        "without",
        "without-h265",
        "without_h265",
        "without-h265-info",
        "without_h265_info",
        "none",
        "off",
        "disable",
        "disabled",
        " Without ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_codec_info_mode(Some(alias)),
            HevcEncodeProbeCodecInfoMode::WithoutH265Info
        );
    }
}

#[test]
fn parse_hevc_encode_probe_primary_mode_defaults_to_submit() {
    assert_eq!(
        parse_hevc_encode_probe_primary_mode(None),
        HevcEncodePrimaryProbeMode::Submit
    );
    assert_eq!(
        parse_hevc_encode_probe_primary_mode(Some("")),
        HevcEncodePrimaryProbeMode::Submit
    );
    assert_eq!(
        parse_hevc_encode_probe_primary_mode(Some("submit")),
        HevcEncodePrimaryProbeMode::Submit
    );
}

#[test]
fn parse_hevc_encode_probe_primary_mode_accepts_aliases() {
    for alias in [
        "scope-only",
        "scope_only",
        "scope",
        "coding-scope",
        "coding_scope",
        "coding",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_primary_mode(Some(alias)),
            HevcEncodePrimaryProbeMode::ScopeOnly
        );
    }
    for alias in [
        "pre-encode-scope",
        "pre_encode_scope",
        "pre-encode-scope-only",
        "pre_encode_scope_only",
        "pre-scope",
        "prescope",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_primary_mode(Some(alias)),
            HevcEncodePrimaryProbeMode::PreEncodeScopeOnly
        );
    }
    for alias in [
        "pre-encode",
        "pre_encode",
        "encode-only",
        "encode_only",
        "encode",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_primary_mode(Some(alias)),
            HevcEncodePrimaryProbeMode::PreEncode
        );
    }
    for alias in [
        "pre-encode-minimal",
        "pre_encode_minimal",
        "minimal",
        "encode-minimal",
        "encode_minimal",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_primary_mode(Some(alias)),
            HevcEncodePrimaryProbeMode::PreEncodeMinimal
        );
    }
}

#[test]
fn parse_hevc_encode_probe_picture_flags_mode_defaults_to_default() {
    assert_eq!(
        parse_hevc_encode_probe_picture_flags_mode(None),
        HevcEncodeProbePictureFlagsMode::Default
    );
    assert_eq!(
        parse_hevc_encode_probe_picture_flags_mode(Some("")),
        HevcEncodeProbePictureFlagsMode::Default
    );
}

#[test]
fn parse_hevc_encode_probe_picture_flags_mode_accepts_non_reference_aliases() {
    for alias in [
        "non-reference",
        "non_reference",
        "nonref",
        "non-ref",
        "non_ref",
        "p-frame-like",
        "p_frame_like",
        " Non-Reference ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_picture_flags_mode(Some(alias)),
            HevcEncodeProbePictureFlagsMode::NonReference
        );
    }
}

#[test]
fn parse_hevc_encode_probe_picture_info_mode_defaults_to_default() {
    assert_eq!(
        parse_hevc_encode_probe_picture_info_mode(None),
        HevcEncodeProbePictureInfoMode::Default
    );
    assert_eq!(
        parse_hevc_encode_probe_picture_info_mode(Some("")),
        HevcEncodeProbePictureInfoMode::Default
    );
    assert_eq!(
        parse_hevc_encode_probe_picture_info_mode(Some("default")),
        HevcEncodeProbePictureInfoMode::Default
    );
}

#[test]
fn parse_hevc_encode_probe_picture_info_mode_accepts_aliases() {
    for alias in [
        "intra-i",
        "intra_i",
        "intra",
        "i",
        "i-frame",
        "i_frame",
        " Intra-I ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_picture_info_mode(Some(alias)),
            HevcEncodeProbePictureInfoMode::IntraI
        );
    }
    for alias in [
        "inter-p",
        "inter_p",
        "inter",
        "p",
        "p-frame",
        "p_frame",
        " Inter-P ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_picture_info_mode(Some(alias)),
            HevcEncodeProbePictureInfoMode::InterP
        );
    }
    for alias in [
        "temporal-1",
        "temporal_1",
        "temporal1",
        "tid-1",
        "tid_1",
        "t1",
        " Temporal-1 ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_picture_info_mode(Some(alias)),
            HevcEncodeProbePictureInfoMode::Temporal1
        );
    }
    for alias in [
        "poc-1",
        "poc_1",
        "poc1",
        "poc-plus-one",
        "poc_plus_one",
        " POC-1 ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_picture_info_mode(Some(alias)),
            HevcEncodeProbePictureInfoMode::Poc1
        );
    }
}

#[test]
fn parse_hevc_encode_probe_rate_control_mode_defaults_to_auto() {
    assert_eq!(
        parse_hevc_encode_probe_rate_control_mode(None),
        HevcEncodeProbeRateControlMode::Auto
    );
    assert_eq!(
        parse_hevc_encode_probe_rate_control_mode(Some("")),
        HevcEncodeProbeRateControlMode::Auto
    );
    assert_eq!(
        parse_hevc_encode_probe_rate_control_mode(Some("auto")),
        HevcEncodeProbeRateControlMode::Auto
    );
}

#[test]
fn parse_hevc_encode_probe_rate_control_mode_accepts_aliases() {
    for alias in ["disabled", "disable", "off", " DisAbled "] {
        assert_eq!(
            parse_hevc_encode_probe_rate_control_mode(Some(alias)),
            HevcEncodeProbeRateControlMode::Disabled
        );
    }
    assert_eq!(
        parse_hevc_encode_probe_rate_control_mode(Some("cbr")),
        HevcEncodeProbeRateControlMode::Cbr
    );
    assert_eq!(
        parse_hevc_encode_probe_rate_control_mode(Some("vbr")),
        HevcEncodeProbeRateControlMode::Vbr
    );
    for alias in ["none", "null", " None "] {
        assert_eq!(
            parse_hevc_encode_probe_rate_control_mode(Some(alias)),
            HevcEncodeProbeRateControlMode::None
        );
    }
}

#[test]
fn resolve_hevc_encode_probe_selected_rate_control_mode_respects_request() {
    let available = vk::VideoEncodeRateControlModeFlagsKHR::DISABLED
        | vk::VideoEncodeRateControlModeFlagsKHR::CBR
        | vk::VideoEncodeRateControlModeFlagsKHR::VBR;
    assert_eq!(
        resolve_hevc_encode_probe_selected_rate_control_mode(
            HevcEncodeProbeRateControlMode::Auto,
            available,
            1
        ),
        Some(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED)
    );
    assert_eq!(
        resolve_hevc_encode_probe_selected_rate_control_mode(
            HevcEncodeProbeRateControlMode::Disabled,
            available,
            1
        ),
        Some(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED)
    );
    assert_eq!(
        resolve_hevc_encode_probe_selected_rate_control_mode(
            HevcEncodeProbeRateControlMode::Cbr,
            available,
            1
        ),
        Some(vk::VideoEncodeRateControlModeFlagsKHR::CBR)
    );
    assert_eq!(
        resolve_hevc_encode_probe_selected_rate_control_mode(
            HevcEncodeProbeRateControlMode::Vbr,
            available,
            1
        ),
        Some(vk::VideoEncodeRateControlModeFlagsKHR::VBR)
    );
    assert_eq!(
        resolve_hevc_encode_probe_selected_rate_control_mode(
            HevcEncodeProbeRateControlMode::None,
            available,
            1
        ),
        None
    );
}

#[test]
fn resolve_hevc_encode_probe_selected_rate_control_mode_returns_none_when_unavailable() {
    assert_eq!(
        resolve_hevc_encode_probe_selected_rate_control_mode(
            HevcEncodeProbeRateControlMode::Disabled,
            vk::VideoEncodeRateControlModeFlagsKHR::CBR,
            1
        ),
        None
    );
    assert_eq!(
        resolve_hevc_encode_probe_selected_rate_control_mode(
            HevcEncodeProbeRateControlMode::Cbr,
            vk::VideoEncodeRateControlModeFlagsKHR::CBR,
            0
        ),
        None
    );
    assert_eq!(
        resolve_hevc_encode_probe_selected_rate_control_mode(
            HevcEncodeProbeRateControlMode::Vbr,
            vk::VideoEncodeRateControlModeFlagsKHR::empty(),
            1
        ),
        None
    );
}

#[test]
fn parse_hevc_encode_probe_maintenance1_mode_defaults_to_auto() {
    assert_eq!(
        parse_hevc_encode_probe_maintenance1_mode(None),
        HevcEncodeProbeMaintenance1Mode::Auto
    );
    assert_eq!(
        parse_hevc_encode_probe_maintenance1_mode(Some("")),
        HevcEncodeProbeMaintenance1Mode::Auto
    );
    assert_eq!(
        parse_hevc_encode_probe_maintenance1_mode(Some("auto")),
        HevcEncodeProbeMaintenance1Mode::Auto
    );
}

#[test]
fn parse_hevc_encode_probe_maintenance1_mode_accepts_aliases() {
    for alias in [
        "on", "enable", "enabled", "force-on", "force_on", "true", "1",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_maintenance1_mode(Some(alias)),
            HevcEncodeProbeMaintenance1Mode::On
        );
    }
    for alias in [
        "off",
        "disable",
        "disabled",
        "force-off",
        "force_off",
        "false",
        "0",
        "none",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_maintenance1_mode(Some(alias)),
            HevcEncodeProbeMaintenance1Mode::Off
        );
    }
}

#[test]
fn resolve_hevc_encode_probe_maintenance1_feature_enabled_respects_mode() {
    assert!(resolve_hevc_encode_probe_maintenance1_feature_enabled(
        HevcEncodeProbeMaintenance1Mode::Auto,
        true,
        true,
    ));
    assert!(resolve_hevc_encode_probe_maintenance1_feature_enabled(
        HevcEncodeProbeMaintenance1Mode::On,
        true,
        true,
    ));
    assert!(!resolve_hevc_encode_probe_maintenance1_feature_enabled(
        HevcEncodeProbeMaintenance1Mode::Off,
        true,
        true,
    ));
    assert!(!resolve_hevc_encode_probe_maintenance1_feature_enabled(
        HevcEncodeProbeMaintenance1Mode::On,
        true,
        false,
    ));
    assert!(!resolve_hevc_encode_probe_maintenance1_feature_enabled(
        HevcEncodeProbeMaintenance1Mode::On,
        false,
        true,
    ));
}

#[test]
fn parse_hevc_encode_probe_session_dpb_mode_defaults_to_default() {
    assert_eq!(
        parse_hevc_encode_probe_session_dpb_mode(None),
        HevcEncodeProbeSessionDpbMode::Default
    );
    assert_eq!(
        parse_hevc_encode_probe_session_dpb_mode(Some("")),
        HevcEncodeProbeSessionDpbMode::Default
    );
    assert_eq!(
        parse_hevc_encode_probe_session_dpb_mode(Some("default")),
        HevcEncodeProbeSessionDpbMode::Default
    );
}

#[test]
fn parse_hevc_encode_probe_session_dpb_mode_accepts_minimal_aliases() {
    for alias in [
        "minimal",
        "minimal-one",
        "minimal_1",
        "minimal1",
        "one",
        "1",
        " Minimal-One ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_session_dpb_mode(Some(alias)),
            HevcEncodeProbeSessionDpbMode::MinimalOne
        );
    }
}

#[test]
fn parse_hevc_encode_probe_picture_resource_extent_mode_defaults_to_coded() {
    assert_eq!(
        parse_hevc_encode_probe_picture_resource_extent_mode(None),
        HevcEncodeProbePictureResourceExtentMode::Coded
    );
    assert_eq!(
        parse_hevc_encode_probe_picture_resource_extent_mode(Some("")),
        HevcEncodeProbePictureResourceExtentMode::Coded
    );
    assert_eq!(
        parse_hevc_encode_probe_picture_resource_extent_mode(Some("coded")),
        HevcEncodeProbePictureResourceExtentMode::Coded
    );
}

#[test]
fn parse_hevc_encode_probe_picture_resource_extent_mode_accepts_image_aliases() {
    for alias in [
        "image",
        "image-aligned",
        "image_aligned",
        "aligned",
        "align",
        "pad",
        "padded",
        "1",
        " Image-Aligned ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_picture_resource_extent_mode(Some(alias)),
            HevcEncodeProbePictureResourceExtentMode::ImageAligned
        );
    }
}

#[test]
fn parse_hevc_encode_probe_session_h265_create_info_mode_defaults_to_with_max_level() {
    assert_eq!(
        parse_hevc_encode_probe_session_h265_create_info_mode(None),
        HevcEncodeProbeSessionH265CreateInfoMode::WithMaxLevel
    );
    assert_eq!(
        parse_hevc_encode_probe_session_h265_create_info_mode(Some("")),
        HevcEncodeProbeSessionH265CreateInfoMode::WithMaxLevel
    );
    assert_eq!(
        parse_hevc_encode_probe_session_h265_create_info_mode(Some("with")),
        HevcEncodeProbeSessionH265CreateInfoMode::WithMaxLevel
    );
}

#[test]
fn parse_hevc_encode_probe_session_h265_create_info_mode_accepts_without_aliases() {
    for alias in [
        "without",
        "none",
        "off",
        "disabled",
        "ffmpeg",
        "ffmpeg-like",
        "0",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_session_h265_create_info_mode(Some(alias)),
            HevcEncodeProbeSessionH265CreateInfoMode::Without
        );
    }
}

#[test]
fn resolve_hevc_encode_probe_session_dpb_limits_respects_mode() {
    assert_eq!(
        resolve_hevc_encode_probe_session_dpb_limits(
            HevcEncodeProbeSessionDpbMode::Default,
            16,
            15,
        ),
        (16, 15)
    );
    assert_eq!(
        resolve_hevc_encode_probe_session_dpb_limits(HevcEncodeProbeSessionDpbMode::Default, 0, 0,),
        (1, 1)
    );
    assert_eq!(
        resolve_hevc_encode_probe_session_dpb_limits(
            HevcEncodeProbeSessionDpbMode::MinimalOne,
            16,
            15,
        ),
        (1, 1)
    );
}

#[test]
fn parse_hevc_encode_probe_reference_list_mode_defaults_to_sentinel() {
    assert_eq!(
        parse_hevc_encode_probe_reference_list_mode(None),
        HevcEncodeProbeReferenceListMode::Sentinel
    );
    assert_eq!(
        parse_hevc_encode_probe_reference_list_mode(Some("")),
        HevcEncodeProbeReferenceListMode::Sentinel
    );
}

#[test]
fn parse_hevc_encode_probe_reference_list_mode_accepts_null_aliases() {
    for alias in [
        "null",
        "null-pointer",
        "null-pointers",
        "null_pointer",
        "null_pointers",
        " Null ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_reference_list_mode(Some(alias)),
            HevcEncodeProbeReferenceListMode::NullPointers
        );
    }
}

#[test]
fn parse_hevc_encode_probe_rps_mode_defaults_to_empty_struct() {
    assert_eq!(
        parse_hevc_encode_probe_rps_mode(None),
        HevcEncodeProbeRpsMode::EmptyStruct
    );
    assert_eq!(
        parse_hevc_encode_probe_rps_mode(Some("")),
        HevcEncodeProbeRpsMode::EmptyStruct
    );
    assert_eq!(
        parse_hevc_encode_probe_rps_mode(Some("empty-struct")),
        HevcEncodeProbeRpsMode::EmptyStruct
    );
}

#[test]
fn parse_hevc_encode_probe_rps_mode_accepts_null_aliases() {
    for alias in ["null", "null-pointers", "null_pointers", " Null "] {
        assert_eq!(
            parse_hevc_encode_probe_rps_mode(Some(alias)),
            HevcEncodeProbeRpsMode::NullPointers
        );
    }
}

#[test]
fn parse_hevc_encode_probe_begin_reference_slot_mode_defaults_to_slot_minus_one() {
    assert_eq!(
        parse_hevc_encode_probe_begin_reference_slot_mode(None),
        HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOne
    );
    assert_eq!(
        parse_hevc_encode_probe_begin_reference_slot_mode(Some("")),
        HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOne
    );
}

#[test]
fn parse_hevc_encode_probe_begin_reference_slot_mode_accepts_none_aliases() {
    for alias in ["none", "off", "disable", "disabled", " None "] {
        assert_eq!(
            parse_hevc_encode_probe_begin_reference_slot_mode(Some(alias)),
            HevcEncodeProbeBeginReferenceSlotMode::None
        );
    }
}

#[test]
fn parse_hevc_encode_probe_begin_reference_slot_mode_accepts_ffmpeg_aliases() {
    for alias in [
        "slot-minus-one-no-codec-info",
        "slot_minus_one_no_codec_info",
        "slot-minus-one-null-pnext",
        "ffmpeg",
        " FFmpeg ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_begin_reference_slot_mode(Some(alias)),
            HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOneNoCodecInfo
        );
    }
}

#[test]
fn parse_hevc_encode_probe_begin_session_parameters_mode_defaults_to_with() {
    assert_eq!(
        parse_hevc_encode_probe_begin_session_parameters_mode(None),
        HevcEncodeProbeBeginSessionParametersMode::With
    );
    assert_eq!(
        parse_hevc_encode_probe_begin_session_parameters_mode(Some("")),
        HevcEncodeProbeBeginSessionParametersMode::With
    );
    assert_eq!(
        parse_hevc_encode_probe_begin_session_parameters_mode(Some("with")),
        HevcEncodeProbeBeginSessionParametersMode::With
    );
}

#[test]
fn parse_hevc_encode_probe_begin_session_parameters_mode_accepts_without_aliases() {
    for alias in [
        "without",
        "none",
        "off",
        "disable",
        "disabled",
        "no-params",
        "no_params",
        "0",
        " Without ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_begin_session_parameters_mode(Some(alias)),
            HevcEncodeProbeBeginSessionParametersMode::Without
        );
    }
}

#[test]
fn parse_hevc_encode_probe_begin_pnext_mode_defaults_to_none() {
    assert_eq!(
        parse_hevc_encode_probe_begin_pnext_mode(None),
        HevcEncodeProbeBeginPnextMode::None
    );
    assert_eq!(
        parse_hevc_encode_probe_begin_pnext_mode(Some("")),
        HevcEncodeProbeBeginPnextMode::None
    );
}

#[test]
fn parse_hevc_encode_probe_begin_pnext_mode_accepts_rate_control_aliases() {
    for alias in [
        "rate-control",
        "rate_control",
        "rc",
        "ffmpeg",
        "ffmpeg-like",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_begin_pnext_mode(Some(alias)),
            HevcEncodeProbeBeginPnextMode::RateControl
        );
    }
}

#[test]
fn parse_hevc_encode_probe_setup_reference_slot_mode_defaults_to_slot_zero() {
    assert_eq!(
        parse_hevc_encode_probe_setup_reference_slot_mode(None),
        HevcEncodeProbeSetupReferenceSlotMode::SlotZero
    );
    assert_eq!(
        parse_hevc_encode_probe_setup_reference_slot_mode(Some("")),
        HevcEncodeProbeSetupReferenceSlotMode::SlotZero
    );
}

#[test]
fn parse_hevc_encode_probe_setup_reference_slot_mode_accepts_none_aliases() {
    for alias in ["none", "off", "disable", "disabled", " None "] {
        assert_eq!(
            parse_hevc_encode_probe_setup_reference_slot_mode(Some(alias)),
            HevcEncodeProbeSetupReferenceSlotMode::None
        );
    }
}

#[test]
fn parse_hevc_encode_probe_dpb_barrier_mode_defaults_to_with() {
    assert_eq!(
        parse_hevc_encode_probe_dpb_barrier_mode(None),
        HevcEncodeProbeDpbBarrierMode::With
    );
    assert_eq!(
        parse_hevc_encode_probe_dpb_barrier_mode(Some("")),
        HevcEncodeProbeDpbBarrierMode::With
    );
    assert_eq!(
        parse_hevc_encode_probe_dpb_barrier_mode(Some("with")),
        HevcEncodeProbeDpbBarrierMode::With
    );
}

#[test]
fn parse_hevc_encode_probe_dpb_barrier_mode_accepts_none_aliases() {
    for alias in [
        "none", "off", "disable", "disabled", "skip", "ffmpeg", " None ",
    ] {
        assert_eq!(
            parse_hevc_encode_probe_dpb_barrier_mode(Some(alias)),
            HevcEncodeProbeDpbBarrierMode::None
        );
    }
}

#[test]
fn parse_hevc_encode_probe_encode_reference_slot_pointer_mode_defaults_to_empty_slice() {
    assert_eq!(
        parse_hevc_encode_probe_encode_reference_slot_pointer_mode(None),
        HevcEncodeProbeEncodeReferenceSlotPointerMode::EmptySlice
    );
    assert_eq!(
        parse_hevc_encode_probe_encode_reference_slot_pointer_mode(Some("")),
        HevcEncodeProbeEncodeReferenceSlotPointerMode::EmptySlice
    );
}

#[test]
fn parse_hevc_encode_probe_encode_reference_slot_pointer_mode_accepts_ffmpeg_aliases() {
    for alias in ["ffmpeg", "non-null", "non_null", "nonnull", " FFmpeg "] {
        assert_eq!(
            parse_hevc_encode_probe_encode_reference_slot_pointer_mode(Some(alias)),
            HevcEncodeProbeEncodeReferenceSlotPointerMode::FfmpegNonNull
        );
    }
}

#[test]
fn hevc_encode_probe_parameter_profile_validation_accepts_repository_main_profile() {
    let sample = load_hevc_encode_probe_parameter_sample_from_path(None)
        .expect("default parameter sample should load from repository bytes");
    let parameter_sets =
        extract_hevc_parameter_sets_annexb(&sample).expect("repository sample must parse");
    validate_hevc_encode_probe_parameter_profile(&parameter_sets)
        .expect("repository sample profile should remain Main");
}

#[test]
fn hevc_encode_probe_parameter_profile_validation_rejects_non_main_profile() {
    let sample = load_hevc_encode_probe_parameter_sample_from_path(None)
        .expect("default parameter sample should load from repository bytes");
    let mut parameter_sets =
        extract_hevc_parameter_sets_annexb(&sample).expect("repository sample must parse");
    parameter_sets
        .parsed_sps
        .profile_tier_level
        .general_profile
        .profile_idc = 4;
    let err = validate_hevc_encode_probe_parameter_profile(&parameter_sets)
        .expect_err("non-Main profile must be rejected");
    assert!(err.contains("profile_idc=4"));
    assert!(err.contains("Main profile"));
}
