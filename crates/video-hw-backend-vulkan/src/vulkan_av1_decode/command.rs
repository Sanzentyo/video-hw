//! Typed AV1 decode command models and Vulkan command-recording views.

use super::*;

pub(super) const AV1_SELECT_SCREEN_CONTENT_TOOLS: u8 = 2;
pub(super) const AV1_SELECT_INTEGER_MV: u8 = 2;
pub(super) const AV1_WARPEDMODEL_PRECISION_IDENTITY: i32 = 1 << 16;
pub(super) const AV1_MAX_SEGMENTS: usize = 8;
pub(super) const AV1_SEG_LVL_MAX: usize = 8;
pub(super) const AV1_SEG_FEATURE_BITS: [usize; AV1_SEG_LVL_MAX] = [8, 6, 6, 6, 6, 3, 0, 0];
pub(super) const AV1_SEG_FEATURE_SIGNED: [bool; AV1_SEG_LVL_MAX] =
    [true, true, true, true, true, false, false, false];

pub(super) const fn bool_to_u32(value: bool) -> u32 {
    value as u32
}

pub(super) const fn bool_to_u8(value: bool) -> u8 {
    value as u8
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Av1DecodePrerequisiteProbe {
    Ready,
    MissingExtensions { missing: Vec<&'static str> },
    MissingDecodeQueueFamily,
    NoCompatibleAdapter,
    SessionBootstrapFailed(String),
    ProbeUnavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av1BitstreamInspection {
    pub obu_count: usize,
    pub temporal_unit_count: usize,
    pub has_sequence_header: bool,
    pub has_frame_payload: bool,
    pub sequence_header_obu_len: Option<usize>,
    pub frame_header_count: usize,
    pub key_frame_count: usize,
    pub inter_frame_count: usize,
    pub intra_only_frame_count: usize,
    pub switch_frame_count: usize,
    pub show_existing_frame_count: usize,
    pub coded_width: Option<u32>,
    pub coded_height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av1DecodeBitstreamSessionProbe {
    pub coded_width: u32,
    pub coded_height: u32,
    pub picture_format: vk::Format,
    pub min_bitstream_buffer_offset_alignment: u64,
    pub min_bitstream_buffer_size_alignment: u64,
    pub session_memory_requirement_count: usize,
    pub session_memory_total_size: u64,
    pub session_memory_max_alignment: u64,
    pub session_memory_bound_count: usize,
    pub bitstream_upload_bytes: usize,
    pub decode_image_layers: u32,
    pub decode_image_barrier_layers: u32,
    pub readback_bytes: u64,
    pub readback_region_count: usize,
    pub readback_sample_stride: u64,
    pub readback_sample_count: u32,
    pub readback_mapped_bytes: usize,
    pub readback_non_zero: bool,
    pub readback_sample: Vec<u8>,
    pub command_record_decode_count: usize,
    pub command_buffer_recorded: bool,
    pub command_buffer_submitted: bool,
    pub decode_query_status_raw: Option<i32>,
}

#[derive(Debug, Clone)]
pub(crate) struct Av1DecodeReadbackFrame {
    pub coded_width: u32,
    pub coded_height: u32,
    pub data: Vec<u8>,
    pub readback_non_zero: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Av1DecodeSubmitSkeleton {
    pub temporal_unit_index: usize,
    pub frame_header_offset: u32,
    pub tile_offsets: Vec<u32>,
    pub tile_sizes: Vec<u32>,
    pub use_128x128_superblock: bool,
    pub key_frame_header: Option<Av1ParsedKeyFrameHeader>,
    pub frame_header_summary: Option<Av1ParsedFrameHeaderSummary>,
    pub std_overrides: Option<Av1StdPictureOverrides>,
}

#[derive(Debug, Clone)]
pub(crate) struct Av1DecodePictureInfoSkeleton {
    pub std_picture_info: StdVideoDecodeAV1PictureInfo,
    pub reference_name_slot_indices: [i32; vk::MAX_VIDEO_AV1_REFERENCES_PER_FRAME_KHR],
    pub frame_header_offset: u32,
    pub tile_offsets: Vec<u32>,
    pub tile_sizes: Vec<u32>,
    pub use_128x128_superblock: bool,
    pub key_frame_header: Option<Av1ParsedKeyFrameHeader>,
    pub frame_header_summary: Option<Av1ParsedFrameHeaderSummary>,
    pub std_overrides: Option<Av1StdPictureOverrides>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Av1StdPictureOverrides {
    pub disable_cdf_update: bool,
    pub current_frame_id: u32,
    pub use_superres: bool,
    pub render_and_frame_size_different: bool,
    pub allow_screen_content_tools: bool,
    pub force_integer_mv: bool,
    pub frame_size_override_flag: bool,
    pub allow_intrabc: bool,
    pub allow_high_precision_mv: bool,
    pub is_filter_switchable: bool,
    pub interpolation_filter: ash::vk::native::StdVideoAV1InterpolationFilter,
    pub is_motion_mode_switchable: bool,
    pub use_ref_frame_mvs: bool,
    pub disable_frame_end_update_cdf: bool,
    pub allow_warped_motion: bool,
    pub reduced_tx_set: bool,
    pub reference_select: bool,
    pub skip_mode_present: bool,
    pub delta_q_present: bool,
    pub delta_q_res: u8,
    pub delta_lf_present: bool,
    pub delta_lf_res: u8,
    pub delta_lf_multi: bool,
    pub segmentation_enabled: bool,
    pub segmentation_update_map: bool,
    pub segmentation_temporal_update: bool,
    pub segmentation_update_data: bool,
    pub uses_lr: bool,
    pub uses_chroma_lr: bool,
    pub apply_grain: bool,
    pub skip_mode_frame: [u8; 2],
    pub coded_denom: u8,
    pub order_hints: [u8; 8],
    pub expected_frame_id: [u32; 8],
    pub tx_mode: ash::vk::native::StdVideoAV1TxMode,
    pub quantization: StdVideoAV1Quantization,
    pub segmentation: StdVideoAV1Segmentation,
    pub loop_filter: StdVideoAV1LoopFilter,
    pub cdef: StdVideoAV1CDEF,
    pub loop_restoration: StdVideoAV1LoopRestoration,
    pub global_motion: StdVideoAV1GlobalMotion,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Av1ParsedKeyFrameHeader {
    pub tile_payload_offset: usize,
    pub disable_cdf_update: bool,
    pub allow_screen_content_tools: bool,
    pub force_integer_mv: bool,
    pub frame_size_override_flag: bool,
    pub order_hint: u8,
    pub allow_intrabc: bool,
    pub disable_frame_end_update_cdf: bool,
    pub base_q_idx: u8,
    pub delta_q_y_dc: i8,
    pub diff_uv_delta: bool,
    pub delta_q_u_dc: i8,
    pub delta_q_u_ac: i8,
    pub delta_q_v_dc: i8,
    pub delta_q_v_ac: i8,
    pub using_qmatrix: bool,
    pub qm_y: u8,
    pub qm_u: u8,
    pub qm_v: u8,
    pub segmentation_enabled: bool,
    pub segmentation_update_map: bool,
    pub segmentation_temporal_update: bool,
    pub segmentation_update_data: bool,
    pub segmentation: StdVideoAV1Segmentation,
    pub delta_q_present: bool,
    pub delta_q_res: u8,
    pub loop_filter_level: [u8; 4],
    pub loop_filter_sharpness: u8,
    pub loop_filter_delta_enabled: bool,
    pub loop_filter_delta_update: bool,
    pub cdef_damping_minus_3: u8,
    pub cdef_bits: u8,
    pub cdef_y_pri_strength: [u8; 8],
    pub cdef_y_sec_strength: [u8; 8],
    pub cdef_uv_pri_strength: [u8; 8],
    pub cdef_uv_sec_strength: [u8; 8],
    pub tx_mode: u32,
    pub reduced_tx_set: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Av1ParsedQuantization {
    pub(super) base_q_idx: u8,
    pub(super) delta_q_y_dc: i8,
    pub(super) diff_uv_delta: bool,
    pub(super) delta_q_u_dc: i8,
    pub(super) delta_q_u_ac: i8,
    pub(super) delta_q_v_dc: i8,
    pub(super) delta_q_v_ac: i8,
    pub(super) using_qmatrix: bool,
    pub(super) qm_y: u8,
    pub(super) qm_u: u8,
    pub(super) qm_v: u8,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Av1ParsedSegmentation {
    pub(super) enabled: bool,
    pub(super) update_map: bool,
    pub(super) temporal_update: bool,
    pub(super) update_data: bool,
    pub(super) std_segmentation: StdVideoAV1Segmentation,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Av1ParsedFrameHeaderSummary {
    pub frame_type: u8,
    pub show_existing_frame: bool,
    pub error_resilient_mode: bool,
    pub order_hint: u8,
    pub enable_order_hint: bool,
    pub order_hint_bits: u32,
    pub primary_ref_frame: u8,
    pub refresh_frame_flags: u8,
    pub ref_frame_idx: [i32; vk::MAX_VIDEO_AV1_REFERENCES_PER_FRAME_KHR],
    pub tile_payload_offset: usize,
    pub std_overrides: Option<Av1StdPictureOverrides>,
}

impl Av1ParsedFrameHeaderSummary {
    pub(super) fn is_key_frame(self) -> bool {
        self.frame_type == 0 && !self.show_existing_frame
    }
}

impl Av1DecodePictureInfoSkeleton {
    pub(crate) fn vk_picture_info(&self) -> vk::VideoDecodeAV1PictureInfoKHR<'_> {
        vk::VideoDecodeAV1PictureInfoKHR::default()
            .std_picture_info(&self.std_picture_info)
            .reference_name_slot_indices(self.reference_name_slot_indices)
            .frame_header_offset(self.frame_header_offset)
            .tile_offsets(&self.tile_offsets)
            .tile_sizes(&self.tile_sizes)
    }
}

pub(super) struct Av1DecodeStdPictureInfoScope {
    pub(super) std_picture_info: StdVideoDecodeAV1PictureInfo,
    pub(super) tile_info: StdVideoAV1TileInfo,
    pub(super) quantization: StdVideoAV1Quantization,
    pub(super) segmentation: StdVideoAV1Segmentation,
    pub(super) loop_filter: StdVideoAV1LoopFilter,
    pub(super) cdef: StdVideoAV1CDEF,
    pub(super) loop_restoration: StdVideoAV1LoopRestoration,
    pub(super) global_motion: StdVideoAV1GlobalMotion,
    pub(super) mi_col_starts: [u16; 64],
    pub(super) mi_row_starts: [u16; 64],
    pub(super) width_in_sbs_minus1: [u16; 64],
    pub(super) height_in_sbs_minus1: [u16; 64],
}

impl Av1DecodeStdPictureInfoScope {
    pub(super) fn new(
        base: StdVideoDecodeAV1PictureInfo,
        coded_width: u32,
        coded_height: u32,
        use_128x128_superblock: bool,
        key_frame_header: Option<&Av1ParsedKeyFrameHeader>,
        std_overrides: Option<&Av1StdPictureOverrides>,
    ) -> Self {
        let mi_cols = u16::try_from(coded_width.div_ceil(4).max(1)).unwrap_or(u16::MAX);
        let mi_rows = u16::try_from(coded_height.div_ceil(4).max(1)).unwrap_or(u16::MAX);
        let sb_mi_shift = if use_128x128_superblock { 5 } else { 4 };
        let sb_mi_size = 1_u16 << sb_mi_shift;
        let sb_cols = mi_cols.div_ceil(sb_mi_size).max(1);
        let sb_rows = mi_rows.div_ceil(sb_mi_size).max(1);
        let mut mi_col_starts = [0_u16; 64];
        let mut mi_row_starts = [0_u16; 64];
        mi_col_starts[1] = sb_cols.saturating_mul(sb_mi_size);
        mi_row_starts[1] = sb_rows.saturating_mul(sb_mi_size);
        let mut width_in_sbs_minus1 = [0_u16; 64];
        let mut height_in_sbs_minus1 = [0_u16; 64];
        width_in_sbs_minus1[0] = sb_cols.saturating_sub(1);
        height_in_sbs_minus1[0] = sb_rows.saturating_sub(1);

        let mut scope = Self {
            std_picture_info: base,
            tile_info: StdVideoAV1TileInfo {
                flags: StdVideoAV1TileInfoFlags {
                    _bitfield_align_1: [],
                    _bitfield_1: StdVideoAV1TileInfoFlags::new_bitfield_1(1, 0),
                },
                TileCols: 1,
                TileRows: 1,
                context_update_tile_id: 0,
                tile_size_bytes_minus_1: 0,
                reserved1: [0; 7],
                pMiColStarts: std::ptr::null(),
                pMiRowStarts: std::ptr::null(),
                pWidthInSbsMinus1: std::ptr::null(),
                pHeightInSbsMinus1: std::ptr::null(),
            },
            quantization: StdVideoAV1Quantization {
                flags: StdVideoAV1QuantizationFlags {
                    _bitfield_align_1: [],
                    _bitfield_1: StdVideoAV1QuantizationFlags::new_bitfield_1(0, 0, 0),
                },
                base_q_idx: 0,
                DeltaQYDc: 0,
                DeltaQUDc: 0,
                DeltaQUAc: 0,
                DeltaQVDc: 0,
                DeltaQVAc: 0,
                qm_y: 0,
                qm_u: 0,
                qm_v: 0,
            },
            segmentation: StdVideoAV1Segmentation {
                FeatureEnabled: [0; 8],
                FeatureData: [[0; 8]; 8],
            },
            loop_filter: StdVideoAV1LoopFilter {
                flags: StdVideoAV1LoopFilterFlags {
                    _bitfield_align_1: [],
                    _bitfield_1: StdVideoAV1LoopFilterFlags::new_bitfield_1(0, 0, 0),
                },
                loop_filter_level: [0; 4],
                loop_filter_sharpness: 0,
                update_ref_delta: 0,
                loop_filter_ref_deltas: [1, 0, 0, 0, -1, 0, -1, -1],
                update_mode_delta: 0,
                loop_filter_mode_deltas: [0; 2],
            },
            cdef: StdVideoAV1CDEF {
                cdef_damping_minus_3: 0,
                cdef_bits: 0,
                cdef_y_pri_strength: [0; 8],
                cdef_y_sec_strength: [0; 8],
                cdef_uv_pri_strength: [0; 8],
                cdef_uv_sec_strength: [0; 8],
            },
            loop_restoration: StdVideoAV1LoopRestoration {
                FrameRestorationType:
                    [StdVideoAV1FrameRestorationType_STD_VIDEO_AV1_FRAME_RESTORATION_TYPE_NONE; 3],
                LoopRestorationSize: [1; 3],
            },
            global_motion: StdVideoAV1GlobalMotion {
                GmType: [0; 8],
                gm_params: av1_identity_global_motion_params(),
            },
            mi_col_starts,
            mi_row_starts,
            width_in_sbs_minus1,
            height_in_sbs_minus1,
        };
        if let Some(header) = key_frame_header {
            scope.std_picture_info.flags._bitfield_1 =
                StdVideoDecodeAV1PictureInfoFlags::new_bitfield_1(
                    1,
                    header.disable_cdf_update as u32,
                    0,
                    0,
                    header.allow_screen_content_tools as u32,
                    0,
                    header.force_integer_mv as u32,
                    header.frame_size_override_flag as u32,
                    0,
                    header.allow_intrabc as u32,
                    0,
                    0,
                    0,
                    0,
                    header.disable_frame_end_update_cdf as u32,
                    0,
                    header.reduced_tx_set as u32,
                    0,
                    0,
                    header.delta_q_present as u32,
                    0,
                    header.segmentation_enabled as u32,
                    header.segmentation_update_map as u32,
                    header.segmentation_temporal_update as u32,
                    header.segmentation_update_data as u32,
                    0,
                    0,
                    0,
                    0,
                    0,
                );
            scope.std_picture_info.OrderHint = header.order_hint;
            scope.std_picture_info.delta_q_res = header.delta_q_res;
            scope.quantization.flags._bitfield_1 = StdVideoAV1QuantizationFlags::new_bitfield_1(
                header.using_qmatrix as u32,
                header.diff_uv_delta as u32,
                0,
            );
            scope.quantization.base_q_idx = header.base_q_idx;
            scope.quantization.DeltaQYDc = header.delta_q_y_dc;
            scope.quantization.DeltaQUDc = header.delta_q_u_dc;
            scope.quantization.DeltaQUAc = header.delta_q_u_ac;
            scope.quantization.DeltaQVDc = header.delta_q_v_dc;
            scope.quantization.DeltaQVAc = header.delta_q_v_ac;
            scope.quantization.qm_y = header.qm_y;
            scope.quantization.qm_u = header.qm_u;
            scope.quantization.qm_v = header.qm_v;
            scope.segmentation = header.segmentation;
            scope.loop_filter.flags._bitfield_1 = StdVideoAV1LoopFilterFlags::new_bitfield_1(
                header.loop_filter_delta_enabled as u32,
                header.loop_filter_delta_update as u32,
                0,
            );
            scope.loop_filter.loop_filter_level = header.loop_filter_level;
            scope.loop_filter.loop_filter_sharpness = header.loop_filter_sharpness;
            scope.cdef.cdef_damping_minus_3 = header.cdef_damping_minus_3;
            scope.cdef.cdef_bits = header.cdef_bits;
            scope.cdef.cdef_y_pri_strength = header.cdef_y_pri_strength;
            scope.cdef.cdef_y_sec_strength = header.cdef_y_sec_strength;
            scope.cdef.cdef_uv_pri_strength = header.cdef_uv_pri_strength;
            scope.cdef.cdef_uv_sec_strength = header.cdef_uv_sec_strength;
            scope.std_picture_info.TxMode = header.tx_mode;
        }
        if let Some(overrides) = std_overrides {
            scope
                .std_picture_info
                .flags
                .set_disable_cdf_update(bool_to_u32(overrides.disable_cdf_update));
            scope
                .std_picture_info
                .flags
                .set_use_superres(bool_to_u32(overrides.use_superres));
            scope
                .std_picture_info
                .flags
                .set_render_and_frame_size_different(bool_to_u32(
                    overrides.render_and_frame_size_different,
                ));
            scope
                .std_picture_info
                .flags
                .set_allow_screen_content_tools(bool_to_u32(overrides.allow_screen_content_tools));
            scope
                .std_picture_info
                .flags
                .set_force_integer_mv(bool_to_u32(overrides.force_integer_mv));
            scope
                .std_picture_info
                .flags
                .set_frame_size_override_flag(bool_to_u32(overrides.frame_size_override_flag));
            scope
                .std_picture_info
                .flags
                .set_allow_intrabc(bool_to_u32(overrides.allow_intrabc));
            scope
                .std_picture_info
                .flags
                .set_allow_high_precision_mv(bool_to_u32(overrides.allow_high_precision_mv));
            scope
                .std_picture_info
                .flags
                .set_is_filter_switchable(bool_to_u32(overrides.is_filter_switchable));
            scope
                .std_picture_info
                .flags
                .set_is_motion_mode_switchable(bool_to_u32(overrides.is_motion_mode_switchable));
            scope
                .std_picture_info
                .flags
                .set_use_ref_frame_mvs(bool_to_u32(overrides.use_ref_frame_mvs));
            scope
                .std_picture_info
                .flags
                .set_disable_frame_end_update_cdf(bool_to_u32(
                    overrides.disable_frame_end_update_cdf,
                ));
            scope
                .std_picture_info
                .flags
                .set_allow_warped_motion(bool_to_u32(overrides.allow_warped_motion));
            scope
                .std_picture_info
                .flags
                .set_reduced_tx_set(bool_to_u32(overrides.reduced_tx_set));
            scope
                .std_picture_info
                .flags
                .set_reference_select(bool_to_u32(overrides.reference_select));
            scope
                .std_picture_info
                .flags
                .set_skip_mode_present(bool_to_u32(overrides.skip_mode_present));
            scope
                .std_picture_info
                .flags
                .set_delta_q_present(bool_to_u32(overrides.delta_q_present));
            scope
                .std_picture_info
                .flags
                .set_delta_lf_present(bool_to_u32(overrides.delta_lf_present));
            scope
                .std_picture_info
                .flags
                .set_delta_lf_multi(bool_to_u32(overrides.delta_lf_multi));
            scope
                .std_picture_info
                .flags
                .set_segmentation_enabled(bool_to_u32(overrides.segmentation_enabled));
            scope
                .std_picture_info
                .flags
                .set_segmentation_update_map(bool_to_u32(overrides.segmentation_update_map));
            scope
                .std_picture_info
                .flags
                .set_segmentation_temporal_update(bool_to_u32(
                    overrides.segmentation_temporal_update,
                ));
            scope
                .std_picture_info
                .flags
                .set_segmentation_update_data(bool_to_u32(overrides.segmentation_update_data));
            scope
                .std_picture_info
                .flags
                .set_UsesLr(bool_to_u32(overrides.uses_lr));
            scope
                .std_picture_info
                .flags
                .set_usesChromaLr(bool_to_u32(overrides.uses_chroma_lr));
            scope
                .std_picture_info
                .flags
                .set_apply_grain(bool_to_u32(overrides.apply_grain));
            scope.std_picture_info.current_frame_id = overrides.current_frame_id;
            scope.std_picture_info.interpolation_filter = overrides.interpolation_filter;
            scope.std_picture_info.TxMode = overrides.tx_mode;
            scope.std_picture_info.delta_q_res = overrides.delta_q_res;
            scope.std_picture_info.delta_lf_res = overrides.delta_lf_res;
            scope.std_picture_info.SkipModeFrame = overrides.skip_mode_frame;
            scope.std_picture_info.coded_denom = overrides.coded_denom;
            scope.std_picture_info.OrderHints = overrides.order_hints;
            scope.std_picture_info.expectedFrameId = overrides.expected_frame_id;
            scope.quantization = overrides.quantization;
            scope.segmentation = overrides.segmentation;
            scope.loop_filter = overrides.loop_filter;
            scope.cdef = overrides.cdef;
            scope.loop_restoration = overrides.loop_restoration;
            scope.global_motion = overrides.global_motion;
        }
        scope
    }

    fn attach_pointers(&mut self) {
        self.tile_info.pMiColStarts = self.mi_col_starts.as_ptr();
        self.tile_info.pMiRowStarts = self.mi_row_starts.as_ptr();
        self.tile_info.pWidthInSbsMinus1 = self.width_in_sbs_minus1.as_ptr();
        self.tile_info.pHeightInSbsMinus1 = self.height_in_sbs_minus1.as_ptr();
        self.std_picture_info.pTileInfo = &self.tile_info;
        self.std_picture_info.pQuantization = &self.quantization;
        self.std_picture_info.pSegmentation = &self.segmentation;
        self.std_picture_info.pLoopFilter = &self.loop_filter;
        self.std_picture_info.pCDEF = &self.cdef;
        self.std_picture_info.pLoopRestoration = &self.loop_restoration;
        self.std_picture_info.pGlobalMotion = &self.global_motion;
        self.std_picture_info.pFilmGrain = std::ptr::null();
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Av1DecodeInfoSkeleton {
    pub temporal_unit_index: usize,
    pub src_buffer_offset: u64,
    pub src_buffer_range: u64,
    pub coded_width: u32,
    pub coded_height: u32,
    pub picture_info: Av1DecodePictureInfoSkeleton,
    pub setup_reference_info: StdVideoDecodeAV1ReferenceInfo,
    pub reference_slot_infos: Vec<StdVideoDecodeAV1ReferenceInfo>,
    pub reference_base_array_layers: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av1DecodeFrameCommandSkeleton {
    pub frame_index: usize,
    pub temporal_unit_index: usize,
    pub setup_slot_index: i32,
    pub dst_base_array_layer: u32,
    pub reference_slot_indices: Vec<i32>,
    pub reference_base_array_layers: Vec<u32>,
    pub src_buffer_offset: u64,
    pub src_buffer_range: u64,
    pub tile_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av1BeginCodingSlotSkeleton {
    pub slot_index: i32,
    pub base_array_layer: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av1DecodeCommandSkeleton {
    pub coded_width: u32,
    pub coded_height: u32,
    pub begin_slots: Vec<Av1BeginCodingSlotSkeleton>,
    pub frames: Vec<Av1DecodeFrameCommandSkeleton>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av1DecodeImagePlan {
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    pub array_layers: u32,
    pub usage: vk::ImageUsageFlags,
}

#[derive(Debug, Clone)]
pub(crate) struct Av1DecodeReadbackPlan {
    pub buffer_size: u64,
    pub sample_stride: u64,
    pub sample_count: u32,
    pub regions: Vec<vk::BufferImageCopy>,
}

#[derive(Debug, Clone)]
pub(crate) struct Av1DecodeBitstreamUploadPlan {
    pub bytes: Vec<u8>,
    pub decodes: Vec<Av1DecodeInfoSkeleton>,
}

impl Av1DecodeBitstreamUploadPlan {
    pub(crate) fn frame_upload_ranges(
        &self,
        command: &Av1DecodeCommandSkeleton,
    ) -> Result<Vec<Range<usize>>, String> {
        if command.frames.len() != self.decodes.len() {
            return Err(format!(
                "AV1 aligned upload frame count mismatch: command_frames={}, decodes={}",
                command.frames.len(),
                self.decodes.len()
            ));
        }

        command
            .frames
            .iter()
            .enumerate()
            .map(|(decode_index, frame)| {
                let decode = &self.decodes[decode_index];
                if frame.frame_index != decode_index {
                    return Err(format!(
                        "AV1 aligned upload frame index mismatch: frame_index={}, decode_index={decode_index}",
                        frame.frame_index
                    ));
                }
                if frame.src_buffer_offset != decode.src_buffer_offset
                    || frame.src_buffer_range != decode.src_buffer_range
                {
                    return Err(format!(
                        "AV1 aligned upload source range mismatch for frame {}: command={}+{}, decode={}+{}",
                        frame.frame_index,
                        frame.src_buffer_offset,
                        frame.src_buffer_range,
                        decode.src_buffer_offset,
                        decode.src_buffer_range
                    ));
                }
                let start = usize::try_from(frame.src_buffer_offset)
                    .map_err(|_| "AV1 aligned upload offset exceeds usize".to_string())?;
                let range_len = usize::try_from(frame.src_buffer_range)
                    .map_err(|_| "AV1 aligned upload range exceeds usize".to_string())?;
                let end = start
                    .checked_add(range_len)
                    .ok_or_else(|| "AV1 aligned upload range overflows usize".to_string())?;
                if end > self.bytes.len() {
                    return Err(format!(
                        "AV1 aligned upload range exceeds upload bytes: end={end}, len={}",
                        self.bytes.len()
                    ));
                }
                Ok(start..end)
            })
            .collect()
    }

    pub(crate) fn frame_submit_bundles(
        &self,
        command: &Av1DecodeCommandSkeleton,
    ) -> Result<Vec<Av1DecodeFrameSubmitBundle>, String> {
        let frame_records = command.frame_record_bundles()?;
        let upload_ranges = self.frame_upload_ranges(command)?;
        if frame_records.len() != upload_ranges.len() {
            return Err(format!(
                "AV1 frame submit bundle count mismatch: records={}, ranges={}",
                frame_records.len(),
                upload_ranges.len()
            ));
        }

        Ok(frame_records
            .into_iter()
            .zip(upload_ranges)
            .map(|(record, upload_range)| Av1DecodeFrameSubmitBundle {
                frame_index: record.frame_index,
                temporal_unit_index: record.temporal_unit_index,
                decode_info_index: record.decode_info_index,
                setup_slot_index: record.setup_slot_index,
                reference_slot_indices: record.reference_slot_indices,
                reference_base_array_layers: record.reference_base_array_layers,
                dst_base_array_layer: record.dst_base_array_layer,
                src_buffer_offset: record.src_buffer_offset,
                src_buffer_range: record.src_buffer_range,
                tile_count: record.tile_count,
                upload_range,
            })
            .collect())
    }

    pub(crate) fn with_frame_decode_info<R>(
        &self,
        command: &Av1DecodeCommandSkeleton,
        frame_index: usize,
        src_buffer: vk::Buffer,
        picture_views: Av1DecodePictureViews,
        f: impl FnOnce(&vk::VideoDecodeInfoKHR<'_>, &Av1DecodeFrameSubmitBundle) -> R,
    ) -> Result<R, String> {
        let bundle = self
            .frame_submit_bundles(command)?
            .into_iter()
            .find(|bundle| bundle.frame_index == frame_index)
            .ok_or_else(|| format!("AV1 frame {frame_index} is missing from submit bundles"))?;
        let decode = self.decodes.get(bundle.decode_info_index).ok_or_else(|| {
            format!(
                "AV1 frame {} references missing decode info {}",
                bundle.frame_index, bundle.decode_info_index
            )
        })?;

        let dst_picture_resource =
            decode.dst_picture_resource(picture_views.dst, bundle.dst_base_array_layer);
        let setup_picture_resource =
            decode.dst_picture_resource(picture_views.reference, bundle.dst_base_array_layer);
        let mut std_picture_scope = Av1DecodeStdPictureInfoScope::new(
            decode.picture_info.std_picture_info,
            decode.coded_width,
            decode.coded_height,
            decode.picture_info.use_128x128_superblock,
            decode.picture_info.key_frame_header.as_ref(),
            decode.picture_info.std_overrides.as_ref(),
        );
        std_picture_scope.attach_pointers();
        let mut av1_picture_info = vk::VideoDecodeAV1PictureInfoKHR::default()
            .std_picture_info(&std_picture_scope.std_picture_info)
            .reference_name_slot_indices(decode.picture_info.reference_name_slot_indices)
            .frame_header_offset(decode.picture_info.frame_header_offset)
            .tile_offsets(&decode.picture_info.tile_offsets)
            .tile_sizes(&decode.picture_info.tile_sizes);
        let mut setup_dpb_info = decode.vk_setup_dpb_slot_info();
        let setup_reference_slot = decode.vk_setup_reference_slot(
            bundle.setup_slot_index,
            &setup_picture_resource,
            &mut setup_dpb_info,
        );
        let reference_picture_resources = decode.reference_picture_resources(
            picture_views.reference,
            &bundle.reference_slot_indices,
            &bundle.reference_base_array_layers,
        )?;
        let reference_std_infos = decode.reference_std_infos(&bundle.reference_slot_indices);
        let mut reference_dpb_infos = decode.reference_dpb_slot_infos(&reference_std_infos);
        let reference_slots = decode.reference_slots(
            &bundle.reference_slot_indices,
            &reference_picture_resources,
            &mut reference_dpb_infos,
        )?;
        let mut vk_decode_info = decode.vk_decode_info_with_setup_reference_slot(
            src_buffer,
            dst_picture_resource,
            &setup_reference_slot,
            &mut av1_picture_info,
        );
        if !reference_slots.is_empty() {
            vk_decode_info = vk_decode_info.reference_slots(&reference_slots);
        }

        Ok(f(&vk_decode_info, &bundle))
    }

    pub(crate) fn with_frame_decode_infos<R>(
        &self,
        command: &Av1DecodeCommandSkeleton,
        src_buffer: vk::Buffer,
        picture_views: Av1DecodePictureViews,
        mut f: impl FnMut(&vk::VideoDecodeInfoKHR<'_>, &Av1DecodeFrameSubmitBundle) -> R,
    ) -> Result<Vec<R>, String> {
        let mut results = Vec::with_capacity(command.frames.len());
        for frame in &command.frames {
            results.push(self.with_frame_decode_info(
                command,
                frame.frame_index,
                src_buffer,
                picture_views,
                |decode_info, bundle| f(decode_info, bundle),
            )?);
        }
        Ok(results)
    }

    pub(crate) fn visit_decode_command_sequence(
        &self,
        command: &Av1DecodeCommandSkeleton,
        video_session: vk::VideoSessionKHR,
        video_session_parameters: vk::VideoSessionParametersKHR,
        src_buffer: vk::Buffer,
        picture_views: Av1DecodePictureViews,
        mut visit: impl FnMut(Av1DecodeCommandVisit<'_>),
    ) -> Result<(), String> {
        let begin_resources = command.begin_picture_resources(picture_views.reference);
        let begin_reference_infos = command.begin_std_reference_infos();
        let mut begin_dpb_infos = command.begin_dpb_slot_infos(&begin_reference_infos)?;
        let begin_reference_slots =
            command.begin_reference_slots(&begin_resources, &mut begin_dpb_infos)?;
        let begin_coding_info = command.vk_begin_coding_info(
            video_session,
            video_session_parameters,
            &begin_reference_slots,
        )?;
        visit(Av1DecodeCommandVisit::BeginCoding(&begin_coding_info));

        let reset_control = command.vk_reset_coding_control_info();
        visit(Av1DecodeCommandVisit::ResetCoding(&reset_control));

        for frame in &command.frames {
            self.with_frame_decode_info(
                command,
                frame.frame_index,
                src_buffer,
                picture_views,
                |decode_info, bundle| {
                    visit(Av1DecodeCommandVisit::DecodeFrame {
                        decode_info,
                        bundle,
                    });
                },
            )?;
        }

        let end_coding_info = command.vk_end_coding_info();
        visit(Av1DecodeCommandVisit::EndCoding(&end_coding_info));

        Ok(())
    }

    pub(crate) fn record_decode_command_sequence(
        &self,
        command: &Av1DecodeCommandSkeleton,
        video_session: vk::VideoSessionKHR,
        video_session_parameters: vk::VideoSessionParametersKHR,
        src_buffer: vk::Buffer,
        picture_views: Av1DecodePictureViews,
        mut recorder: impl FnMut(Av1DecodeCommandVisit<'_>) -> Result<(), String>,
    ) -> Result<Av1DecodeCommandRecordSummary, String> {
        let mut summary = Av1DecodeCommandRecordSummary::default();
        self.visit_decode_command_sequence(
            command,
            video_session,
            video_session_parameters,
            src_buffer,
            picture_views,
            |visit| {
                match &visit {
                    Av1DecodeCommandVisit::BeginCoding(_) => summary.begin_count += 1,
                    Av1DecodeCommandVisit::ResetCoding(_) => summary.reset_count += 1,
                    Av1DecodeCommandVisit::DecodeFrame { .. } => summary.decode_count += 1,
                    Av1DecodeCommandVisit::EndCoding(_) => summary.end_count += 1,
                }
                if summary.first_error.is_none()
                    && let Err(err) = recorder(visit)
                {
                    summary.first_error = Some(err);
                }
            },
        )?;

        if let Some(err) = summary.first_error.take() {
            return Err(err);
        }
        Ok(summary)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av1DecodeFrameRecordBundle {
    pub frame_index: usize,
    pub temporal_unit_index: usize,
    pub decode_info_index: usize,
    pub setup_slot_index: i32,
    pub reference_slot_indices: Vec<i32>,
    pub reference_base_array_layers: Vec<u32>,
    pub dst_base_array_layer: u32,
    pub src_buffer_offset: u64,
    pub src_buffer_range: u64,
    pub tile_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av1DecodeFrameSubmitBundle {
    pub frame_index: usize,
    pub temporal_unit_index: usize,
    pub decode_info_index: usize,
    pub setup_slot_index: i32,
    pub reference_slot_indices: Vec<i32>,
    pub reference_base_array_layers: Vec<u32>,
    pub dst_base_array_layer: u32,
    pub src_buffer_offset: u64,
    pub src_buffer_range: u64,
    pub tile_count: usize,
    pub upload_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Av1DecodeRecordStep {
    BeginCoding {
        reference_slot_count: usize,
    },
    ResetCoding,
    DecodeFrame {
        frame_index: usize,
        temporal_unit_index: usize,
        setup_slot_index: i32,
        reference_slot_count: usize,
        src_buffer_offset: u64,
        src_buffer_range: u64,
        tile_count: usize,
    },
    EndCoding,
}

pub(crate) enum Av1DecodeCommandVisit<'a> {
    BeginCoding(&'a vk::VideoBeginCodingInfoKHR<'a>),
    ResetCoding(&'a vk::VideoCodingControlInfoKHR<'a>),
    DecodeFrame {
        decode_info: &'a vk::VideoDecodeInfoKHR<'a>,
        bundle: &'a Av1DecodeFrameSubmitBundle,
    },
    EndCoding(&'a vk::VideoEndCodingInfoKHR<'a>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Av1DecodeCommandRecordSummary {
    pub begin_count: usize,
    pub reset_count: usize,
    pub decode_count: usize,
    pub end_count: usize,
    pub decode_query_status_raw: Option<i32>,
    pub(super) first_error: Option<String>,
}

impl Av1DecodeCommandRecordSummary {
    pub(crate) fn validate_for_command(
        &self,
        command: &Av1DecodeCommandSkeleton,
    ) -> Result<(), String> {
        if self.begin_count != 1 {
            return Err(format!(
                "AV1 decode record expected one begin command, got {}",
                self.begin_count
            ));
        }
        if self.reset_count != 1 {
            return Err(format!(
                "AV1 decode record expected one RESET command, got {}",
                self.reset_count
            ));
        }
        if self.decode_count != command.frames.len() {
            return Err(format!(
                "AV1 decode record expected {} decode commands, got {}",
                command.frames.len(),
                self.decode_count
            ));
        }
        if self.end_count != 1 {
            return Err(format!(
                "AV1 decode record expected one end command, got {}",
                self.end_count
            ));
        }
        Ok(())
    }
}

impl Av1DecodeCommandSkeleton {
    pub(crate) fn coded_extent(&self) -> vk::Extent2D {
        vk::Extent2D {
            width: self.coded_width,
            height: self.coded_height,
        }
    }

    pub(crate) fn image_array_layers(&self) -> Result<u32, String> {
        if self.begin_slots.is_empty() {
            return Err("AV1 decode image requires at least one begin slot".to_string());
        }
        let begin_layers = self.begin_slots.len();
        let frame_layers = self
            .frames
            .iter()
            .map(|frame| usize::try_from(frame.dst_base_array_layer).unwrap_or(usize::MAX - 1) + 1)
            .max()
            .unwrap_or(0);
        u32::try_from(begin_layers.max(frame_layers))
            .map_err(|_| "AV1 decode image array layer count exceeds u32 range".to_string())
    }

    pub(crate) fn decode_image_plan(
        &self,
        format: vk::Format,
    ) -> Result<Av1DecodeImagePlan, String> {
        let array_layers = self.image_array_layers()?;
        Ok(Av1DecodeImagePlan {
            format,
            extent: self.coded_extent(),
            array_layers,
            usage: vk::ImageUsageFlags::VIDEO_DECODE_DST_KHR
                | vk::ImageUsageFlags::VIDEO_DECODE_DPB_KHR
                | vk::ImageUsageFlags::TRANSFER_SRC,
        })
    }

    pub(crate) fn vk_decode_image_create_info(
        &self,
        plan: &Av1DecodeImagePlan,
    ) -> vk::ImageCreateInfo<'static> {
        vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(plan.format)
            .extent(vk::Extent3D {
                width: plan.extent.width,
                height: plan.extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(plan.array_layers)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(plan.usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
    }

    pub(crate) fn vk_decode_source_memory_barrier() -> vk::MemoryBarrier2<'static> {
        vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::HOST)
            .src_access_mask(vk::AccessFlags2::HOST_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
            .dst_access_mask(vk::AccessFlags2::VIDEO_DECODE_READ_KHR)
    }

    pub(crate) fn vk_decode_image_init_barrier(
        &self,
        image: vk::Image,
    ) -> Result<vk::ImageMemoryBarrier2<'static>, String> {
        Ok(vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .src_access_mask(vk::AccessFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
            .dst_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::VIDEO_DECODE_DPB_KHR)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: self.image_array_layers()?,
            }))
    }

    pub(crate) fn decode_readback_plan(
        &self,
        format: vk::Format,
    ) -> Result<Av1DecodeReadbackPlan, String> {
        build_av1_decode_readback_plan(
            format,
            self.coded_width,
            self.coded_height,
            self.image_array_layers()?,
        )
    }

    pub(crate) fn begin_picture_resources<'a>(
        &self,
        image_view: vk::ImageView,
    ) -> Vec<vk::VideoPictureResourceInfoKHR<'a>> {
        self.begin_slots
            .iter()
            .map(|slot| {
                vk::VideoPictureResourceInfoKHR::default()
                    .coded_offset(vk::Offset2D { x: 0, y: 0 })
                    .coded_extent(self.coded_extent())
                    .base_array_layer(slot.base_array_layer)
                    .image_view_binding(image_view)
            })
            .collect()
    }

    pub(crate) fn frame_picture_resources<'a>(
        &self,
        image_view: vk::ImageView,
    ) -> Result<Vec<vk::VideoPictureResourceInfoKHR<'a>>, String> {
        self.frame_record_bundles()?
            .iter()
            .map(|bundle| {
                Ok(vk::VideoPictureResourceInfoKHR::default()
                    .coded_offset(vk::Offset2D { x: 0, y: 0 })
                    .coded_extent(self.coded_extent())
                    .base_array_layer(bundle.dst_base_array_layer)
                    .image_view_binding(image_view))
            })
            .collect()
    }

    pub(crate) fn frame_record_bundles(&self) -> Result<Vec<Av1DecodeFrameRecordBundle>, String> {
        self.frames
            .iter()
            .enumerate()
            .map(|(decode_info_index, frame)| {
                if frame.frame_index != decode_info_index {
                    return Err(format!(
                        "AV1 frame/decode-info index mismatch: frame_index={}, decode_info_index={decode_info_index}",
                        frame.frame_index
                    ));
                }
                self
                    .begin_slots
                    .iter()
                    .find(|slot| slot.slot_index == frame.setup_slot_index)
                    .ok_or_else(|| {
                        format!(
                            "AV1 frame {} references unavailable setup slot {}",
                            frame.frame_index, frame.setup_slot_index
                        )
                    })?;
                Ok(Av1DecodeFrameRecordBundle {
                    frame_index: frame.frame_index,
                    temporal_unit_index: frame.temporal_unit_index,
                    decode_info_index,
                    setup_slot_index: frame.setup_slot_index,
                    reference_slot_indices: frame.reference_slot_indices.clone(),
                    reference_base_array_layers: frame.reference_base_array_layers.clone(),
                    dst_base_array_layer: frame.dst_base_array_layer,
                    src_buffer_offset: frame.src_buffer_offset,
                    src_buffer_range: frame.src_buffer_range,
                    tile_count: frame.tile_count,
                })
            })
            .collect()
    }

    pub(crate) fn begin_std_reference_infos(&self) -> Vec<StdVideoDecodeAV1ReferenceInfo> {
        vec![key_frame_std_reference_info_for_order_hint(0); self.begin_slots.len()]
    }

    pub(crate) fn begin_dpb_slot_infos<'a>(
        &self,
        reference_infos: &'a [StdVideoDecodeAV1ReferenceInfo],
    ) -> Result<Vec<vk::VideoDecodeAV1DpbSlotInfoKHR<'a>>, String> {
        if reference_infos.len() != self.begin_slots.len() {
            return Err(format!(
                "AV1 begin reference info count mismatch: refs={}, slots={}",
                reference_infos.len(),
                self.begin_slots.len()
            ));
        }

        Ok(reference_infos
            .iter()
            .map(|reference_info| {
                vk::VideoDecodeAV1DpbSlotInfoKHR::default().std_reference_info(reference_info)
            })
            .collect())
    }

    pub(crate) fn begin_reference_slots<'a>(
        &self,
        picture_resources: &'a [vk::VideoPictureResourceInfoKHR<'a>],
        dpb_slot_infos: &'a mut [vk::VideoDecodeAV1DpbSlotInfoKHR<'a>],
    ) -> Result<Vec<vk::VideoReferenceSlotInfoKHR<'a>>, String> {
        if picture_resources.len() != self.begin_slots.len() {
            return Err(format!(
                "AV1 begin picture resource count mismatch: resources={}, slots={}",
                picture_resources.len(),
                self.begin_slots.len()
            ));
        }
        if dpb_slot_infos.len() != self.begin_slots.len() {
            return Err(format!(
                "AV1 begin DPB slot info count mismatch: dpb_infos={}, slots={}",
                dpb_slot_infos.len(),
                self.begin_slots.len()
            ));
        }

        Ok(Vec::new())
    }

    pub(crate) fn vk_begin_coding_info<'a>(
        &self,
        video_session: vk::VideoSessionKHR,
        video_session_parameters: vk::VideoSessionParametersKHR,
        reference_slots: &'a [vk::VideoReferenceSlotInfoKHR<'a>],
    ) -> Result<vk::VideoBeginCodingInfoKHR<'a>, String> {
        Ok(vk::VideoBeginCodingInfoKHR::default()
            .video_session(video_session)
            .video_session_parameters(video_session_parameters)
            .reference_slots(reference_slots))
    }

    pub(crate) fn vk_reset_coding_control_info(&self) -> vk::VideoCodingControlInfoKHR<'static> {
        vk::VideoCodingControlInfoKHR::default().flags(vk::VideoCodingControlFlagsKHR::RESET)
    }

    pub(crate) fn vk_end_coding_info(&self) -> vk::VideoEndCodingInfoKHR<'static> {
        vk::VideoEndCodingInfoKHR::default()
    }

    pub(crate) fn record_steps(&self) -> Vec<Av1DecodeRecordStep> {
        let mut steps = Vec::with_capacity(self.frames.len() + 3);
        steps.push(Av1DecodeRecordStep::BeginCoding {
            reference_slot_count: 0,
        });
        steps.push(Av1DecodeRecordStep::ResetCoding);
        steps.extend(
            self.frames
                .iter()
                .map(|frame| Av1DecodeRecordStep::DecodeFrame {
                    frame_index: frame.frame_index,
                    temporal_unit_index: frame.temporal_unit_index,
                    setup_slot_index: frame.setup_slot_index,
                    reference_slot_count: frame.reference_slot_indices.len(),
                    src_buffer_offset: frame.src_buffer_offset,
                    src_buffer_range: frame.src_buffer_range,
                    tile_count: frame.tile_count,
                }),
        );
        steps.push(Av1DecodeRecordStep::EndCoding);
        steps
    }
}

impl Av1DecodeInfoSkeleton {
    pub(crate) fn tile_count(&self) -> usize {
        self.picture_info.tile_offsets.len()
    }

    pub(crate) fn coded_extent(&self) -> vk::Extent2D {
        vk::Extent2D {
            width: self.coded_width,
            height: self.coded_height,
        }
    }

    pub(crate) fn dst_picture_resource<'a>(
        &self,
        image_view: vk::ImageView,
        base_array_layer: u32,
    ) -> vk::VideoPictureResourceInfoKHR<'a> {
        vk::VideoPictureResourceInfoKHR::default()
            .coded_offset(vk::Offset2D { x: 0, y: 0 })
            .coded_extent(self.coded_extent())
            .base_array_layer(base_array_layer)
            .image_view_binding(image_view)
    }

    pub(crate) fn reference_slot_indices(&self) -> Vec<i32> {
        let mut indices = Vec::new();
        for slot_index in self.picture_info.reference_name_slot_indices {
            if slot_index >= 0 && !indices.contains(&slot_index) {
                indices.push(slot_index);
            }
        }
        indices
    }

    pub(crate) fn reference_picture_resources<'a>(
        &self,
        image_view: vk::ImageView,
        reference_slot_indices: &[i32],
        reference_base_array_layers: &[u32],
    ) -> Result<Vec<vk::VideoPictureResourceInfoKHR<'a>>, String> {
        if reference_slot_indices.len() != reference_base_array_layers.len() {
            return Err(format!(
                "AV1 reference slot/base-layer count mismatch: slots={}, base_layers={}",
                reference_slot_indices.len(),
                reference_base_array_layers.len()
            ));
        }
        reference_slot_indices
            .iter()
            .zip(reference_base_array_layers)
            .map(|(slot_index, base_array_layer)| {
                if *slot_index < 0 {
                    return Err(format!(
                        "AV1 reference slot index is negative: {slot_index}"
                    ));
                }
                Ok(self.dst_picture_resource(image_view, *base_array_layer))
            })
            .collect()
    }

    pub(crate) fn reference_std_infos(
        &self,
        reference_slot_indices: &[i32],
    ) -> Vec<StdVideoDecodeAV1ReferenceInfo> {
        if self.reference_slot_infos.len() == reference_slot_indices.len() {
            return self.reference_slot_infos.clone();
        }
        reference_slot_indices
            .iter()
            .map(|_| key_frame_std_reference_info_for_order_hint(0))
            .collect()
    }

    pub(crate) fn reference_dpb_slot_infos<'a>(
        &self,
        reference_infos: &'a [StdVideoDecodeAV1ReferenceInfo],
    ) -> Vec<vk::VideoDecodeAV1DpbSlotInfoKHR<'a>> {
        reference_infos
            .iter()
            .map(|reference_info| {
                vk::VideoDecodeAV1DpbSlotInfoKHR::default().std_reference_info(reference_info)
            })
            .collect()
    }

    pub(crate) fn reference_slots<'a>(
        &self,
        reference_slot_indices: &[i32],
        picture_resources: &'a [vk::VideoPictureResourceInfoKHR<'a>],
        dpb_slot_infos: &'a mut [vk::VideoDecodeAV1DpbSlotInfoKHR<'a>],
    ) -> Result<Vec<vk::VideoReferenceSlotInfoKHR<'a>>, String> {
        if picture_resources.len() != reference_slot_indices.len() {
            return Err(format!(
                "AV1 reference picture resource count mismatch: resources={}, slots={}",
                picture_resources.len(),
                reference_slot_indices.len()
            ));
        }
        if dpb_slot_infos.len() != reference_slot_indices.len() {
            return Err(format!(
                "AV1 reference DPB info count mismatch: dpb_infos={}, slots={}",
                dpb_slot_infos.len(),
                reference_slot_indices.len()
            ));
        }

        Ok(reference_slot_indices
            .iter()
            .zip(picture_resources.iter())
            .zip(dpb_slot_infos.iter_mut())
            .map(|((slot_index, picture_resource), dpb_slot_info)| {
                vk::VideoReferenceSlotInfoKHR::default()
                    .slot_index(*slot_index)
                    .picture_resource(picture_resource)
                    .push_next(dpb_slot_info)
            })
            .collect())
    }

    pub(crate) fn vk_decode_info<'a>(
        &'a self,
        src_buffer: vk::Buffer,
        dst_picture_resource: vk::VideoPictureResourceInfoKHR<'a>,
        av1_picture_info: &'a mut vk::VideoDecodeAV1PictureInfoKHR<'a>,
    ) -> vk::VideoDecodeInfoKHR<'a> {
        vk::VideoDecodeInfoKHR::default()
            .src_buffer(src_buffer)
            .src_buffer_offset(self.src_buffer_offset)
            .src_buffer_range(self.src_buffer_range)
            .dst_picture_resource(dst_picture_resource)
            .push_next(av1_picture_info)
    }

    pub(crate) fn vk_decode_info_with_setup_reference_slot<'a>(
        &'a self,
        src_buffer: vk::Buffer,
        dst_picture_resource: vk::VideoPictureResourceInfoKHR<'a>,
        setup_reference_slot: &'a vk::VideoReferenceSlotInfoKHR<'a>,
        av1_picture_info: &'a mut vk::VideoDecodeAV1PictureInfoKHR<'a>,
    ) -> vk::VideoDecodeInfoKHR<'a> {
        self.vk_decode_info(src_buffer, dst_picture_resource, av1_picture_info)
            .setup_reference_slot(setup_reference_slot)
    }

    pub(crate) fn vk_setup_dpb_slot_info(&self) -> vk::VideoDecodeAV1DpbSlotInfoKHR<'_> {
        vk::VideoDecodeAV1DpbSlotInfoKHR::default().std_reference_info(&self.setup_reference_info)
    }

    pub(crate) fn vk_setup_reference_slot<'a>(
        &self,
        slot_index: i32,
        picture_resource: &'a vk::VideoPictureResourceInfoKHR<'a>,
        av1_dpb_slot_info: &'a mut vk::VideoDecodeAV1DpbSlotInfoKHR<'a>,
    ) -> vk::VideoReferenceSlotInfoKHR<'a> {
        vk::VideoReferenceSlotInfoKHR::default()
            .slot_index(slot_index)
            .picture_resource(picture_resource)
            .push_next(av1_dpb_slot_info)
    }
}
