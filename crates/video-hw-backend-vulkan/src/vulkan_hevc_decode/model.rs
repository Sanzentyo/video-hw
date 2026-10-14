//! Shared typed HEVC decode models, parser state, and bootstrap typestate markers.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HevcDecodePrerequisiteProbe {
    Ready,
    MissingExtensions { missing: Vec<&'static str> },
    MissingDecodeQueueFamily,
    NoCompatibleAdapter,
    DeviceInitializationFailed(String),
    ProbeUnavailable(String),
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ExtensionFlags {
    pub(super) has_video_queue: bool,
    pub(super) has_video_decode_queue: bool,
    pub(super) has_video_decode_h265: bool,
    pub(super) has_video_maintenance1: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AdapterDecodeSupport {
    pub(super) extensions: ExtensionFlags,
    pub(super) decode_queue_family_index: Option<DecodeQueueFamilyIndex>,
}

#[derive(Debug, Clone)]
pub(crate) struct HevcParameterSets {
    pub vps: Vec<u8>,
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    pub first_vcl_nalus: Vec<Vec<u8>>,
    pub parsed_sps: SpsRbsp,
    pub coded_width: u32,
    pub coded_height: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct HevcDecodeSessionBootstrap {
    pub coded_width: u32,
    pub coded_height: u32,
    pub min_coded_width: u32,
    pub min_coded_height: u32,
    pub max_coded_width: u32,
    pub max_coded_height: u32,
    pub max_dpb_slots: u32,
    pub max_active_reference_pictures: u32,
    pub max_level_idc: u32,
    pub decode_output_formats: Vec<vk::Format>,
    pub video_session_create_probe: HevcVideoSessionCreateProbe,
    pub video_session_parameters_create_probe: HevcVideoSessionParametersCreateProbe,
    pub decode_submit_skeleton_probe: HevcDecodeSubmitSkeletonProbe,
    pub decode_submit_execution_probe: HevcDecodeSubmitExecutionProbe,
}

#[derive(Debug, Clone)]
pub(crate) enum HevcVideoSessionCreateProbe {
    Created,
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) enum HevcVideoSessionParametersCreateProbe {
    Created,
    Failed(String),
    Skipped(String),
}

#[derive(Debug, Clone)]
pub(crate) enum HevcDecodeSubmitSkeletonProbe {
    Ready(HevcDecodeSubmitSkeleton),
    Failed(String),
    Skipped(String),
}

#[derive(Debug, Clone)]
pub(crate) enum HevcDecodeSubmitExecutionProbe {
    Ready {
        queue_family_index: u32,
        output_format: vk::Format,
        coded_width: u32,
        coded_height: u32,
        readback_non_zero: bool,
        readback_bytes: usize,
        readback_planes: u32,
        readback_sample_stride: usize,
        readback_sample_count: u32,
        readback_sample: Vec<u8>,
        submitted_access_units: u32,
        experimental_dpb_enabled: bool,
        experimental_dpb_mode: &'static str,
        experimental_dpb_status: String,
    },
    Failed(String),
    Skipped(String),
}

#[derive(Debug, Clone)]
pub(crate) struct HevcDecodeSubmitSkeleton {
    pub vps_id: u8,
    pub sps_id: u8,
    pub pps_id: u8,
    pub vcl_nalu_count: usize,
    pub first_slice_nal_type: Option<u8>,
    pub first_slice_pps_id: Option<u8>,
    pub first_slice_pic_order_cnt_lsb: Option<u16>,
    pub planned_dpb_slots: Vec<u8>,
    pub planned_reference_slots: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HevcAccessUnitHeader {
    pub nal_unit_type: u8,
    pub pps_id: u8,
    pub pic_order_cnt_lsb: Option<u16>,
    /// H.265 spec temporal identifier (0-based; derived from `nuh_temporal_id_plus1 - 1`).
    pub temporal_id: u8,
    /// Full (unwrapped) picture order count per H.265 spec section 8.3.1.
    pub poc_full: i32,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedHevcParameterSets(pub(super) HevcParameterSets);

#[derive(Debug, Clone, Copy)]
pub(super) struct ParsedHevcVps {
    pub(super) vps_video_parameter_set_id: u8,
    pub(super) vps_max_sub_layers_minus1: u8,
    pub(super) vps_temporal_id_nesting_flag: bool,
    pub(super) profile_tier_level: StdVideoH265ProfileTierLevel,
    pub(super) vps_sub_layer_ordering_info_present_flag: bool,
    pub(super) vps_dec_pic_buf_mgr: StdVideoH265DecPicBufMgr,
    pub(super) vps_timing_info_present_flag: bool,
    pub(super) vps_num_units_in_tick: u32,
    pub(super) vps_time_scale: u32,
    pub(super) vps_poc_proportional_to_timing_flag: bool,
    pub(super) vps_num_ticks_poc_diff_one_minus1: u32,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedHevcPps {
    pub(super) pps_pic_parameter_set_id: u8,
    pub(super) pps_seq_parameter_set_id: u8,
    pub(super) num_extra_slice_header_bits: u8,
    pub(super) num_ref_idx_l0_default_active_minus1: u8,
    pub(super) num_ref_idx_l1_default_active_minus1: u8,
    pub(super) init_qp_minus26: i8,
    pub(super) diff_cu_qp_delta_depth: u8,
    pub(super) pps_cb_qp_offset: i8,
    pub(super) pps_cr_qp_offset: i8,
    pub(super) pps_beta_offset_div2: i8,
    pub(super) pps_tc_offset_div2: i8,
    pub(super) log2_parallel_merge_level_minus2: u8,
    pub(super) num_tile_columns_minus1: u8,
    pub(super) num_tile_rows_minus1: u8,
    pub(super) column_width_minus1: [u16; 19],
    pub(super) row_height_minus1: [u16; 21],
    pub(super) dependent_slice_segments_enabled_flag: bool,
    pub(super) output_flag_present_flag: bool,
    pub(super) sign_data_hiding_enabled_flag: bool,
    pub(super) cabac_init_present_flag: bool,
    pub(super) constrained_intra_pred_flag: bool,
    pub(super) transform_skip_enabled_flag: bool,
    pub(super) cu_qp_delta_enabled_flag: bool,
    pub(super) pps_slice_chroma_qp_offsets_present_flag: bool,
    pub(super) weighted_pred_flag: bool,
    pub(super) weighted_bipred_flag: bool,
    pub(super) transquant_bypass_enabled_flag: bool,
    pub(super) tiles_enabled_flag: bool,
    pub(super) entropy_coding_sync_enabled_flag: bool,
    pub(super) uniform_spacing_flag: bool,
    pub(super) loop_filter_across_tiles_enabled_flag: bool,
    pub(super) pps_loop_filter_across_slices_enabled_flag: bool,
    pub(super) deblocking_filter_control_present_flag: bool,
    pub(super) deblocking_filter_override_enabled_flag: bool,
    pub(super) pps_deblocking_filter_disabled_flag: bool,
    pub(super) lists_modification_present_flag: bool,
    pub(super) slice_segment_header_extension_present_flag: bool,
}

/// Absolute POC values for the short-term reference picture sets derived from an inline
/// `st_ref_pic_set()` syntax element, plus the count of delta POCs in the referenced
/// prediction RPS (zero for non-predicted inline RPS).  Both `before` and `after` are stored
/// most-recent-first / nearest-first respectively, matching the ordering required by
/// `RefPicSetStCurrBefore` / `RefPicSetStCurrAfter` in the Vulkan Video API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct HevcInlineRefPicSetPocs {
    pub(super) before: [i32; HEVC_REF_PIC_SET_LIST_SIZE],
    pub(super) after: [i32; HEVC_REF_PIC_SET_LIST_SIZE],
    pub(super) before_count: usize,
    pub(super) after_count: usize,
    /// `NumDeltaPocsOfRefRpsIdx`: non-zero only when inter-RPS prediction is used.
    pub(super) num_delta_pocs_of_ref_rps_idx: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct HevcSpsShortTermRefPicSetPredictionMetadata {
    pub(super) inter_ref_pic_set_prediction_flag: bool,
    pub(super) delta_idx_minus1: u32,
    pub(super) delta_rps_sign: bool,
    pub(super) abs_delta_rps_minus1: u32,
    pub(super) used_by_curr_pic_flag: u16,
    pub(super) use_delta_flag: u16,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ParsedHevcSliceHeader {
    pub(super) nal_unit_type: u8,
    pub(super) pps_id: u8,
    pub(super) is_first_slice_segment: bool,
    pub(super) pic_order_cnt_lsb: Option<u16>,
    pub(super) slice_type: Option<u8>,
    pub(super) short_term_ref_pic_set_idx: Option<usize>,
    /// Absolute POC values for each reference in the inline `st_ref_pic_set()`, or `None`
    /// when the SPS-indexed path is used (or for IDR frames).
    pub(super) inline_short_term_ref_pic_set_pocs: Option<HevcInlineRefPicSetPocs>,
    /// Bits consumed by `st_ref_pic_set()` syntax in the slice header (when sps_flag=0),
    /// or by `short_term_ref_pic_set_idx` (when sps_flag=1 and count>1).  The 1-bit
    /// `short_term_ref_pic_set_sps_flag` is NOT included.  Zero for IDR frames and for
    /// SPS-based RPS with exactly one SPS entry.
    pub(super) num_bits_for_st_ref_pic_set_in_slice: u16,
    /// `NumDeltaPocsOfRefRpsIdx`: number of delta POCs in the referenced ST-RPS when
    /// inter-RPS prediction is used; zero for non-predicted inline RPS and for IDR frames.
    pub(super) num_delta_pocs_of_ref_rps_idx: u8,
}

pub(super) const MAX_HEVC_SUBMIT_PROBE_ACCESS_UNITS: usize = 16;
pub(super) const HEVC_REF_PIC_SET_LIST_SIZE: usize = 8;
pub(super) const HEVC_NO_REFERENCE_PICTURE: u8 = u8::MAX;
pub(super) const HEVC_NAL_HEADER_SIZE: u32 = 2;
pub(super) const HEVC_DECODE_BOOTSTRAP_CACHE_CAPACITY: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct HevcDecodeBootstrapCacheKey {
    pub(super) bitstream_hash: u64,
    pub(super) bitstream_len: usize,
    pub(super) submit_probe_access_unit_limit: Option<usize>,
    pub(super) physical_device_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HevcExperimentalDpbMode {
    Off,
    Auto,
    On,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HevcExperimentalDpbDecision {
    DisabledOff,
    EnabledOn,
    EnabledAuto,
    DisabledAutoMarkerPresent,
    DisabledAutoMarkerWriteFailed,
}

#[derive(Debug, Clone)]
pub(super) struct HevcExperimentalDpbConfiguration {
    pub(super) enabled: bool,
    pub(super) mode: HevcExperimentalDpbMode,
    pub(super) status: String,
    pub(super) marker_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HevcSubmitProbeAccessUnit {
    pub(super) header: HevcAccessUnitHeader,
    /// Byte offset of this access unit's data from the start of the bitstream buffer.
    /// Aligned to `min_bitstream_buffer_offset_alignment`.
    pub(super) buffer_offset: u64,
    /// Byte range of this access unit's data. Aligned to `min_bitstream_buffer_size_alignment`.
    pub(super) buffer_range: u64,
    /// Exact byte size of this access unit's Annex-B VCL data before range alignment.
    pub(super) vcl_size: u64,
    /// Byte offset from `buffer_offset` to the first byte of the slice segment RBSP payload
    /// (i.e. past the Annex-B start code and the 2-byte NAL unit header). The decode loop may
    /// translate this to a driver-facing start-code or NAL-header offset at submit time.
    pub(super) slice_segment_offset: u32,
    pub(super) slice_type: Option<u8>,
    pub(super) short_term_ref_pic_set_idx: Option<usize>,
    pub(super) inline_short_term_ref_pic_set_pocs: Option<HevcInlineRefPicSetPocs>,
    pub(super) num_bits_for_st_ref_pic_set_in_slice: u16,
    pub(super) num_delta_pocs_of_ref_rps_idx: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HevcActiveReferenceSlot {
    pub(super) slot: usize,
    pub(super) pic_order_cnt_val: i32,
}

#[derive(Debug, Clone)]
pub(super) struct HevcSubmitProbeBitstreamPayload {
    pub(super) bytes: Vec<u8>,
    pub(super) access_units: Vec<HevcSubmitProbeAccessUnit>,
    pub(super) parsed_pps: ParsedHevcPps,
}

#[derive(Debug, Clone)]
pub(crate) struct HevcStdParameterSetStorage {
    pub(super) vps: [StdVideoH265VideoParameterSet; 1],
    pub(super) sps: [StdVideoH265SequenceParameterSet; 1],
    pub(super) pps: [StdVideoH265PictureParameterSet; 1],
    pub(super) profile_tier_level: Box<StdVideoH265ProfileTierLevel>,
    pub(super) vps_profile_tier_level: Box<StdVideoH265ProfileTierLevel>,
    pub(super) dec_pic_buf_mgr: Box<StdVideoH265DecPicBufMgr>,
    pub(super) short_term_ref_pic_sets: Box<[StdVideoH265ShortTermRefPicSet]>,
    pub(super) long_term_ref_pics_sps: Option<Box<StdVideoH265LongTermRefPicsSps>>,
    pub(super) sequence_parameter_set_vui: Option<Box<StdVideoH265SequenceParameterSetVui>>,
    pub(super) sequence_parameter_set_vui_hrd: Option<Box<StdVideoH265HrdParameters>>,
    pub(super) vps_dec_pic_buf_mgr: Box<StdVideoH265DecPicBufMgr>,
    pub(super) vps_hrd_parameters: Box<StdVideoH265HrdParameters>,
}

impl HevcStdParameterSetStorage {
    pub(super) fn add_info(&self) -> vk::VideoDecodeH265SessionParametersAddInfoKHR<'_> {
        vk::VideoDecodeH265SessionParametersAddInfoKHR::default()
            .std_vp_ss(&self.vps)
            .std_sp_ss(&self.sps)
            .std_pp_ss(&self.pps)
    }

    pub(crate) fn encode_add_info_with_filter(
        &self,
        include_vps: bool,
        include_sps: bool,
        include_pps: bool,
    ) -> vk::VideoEncodeH265SessionParametersAddInfoKHR<'_> {
        let vps: &[StdVideoH265VideoParameterSet] = if include_vps { &self.vps } else { &[] };
        let sps: &[StdVideoH265SequenceParameterSet] = if include_sps { &self.sps } else { &[] };
        let pps: &[StdVideoH265PictureParameterSet] = if include_pps { &self.pps } else { &[] };
        vk::VideoEncodeH265SessionParametersAddInfoKHR::default()
            .std_vp_ss(vps)
            .std_sp_ss(sps)
            .std_pp_ss(pps)
    }

    pub(crate) fn encode_parameter_set_ids(&self) -> (u8, u8, u8) {
        (
            self.vps[0].vps_video_parameter_set_id,
            self.sps[0].sps_seq_parameter_set_id,
            self.pps[0].pps_pic_parameter_set_id,
        )
    }

    pub(crate) fn encode_pps_init_qp_minus26(&self) -> i8 {
        self.pps[0].init_qp_minus26
    }

    #[cfg(test)]
    pub(crate) fn encode_sps_picture_size(&self) -> (u32, u32) {
        (
            self.sps[0].pic_width_in_luma_samples,
            self.sps[0].pic_height_in_luma_samples,
        )
    }

    pub(crate) fn override_encode_sps_vui_parameters_present_flag(&mut self, present: bool) {
        self.sps[0]
            .flags
            .set_vui_parameters_present_flag(bool_to_u32(present));
    }

    pub(crate) fn override_encode_sps_picture_size(&mut self, coded_width: u32, coded_height: u32) {
        self.sps[0].pic_width_in_luma_samples = coded_width;
        self.sps[0].pic_height_in_luma_samples = coded_height;
        self.sps[0].conf_win_left_offset = 0;
        self.sps[0].conf_win_right_offset = 0;
        self.sps[0].conf_win_top_offset = 0;
        self.sps[0].conf_win_bottom_offset = 0;
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RbspBitReader<'a> {
    pub(super) data: &'a [u8],
    pub(super) bit_offset: usize,
}

impl<'a> RbspBitReader<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_offset: 0,
        }
    }

    pub(super) fn read_flag(&mut self) -> Result<bool, String> {
        Ok(self.read_bits(1)? != 0)
    }

    pub(super) fn read_bits(&mut self, bit_count: usize) -> Result<u32, String> {
        if bit_count > 32 {
            return Err(format!(
                "cannot read more than 32 bits at once (requested {bit_count})"
            ));
        }
        if bit_count == 0 {
            return Ok(0);
        }

        let mut value = 0_u32;
        for _ in 0..bit_count {
            let byte_index = self.bit_offset / 8;
            let bit_in_byte = 7 - (self.bit_offset % 8);
            let Some(byte) = self.data.get(byte_index) else {
                return Err("unexpected end of RBSP while reading bits".to_string());
            };
            value = (value << 1) | u32::from((byte >> bit_in_byte) & 1);
            self.bit_offset += 1;
        }
        Ok(value)
    }

    pub(super) fn read_ue(&mut self) -> Result<u32, String> {
        let mut leading_zeros = 0_usize;
        while !self.read_flag()? {
            leading_zeros += 1;
            if leading_zeros > 31 {
                return Err("unsupported Exp-Golomb value wider than 32 bits".to_string());
            }
        }

        if leading_zeros == 0 {
            return Ok(0);
        }

        let suffix = self.read_bits(leading_zeros)?;
        let code_num = ((1_u64 << leading_zeros) - 1) + u64::from(suffix);
        u32::try_from(code_num)
            .map_err(|_| format!("Exp-Golomb value {code_num} exceeds u32 range"))
    }

    pub(super) fn read_se(&mut self) -> Result<i32, String> {
        let code_num = i64::from(self.read_ue()?);
        let signed = if (code_num & 1) == 0 {
            -(code_num / 2)
        } else {
            (code_num + 1) / 2
        };
        i32::try_from(signed)
            .map_err(|_| format!("signed Exp-Golomb value {signed} exceeds i32 range"))
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DecodeQueueFamilyIndex(pub(super) u32);

#[derive(Debug, Clone)]
pub(super) struct AwaitingCapabilityProbe;

#[derive(Debug, Clone)]
pub(super) struct CapabilityProbeComplete {
    pub(super) physical_device: vk::PhysicalDevice,
    pub(super) queue_family_index: DecodeQueueFamilyIndex,
    pub(super) capability_snapshot: HevcCapabilitySnapshot,
    pub(super) decode_output_formats: Vec<vk::Format>,
}

#[derive(Debug, Clone)]
pub(super) struct HevcCapabilitySnapshot {
    pub(super) min_bitstream_buffer_offset_alignment: vk::DeviceSize,
    pub(super) min_bitstream_buffer_size_alignment: vk::DeviceSize,
    pub(super) picture_access_granularity: vk::Extent2D,
    pub(super) min_coded_extent: vk::Extent2D,
    pub(super) max_coded_extent: vk::Extent2D,
    pub(super) max_dpb_slots: u32,
    pub(super) max_active_reference_pictures: u32,
    pub(super) max_level_idc: u32,
    pub(super) std_header_version: vk::ExtensionProperties,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HevcSliceSegmentOffsetMode {
    RbspPayload,
    NaluHeader,
    AnnexBStartCode,
    GlobalRbspPayload,
    MemoryBindingAbsolute,
    Fixed(u32),
}

impl HevcSliceSegmentOffsetMode {
    pub(super) fn slice_segment_offset(self, access_unit: &HevcSubmitProbeAccessUnit) -> u32 {
        match self {
            Self::RbspPayload => access_unit.slice_segment_offset,
            Self::GlobalRbspPayload => access_unit.slice_segment_offset,
            Self::NaluHeader => access_unit
                .slice_segment_offset
                .saturating_sub(HEVC_NAL_HEADER_SIZE),
            Self::AnnexBStartCode => 0,
            Self::MemoryBindingAbsolute => access_unit
                .buffer_offset
                .saturating_add(u64::from(access_unit.slice_segment_offset))
                .try_into()
                .unwrap_or(u32::MAX),
            Self::Fixed(offset) => offset,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct HevcDecodeSubmitExecutionContext<'a> {
    pub(super) instance: &'a ash::Instance,
    pub(super) physical_device: vk::PhysicalDevice,
    pub(super) queue_family_index: DecodeQueueFamilyIndex,
    pub(super) output_format: vk::Format,
    pub(super) capability_snapshot: &'a HevcCapabilitySnapshot,
    pub(super) parameter_sets: &'a HevcParameterSets,
    pub(super) bitstream: &'a [u8],
    pub(super) submit_probe_access_unit_limit: usize,
}

#[derive(Debug, Clone)]
pub(super) struct HevcSessionBootstrapMachine<State> {
    pub(super) parsed: ParsedHevcParameterSets,
    pub(super) state: State,
    pub(super) _marker: PhantomData<State>,
}

impl ExtensionFlags {
    pub(super) fn supports_hevc_decode(self) -> bool {
        self.has_video_queue && self.has_video_decode_queue && self.has_video_decode_h265
    }

    pub(super) fn union_assign(&mut self, other: Self) {
        self.has_video_queue |= other.has_video_queue;
        self.has_video_decode_queue |= other.has_video_decode_queue;
        self.has_video_decode_h265 |= other.has_video_decode_h265;
        self.has_video_maintenance1 |= other.has_video_maintenance1;
    }
}
