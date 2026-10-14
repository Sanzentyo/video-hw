//! HEVC picture syntax and per-picture encode planning.

use super::*;

pub(super) fn hevc_encode_probe_picture_type(
    mode: HevcEncodeProbePictureInfoMode,
) -> ash::vk::native::StdVideoH265PictureType {
    match mode {
        HevcEncodeProbePictureInfoMode::Default | HevcEncodeProbePictureInfoMode::Temporal1 => {
            StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR
        }
        HevcEncodeProbePictureInfoMode::IntraI => {
            StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_I
        }
        HevcEncodeProbePictureInfoMode::InterP | HevcEncodeProbePictureInfoMode::Poc1 => {
            StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_P
        }
    }
}

pub(super) fn hevc_encode_probe_slice_type(
    mode: HevcEncodeProbePictureInfoMode,
) -> ash::vk::native::StdVideoH265SliceType {
    match mode {
        HevcEncodeProbePictureInfoMode::InterP | HevcEncodeProbePictureInfoMode::Poc1 => {
            StdVideoH265SliceType_STD_VIDEO_H265_SLICE_TYPE_P
        }
        HevcEncodeProbePictureInfoMode::Default
        | HevcEncodeProbePictureInfoMode::IntraI
        | HevcEncodeProbePictureInfoMode::Temporal1 => {
            StdVideoH265SliceType_STD_VIDEO_H265_SLICE_TYPE_I
        }
    }
}

pub(super) fn hevc_encode_probe_temporal_id(mode: HevcEncodeProbePictureInfoMode) -> u8 {
    match mode {
        HevcEncodeProbePictureInfoMode::Temporal1 => 1,
        _ => 0,
    }
}

pub(super) fn hevc_encode_probe_pic_order_cnt_val(mode: HevcEncodeProbePictureInfoMode) -> i32 {
    match mode {
        HevcEncodeProbePictureInfoMode::Poc1 => 1,
        _ => 0,
    }
}

pub(super) fn hevc_encode_probe_constant_qp(
    selected_rate_control_mode: Option<vk::VideoEncodeRateControlModeFlagsKHR>,
    override_value: Option<&str>,
) -> i32 {
    if selected_rate_control_mode
        .is_some_and(|mode| mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED)
    {
        0
    } else {
        parse_hevc_encode_probe_constant_qp(override_value).unwrap_or(18)
    }
}

pub(super) fn resolve_hevc_encode_probe_constant_qp(
    selected_rate_control_mode: Option<vk::VideoEncodeRateControlModeFlagsKHR>,
) -> i32 {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_CONSTANT_QP";
    let override_value = std::env::var(ENV_VAR).ok();
    hevc_encode_probe_constant_qp(selected_rate_control_mode, override_value.as_deref())
}

fn parse_hevc_encode_probe_constant_qp(value: Option<&str>) -> Option<i32> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|value| (0..=51).contains(value))
}

pub(super) fn resolve_hevc_encode_probe_quality_level(max_quality_levels: u32) -> u32 {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_QUALITY_LEVEL";
    let override_value = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_quality_level(override_value.as_deref(), max_quality_levels)
}

pub(super) fn parse_hevc_encode_probe_quality_level(
    value: Option<&str>,
    max_quality_levels: u32,
) -> u32 {
    let upper_bound = max_quality_levels.saturating_sub(1);
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u32>().ok())
        .map(|value| value.min(upper_bound))
        .unwrap_or(0)
}

pub(super) fn hevc_encode_probe_slice_qp_delta(
    constant_qp: i32,
    pps_init_qp_minus26: i8,
) -> Result<i8, String> {
    let pps_init_qp = i32::from(pps_init_qp_minus26) + 26;
    i8::try_from(constant_qp - pps_init_qp).map_err(|_| {
        format!(
            "slice_qp_delta is out of range for constant_qp={constant_qp}, pps_init_qp_minus26={pps_init_qp_minus26}"
        )
    })
}

pub(super) fn hevc_encode_std_slice_segment_header_flags(
    sample_adaptive_offset_enabled: bool,
) -> StdVideoEncodeH265SliceSegmentHeaderFlags {
    let mut flags = StdVideoEncodeH265SliceSegmentHeaderFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
    };
    flags.set_first_slice_segment_in_pic_flag(1);
    flags.set_dependent_slice_segment_flag(0);
    flags.set_slice_sao_luma_flag(u32::from(sample_adaptive_offset_enabled));
    flags.set_slice_sao_chroma_flag(u32::from(sample_adaptive_offset_enabled));
    flags.set_num_ref_idx_active_override_flag(0);
    flags.set_mvd_l1_zero_flag(0);
    flags.set_cabac_init_flag(0);
    flags.set_cu_chroma_qp_offset_enabled_flag(0);
    flags.set_deblocking_filter_override_flag(0);
    flags.set_slice_deblocking_filter_disabled_flag(0);
    flags.set_collocated_from_l0_flag(1);
    flags.set_slice_loop_filter_across_slices_enabled_flag(0);
    flags
}

pub(super) fn hevc_encode_h265_rate_control_info() -> vk::VideoEncodeH265RateControlInfoKHR<'static>
{
    vk::VideoEncodeH265RateControlInfoKHR::default()
        .flags(
            vk::VideoEncodeH265RateControlFlagsKHR::REFERENCE_PATTERN_FLAT
                | vk::VideoEncodeH265RateControlFlagsKHR::REGULAR_GOP,
        )
        .gop_frame_count(30)
        .idr_period(30)
        .consecutive_b_frame_count(0)
        .sub_layer_count(0)
}

pub(super) fn hevc_encode_pre_encode_probe_mode_label(
    mode: HevcEncodePreEncodeProbeMode,
) -> &'static str {
    match mode {
        HevcEncodePreEncodeProbeMode::ScopeOnly => "scope-only",
        HevcEncodePreEncodeProbeMode::WithEncode => "with-encode",
        HevcEncodePreEncodeProbeMode::WithEncodeMinimal => "with-encode-minimal",
    }
}

pub(super) fn hevc_encode_primary_probe_mode_label(
    mode: HevcEncodePrimaryProbeMode,
) -> &'static str {
    match mode {
        HevcEncodePrimaryProbeMode::Submit => "submit",
        HevcEncodePrimaryProbeMode::ScopeOnly => "scope-only",
        HevcEncodePrimaryProbeMode::PreEncodeScopeOnly => "pre-encode-scope-only",
        HevcEncodePrimaryProbeMode::PreEncode => "pre-encode",
        HevcEncodePrimaryProbeMode::PreEncodeMinimal => "pre-encode-minimal",
    }
}

pub(super) fn parse_hevc_encode_probe_reference_index_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeReferenceIndexMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("zero") | Some("0") => HevcEncodeProbeReferenceIndexMode::Zero,
        _ => HevcEncodeProbeReferenceIndexMode::MinusOne,
    }
}

pub(super) fn hevc_encode_probe_reference_index_mode_label(
    mode: HevcEncodeProbeReferenceIndexMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeReferenceIndexMode::MinusOne => "minus-one",
        HevcEncodeProbeReferenceIndexMode::Zero => "zero",
    }
}

pub(super) fn hevc_encode_probe_reference_idx_minus1(
    mode: HevcEncodeProbeReferenceIndexMode,
) -> u8 {
    match mode {
        HevcEncodeProbeReferenceIndexMode::MinusOne => u8::MAX,
        HevcEncodeProbeReferenceIndexMode::Zero => 0,
    }
}

pub(super) fn resolve_hevc_encode_parameter_mode_with_vui_safety(
    requested_mode: HevcEncodeParameterMode,
    sample_override_enabled: bool,
    sample_has_vui_parameters: bool,
    safety_mode: HevcEncodeProbeParameterVuiSafetyMode,
) -> HevcEncodeParameterMode {
    match safety_mode {
        HevcEncodeProbeParameterVuiSafetyMode::Auto => {
            if sample_override_enabled
                && sample_has_vui_parameters
                && matches!(requested_mode, HevcEncodeParameterMode::Sample)
            {
                HevcEncodeParameterMode::SampleSpsVuiFlagOff
            } else {
                requested_mode
            }
        }
        HevcEncodeProbeParameterVuiSafetyMode::Preserve => requested_mode,
        HevcEncodeProbeParameterVuiSafetyMode::ForceOff => {
            if sample_has_vui_parameters
                && matches!(requested_mode, HevcEncodeParameterMode::Sample)
            {
                HevcEncodeParameterMode::SampleSpsVuiFlagOff
            } else {
                requested_mode
            }
        }
    }
}
