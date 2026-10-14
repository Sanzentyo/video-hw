//! Environment-driven diagnostic configuration and HEVC syntax planning.

use super::*;

pub(super) fn resolve_hevc_encode_parameter_mode() -> HevcEncodeParameterMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_parameter_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_parameter_size_mode() -> HevcEncodeProbeParameterSizeMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SIZE_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_parameter_size_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_parameter_vui_safety_mode()
-> HevcEncodeProbeParameterVuiSafetyMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_VUI_SAFETY";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_parameter_vui_safety_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_image_view_mode() -> HevcEncodeProbeImageViewMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_IMAGE_VIEW_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_image_view_mode(mode.as_deref())
}

pub(super) fn parse_hevc_encode_parameter_mode(mode: Option<&str>) -> HevcEncodeParameterMode {
    let normalized_mode = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized_mode.as_deref() {
        Some("sample-no-add")
        | Some("sample_no_add")
        | Some("sample-no-add-info")
        | Some("sample_no_add_info")
        | Some("sample-minimal") => HevcEncodeParameterMode::SampleNoAddInfo,
        Some("sample-vps-only")
        | Some("sample_vps_only")
        | Some("sample-vps")
        | Some("vps-only")
        | Some("vps_only") => HevcEncodeParameterMode::SampleVpsOnly,
        Some("sample-sps-only")
        | Some("sample_sps_only")
        | Some("sample-sps")
        | Some("sps-only")
        | Some("sps_only") => HevcEncodeParameterMode::SampleSpsOnly,
        Some("sample-sps-no-vui")
        | Some("sample_sps_no_vui")
        | Some("sample-sps-no-vui-info")
        | Some("sps-no-vui")
        | Some("sps_no_vui") => HevcEncodeParameterMode::SampleSpsNoVui,
        Some("sample-sps-vui-flag-off")
        | Some("sample_sps_vui_flag_off")
        | Some("sample-sps-flag-vui-off")
        | Some("sps-vui-flag-off")
        | Some("sps_vui_flag_off") => HevcEncodeParameterMode::SampleSpsVuiFlagOff,
        Some("sample-sps-no-vui-flag-on")
        | Some("sample_sps_no_vui_flag_on")
        | Some("sample-sps-vui-flag-on")
        | Some("sps-no-vui-flag-on")
        | Some("sps_no_vui_flag_on") => HevcEncodeParameterMode::SampleSpsNoVuiFlagOn,
        Some("sample-sps-no-rps")
        | Some("sample_sps_no_rps")
        | Some("sample-sps-no-ref-sets")
        | Some("sps-no-rps")
        | Some("sps_no_rps") => HevcEncodeParameterMode::SampleSpsNoRps,
        Some("sample-sps-safe-flags")
        | Some("sample_sps_safe_flags")
        | Some("sample-sps-libx265-flags")
        | Some("sps-safe-flags")
        | Some("sps_safe_flags") => HevcEncodeParameterMode::SampleSpsSafeFlags,
        Some("sample-sps-level")
        | Some("sample_sps_level")
        | Some("sample-sps-level-idc")
        | Some("sps-level")
        | Some("sps_level") => HevcEncodeParameterMode::SampleSpsLevel,
        Some("sample-sps-sub-layer-ordering")
        | Some("sample_sps_sub_layer_ordering")
        | Some("sample-sps-ordering")
        | Some("sps-ordering")
        | Some("sps_ordering") => HevcEncodeParameterMode::SampleSpsSubLayerOrdering,
        Some("sample-sps-level-ordering")
        | Some("sample_sps_level_ordering")
        | Some("sample-sps-libx265-shape")
        | Some("sps-level-ordering")
        | Some("sps_level_ordering") => HevcEncodeParameterMode::SampleSpsLevelOrdering,
        Some("sample-pps-only")
        | Some("sample_pps_only")
        | Some("sample-pps")
        | Some("pps-only")
        | Some("pps_only") => HevcEncodeParameterMode::SamplePpsOnly,
        Some("empty")
        | Some("template")
        | Some("minimal")
        | Some("empty-template")
        | Some("empty_template") => HevcEncodeParameterMode::EmptyTemplate,
        _ => HevcEncodeParameterMode::Sample,
    }
}

pub(super) fn parse_hevc_encode_probe_parameter_size_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeParameterSizeMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("coded") | Some("probe") | Some("probe-coded") | Some("probe_coded")
        | Some("ffmpeg") | Some("ffmpeg-like") | Some("ffmpeg_like") => {
            HevcEncodeProbeParameterSizeMode::Coded
        }
        _ => HevcEncodeProbeParameterSizeMode::Sample,
    }
}

pub(super) fn hevc_encode_probe_parameter_size_mode_label(
    mode: HevcEncodeProbeParameterSizeMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeParameterSizeMode::Sample => "sample",
        HevcEncodeProbeParameterSizeMode::Coded => "coded",
    }
}

pub(super) fn parse_hevc_encode_probe_parameter_vui_safety_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeParameterVuiSafetyMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("preserve") | Some("keep") | Some("keep-vui") | Some("keep_vui") | Some("ffmpeg")
        | Some("ffmpeg-like") | Some("ffmpeg_like") => {
            HevcEncodeProbeParameterVuiSafetyMode::Preserve
        }
        Some("force-off") | Some("force_off") | Some("off") | Some("disable")
        | Some("disabled") => HevcEncodeProbeParameterVuiSafetyMode::ForceOff,
        _ => HevcEncodeProbeParameterVuiSafetyMode::Auto,
    }
}

pub(super) fn hevc_encode_probe_parameter_vui_safety_mode_label(
    mode: HevcEncodeProbeParameterVuiSafetyMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeParameterVuiSafetyMode::Auto => "auto",
        HevcEncodeProbeParameterVuiSafetyMode::Preserve => "preserve",
        HevcEncodeProbeParameterVuiSafetyMode::ForceOff => "force-off",
    }
}

pub(super) fn parse_hevc_encode_probe_image_view_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeImageViewMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("no-ycbcr")
        | Some("no_ycbcr")
        | Some("without-ycbcr")
        | Some("without_ycbcr")
        | Some("plain")
        | Some("ffmpeg-no-ycbcr")
        | Some("ffmpeg_no_ycbcr") => HevcEncodeProbeImageViewMode::NoYcbcrConversion,
        _ => HevcEncodeProbeImageViewMode::YcbcrConversion,
    }
}

pub(super) fn hevc_encode_probe_image_view_mode_label(
    mode: HevcEncodeProbeImageViewMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeImageViewMode::YcbcrConversion => "ycbcr-conversion",
        HevcEncodeProbeImageViewMode::NoYcbcrConversion => "no-ycbcr-conversion",
    }
}

pub(super) fn hevc_encode_parameter_mode_add_info_filter(
    mode: HevcEncodeParameterMode,
) -> Option<(bool, bool, bool)> {
    match mode {
        HevcEncodeParameterMode::Sample => Some((true, true, true)),
        HevcEncodeParameterMode::SampleNoAddInfo => None,
        HevcEncodeParameterMode::SampleVpsOnly => Some((true, false, false)),
        HevcEncodeParameterMode::SampleSpsOnly => Some((false, true, false)),
        HevcEncodeParameterMode::SampleSpsNoVui => Some((false, true, false)),
        HevcEncodeParameterMode::SampleSpsVuiFlagOff => Some((false, true, false)),
        HevcEncodeParameterMode::SampleSpsNoVuiFlagOn => Some((false, true, false)),
        HevcEncodeParameterMode::SampleSpsNoRps => Some((false, true, false)),
        HevcEncodeParameterMode::SampleSpsSafeFlags => Some((false, true, false)),
        HevcEncodeParameterMode::SampleSpsLevel => Some((false, true, false)),
        HevcEncodeParameterMode::SampleSpsSubLayerOrdering => Some((false, true, false)),
        HevcEncodeParameterMode::SampleSpsLevelOrdering => Some((false, true, false)),
        HevcEncodeParameterMode::SamplePpsOnly => Some((false, false, true)),
        HevcEncodeParameterMode::EmptyTemplate => None,
    }
}

pub(super) fn apply_hevc_encode_parameter_mode_std_overrides(
    std_parameter_storage: &mut HevcStdParameterSetStorage,
    mode: HevcEncodeParameterMode,
) {
    match mode {
        HevcEncodeParameterMode::SampleSpsVuiFlagOff => {
            std_parameter_storage.override_encode_sps_vui_parameters_present_flag(false);
        }
        HevcEncodeParameterMode::SampleSpsNoVuiFlagOn => {
            std_parameter_storage.override_encode_sps_vui_parameters_present_flag(true);
        }
        _ => {}
    }
}

pub(super) fn apply_hevc_encode_parameter_mode_sps_overrides(
    parameter_sets: &mut HevcParameterSets,
    mode: HevcEncodeParameterMode,
) {
    fn apply_sps_level_override(parameter_sets: &mut HevcParameterSets) {
        const LIBX265_MAIN_LEVEL_IDC: u8 = 63;
        parameter_sets
            .parsed_sps
            .profile_tier_level
            .general_profile
            .level_idc = Some(LIBX265_MAIN_LEVEL_IDC);
    }

    fn apply_sps_sub_layer_ordering_override(parameter_sets: &mut HevcParameterSets) {
        const LIBX265_MAX_DEC_PIC_BUFFERING_MINUS1: u64 = 4;
        const LIBX265_MAX_NUM_REORDER_PICS: u64 = 2;
        const LIBX265_MAX_LATENCY_INCREASE_PLUS1: u32 = 5;

        for value in &mut parameter_sets
            .parsed_sps
            .sub_layer_ordering_info
            .sps_max_dec_pic_buffering_minus1
        {
            *value = LIBX265_MAX_DEC_PIC_BUFFERING_MINUS1;
        }
        for value in &mut parameter_sets
            .parsed_sps
            .sub_layer_ordering_info
            .sps_max_num_reorder_pics
        {
            *value = LIBX265_MAX_NUM_REORDER_PICS;
        }
        for value in &mut parameter_sets
            .parsed_sps
            .sub_layer_ordering_info
            .sps_max_latency_increase_plus1
        {
            *value = LIBX265_MAX_LATENCY_INCREASE_PLUS1;
        }
    }

    match mode {
        HevcEncodeParameterMode::SampleSpsNoVui => {
            parameter_sets.parsed_sps.vui_parameters = None;
        }
        HevcEncodeParameterMode::SampleSpsNoVuiFlagOn => {
            parameter_sets.parsed_sps.vui_parameters = None;
        }
        HevcEncodeParameterMode::SampleSpsNoRps => {
            let short_term_ref_pic_sets = &mut parameter_sets.parsed_sps.short_term_ref_pic_sets;
            short_term_ref_pic_sets.num_delta_pocs.clear();
            short_term_ref_pic_sets.num_negative_pics.clear();
            short_term_ref_pic_sets.num_positive_pics.clear();
            short_term_ref_pic_sets.delta_poc_s0.clear();
            short_term_ref_pic_sets.used_by_curr_pic_s0.clear();
            short_term_ref_pic_sets.delta_poc_s1.clear();
            short_term_ref_pic_sets.used_by_curr_pic_s1.clear();
        }
        HevcEncodeParameterMode::SampleSpsSafeFlags => {
            parameter_sets.parsed_sps.amp_enabled_flag = false;
            parameter_sets.parsed_sps.sps_temporal_mvp_enabled_flag = true;
            parameter_sets
                .parsed_sps
                .strong_intra_smoothing_enabled_flag = true;
        }
        HevcEncodeParameterMode::SampleSpsLevel => {
            apply_sps_level_override(parameter_sets);
        }
        HevcEncodeParameterMode::SampleSpsSubLayerOrdering => {
            apply_sps_sub_layer_ordering_override(parameter_sets);
        }
        HevcEncodeParameterMode::SampleSpsLevelOrdering => {
            apply_sps_level_override(parameter_sets);
            apply_sps_sub_layer_ordering_override(parameter_sets);
        }
        _ => {}
    }
}

pub(super) fn apply_hevc_encode_capability_sps_overrides(
    parameter_sets: &mut HevcParameterSets,
    std_syntax_flags: vk::VideoEncodeH265StdFlagsKHR,
) {
    parameter_sets
        .parsed_sps
        .sample_adaptive_offset_enabled_flag = std_syntax_flags
        .contains(vk::VideoEncodeH265StdFlagsKHR::SAMPLE_ADAPTIVE_OFFSET_ENABLED_FLAG_SET);
    parameter_sets.parsed_sps.sps_temporal_mvp_enabled_flag = false;
}

pub(super) fn hevc_encode_parameter_mode_label(mode: HevcEncodeParameterMode) -> &'static str {
    match mode {
        HevcEncodeParameterMode::Sample => "sample",
        HevcEncodeParameterMode::SampleNoAddInfo => "sample-no-add-info",
        HevcEncodeParameterMode::SampleVpsOnly => "sample-vps-only",
        HevcEncodeParameterMode::SampleSpsOnly => "sample-sps-only",
        HevcEncodeParameterMode::SampleSpsNoVui => "sample-sps-no-vui",
        HevcEncodeParameterMode::SampleSpsVuiFlagOff => "sample-sps-vui-flag-off",
        HevcEncodeParameterMode::SampleSpsNoVuiFlagOn => "sample-sps-no-vui-flag-on",
        HevcEncodeParameterMode::SampleSpsNoRps => "sample-sps-no-rps",
        HevcEncodeParameterMode::SampleSpsSafeFlags => "sample-sps-safe-flags",
        HevcEncodeParameterMode::SampleSpsLevel => "sample-sps-level",
        HevcEncodeParameterMode::SampleSpsSubLayerOrdering => "sample-sps-sub-layer-ordering",
        HevcEncodeParameterMode::SampleSpsLevelOrdering => "sample-sps-level-ordering",
        HevcEncodeParameterMode::SamplePpsOnly => "sample-pps-only",
        HevcEncodeParameterMode::EmptyTemplate => "empty-template",
    }
}

pub(super) fn resolve_hevc_encode_probe_reference_list_mode() -> HevcEncodeProbeReferenceListMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_LIST_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_reference_list_mode(mode.as_deref())
}

pub(super) fn parse_hevc_encode_probe_reference_list_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeReferenceListMode {
    let normalized = mode
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    match normalized.as_deref() {
        Some("zero") | Some("zeroed") | Some("0") => HevcEncodeProbeReferenceListMode::Zero,
        Some("null")
        | Some("null-pointer")
        | Some("null-pointers")
        | Some("null_pointer")
        | Some("null_pointers") => HevcEncodeProbeReferenceListMode::NullPointers,
        _ => HevcEncodeProbeReferenceListMode::Sentinel,
    }
}

pub(super) fn hevc_encode_probe_reference_list_mode_label(
    mode: HevcEncodeProbeReferenceListMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeReferenceListMode::Sentinel => "sentinel",
        HevcEncodeProbeReferenceListMode::Zero => "zero",
        HevcEncodeProbeReferenceListMode::NullPointers => "null-pointers",
    }
}

pub(super) fn resolve_hevc_encode_probe_reference_index_mode() -> HevcEncodeProbeReferenceIndexMode
{
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_IDX_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_reference_index_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_rps_mode() -> HevcEncodeProbeRpsMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_RPS_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_rps_mode(mode.as_deref())
}

pub(super) fn parse_hevc_encode_probe_rps_mode(mode: Option<&str>) -> HevcEncodeProbeRpsMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("null") | Some("null-pointers") | Some("null_pointers") => {
            HevcEncodeProbeRpsMode::NullPointers
        }
        _ => HevcEncodeProbeRpsMode::EmptyStruct,
    }
}

pub(super) fn hevc_encode_probe_rps_mode_label(mode: HevcEncodeProbeRpsMode) -> &'static str {
    match mode {
        HevcEncodeProbeRpsMode::EmptyStruct => "empty-struct",
        HevcEncodeProbeRpsMode::NullPointers => "null-pointers",
    }
}

pub(super) fn resolve_hevc_encode_probe_begin_reference_slot_mode()
-> HevcEncodeProbeBeginReferenceSlotMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_REFERENCE_SLOT_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_begin_reference_slot_mode(mode.as_deref())
}

pub(super) fn parse_hevc_encode_probe_begin_reference_slot_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeBeginReferenceSlotMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("slot-minus-one-no-codec-info")
        | Some("slot_minus_one_no_codec_info")
        | Some("slot-minus-one-null-pnext")
        | Some("slot_minus_one_null_pnext")
        | Some("ffmpeg") => HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOneNoCodecInfo,
        Some("none") | Some("off") | Some("disable") | Some("disabled") => {
            HevcEncodeProbeBeginReferenceSlotMode::None
        }
        _ => HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOne,
    }
}

pub(super) fn hevc_encode_probe_begin_reference_slot_mode_label(
    mode: HevcEncodeProbeBeginReferenceSlotMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOne => "slot-minus-one",
        HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOneNoCodecInfo => {
            "slot-minus-one-no-codec-info"
        }
        HevcEncodeProbeBeginReferenceSlotMode::None => "none",
    }
}

pub(super) fn resolve_hevc_encode_probe_setup_reference_slot_mode()
-> HevcEncodeProbeSetupReferenceSlotMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_SETUP_REFERENCE_SLOT_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_setup_reference_slot_mode(mode.as_deref())
}

pub(super) fn parse_hevc_encode_probe_setup_reference_slot_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeSetupReferenceSlotMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("none") | Some("off") | Some("disable") | Some("disabled") => {
            HevcEncodeProbeSetupReferenceSlotMode::None
        }
        _ => HevcEncodeProbeSetupReferenceSlotMode::SlotZero,
    }
}

pub(super) fn hevc_encode_probe_setup_reference_slot_mode_label(
    mode: HevcEncodeProbeSetupReferenceSlotMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeSetupReferenceSlotMode::SlotZero => "slot-zero",
        HevcEncodeProbeSetupReferenceSlotMode::None => "none",
    }
}

pub(super) fn resolve_hevc_encode_probe_dpb_barrier_mode() -> HevcEncodeProbeDpbBarrierMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_DPB_BARRIER_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_dpb_barrier_mode(mode.as_deref())
}

pub(super) fn parse_hevc_encode_probe_dpb_barrier_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeDpbBarrierMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("none") | Some("off") | Some("disable") | Some("disabled") | Some("skip")
        | Some("ffmpeg") => HevcEncodeProbeDpbBarrierMode::None,
        _ => HevcEncodeProbeDpbBarrierMode::With,
    }
}

pub(super) fn hevc_encode_probe_dpb_barrier_mode_label(
    mode: HevcEncodeProbeDpbBarrierMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeDpbBarrierMode::With => "with",
        HevcEncodeProbeDpbBarrierMode::None => "none",
    }
}

pub(super) fn resolve_hevc_encode_probe_encode_reference_slot_pointer_mode()
-> HevcEncodeProbeEncodeReferenceSlotPointerMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_SLOT_POINTER_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_encode_reference_slot_pointer_mode(mode.as_deref())
}

pub(super) fn parse_hevc_encode_probe_encode_reference_slot_pointer_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeEncodeReferenceSlotPointerMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("ffmpeg") | Some("non-null") | Some("non_null") | Some("nonnull") => {
            HevcEncodeProbeEncodeReferenceSlotPointerMode::FfmpegNonNull
        }
        _ => HevcEncodeProbeEncodeReferenceSlotPointerMode::EmptySlice,
    }
}

pub(super) fn hevc_encode_probe_encode_reference_slot_pointer_mode_label(
    mode: HevcEncodeProbeEncodeReferenceSlotPointerMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeEncodeReferenceSlotPointerMode::EmptySlice => "empty-slice",
        HevcEncodeProbeEncodeReferenceSlotPointerMode::FfmpegNonNull => "ffmpeg-non-null",
    }
}

pub(super) fn resolve_hevc_encode_probe_control_mode() -> HevcEncodeProbeControlMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_CONTROL_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_control_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_nalu_mode() -> HevcEncodeProbeNaluMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_NALU_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_nalu_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_codec_info_mode() -> HevcEncodeProbeCodecInfoMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_CODEC_INFO_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_codec_info_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_picture_flags_mode() -> HevcEncodeProbePictureFlagsMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_FLAGS_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_picture_flags_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_picture_info_mode() -> HevcEncodeProbePictureInfoMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_INFO_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_picture_info_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_rate_control_mode() -> HevcEncodeProbeRateControlMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_RATE_CONTROL_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_rate_control_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_maintenance1_mode() -> HevcEncodeProbeMaintenance1Mode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_MAINTENANCE1_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_maintenance1_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_session_dpb_mode() -> HevcEncodeProbeSessionDpbMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_SESSION_DPB_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_session_dpb_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_session_h265_create_info_mode()
-> HevcEncodeProbeSessionH265CreateInfoMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_SESSION_H265_CREATE_INFO_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_session_h265_create_info_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_begin_session_parameters_mode()
-> HevcEncodeProbeBeginSessionParametersMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_SESSION_PARAMETERS_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_begin_session_parameters_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_begin_pnext_mode() -> HevcEncodeProbeBeginPnextMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_PNEXT_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_begin_pnext_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_picture_resource_extent_mode()
-> HevcEncodeProbePictureResourceExtentMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_RESOURCE_EXTENT_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_picture_resource_extent_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_source_picture_resource_extent_mode()
-> HevcEncodeProbePictureResourceExtentMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_SOURCE_PICTURE_RESOURCE_EXTENT_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_picture_resource_extent_mode(mode.as_deref())
}

pub(super) fn resolve_hevc_encode_probe_primary_mode() -> HevcEncodePrimaryProbeMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PRIMARY_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_primary_mode(mode.as_deref())
}

pub(super) fn parse_hevc_encode_probe_maintenance1_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeMaintenance1Mode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("on") | Some("enable") | Some("enabled") | Some("force-on") | Some("force_on")
        | Some("true") | Some("1") => HevcEncodeProbeMaintenance1Mode::On,
        Some("off") | Some("disable") | Some("disabled") | Some("force-off")
        | Some("force_off") | Some("false") | Some("0") | Some("none") => {
            HevcEncodeProbeMaintenance1Mode::Off
        }
        _ => HevcEncodeProbeMaintenance1Mode::Auto,
    }
}

pub(super) fn parse_hevc_encode_probe_session_dpb_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeSessionDpbMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("minimal") | Some("minimal-one") | Some("minimal_1") | Some("minimal1")
        | Some("one") | Some("1") => HevcEncodeProbeSessionDpbMode::MinimalOne,
        _ => HevcEncodeProbeSessionDpbMode::Default,
    }
}

pub(super) fn parse_hevc_encode_probe_session_h265_create_info_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeSessionH265CreateInfoMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("without") | Some("none") | Some("off") | Some("disable") | Some("disabled")
        | Some("ffmpeg") | Some("ffmpeg-like") | Some("ffmpeg_like") | Some("0") => {
            HevcEncodeProbeSessionH265CreateInfoMode::Without
        }
        _ => HevcEncodeProbeSessionH265CreateInfoMode::WithMaxLevel,
    }
}

pub(super) fn parse_hevc_encode_probe_begin_session_parameters_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeBeginSessionParametersMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("without") | Some("none") | Some("off") | Some("disable") | Some("disabled")
        | Some("no-params") | Some("no_params") | Some("0") => {
            HevcEncodeProbeBeginSessionParametersMode::Without
        }
        _ => HevcEncodeProbeBeginSessionParametersMode::With,
    }
}

pub(super) fn parse_hevc_encode_probe_begin_pnext_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeBeginPnextMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("rate-control") | Some("rate_control") | Some("rc") | Some("ffmpeg")
        | Some("ffmpeg-like") | Some("ffmpeg_like") => HevcEncodeProbeBeginPnextMode::RateControl,
        _ => HevcEncodeProbeBeginPnextMode::None,
    }
}

pub(super) fn parse_hevc_encode_probe_picture_resource_extent_mode(
    mode: Option<&str>,
) -> HevcEncodeProbePictureResourceExtentMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("image")
        | Some("image-aligned")
        | Some("image_aligned")
        | Some("aligned")
        | Some("align")
        | Some("pad")
        | Some("padded")
        | Some("1") => HevcEncodeProbePictureResourceExtentMode::ImageAligned,
        _ => HevcEncodeProbePictureResourceExtentMode::Coded,
    }
}

pub(super) fn parse_hevc_encode_probe_primary_mode(
    mode: Option<&str>,
) -> HevcEncodePrimaryProbeMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("scope-only") | Some("scope_only") | Some("scope") | Some("coding-scope")
        | Some("coding_scope") | Some("coding") => HevcEncodePrimaryProbeMode::ScopeOnly,
        Some("pre-encode-scope")
        | Some("pre_encode_scope")
        | Some("pre-encode-scope-only")
        | Some("pre_encode_scope_only")
        | Some("pre-scope")
        | Some("prescope") => HevcEncodePrimaryProbeMode::PreEncodeScopeOnly,
        Some("pre-encode") | Some("pre_encode") | Some("encode-only") | Some("encode_only")
        | Some("encode") => HevcEncodePrimaryProbeMode::PreEncode,
        Some("pre-encode-minimal")
        | Some("pre_encode_minimal")
        | Some("minimal")
        | Some("encode-minimal")
        | Some("encode_minimal") => HevcEncodePrimaryProbeMode::PreEncodeMinimal,
        _ => HevcEncodePrimaryProbeMode::Submit,
    }
}

pub(super) fn hevc_encode_probe_maintenance1_mode_label(
    mode: HevcEncodeProbeMaintenance1Mode,
) -> &'static str {
    match mode {
        HevcEncodeProbeMaintenance1Mode::Auto => "auto",
        HevcEncodeProbeMaintenance1Mode::On => "on",
        HevcEncodeProbeMaintenance1Mode::Off => "off",
    }
}

pub(super) fn hevc_encode_probe_session_dpb_mode_label(
    mode: HevcEncodeProbeSessionDpbMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeSessionDpbMode::Default => "default",
        HevcEncodeProbeSessionDpbMode::MinimalOne => "minimal-one",
    }
}

pub(super) fn hevc_encode_probe_session_h265_create_info_mode_label(
    mode: HevcEncodeProbeSessionH265CreateInfoMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeSessionH265CreateInfoMode::WithMaxLevel => "with-max-level",
        HevcEncodeProbeSessionH265CreateInfoMode::Without => "without",
    }
}

pub(super) fn hevc_encode_probe_begin_session_parameters_mode_label(
    mode: HevcEncodeProbeBeginSessionParametersMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeBeginSessionParametersMode::With => "with",
        HevcEncodeProbeBeginSessionParametersMode::Without => "without",
    }
}

pub(super) fn hevc_encode_probe_begin_pnext_mode_label(
    mode: HevcEncodeProbeBeginPnextMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeBeginPnextMode::None => "none",
        HevcEncodeProbeBeginPnextMode::RateControl => "rate-control",
    }
}

pub(super) fn hevc_encode_probe_picture_resource_extent_mode_label(
    mode: HevcEncodeProbePictureResourceExtentMode,
) -> &'static str {
    match mode {
        HevcEncodeProbePictureResourceExtentMode::Coded => "coded",
        HevcEncodeProbePictureResourceExtentMode::ImageAligned => "image-aligned",
    }
}

pub(super) fn resolve_hevc_encode_probe_session_dpb_limits(
    mode: HevcEncodeProbeSessionDpbMode,
    capability_max_dpb_slots: u32,
    capability_max_active_reference_pictures: u32,
) -> (u32, u32) {
    match mode {
        HevcEncodeProbeSessionDpbMode::Default => (
            capability_max_dpb_slots.max(1),
            capability_max_active_reference_pictures.max(1),
        ),
        HevcEncodeProbeSessionDpbMode::MinimalOne => (1, 1),
    }
}

pub(super) fn resolve_hevc_encode_probe_maintenance1_feature_enabled(
    mode: HevcEncodeProbeMaintenance1Mode,
    extension_available: bool,
    feature_supported: bool,
) -> bool {
    match mode {
        HevcEncodeProbeMaintenance1Mode::Auto | HevcEncodeProbeMaintenance1Mode::On => {
            extension_available && feature_supported
        }
        HevcEncodeProbeMaintenance1Mode::Off => false,
    }
}

pub(super) fn parse_hevc_encode_probe_control_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeControlMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("ffmpeg") | Some("ffmpeg-like") | Some("ffmpeg_like") | Some("compat") => {
            HevcEncodeProbeControlMode::Ffmpeg
        }
        Some("none") | Some("off") | Some("disable") | Some("disabled") | Some("no-control")
        | Some("no_control") => HevcEncodeProbeControlMode::None,
        _ => HevcEncodeProbeControlMode::Default,
    }
}

pub(super) fn hevc_encode_probe_control_mode_label(
    mode: HevcEncodeProbeControlMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeControlMode::Default => "default",
        HevcEncodeProbeControlMode::Ffmpeg => "ffmpeg",
        HevcEncodeProbeControlMode::None => "none",
    }
}

pub(super) fn parse_hevc_encode_probe_nalu_mode(mode: Option<&str>) -> HevcEncodeProbeNaluMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("empty") | Some("none") | Some("off") | Some("no-slices") | Some("no_slices") => {
            HevcEncodeProbeNaluMode::Empty
        }
        _ => HevcEncodeProbeNaluMode::SingleSlice,
    }
}

pub(super) fn hevc_encode_probe_nalu_mode_label(mode: HevcEncodeProbeNaluMode) -> &'static str {
    match mode {
        HevcEncodeProbeNaluMode::SingleSlice => "single-slice",
        HevcEncodeProbeNaluMode::Empty => "empty",
    }
}

pub(super) fn parse_hevc_encode_probe_codec_info_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeCodecInfoMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("with-h265-info-std-picture-only")
        | Some("with_h265_info_std_picture_only")
        | Some("with-h265-info-std-only")
        | Some("with_h265_info_std_only")
        | Some("std-picture-only")
        | Some("std_picture_only")
        | Some("std-only")
        | Some("std_only")
        | Some("std") => HevcEncodeProbeCodecInfoMode::WithH265InfoStdPictureOnly,
        Some("with-h265-info-empty-std-picture")
        | Some("with_h265_info_empty_std_picture")
        | Some("with-h265-info-empty-std")
        | Some("with_h265_info_empty_std")
        | Some("with-h265-empty-std")
        | Some("with_h265_empty_std")
        | Some("empty-std-picture")
        | Some("empty_std_picture")
        | Some("empty-std")
        | Some("empty_std") => HevcEncodeProbeCodecInfoMode::WithH265InfoEmptyStdPicture,
        Some("with-h265-info-minimal")
        | Some("with_h265_info_minimal")
        | Some("with-h265-minimal")
        | Some("with_h265_minimal")
        | Some("minimal-h265-info")
        | Some("minimal_h265_info")
        | Some("minimal") => HevcEncodeProbeCodecInfoMode::WithH265InfoMinimal,
        Some("with-h265-info-no-std-picture")
        | Some("with_h265_info_no_std_picture")
        | Some("with-h265-info-no-std")
        | Some("with_h265_info_no_std")
        | Some("with-h265-no-std")
        | Some("with_h265_no_std")
        | Some("no-std-picture")
        | Some("no_std_picture")
        | Some("nostd") => HevcEncodeProbeCodecInfoMode::WithH265InfoNoStdPicture,
        Some("without")
        | Some("without-h265")
        | Some("without_h265")
        | Some("without-h265-info")
        | Some("without_h265_info")
        | Some("none")
        | Some("off")
        | Some("disable")
        | Some("disabled") => HevcEncodeProbeCodecInfoMode::WithoutH265Info,
        _ => HevcEncodeProbeCodecInfoMode::WithH265Info,
    }
}

pub(super) fn hevc_encode_probe_codec_info_mode_label(
    mode: HevcEncodeProbeCodecInfoMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeCodecInfoMode::WithH265Info => "with-h265-info",
        HevcEncodeProbeCodecInfoMode::WithH265InfoStdPictureOnly => {
            "with-h265-info-std-picture-only"
        }
        HevcEncodeProbeCodecInfoMode::WithH265InfoEmptyStdPicture => {
            "with-h265-info-empty-std-picture"
        }
        HevcEncodeProbeCodecInfoMode::WithH265InfoMinimal => "with-h265-info-minimal",
        HevcEncodeProbeCodecInfoMode::WithH265InfoNoStdPicture => "with-h265-info-no-std-picture",
        HevcEncodeProbeCodecInfoMode::WithoutH265Info => "without-h265-info",
    }
}

pub(super) fn parse_hevc_encode_probe_picture_flags_mode(
    mode: Option<&str>,
) -> HevcEncodeProbePictureFlagsMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("non-reference")
        | Some("non_reference")
        | Some("nonref")
        | Some("non-ref")
        | Some("non_ref")
        | Some("p-frame-like")
        | Some("p_frame_like") => HevcEncodeProbePictureFlagsMode::NonReference,
        _ => HevcEncodeProbePictureFlagsMode::Default,
    }
}

pub(super) fn hevc_encode_probe_picture_flags_mode_label(
    mode: HevcEncodeProbePictureFlagsMode,
) -> &'static str {
    match mode {
        HevcEncodeProbePictureFlagsMode::Default => "default",
        HevcEncodeProbePictureFlagsMode::NonReference => "non-reference",
    }
}

pub(super) fn parse_hevc_encode_probe_picture_info_mode(
    mode: Option<&str>,
) -> HevcEncodeProbePictureInfoMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("intra-i") | Some("intra_i") | Some("intra") | Some("i") | Some("i-frame")
        | Some("i_frame") => HevcEncodeProbePictureInfoMode::IntraI,
        Some("inter-p") | Some("inter_p") | Some("inter") | Some("p") | Some("p-frame")
        | Some("p_frame") => HevcEncodeProbePictureInfoMode::InterP,
        Some("temporal-1") | Some("temporal_1") | Some("temporal1") | Some("tid-1")
        | Some("tid_1") | Some("t1") => HevcEncodeProbePictureInfoMode::Temporal1,
        Some("poc-1") | Some("poc_1") | Some("poc1") | Some("poc-plus-one")
        | Some("poc_plus_one") => HevcEncodeProbePictureInfoMode::Poc1,
        _ => HevcEncodeProbePictureInfoMode::Default,
    }
}

pub(super) fn hevc_encode_probe_picture_info_mode_label(
    mode: HevcEncodeProbePictureInfoMode,
) -> &'static str {
    match mode {
        HevcEncodeProbePictureInfoMode::Default => "default",
        HevcEncodeProbePictureInfoMode::IntraI => "intra-i",
        HevcEncodeProbePictureInfoMode::InterP => "inter-p",
        HevcEncodeProbePictureInfoMode::Temporal1 => "temporal-1",
        HevcEncodeProbePictureInfoMode::Poc1 => "poc-1",
    }
}

pub(super) fn parse_hevc_encode_probe_rate_control_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeRateControlMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("disabled") | Some("disable") | Some("off") => {
            HevcEncodeProbeRateControlMode::Disabled
        }
        Some("cbr") => HevcEncodeProbeRateControlMode::Cbr,
        Some("vbr") => HevcEncodeProbeRateControlMode::Vbr,
        Some("none") | Some("null") => HevcEncodeProbeRateControlMode::None,
        _ => HevcEncodeProbeRateControlMode::Auto,
    }
}

pub(super) fn hevc_encode_probe_rate_control_mode_label(
    mode: HevcEncodeProbeRateControlMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeRateControlMode::Auto => "auto",
        HevcEncodeProbeRateControlMode::Disabled => "disabled",
        HevcEncodeProbeRateControlMode::Cbr => "cbr",
        HevcEncodeProbeRateControlMode::Vbr => "vbr",
        HevcEncodeProbeRateControlMode::None => "none",
    }
}

pub(super) fn resolve_hevc_encode_probe_selected_rate_control_mode(
    requested_mode: HevcEncodeProbeRateControlMode,
    available_modes: vk::VideoEncodeRateControlModeFlagsKHR,
    max_rate_control_layers: u32,
) -> Option<vk::VideoEncodeRateControlModeFlagsKHR> {
    match requested_mode {
        HevcEncodeProbeRateControlMode::Auto => {
            select_hevc_encode_rate_control_mode(available_modes, max_rate_control_layers)
        }
        HevcEncodeProbeRateControlMode::Disabled => {
            if available_modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED) {
                Some(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED)
            } else {
                None
            }
        }
        HevcEncodeProbeRateControlMode::Cbr => {
            if max_rate_control_layers > 0
                && available_modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::CBR)
            {
                Some(vk::VideoEncodeRateControlModeFlagsKHR::CBR)
            } else {
                None
            }
        }
        HevcEncodeProbeRateControlMode::Vbr => {
            if max_rate_control_layers > 0
                && available_modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::VBR)
            {
                Some(vk::VideoEncodeRateControlModeFlagsKHR::VBR)
            } else {
                None
            }
        }
        HevcEncodeProbeRateControlMode::None => None,
    }
}
