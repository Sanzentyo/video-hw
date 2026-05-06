use std::ffi::CStr;
use std::ops::Range;
use std::sync::OnceLock;

use ash::vk;
use ash::vk::native::{
    StdVideoAV1CDEF, StdVideoAV1ChromaSamplePosition_STD_VIDEO_AV1_CHROMA_SAMPLE_POSITION_UNKNOWN,
    StdVideoAV1ColorConfig, StdVideoAV1ColorConfigFlags,
    StdVideoAV1ColorPrimaries_STD_VIDEO_AV1_COLOR_PRIMARIES_BT_UNSPECIFIED,
    StdVideoAV1FrameRestorationType_STD_VIDEO_AV1_FRAME_RESTORATION_TYPE_NONE,
    StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY, StdVideoAV1GlobalMotion,
    StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_SWITCHABLE,
    StdVideoAV1LoopFilter, StdVideoAV1LoopFilterFlags, StdVideoAV1LoopRestoration,
    StdVideoAV1MatrixCoefficients_STD_VIDEO_AV1_MATRIX_COEFFICIENTS_UNSPECIFIED,
    StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_HIGH, StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN,
    StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_PROFESSIONAL, StdVideoAV1Quantization,
    StdVideoAV1QuantizationFlags, StdVideoAV1Segmentation, StdVideoAV1SequenceHeader,
    StdVideoAV1SequenceHeaderFlags, StdVideoAV1TileInfo, StdVideoAV1TileInfoFlags,
    StdVideoAV1TimingInfo, StdVideoAV1TimingInfoFlags,
    StdVideoAV1TransferCharacteristics_STD_VIDEO_AV1_TRANSFER_CHARACTERISTICS_UNSPECIFIED,
    StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_LARGEST,
    StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_SELECT, StdVideoDecodeAV1PictureInfo,
    StdVideoDecodeAV1PictureInfoFlags, StdVideoDecodeAV1ReferenceInfo,
    StdVideoDecodeAV1ReferenceInfoFlags,
};

const AV1_SELECT_SCREEN_CONTENT_TOOLS: u8 = 2;
const AV1_SELECT_INTEGER_MV: u8 = 2;

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
    pub readback_mapped_bytes: usize,
    pub readback_non_zero: bool,
    pub readback_sample: Vec<u8>,
    pub command_record_decode_count: usize,
    pub command_buffer_recorded: bool,
    pub command_buffer_submitted: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Av1DecodeReadbackFrame {
    pub coded_width: u32,
    pub coded_height: u32,
    pub data: Vec<u8>,
    pub readback_non_zero: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av1DecodeSubmitSkeleton {
    pub temporal_unit_index: usize,
    pub frame_header_offset: u32,
    pub tile_offsets: Vec<u32>,
    pub tile_sizes: Vec<u32>,
    pub key_frame_header: Option<Av1ParsedKeyFrameHeader>,
}

#[derive(Debug, Clone)]
pub(crate) struct Av1DecodePictureInfoSkeleton {
    pub std_picture_info: StdVideoDecodeAV1PictureInfo,
    pub reference_name_slot_indices: [i32; vk::MAX_VIDEO_AV1_REFERENCES_PER_FRAME_KHR],
    pub frame_header_offset: u32,
    pub tile_offsets: Vec<u32>,
    pub tile_sizes: Vec<u32>,
    pub key_frame_header: Option<Av1ParsedKeyFrameHeader>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

struct Av1DecodeStdPictureInfoScope {
    std_picture_info: StdVideoDecodeAV1PictureInfo,
    tile_info: StdVideoAV1TileInfo,
    quantization: StdVideoAV1Quantization,
    segmentation: StdVideoAV1Segmentation,
    loop_filter: StdVideoAV1LoopFilter,
    cdef: StdVideoAV1CDEF,
    loop_restoration: StdVideoAV1LoopRestoration,
    global_motion: StdVideoAV1GlobalMotion,
    mi_col_starts: [u16; 64],
    mi_row_starts: [u16; 64],
    width_in_sbs_minus1: [u16; 64],
    height_in_sbs_minus1: [u16; 64],
}

impl Av1DecodeStdPictureInfoScope {
    fn new(
        base: StdVideoDecodeAV1PictureInfo,
        coded_width: u32,
        coded_height: u32,
        key_frame_header: Option<&Av1ParsedKeyFrameHeader>,
    ) -> Self {
        let sb_cols = u16::try_from(coded_width.div_ceil(64).max(1)).unwrap_or(u16::MAX);
        let sb_rows = u16::try_from(coded_height.div_ceil(64).max(1)).unwrap_or(u16::MAX);
        let mi_cols = u16::try_from(coded_width.div_ceil(4).max(1)).unwrap_or(u16::MAX);
        let mi_rows = u16::try_from(coded_height.div_ceil(4).max(1)).unwrap_or(u16::MAX);
        let mut mi_col_starts = [0_u16; 64];
        let mut mi_row_starts = [0_u16; 64];
        mi_col_starts[1] = mi_cols;
        mi_row_starts[1] = mi_rows;
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
                gm_params: [[0; 6]; 8],
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
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                );
            scope.std_picture_info.OrderHint = header.order_hint;
            scope.std_picture_info.delta_q_res = header.delta_q_res;
            scope.quantization.base_q_idx = header.base_q_idx;
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av1DecodeFrameCommandSkeleton {
    pub frame_index: usize,
    pub temporal_unit_index: usize,
    pub setup_slot_index: i32,
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
        image_view: vk::ImageView,
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
            decode.dst_picture_resource(image_view, bundle.dst_base_array_layer);
        let mut std_picture_scope = Av1DecodeStdPictureInfoScope::new(
            decode.picture_info.std_picture_info,
            decode.coded_width,
            decode.coded_height,
            decode.picture_info.key_frame_header.as_ref(),
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
            &dst_picture_resource,
            &mut setup_dpb_info,
        );
        let vk_decode_info = decode.vk_decode_info_with_setup_reference_slot(
            src_buffer,
            dst_picture_resource,
            &setup_reference_slot,
            &mut av1_picture_info,
        );

        Ok(f(&vk_decode_info, &bundle))
    }

    pub(crate) fn with_frame_decode_infos<R>(
        &self,
        command: &Av1DecodeCommandSkeleton,
        src_buffer: vk::Buffer,
        image_view: vk::ImageView,
        mut f: impl FnMut(&vk::VideoDecodeInfoKHR<'_>, &Av1DecodeFrameSubmitBundle) -> R,
    ) -> Result<Vec<R>, String> {
        let mut results = Vec::with_capacity(command.frames.len());
        for frame in &command.frames {
            results.push(self.with_frame_decode_info(
                command,
                frame.frame_index,
                src_buffer,
                image_view,
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
        image_view: vk::ImageView,
        mut visit: impl FnMut(Av1DecodeCommandVisit<'_>),
    ) -> Result<(), String> {
        let begin_resources = command.begin_picture_resources(image_view);
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
                image_view,
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
        image_view: vk::ImageView,
        mut recorder: impl FnMut(Av1DecodeCommandVisit<'_>) -> Result<(), String>,
    ) -> Result<Av1DecodeCommandRecordSummary, String> {
        let mut summary = Av1DecodeCommandRecordSummary::default();
        self.visit_decode_command_sequence(
            command,
            video_session,
            video_session_parameters,
            src_buffer,
            image_view,
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
    first_error: Option<String>,
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
        u32::try_from(self.begin_slots.len())
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
            .new_layout(vk::ImageLayout::VIDEO_DECODE_DST_KHR)
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
        build_av1_decode_readback_plan(format, self.coded_width, self.coded_height)
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
                let slot = self
                    .begin_slots
                    .iter()
                    .find(|slot| slot.slot_index == frame.setup_slot_index)
                    .ok_or_else(|| {
                        format!(
                            "AV1 frame {} references unavailable setup slot {}",
                            frame.frame_index, frame.setup_slot_index
                        )
                    })?;
                let setup_slot_index = u32::try_from(frame.setup_slot_index)
                    .map_err(|_| "AV1 frame setup slot index is negative".to_string())?;
                if slot.base_array_layer != setup_slot_index {
                    return Err(format!(
                        "AV1 setup slot/base layer mismatch: slot={}, base_layer={}",
                        frame.setup_slot_index, slot.base_array_layer
                    ));
                }
                Ok(Av1DecodeFrameRecordBundle {
                    frame_index: frame.frame_index,
                    temporal_unit_index: frame.temporal_unit_index,
                    decode_info_index,
                    setup_slot_index: frame.setup_slot_index,
                    dst_base_array_layer: slot.base_array_layer,
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

        let mut slots = Vec::with_capacity(self.begin_slots.len());
        for ((_slot, picture_resource), dpb_slot_info) in self
            .begin_slots
            .iter()
            .zip(picture_resources.iter())
            .zip(dpb_slot_infos.iter_mut())
        {
            slots.push(
                vk::VideoReferenceSlotInfoKHR::default()
                    .slot_index(-1)
                    .picture_resource(picture_resource)
                    .push_next(dpb_slot_info),
            );
        }
        Ok(slots)
    }

    pub(crate) fn vk_begin_coding_info<'a>(
        &self,
        video_session: vk::VideoSessionKHR,
        video_session_parameters: vk::VideoSessionParametersKHR,
        reference_slots: &'a [vk::VideoReferenceSlotInfoKHR<'a>],
    ) -> Result<vk::VideoBeginCodingInfoKHR<'a>, String> {
        if reference_slots.len() != self.begin_slots.len() {
            return Err(format!(
                "AV1 begin reference slot count mismatch: reference_slots={}, slots={}",
                reference_slots.len(),
                self.begin_slots.len()
            ));
        }

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
            reference_slot_count: self.begin_slots.len(),
        });
        steps.push(Av1DecodeRecordStep::ResetCoding);
        steps.extend(
            self.frames
                .iter()
                .map(|frame| Av1DecodeRecordStep::DecodeFrame {
                    frame_index: frame.frame_index,
                    temporal_unit_index: frame.temporal_unit_index,
                    setup_slot_index: frame.setup_slot_index,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedAv1SequenceHeader {
    pub seq_profile: u8,
    pub still_picture: bool,
    pub reduced_still_picture_header: bool,
    pub frame_width_bits_minus_1: u8,
    pub frame_height_bits_minus_1: u8,
    pub max_frame_width_minus_1: u32,
    pub max_frame_height_minus_1: u32,
    pub use_128x128_superblock: bool,
    pub enable_filter_intra: bool,
    pub enable_intra_edge_filter: bool,
    pub enable_interintra_compound: bool,
    pub enable_masked_compound: bool,
    pub enable_warped_motion: bool,
    pub enable_dual_filter: bool,
    pub enable_order_hint: bool,
    pub enable_jnt_comp: bool,
    pub enable_ref_frame_mvs: bool,
    pub frame_id_numbers_present_flag: bool,
    pub enable_superres: bool,
    pub enable_cdef: bool,
    pub enable_restoration: bool,
    pub film_grain_params_present: bool,
    pub timing_info_present_flag: bool,
    pub initial_display_delay_present_flag: bool,
    pub order_hint_bits_minus_1: u8,
    pub seq_force_screen_content_tools: u8,
    pub seq_force_integer_mv: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av1ObuType {
    SequenceHeader,
    TemporalDelimiter,
    FrameHeader,
    TileGroup,
    Metadata,
    Frame,
    RedundantFrameHeader,
    TileList,
    Padding,
    Unknown(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av1ObuRecord {
    obu_type: Av1ObuType,
    obu_range: Range<usize>,
    payload_range: Range<usize>,
    temporal_unit_index: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct Av1DecodeExtensionFlags {
    has_video_queue: bool,
    has_video_decode_queue: bool,
    has_video_decode_av1: bool,
}

#[derive(Debug, Clone, Copy)]
struct Av1AdapterDecodeSupport {
    extensions: Av1DecodeExtensionFlags,
    decode_queue_family_index: Option<u32>,
    decode_transfer_queue_family_index: Option<u32>,
}

#[derive(Debug, Clone)]
struct Av1DecodeCapabilitySnapshot {
    min_coded_width: u32,
    min_coded_height: u32,
    max_coded_width: u32,
    max_coded_height: u32,
    min_bitstream_buffer_offset_alignment: u64,
    min_bitstream_buffer_size_alignment: u64,
    max_dpb_slots: u32,
    max_active_reference_pictures: u32,
    max_level: ash::vk::native::StdVideoAV1Level,
    std_header_version: vk::ExtensionProperties,
    decode_output_formats: Vec<vk::Format>,
}

#[derive(Debug, Clone, Copy)]
struct Av1DecodeSessionParameterProbeConfig<'a> {
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
    capability_snapshot: &'a Av1DecodeCapabilitySnapshot,
    picture_format: vk::Format,
    coded_width: u32,
    coded_height: u32,
    std_sequence_header: &'a StdVideoAV1SequenceHeader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av1DecodeSessionBootstrapSummary {
    memory_requirement_count: usize,
    memory_total_size: u64,
    memory_max_alignment: u64,
    memory_bound_count: usize,
}

#[derive(Debug, Clone)]
struct Av1DecodeSessionMemoryPlan {
    requirements: Vec<vk::VideoSessionMemoryRequirementsKHR<'static>>,
    summary: Av1DecodeSessionBootstrapSummary,
}

struct Av1DecodeSourceBufferResource {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

struct Av1DecodeImageResource {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

struct Av1DecodeReadbackBufferResource {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Av1DecodeReadbackSample {
    mapped_bytes: usize,
    non_zero: bool,
    data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Av1DecodeBitstreamSessionProbeOptions {
    record_command_buffer: bool,
    submit_command_buffer: bool,
    readback: bool,
}

struct Av1DecodeSessionResource {
    session: vk::VideoSessionKHR,
    parameters: vk::VideoSessionParametersKHR,
    memories: Vec<vk::DeviceMemory>,
    summary: Av1DecodeSessionBootstrapSummary,
}

struct Av1DecodeCommandRecordConfig<'a> {
    instance: &'a ash::Instance,
    device: &'a ash::Device,
    queue_family_index: u32,
    submit_command_buffer: bool,
    upload_plan: &'a Av1DecodeBitstreamUploadPlan,
    command: &'a Av1DecodeCommandSkeleton,
    video_session: vk::VideoSessionKHR,
    video_session_parameters: vk::VideoSessionParametersKHR,
    source_buffer: vk::Buffer,
    decode_image: vk::Image,
    image_view: vk::ImageView,
    readback: Option<Av1DecodeCommandReadbackConfig<'a>>,
}

struct Av1DecodeCommandReadbackConfig<'a> {
    buffer: vk::Buffer,
    plan: &'a Av1DecodeReadbackPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av1DecodeCommandBufferRecordMode {
    BarrierOnly,
    BeginEnd,
    ResetEnd,
    FirstDecode,
    Full,
}

impl Av1DecodeExtensionFlags {
    fn supports_av1_decode(self) -> bool {
        self.has_video_queue && self.has_video_decode_queue && self.has_video_decode_av1
    }

    fn union_assign(&mut self, other: Self) {
        self.has_video_queue |= other.has_video_queue;
        self.has_video_decode_queue |= other.has_video_decode_queue;
        self.has_video_decode_av1 |= other.has_video_decode_av1;
    }
}

pub(crate) fn probe_av1_decode_prerequisites() -> Av1DecodePrerequisiteProbe {
    static CACHE: OnceLock<Av1DecodePrerequisiteProbe> = OnceLock::new();
    CACHE.get_or_init(run_av1_decode_probe).clone()
}

pub(crate) fn inspect_av1_low_overhead_obus(
    bitstream: &[u8],
) -> Result<Av1BitstreamInspection, String> {
    let records = parse_av1_low_overhead_obus(bitstream)?;
    let has_sequence_header = records
        .iter()
        .any(|record| record.obu_type == Av1ObuType::SequenceHeader);
    let has_frame_payload = records.iter().any(|record| {
        matches!(
            record.obu_type,
            Av1ObuType::Frame | Av1ObuType::FrameHeader | Av1ObuType::TileGroup
        )
    });
    let sequence_header_obu_len = records
        .iter()
        .find(|record| record.obu_type == Av1ObuType::SequenceHeader)
        .map(|record| record.obu_range.len());
    let parsed_sequence_header = records
        .iter()
        .find(|record| record.obu_type == Av1ObuType::SequenceHeader)
        .map(|record| parse_av1_sequence_header_payload(&bitstream[record.payload_range.clone()]))
        .transpose()?;
    let _std_sequence_header = parsed_sequence_header
        .as_ref()
        .map(build_av1_std_sequence_header)
        .transpose()?;
    let temporal_unit_count = records
        .iter()
        .map(|record| record.temporal_unit_index)
        .max()
        .map_or(0, |index| index + 1);

    Ok(Av1BitstreamInspection {
        obu_count: records.len(),
        temporal_unit_count,
        has_sequence_header,
        has_frame_payload,
        sequence_header_obu_len,
        coded_width: parsed_sequence_header
            .as_ref()
            .map(ParsedAv1SequenceHeader::coded_width),
        coded_height: parsed_sequence_header
            .as_ref()
            .map(ParsedAv1SequenceHeader::coded_height),
    })
}

pub(crate) fn extract_av1_std_sequence_header(
    bitstream: &[u8],
) -> Result<StdVideoAV1SequenceHeader, String> {
    let records = parse_av1_low_overhead_obus(bitstream)?;
    let sequence_header = records
        .iter()
        .find(|record| record.obu_type == Av1ObuType::SequenceHeader)
        .ok_or_else(|| "missing AV1 sequence header OBU".to_string())?;
    let parsed =
        parse_av1_sequence_header_payload(&bitstream[sequence_header.payload_range.clone()])?;
    build_av1_std_sequence_header(&parsed)
}

pub(crate) fn probe_av1_decode_session_parameters_for_bitstream(
    bitstream: &[u8],
) -> Result<Av1DecodeBitstreamSessionProbe, String> {
    let options = Av1DecodeBitstreamSessionProbeOptions::from_env();
    probe_av1_decode_session_parameters_for_bitstream_with_options(bitstream, options)
}

pub(crate) fn decode_av1_bitstream_to_nv12(
    bitstream: &[u8],
) -> Result<Av1DecodeReadbackFrame, String> {
    let probe = probe_av1_decode_session_parameters_for_bitstream_with_options(
        bitstream,
        Av1DecodeBitstreamSessionProbeOptions {
            record_command_buffer: true,
            submit_command_buffer: true,
            readback: true,
        },
    )?;
    if probe.picture_format != vk::Format::G8_B8R8_2PLANE_420_UNORM {
        return Err(format!(
            "Vulkan AV1 decode output currently requires G8_B8R8_2PLANE_420_UNORM readback (got {:?})",
            probe.picture_format
        ));
    }
    if probe.readback_sample.len() != probe.readback_mapped_bytes {
        return Err(format!(
            "Vulkan AV1 decode readback did not retain full mapped payload: mapped={}, retained={}",
            probe.readback_mapped_bytes,
            probe.readback_sample.len()
        ));
    }
    Ok(Av1DecodeReadbackFrame {
        coded_width: probe.coded_width,
        coded_height: probe.coded_height,
        data: probe.readback_sample,
        readback_non_zero: probe.readback_non_zero,
    })
}

fn probe_av1_decode_session_parameters_for_bitstream_with_options(
    bitstream: &[u8],
    options: Av1DecodeBitstreamSessionProbeOptions,
) -> Result<Av1DecodeBitstreamSessionProbe, String> {
    let std_sequence_header = extract_av1_std_sequence_header(bitstream)?;
    let coded_width = u32::from(std_sequence_header.max_frame_width_minus_1) + 1;
    let coded_height = u32::from(std_sequence_header.max_frame_height_minus_1) + 1;

    // SAFETY: Loading the Vulkan loader only initializes function pointers. No raw
    // handles escape this function.
    let entry = unsafe { ash::Entry::load() }
        .map_err(|err| format!("failed to load Vulkan entry: {err}"))?;
    // SAFETY: Ash's default instance create info is valid for probing. The instance
    // is destroyed before returning.
    let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default(), None) }
        .map_err(|err| format!("failed to create Vulkan instance: {err}"))?;

    let result = probe_av1_decode_session_parameters_for_bitstream_with_instance(
        &entry,
        &instance,
        bitstream,
        &std_sequence_header,
        coded_width,
        coded_height,
        options,
    );

    // SAFETY: The instance was created in this function and is not used afterwards.
    unsafe {
        instance.destroy_instance(None);
    }
    result
}

impl Av1DecodeBitstreamSessionProbeOptions {
    fn from_env() -> Self {
        let record_command_buffer =
            std::env::var("VIDEO_HW_VULKAN_AV1_RECORD_COMMAND_BUFFER").as_deref() == Ok("1");
        let submit_command_buffer = record_command_buffer
            && std::env::var("VIDEO_HW_VULKAN_AV1_SUBMIT_COMMAND_BUFFER").as_deref() == Ok("1");
        let readback = std::env::var("VIDEO_HW_VULKAN_AV1_READBACK").as_deref() == Ok("1");
        Self {
            record_command_buffer,
            submit_command_buffer,
            readback,
        }
    }
}

pub(crate) fn build_av1_decode_submit_skeleton(
    bitstream: &[u8],
) -> Result<Av1DecodeSubmitSkeleton, String> {
    build_av1_decode_submit_skeletons(bitstream)?
        .into_iter()
        .next()
        .ok_or_else(|| "missing AV1 frame or tile-group OBU for decode submit".to_string())
}

pub(crate) fn build_av1_decode_submit_skeletons(
    bitstream: &[u8],
) -> Result<Vec<Av1DecodeSubmitSkeleton>, String> {
    let records = parse_av1_low_overhead_obus(bitstream)?;
    let mut submits = Vec::new();
    let mut consumed_tile_groups = vec![false; records.len()];

    for (index, record) in records.iter().enumerate() {
        match record.obu_type {
            Av1ObuType::Frame => {
                submits.push(build_av1_frame_obu_submit_skeleton(bitstream, record)?);
            }
            Av1ObuType::FrameHeader => {
                let tile_groups: Vec<(usize, &Av1ObuRecord)> = records
                    .iter()
                    .enumerate()
                    .skip(index + 1)
                    .take_while(|(_, next)| next.temporal_unit_index == record.temporal_unit_index)
                    .take_while(|(_, next)| next.obu_type != Av1ObuType::FrameHeader)
                    .filter(|(_, next)| next.obu_type == Av1ObuType::TileGroup)
                    .collect();
                if !tile_groups.is_empty() {
                    for (tile_index, _) in &tile_groups {
                        consumed_tile_groups[*tile_index] = true;
                    }
                    submits.push(build_av1_tile_group_submit_skeleton(record, &tile_groups)?);
                }
            }
            Av1ObuType::TileGroup if !consumed_tile_groups[index] => {
                return Err(
                    "AV1 tile-group OBU is missing a preceding frame-header OBU".to_string()
                );
            }
            _ => {}
        }
    }

    if submits.is_empty() {
        return Err("missing AV1 frame or tile-group OBU for decode submit".to_string());
    }

    Ok(submits)
}

pub(crate) fn build_av1_decode_info_skeletons(
    bitstream: &[u8],
) -> Result<Vec<Av1DecodeInfoSkeleton>, String> {
    ensure_av1_sequence_header_before_first_frame(bitstream)?;
    let std_sequence_header = extract_av1_std_sequence_header(bitstream)?;
    let coded_width = u32::from(std_sequence_header.max_frame_width_minus_1) + 1;
    let coded_height = u32::from(std_sequence_header.max_frame_height_minus_1) + 1;

    build_av1_decode_submit_skeletons(bitstream)?
        .into_iter()
        .map(|submit| {
            build_av1_decode_info_skeleton_from_submit(
                bitstream.len(),
                coded_width,
                coded_height,
                submit,
            )
        })
        .collect()
}

fn build_av1_decode_info_skeleton_from_submit(
    bitstream_len: usize,
    coded_width: u32,
    coded_height: u32,
    submit: Av1DecodeSubmitSkeleton,
) -> Result<Av1DecodeInfoSkeleton, String> {
    let picture_info = build_av1_decode_picture_info_skeleton(&submit)?;
    let (src_buffer_offset, src_buffer_range) =
        av1_decode_source_range(bitstream_len, &picture_info)?;
    let rebased_picture_info = rebase_av1_picture_info_offsets(picture_info, src_buffer_offset)?;

    Ok(Av1DecodeInfoSkeleton {
        temporal_unit_index: submit.temporal_unit_index,
        src_buffer_offset: u64::from(src_buffer_offset),
        src_buffer_range: u64::from(src_buffer_range),
        coded_width,
        coded_height,
        setup_reference_info: key_frame_std_reference_info(&rebased_picture_info),
        picture_info: rebased_picture_info,
    })
}

pub(crate) fn build_av1_decode_info_skeleton(
    bitstream: &[u8],
) -> Result<Av1DecodeInfoSkeleton, String> {
    build_av1_decode_info_skeletons(bitstream)?
        .into_iter()
        .next()
        .ok_or_else(|| "missing AV1 frame or tile-group OBU for decode info".to_string())
}

pub(crate) fn build_av1_key_frame_decode_command_skeleton(
    bitstream: &[u8],
    max_dpb_slots: u32,
) -> Result<Av1DecodeCommandSkeleton, String> {
    let decodes = build_av1_decode_info_skeletons(bitstream)?;
    build_av1_key_frame_decode_command_skeleton_from_decodes(&decodes, max_dpb_slots)
}

pub(crate) fn build_av1_aligned_decode_bitstream_upload_plan(
    bitstream: &[u8],
    offset_alignment: u64,
    size_alignment: u64,
) -> Result<Av1DecodeBitstreamUploadPlan, String> {
    let mut bytes = Vec::new();
    let mut aligned_decodes = Vec::new();

    for decode in build_av1_decode_info_skeletons(bitstream)? {
        let current_len =
            u64::try_from(bytes.len()).map_err(|_| "AV1 upload buffer length exceeds u64")?;
        let aligned_offset = align_up_av1_decode_value(current_len, offset_alignment);
        let padding = usize::try_from(aligned_offset.saturating_sub(current_len))
            .map_err(|_| "AV1 upload offset padding exceeds usize")?;
        bytes.resize(bytes.len() + padding, 0);

        let source_start = usize::try_from(decode.src_buffer_offset)
            .map_err(|_| "AV1 decode source offset exceeds usize")?;
        let source_len = usize::try_from(decode.src_buffer_range)
            .map_err(|_| "AV1 decode source range exceeds usize")?;
        let source_end = source_start
            .checked_add(source_len)
            .ok_or_else(|| "AV1 decode source range overflows usize".to_string())?;
        let source = bitstream
            .get(source_start..source_end)
            .ok_or_else(|| "AV1 decode source range exceeds bitstream length".to_string())?;
        bytes.extend_from_slice(source);

        let aligned_range = align_up_av1_decode_value(decode.src_buffer_range, size_alignment);
        let required_len = usize::try_from(aligned_offset.saturating_add(aligned_range))
            .map_err(|_| "AV1 aligned upload range exceeds usize")?;
        if bytes.len() < required_len {
            bytes.resize(required_len, 0);
        }

        let mut aligned_decode = decode;
        aligned_decode.src_buffer_offset = aligned_offset;
        aligned_decode.src_buffer_range = aligned_range;
        aligned_decodes.push(aligned_decode);
    }

    if aligned_decodes.is_empty() {
        return Err("missing AV1 frames for aligned decode upload plan".to_string());
    }

    Ok(Av1DecodeBitstreamUploadPlan {
        bytes,
        decodes: aligned_decodes,
    })
}

pub(crate) fn build_av1_aligned_key_frame_decode_command_skeleton(
    bitstream: &[u8],
    max_dpb_slots: u32,
    offset_alignment: u64,
    size_alignment: u64,
) -> Result<(Av1DecodeBitstreamUploadPlan, Av1DecodeCommandSkeleton), String> {
    let upload_plan = build_av1_aligned_decode_bitstream_upload_plan(
        bitstream,
        offset_alignment,
        size_alignment,
    )?;
    let command = build_av1_key_frame_decode_command_skeleton_from_decodes(
        &upload_plan.decodes,
        max_dpb_slots,
    )?;

    Ok((upload_plan, command))
}

fn build_av1_key_frame_decode_command_skeleton_from_decodes(
    decodes: &[Av1DecodeInfoSkeleton],
    max_dpb_slots: u32,
) -> Result<Av1DecodeCommandSkeleton, String> {
    if max_dpb_slots == 0 {
        return Err("AV1 decode command skeleton requires at least one DPB slot".to_string());
    }
    let max_dpb_slots = usize::try_from(max_dpb_slots)
        .map_err(|_| "AV1 max_dpb_slots does not fit usize".to_string())?;
    let first = decodes
        .first()
        .ok_or_else(|| "missing AV1 frames for decode command skeleton".to_string())?;

    let planned_slot_count = decodes.len().min(max_dpb_slots).max(1);
    let begin_slots = (0..planned_slot_count)
        .map(|slot| {
            let slot_index = i32::try_from(slot)
                .map_err(|_| "AV1 begin-coding slot index exceeds i32 range".to_string())?;
            let base_array_layer = u32::try_from(slot)
                .map_err(|_| "AV1 begin-coding slot index exceeds u32 range".to_string())?;
            Ok(Av1BeginCodingSlotSkeleton {
                slot_index,
                base_array_layer,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let frames = decodes
        .iter()
        .enumerate()
        .map(|(frame_index, decode)| {
            let setup_slot_index = i32::try_from(frame_index % max_dpb_slots)
                .map_err(|_| "AV1 setup slot index exceeds i32 range".to_string())?;
            Ok(Av1DecodeFrameCommandSkeleton {
                frame_index,
                temporal_unit_index: decode.temporal_unit_index,
                setup_slot_index,
                src_buffer_offset: decode.src_buffer_offset,
                src_buffer_range: decode.src_buffer_range,
                tile_count: decode.tile_count(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Av1DecodeCommandSkeleton {
        coded_width: first.coded_width,
        coded_height: first.coded_height,
        begin_slots,
        frames,
    })
}

fn align_up_av1_decode_value(value: u64, alignment: u64) -> u64 {
    if alignment <= 1 {
        return value;
    }
    let remainder = value % alignment;
    if remainder == 0 {
        value
    } else {
        value.saturating_add(alignment.saturating_sub(remainder))
    }
}

pub(crate) fn build_av1_decode_picture_info_skeleton(
    submit: &Av1DecodeSubmitSkeleton,
) -> Result<Av1DecodePictureInfoSkeleton, String> {
    if submit.tile_offsets.is_empty() {
        return Err("AV1 decode picture info requires at least one tile offset".to_string());
    }
    if submit.tile_offsets.len() != submit.tile_sizes.len() {
        return Err(format!(
            "AV1 decode picture info tile offset/size count mismatch: offsets={}, sizes={}",
            submit.tile_offsets.len(),
            submit.tile_sizes.len()
        ));
    }
    if submit.tile_sizes.contains(&0) {
        return Err("AV1 decode picture info contains an empty tile payload".to_string());
    }

    Ok(Av1DecodePictureInfoSkeleton {
        std_picture_info: key_frame_std_picture_info(),
        reference_name_slot_indices: [-1; vk::MAX_VIDEO_AV1_REFERENCES_PER_FRAME_KHR],
        frame_header_offset: submit.frame_header_offset,
        tile_offsets: submit.tile_offsets.clone(),
        tile_sizes: submit.tile_sizes.clone(),
        key_frame_header: submit.key_frame_header,
    })
}

fn ensure_av1_sequence_header_before_first_frame(bitstream: &[u8]) -> Result<(), String> {
    let records = parse_av1_low_overhead_obus(bitstream)?;
    let sequence_header_start = records
        .iter()
        .find(|record| record.obu_type == Av1ObuType::SequenceHeader)
        .map(|record| record.obu_range.start)
        .ok_or_else(|| "missing AV1 sequence header OBU before first frame".to_string())?;
    let first_frame_start = records
        .iter()
        .find(|record| {
            matches!(
                record.obu_type,
                Av1ObuType::Frame | Av1ObuType::FrameHeader | Av1ObuType::TileGroup
            )
        })
        .map(|record| record.obu_range.start);

    if first_frame_start.is_some_and(|frame_start| sequence_header_start > frame_start) {
        return Err("AV1 sequence header OBU appears after the first frame payload".to_string());
    }

    Ok(())
}

impl ParsedAv1SequenceHeader {
    fn coded_width(&self) -> u32 {
        self.max_frame_width_minus_1 + 1
    }

    fn coded_height(&self) -> u32 {
        self.max_frame_height_minus_1 + 1
    }
}

fn av1_decode_source_range(
    bitstream_len: usize,
    picture_info: &Av1DecodePictureInfoSkeleton,
) -> Result<(u32, u32), String> {
    let frame_header_offset = picture_info.frame_header_offset;
    let mut range_start = frame_header_offset;
    let mut range_end = frame_header_offset
        .checked_add(1)
        .ok_or_else(|| "AV1 frame header offset overflows u32".to_string())?;

    for (&tile_offset, &tile_size) in picture_info
        .tile_offsets
        .iter()
        .zip(picture_info.tile_sizes.iter())
    {
        if tile_size == 0 {
            return Err("AV1 decode info contains an empty tile payload".to_string());
        }
        let tile_end = tile_offset
            .checked_add(tile_size)
            .ok_or_else(|| "AV1 tile payload range overflows u32".to_string())?;
        range_start = range_start.min(tile_offset);
        range_end = range_end.max(tile_end);
    }

    let bitstream_len = u32::try_from(bitstream_len)
        .map_err(|_| "AV1 bitstream is too large for Vulkan u32 offsets".to_string())?;
    if range_end > bitstream_len {
        return Err(format!(
            "AV1 decode source range exceeds bitstream length: end={range_end}, len={bitstream_len}"
        ));
    }
    let range_len = range_end
        .checked_sub(range_start)
        .ok_or_else(|| "AV1 decode source range is inverted".to_string())?;
    if range_len == 0 {
        return Err("AV1 decode source range is empty".to_string());
    }

    Ok((range_start, range_len))
}

fn rebase_av1_picture_info_offsets(
    mut picture_info: Av1DecodePictureInfoSkeleton,
    src_buffer_offset: u32,
) -> Result<Av1DecodePictureInfoSkeleton, String> {
    picture_info.frame_header_offset = picture_info
        .frame_header_offset
        .checked_sub(src_buffer_offset)
        .ok_or_else(|| "AV1 frame header offset precedes source buffer range".to_string())?;
    for tile_offset in &mut picture_info.tile_offsets {
        *tile_offset = tile_offset
            .checked_sub(src_buffer_offset)
            .ok_or_else(|| "AV1 tile offset precedes source buffer range".to_string())?;
    }
    Ok(picture_info)
}

fn key_frame_std_reference_info(
    picture_info: &Av1DecodePictureInfoSkeleton,
) -> StdVideoDecodeAV1ReferenceInfo {
    key_frame_std_reference_info_for_order_hint(picture_info.std_picture_info.OrderHint)
}

fn key_frame_std_reference_info_for_order_hint(order_hint: u8) -> StdVideoDecodeAV1ReferenceInfo {
    StdVideoDecodeAV1ReferenceInfo {
        flags: StdVideoDecodeAV1ReferenceInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoDecodeAV1ReferenceInfoFlags::new_bitfield_1(0, 0, 0),
        },
        frame_type: StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY as u8,
        RefFrameSignBias: 0,
        OrderHint: order_hint,
        SavedOrderHints: [0; 8],
    }
}

fn key_frame_std_picture_info() -> StdVideoDecodeAV1PictureInfo {
    StdVideoDecodeAV1PictureInfo {
        flags: StdVideoDecodeAV1PictureInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoDecodeAV1PictureInfoFlags::new_bitfield_1(
                1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0,
            ),
        },
        frame_type: StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY,
        current_frame_id: 0,
        OrderHint: 0,
        primary_ref_frame: 7,
        refresh_frame_flags: 0xff,
        reserved1: 0,
        interpolation_filter:
            StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_SWITCHABLE,
        TxMode: StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_SELECT,
        delta_q_res: 0,
        delta_lf_res: 0,
        SkipModeFrame: [0; 2],
        coded_denom: 0,
        reserved2: [0; 3],
        OrderHints: [0; 8],
        expectedFrameId: [0; 8],
        pTileInfo: std::ptr::null(),
        pQuantization: std::ptr::null(),
        pSegmentation: std::ptr::null(),
        pLoopFilter: std::ptr::null(),
        pCDEF: std::ptr::null(),
        pLoopRestoration: std::ptr::null(),
        pGlobalMotion: std::ptr::null(),
        pFilmGrain: std::ptr::null(),
    }
}

fn build_av1_frame_obu_submit_skeleton(
    bitstream: &[u8],
    frame_record: &Av1ObuRecord,
) -> Result<Av1DecodeSubmitSkeleton, String> {
    let frame_header_offset = u32::try_from(frame_record.payload_range.start)
        .map_err(|_| "AV1 frame header offset exceeds u32 range".to_string())?;
    let key_frame_header = parse_av1_key_frame_obu_header(
        bitstream
            .get(frame_record.payload_range.clone())
            .ok_or_else(|| "AV1 frame OBU payload range exceeds bitstream".to_string())?,
    )
    .ok();
    let tile_payload_offset = key_frame_header
        .map(|header| header.tile_payload_offset)
        .unwrap_or(0);
    let tile_offset = frame_header_offset
        .checked_add(
            u32::try_from(tile_payload_offset)
                .map_err(|_| "AV1 frame OBU tile payload offset exceeds u32 range".to_string())?,
        )
        .ok_or_else(|| "AV1 frame OBU tile payload offset overflows u32".to_string())?;
    let tile_size = u32::try_from(
        frame_record
            .payload_range
            .len()
            .checked_sub(tile_payload_offset)
            .ok_or_else(|| "AV1 frame OBU tile payload offset exceeds payload size".to_string())?,
    )
    .map_err(|_| "AV1 frame OBU payload size exceeds u32 range".to_string())?;
    if tile_size == 0 {
        return Err("AV1 frame OBU payload is empty".to_string());
    }
    Ok(Av1DecodeSubmitSkeleton {
        temporal_unit_index: frame_record.temporal_unit_index,
        frame_header_offset: tile_offset,
        tile_offsets: vec![tile_offset],
        tile_sizes: vec![tile_size],
        key_frame_header,
    })
}

fn build_av1_tile_group_submit_skeleton(
    frame_header_record: &Av1ObuRecord,
    tile_group_records: &[(usize, &Av1ObuRecord)],
) -> Result<Av1DecodeSubmitSkeleton, String> {
    let frame_header_offset = u32::try_from(frame_header_record.payload_range.start)
        .map_err(|_| "AV1 frame header offset exceeds u32 range".to_string())?;

    let mut tile_offsets = Vec::with_capacity(tile_group_records.len());
    let mut tile_sizes = Vec::with_capacity(tile_group_records.len());
    for (_, tile_group_record) in tile_group_records {
        let tile_offset = u32::try_from(tile_group_record.payload_range.start)
            .map_err(|_| "AV1 tile offset exceeds u32 range".to_string())?;
        let tile_size = u32::try_from(tile_group_record.payload_range.len())
            .map_err(|_| "AV1 tile group payload size exceeds u32 range".to_string())?;
        if tile_size == 0 {
            return Err("AV1 tile-group OBU payload is empty".to_string());
        }
        tile_offsets.push(tile_offset);
        tile_sizes.push(tile_size);
    }

    Ok(Av1DecodeSubmitSkeleton {
        temporal_unit_index: frame_header_record.temporal_unit_index,
        frame_header_offset,
        tile_offsets,
        tile_sizes,
        key_frame_header: None,
    })
}

fn parse_av1_key_frame_obu_header(payload: &[u8]) -> Result<Av1ParsedKeyFrameHeader, String> {
    let mut bits = BitReader::new(payload);
    let show_existing_frame = bits.read_bool("show_existing_frame")?;
    if show_existing_frame {
        return Err("show_existing_frame has no tile payload".to_string());
    }
    let frame_type = bits.read_bits_u8(2, "frame_type")?;
    if frame_type != 0 {
        return Err(format!(
            "only AV1 key-frame OBU tile offset parsing is implemented, got frame_type={frame_type}"
        ));
    }
    let show_frame = bits.read_bool("show_frame")?;
    if !show_frame {
        let _showable_frame = bits.read_bool("showable_frame")?;
    }
    let disable_cdf_update = bits.read_bool("disable_cdf_update")?;
    let allow_screen_content_tools = bits.read_bool("allow_screen_content_tools")?;
    let force_integer_mv = bits.read_bool("force_integer_mv")?;
    let frame_size_override_flag = bits.read_bool("frame_size_override_flag")?;
    let order_hint = bits.read_bits_u8(7, "order_hint")?;
    let render_and_frame_size_different = bits.read_bool("render_and_frame_size_different")?;
    if render_and_frame_size_different {
        return Err(
            "render_and_frame_size_different AV1 key-frame parsing is not implemented".to_string(),
        );
    }
    let allow_intrabc = bits.read_bool("allow_intrabc")?;
    let disable_frame_end_update_cdf = bits.read_bool("disable_frame_end_update_cdf")?;
    let uniform_tile_spacing_flag = bits.read_bool("uniform_tile_spacing_flag")?;
    if !uniform_tile_spacing_flag {
        return Err("non-uniform AV1 tile spacing parsing is not implemented".to_string());
    }
    while bits.read_bool("tile_cols_log2")? {}
    while bits.read_bool("tile_rows_log2")? {}

    let base_q_idx = bits.read_bits_u8(8, "base_q_idx")?;
    skip_av1_delta_q(&mut bits, "delta_q_y_dc")?;
    skip_av1_delta_q(&mut bits, "delta_q_u_dc")?;
    skip_av1_delta_q(&mut bits, "delta_q_u_ac")?;
    let using_qmatrix = bits.read_bool("using_qmatrix")?;
    if using_qmatrix {
        return Err("AV1 qmatrix parsing is not implemented".to_string());
    }

    let segmentation_enabled = bits.read_bool("segmentation_enabled")?;
    if segmentation_enabled {
        return Err("AV1 segmentation parsing is not implemented".to_string());
    }
    let delta_q_present = bits.read_bool("delta_q_present")?;
    let mut delta_q_res = 0;
    if delta_q_present {
        delta_q_res = bits.read_bits_u8(2, "delta_q_res")?;
    }

    let loop_filter_level = [
        bits.read_bits_u8(6, "loop_filter_level[0]")?,
        bits.read_bits_u8(6, "loop_filter_level[1]")?,
        bits.read_bits_u8(6, "loop_filter_level[2]")?,
        bits.read_bits_u8(6, "loop_filter_level[3]")?,
    ];
    let mut loop_filter_sharpness = 0;
    let mut loop_filter_delta_enabled = false;
    let mut loop_filter_delta_update = false;
    if loop_filter_level.iter().any(|&level| level != 0) {
        loop_filter_sharpness = bits.read_bits_u8(3, "loop_filter_sharpness")?;
        loop_filter_delta_enabled = bits.read_bool("loop_filter_delta_enabled")?;
        if loop_filter_delta_enabled {
            loop_filter_delta_update = bits.read_bool("loop_filter_delta_update")?;
            if loop_filter_delta_update {
                return Err("AV1 loop-filter delta update parsing is not implemented".to_string());
            }
        }
    }

    let cdef_damping_minus_3 = bits.read_bits_u8(2, "cdef_damping_minus_3")?;
    let cdef_bits = bits.read_bits_u8(2, "cdef_bits")?;
    let mut cdef_y_pri_strength = [0_u8; 8];
    let mut cdef_y_sec_strength = [0_u8; 8];
    let mut cdef_uv_pri_strength = [0_u8; 8];
    let mut cdef_uv_sec_strength = [0_u8; 8];
    for index in 0..(1_u8 << cdef_bits) {
        let index = usize::from(index);
        cdef_y_pri_strength[index] = bits.read_bits_u8(4, "cdef_y_pri_strength")?;
        cdef_y_sec_strength[index] = bits.read_bits_u8(2, "cdef_y_sec_strength")?;
        cdef_uv_pri_strength[index] = bits.read_bits_u8(4, "cdef_uv_pri_strength")?;
        cdef_uv_sec_strength[index] = bits.read_bits_u8(2, "cdef_uv_sec_strength")?;
    }

    let tx_mode_select = bits.read_bool("tx_mode_select")?;
    let tx_mode = if tx_mode_select {
        StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_SELECT
    } else {
        StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_LARGEST
    };
    let reduced_tx_set = bits.read_bool("reduced_tx_set")?;
    bits.align_to_next_byte_with_zero_bits("frame_header_obu_byte_alignment")?;
    Ok(Av1ParsedKeyFrameHeader {
        tile_payload_offset: bits.byte_offset(),
        disable_cdf_update,
        allow_screen_content_tools,
        force_integer_mv,
        frame_size_override_flag,
        order_hint,
        allow_intrabc,
        disable_frame_end_update_cdf,
        base_q_idx,
        delta_q_present,
        delta_q_res,
        loop_filter_level,
        loop_filter_sharpness,
        loop_filter_delta_enabled,
        loop_filter_delta_update,
        cdef_damping_minus_3,
        cdef_bits,
        cdef_y_pri_strength,
        cdef_y_sec_strength,
        cdef_uv_pri_strength,
        cdef_uv_sec_strength,
        tx_mode,
        reduced_tx_set,
    })
}

fn skip_av1_delta_q(bits: &mut BitReader<'_>, field_name: &str) -> Result<(), String> {
    let delta_coded = bits.read_bool(field_name)?;
    if delta_coded {
        let _delta_q = bits.read_bits_u8(7, field_name)?;
    }
    Ok(())
}

fn build_av1_std_sequence_header(
    parsed: &ParsedAv1SequenceHeader,
) -> Result<StdVideoAV1SequenceHeader, String> {
    let seq_profile = match parsed.seq_profile {
        0 => StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN,
        1 => StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_HIGH,
        2 => StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_PROFESSIONAL,
        other => return Err(format!("unsupported AV1 seq_profile {other}")),
    };
    let max_frame_width_minus_1 = u16::try_from(parsed.max_frame_width_minus_1)
        .map_err(|_| "max_frame_width_minus_1 exceeds Vulkan std u16 range".to_string())?;
    let max_frame_height_minus_1 = u16::try_from(parsed.max_frame_height_minus_1)
        .map_err(|_| "max_frame_height_minus_1 exceeds Vulkan std u16 range".to_string())?;

    Ok(StdVideoAV1SequenceHeader {
        flags: StdVideoAV1SequenceHeaderFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoAV1SequenceHeaderFlags::new_bitfield_1(
                parsed.still_picture as u32,
                parsed.reduced_still_picture_header as u32,
                parsed.use_128x128_superblock as u32,
                parsed.enable_filter_intra as u32,
                parsed.enable_intra_edge_filter as u32,
                parsed.enable_interintra_compound as u32,
                parsed.enable_masked_compound as u32,
                parsed.enable_warped_motion as u32,
                parsed.enable_dual_filter as u32,
                parsed.enable_order_hint as u32,
                parsed.enable_jnt_comp as u32,
                parsed.enable_ref_frame_mvs as u32,
                parsed.frame_id_numbers_present_flag as u32,
                parsed.enable_superres as u32,
                parsed.enable_cdef as u32,
                parsed.enable_restoration as u32,
                parsed.film_grain_params_present as u32,
                parsed.timing_info_present_flag as u32,
                parsed.initial_display_delay_present_flag as u32,
                0,
            ),
        },
        seq_profile,
        frame_width_bits_minus_1: parsed.frame_width_bits_minus_1,
        frame_height_bits_minus_1: parsed.frame_height_bits_minus_1,
        max_frame_width_minus_1,
        max_frame_height_minus_1,
        delta_frame_id_length_minus_2: 0,
        additional_frame_id_length_minus_1: 0,
        order_hint_bits_minus_1: parsed.order_hint_bits_minus_1,
        seq_force_integer_mv: parsed.seq_force_integer_mv,
        seq_force_screen_content_tools: parsed.seq_force_screen_content_tools,
        reserved1: [0; 5],
        pColorConfig: std::ptr::null(),
        pTimingInfo: std::ptr::null(),
    })
}

fn run_av1_decode_probe() -> Av1DecodePrerequisiteProbe {
    // SAFETY: Loading the Vulkan loader only initializes function pointers. No raw
    // handles escape this function.
    let entry = match unsafe { ash::Entry::load() } {
        Ok(entry) => entry,
        Err(err) => {
            return Av1DecodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to load Vulkan entry: {err}"
            ));
        }
    };

    // SAFETY: Ash's default instance create info is valid for probing. The instance
    // is destroyed before returning.
    let instance = match unsafe { entry.create_instance(&vk::InstanceCreateInfo::default(), None) }
    {
        Ok(instance) => instance,
        Err(err) => {
            return Av1DecodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to create Vulkan instance: {err}"
            ));
        }
    };

    let probe_result = (|| -> Result<Av1DecodePrerequisiteProbe, String> {
        // SAFETY: We only query physical-device handles owned by this instance.
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;
        if physical_devices.is_empty() {
            return Ok(Av1DecodePrerequisiteProbe::NoCompatibleAdapter);
        }

        let mut observed_extensions = Av1DecodeExtensionFlags::default();
        let mut observed_decode_queue = false;
        let mut device_init_errors = Vec::new();

        for physical_device in physical_devices {
            let support = query_av1_adapter_decode_support(&instance, physical_device)
                .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
            observed_extensions.union_assign(support.extensions);
            observed_decode_queue |= support.decode_queue_family_index.is_some();

            if support.extensions.supports_av1_decode()
                && let Some(queue_family_index) = support.decode_queue_family_index
            {
                let snapshot = match query_av1_decode_capability_snapshot(
                    &entry,
                    &instance,
                    physical_device,
                ) {
                    Ok(snapshot) => {
                        if let Err(err) = validate_av1_decode_capability_snapshot(&snapshot) {
                            device_init_errors.push(err);
                            continue;
                        }
                        snapshot
                    }
                    Err(err) => {
                        device_init_errors.push(err);
                        continue;
                    }
                };
                match probe_av1_decode_session_parameters(
                    &instance,
                    physical_device,
                    queue_family_index,
                    &snapshot,
                ) {
                    Ok(()) => return Ok(Av1DecodePrerequisiteProbe::Ready),
                    Err(err) => device_init_errors.push(err),
                }
            }
        }

        let mut missing = Vec::new();
        if !observed_extensions.has_video_queue {
            missing.push("VK_KHR_video_queue");
        }
        if !observed_extensions.has_video_decode_queue {
            missing.push("VK_KHR_video_decode_queue");
        }
        if !observed_extensions.has_video_decode_av1 {
            missing.push("VK_KHR_video_decode_av1");
        }
        if !missing.is_empty() {
            return Ok(Av1DecodePrerequisiteProbe::MissingExtensions { missing });
        }
        if !observed_decode_queue {
            return Ok(Av1DecodePrerequisiteProbe::MissingDecodeQueueFamily);
        }
        if !device_init_errors.is_empty() {
            return Ok(Av1DecodePrerequisiteProbe::SessionBootstrapFailed(
                device_init_errors.join("; "),
            ));
        }

        Ok(Av1DecodePrerequisiteProbe::NoCompatibleAdapter)
    })();

    // SAFETY: The instance was created in this function and is not used afterwards.
    unsafe {
        instance.destroy_instance(None);
    }

    probe_result.unwrap_or_else(Av1DecodePrerequisiteProbe::ProbeUnavailable)
}

fn query_av1_adapter_decode_support(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Result<Av1AdapterDecodeSupport, vk::Result> {
    // SAFETY: `physical_device` came from this instance.
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical_device) }?;
    let mut flags = Av1DecodeExtensionFlags::default();
    for extension in extensions {
        // SAFETY: Vulkan extension names are fixed-size, null-terminated C strings.
        let name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        flags.has_video_queue |= name == vk::KHR_VIDEO_QUEUE_NAME;
        flags.has_video_decode_queue |= name == vk::KHR_VIDEO_DECODE_QUEUE_NAME;
        flags.has_video_decode_av1 |= name == vk::KHR_VIDEO_DECODE_AV1_NAME;
    }

    let decode_queue_family_index = query_video_codec_queue_family_index(
        instance,
        physical_device,
        vk::QueueFlags::VIDEO_DECODE_KHR,
        vk::VideoCodecOperationFlagsKHR::DECODE_AV1,
    );
    let decode_transfer_queue_family_index = query_video_codec_queue_family_index(
        instance,
        physical_device,
        vk::QueueFlags::VIDEO_DECODE_KHR | vk::QueueFlags::TRANSFER,
        vk::VideoCodecOperationFlagsKHR::DECODE_AV1,
    );

    Ok(Av1AdapterDecodeSupport {
        extensions: flags,
        decode_queue_family_index,
        decode_transfer_queue_family_index,
    })
}

fn query_av1_decode_capability_snapshot(
    entry: &ash::Entry,
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Result<Av1DecodeCapabilitySnapshot, String> {
    let video_queue = ash::khr::video_queue::Instance::new(entry, instance);
    let mut decode_av1_profile = vk::VideoDecodeAV1ProfileInfoKHR::default()
        .std_profile(StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN)
        .film_grain_support(false);
    let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
    let profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_AV1)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut decode_av1_profile)
        .push_next(&mut decode_usage);

    let mut decode_capabilities = vk::VideoDecodeCapabilitiesKHR::default();
    let mut decode_av1_capabilities = vk::VideoDecodeAV1CapabilitiesKHR::default();
    let mut capabilities = vk::VideoCapabilitiesKHR::default()
        .push_next(&mut decode_av1_capabilities)
        .push_next(&mut decode_capabilities);

    // SAFETY: All chained structs are stack-allocated and live for the call.
    let result = unsafe {
        (video_queue.fp().get_physical_device_video_capabilities_khr)(
            physical_device,
            &profile,
            &mut capabilities,
        )
    };
    if result != vk::Result::SUCCESS {
        return Err(format!(
            "AV1 decode video capabilities query failed: {result:?}"
        ));
    }

    let decode_output_formats =
        query_av1_decode_output_formats(&video_queue, physical_device, profile)?;
    let min_coded_width = capabilities.min_coded_extent.width;
    let min_coded_height = capabilities.min_coded_extent.height;
    let max_coded_width = capabilities.max_coded_extent.width;
    let max_coded_height = capabilities.max_coded_extent.height;
    let min_bitstream_buffer_offset_alignment = capabilities.min_bitstream_buffer_offset_alignment;
    let min_bitstream_buffer_size_alignment = capabilities.min_bitstream_buffer_size_alignment;
    let max_dpb_slots = capabilities.max_dpb_slots;
    let max_active_reference_pictures = capabilities.max_active_reference_pictures;
    let std_header_version = capabilities.std_header_version;
    let max_level = decode_av1_capabilities.max_level;

    Ok(Av1DecodeCapabilitySnapshot {
        min_coded_width,
        min_coded_height,
        max_coded_width,
        max_coded_height,
        min_bitstream_buffer_offset_alignment,
        min_bitstream_buffer_size_alignment,
        max_dpb_slots,
        max_active_reference_pictures,
        max_level,
        std_header_version,
        decode_output_formats,
    })
}

fn validate_av1_decode_capability_snapshot(
    snapshot: &Av1DecodeCapabilitySnapshot,
) -> Result<(), String> {
    if snapshot.decode_output_formats.is_empty() {
        return Err("AV1 decode capability query returned no output formats".to_string());
    }
    if snapshot.max_coded_width < snapshot.min_coded_width
        || snapshot.max_coded_height < snapshot.min_coded_height
    {
        return Err(format!(
            "AV1 decode capability query returned invalid coded extent range: min={}x{}, max={}x{}",
            snapshot.min_coded_width,
            snapshot.min_coded_height,
            snapshot.max_coded_width,
            snapshot.max_coded_height
        ));
    }
    if snapshot.min_coded_width == 0 || snapshot.min_coded_height == 0 {
        return Err(format!(
            "AV1 decode capability query returned zero minimum coded extent: {}x{}",
            snapshot.min_coded_width, snapshot.min_coded_height
        ));
    }
    if snapshot.max_dpb_slots == 0 {
        return Err("AV1 decode capability query returned max_dpb_slots=0".to_string());
    }
    if snapshot.max_active_reference_pictures > snapshot.max_dpb_slots {
        return Err(format!(
            "AV1 decode capability query returned max_active_reference_pictures={} greater than max_dpb_slots={}",
            snapshot.max_active_reference_pictures, snapshot.max_dpb_slots
        ));
    }
    let _max_level = snapshot.max_level;
    let _std_header_version = snapshot.std_header_version;
    Ok(())
}

fn query_av1_decode_output_formats(
    video_queue: &ash::khr::video_queue::Instance,
    physical_device: vk::PhysicalDevice,
    profile: vk::VideoProfileInfoKHR<'_>,
) -> Result<Vec<vk::Format>, String> {
    let profiles = [profile];
    let mut profile_list = vk::VideoProfileListInfoKHR::default().profiles(&profiles);
    let format_info = vk::PhysicalDeviceVideoFormatInfoKHR::default()
        .image_usage(
            vk::ImageUsageFlags::VIDEO_DECODE_DST_KHR
                | vk::ImageUsageFlags::VIDEO_DECODE_DPB_KHR
                | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .push_next(&mut profile_list);

    let mut property_count = 0_u32;
    // SAFETY: Count query only writes the property count.
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
            "AV1 decode video format count query failed: {count_result:?}"
        ));
    }
    if property_count == 0 {
        return Ok(Vec::new());
    }

    let mut properties = vec![vk::VideoFormatPropertiesKHR::default(); property_count as usize];
    // SAFETY: `properties` has capacity for the count returned above.
    let query_result = unsafe {
        (video_queue
            .fp()
            .get_physical_device_video_format_properties_khr)(
            physical_device,
            &format_info,
            &mut property_count,
            properties.as_mut_ptr(),
        )
    };
    if query_result != vk::Result::SUCCESS {
        return Err(format!(
            "AV1 decode video format query failed: {query_result:?}"
        ));
    }

    properties.truncate(property_count as usize);
    Ok(properties
        .into_iter()
        .map(|property| property.format)
        .collect())
}

fn query_video_codec_queue_family_index(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    required_queue_flag: vk::QueueFlags,
    required_codec_operation: vk::VideoCodecOperationFlagsKHR,
) -> Option<u32> {
    // SAFETY: This reads immutable queue-family metadata for a valid physical device.
    let queue_count =
        unsafe { instance.get_physical_device_queue_family_properties2_len(physical_device) };
    if queue_count == 0 {
        return None;
    }

    let mut queue_properties2 = vec![vk::QueueFamilyProperties2::default(); queue_count];
    let mut video_properties = vec![vk::QueueFamilyVideoPropertiesKHR::default(); queue_count];
    for (queue_property, video_property) in queue_properties2
        .iter_mut()
        .zip(video_properties.iter_mut())
    {
        *queue_property = queue_property.push_next(video_property);
    }

    // SAFETY: The pNext chains above live for the duration of this call.
    unsafe {
        instance
            .get_physical_device_queue_family_properties2(physical_device, &mut queue_properties2)
    };

    let queue_family_properties = queue_properties2
        .iter()
        .map(|property| property.queue_family_properties)
        .collect::<Vec<_>>();
    let codec_operations = video_properties
        .iter()
        .map(|property| property.video_codec_operations)
        .collect::<Vec<_>>();

    find_video_codec_queue_family_index(
        &queue_family_properties,
        &codec_operations,
        required_queue_flag,
        required_codec_operation,
    )
}

fn find_video_codec_queue_family_index(
    queue_family_properties: &[vk::QueueFamilyProperties],
    codec_operations: &[vk::VideoCodecOperationFlagsKHR],
    required_queue_flag: vk::QueueFlags,
    required_codec_operation: vk::VideoCodecOperationFlagsKHR,
) -> Option<u32> {
    let has_codec_operation_metadata = queue_family_properties
        .iter()
        .zip(codec_operations.iter())
        .any(|(queue_family, codec_operation)| {
            queue_family.queue_count > 0
                && queue_family.queue_flags.contains(required_queue_flag)
                && !codec_operation.is_empty()
        });

    let strict_match = queue_family_properties
        .iter()
        .zip(codec_operations.iter())
        .enumerate()
        .find_map(|(index, (queue_family, codec_operation))| {
            (queue_family.queue_count > 0
                && queue_family.queue_flags.contains(required_queue_flag)
                && codec_operation.contains(required_codec_operation))
            .then(|| u32::try_from(index).ok())
            .flatten()
        });
    if strict_match.is_some() || has_codec_operation_metadata {
        return strict_match;
    }

    queue_family_properties
        .iter()
        .enumerate()
        .find_map(|(index, queue_family)| {
            (queue_family.queue_count > 0 && queue_family.queue_flags.contains(required_queue_flag))
                .then(|| u32::try_from(index).ok())
                .flatten()
        })
}

fn probe_av1_decode_session_parameters(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
    capability_snapshot: &Av1DecodeCapabilitySnapshot,
) -> Result<(), String> {
    let device = create_av1_decode_device(instance, physical_device, queue_family_index)?;
    let result = create_and_destroy_av1_decode_session_parameters(
        instance,
        &device,
        physical_device,
        queue_family_index,
        capability_snapshot,
    );
    // SAFETY: The device is no longer used after this point.
    unsafe {
        device.destroy_device(None);
    }
    result.map(|_| ())
}

fn probe_av1_decode_session_parameters_for_bitstream_with_instance(
    entry: &ash::Entry,
    instance: &ash::Instance,
    bitstream: &[u8],
    std_sequence_header: &StdVideoAV1SequenceHeader,
    coded_width: u32,
    coded_height: u32,
    options: Av1DecodeBitstreamSessionProbeOptions,
) -> Result<Av1DecodeBitstreamSessionProbe, String> {
    // SAFETY: `instance` is valid here; we only enumerate physical device handles.
    let physical_devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;
    if physical_devices.is_empty() {
        return Err(
            "no Vulkan physical devices available for AV1 bitstream session probe".to_string(),
        );
    }

    let mut probe_errors = Vec::new();
    for physical_device in physical_devices {
        let support = query_av1_adapter_decode_support(instance, physical_device)
            .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
        if !support.extensions.supports_av1_decode() {
            continue;
        }
        let queue_family_index = if options.readback {
            support.decode_transfer_queue_family_index
        } else {
            support.decode_queue_family_index
        };
        let Some(queue_family_index) = queue_family_index else {
            if options.readback {
                probe_errors.push(
                        "AV1 decode readback requires a queue family with VIDEO_DECODE and TRANSFER support".to_string(),
                    );
            }
            continue;
        };
        let snapshot = match query_av1_decode_capability_snapshot(entry, instance, physical_device)
        {
            Ok(snapshot) => snapshot,
            Err(err) => {
                probe_errors.push(err);
                continue;
            }
        };
        if let Err(err) = validate_av1_decode_capability_snapshot(&snapshot) {
            probe_errors.push(err);
            continue;
        }
        if coded_width < snapshot.min_coded_width
            || coded_width > snapshot.max_coded_width
            || coded_height < snapshot.min_coded_height
            || coded_height > snapshot.max_coded_height
        {
            probe_errors.push(format!(
                "AV1 bitstream coded extent {coded_width}x{coded_height} is outside adapter range {}x{}..{}x{}",
                snapshot.min_coded_width,
                snapshot.min_coded_height,
                snapshot.max_coded_width,
                snapshot.max_coded_height
            ));
            continue;
        }
        let picture_format = *snapshot
            .decode_output_formats
            .first()
            .ok_or_else(|| "AV1 bitstream session probe has no output format".to_string())?;
        let device = match create_av1_decode_device(instance, physical_device, queue_family_index) {
            Ok(device) => device,
            Err(err) => {
                probe_errors.push(err);
                continue;
            }
        };
        let upload_plan = match build_av1_aligned_decode_bitstream_upload_plan(
            bitstream,
            snapshot.min_bitstream_buffer_offset_alignment,
            snapshot.min_bitstream_buffer_size_alignment,
        ) {
            Ok(plan) => plan,
            Err(err) => {
                probe_errors.push(err);
                // SAFETY: The device is no longer used after this point.
                unsafe {
                    device.destroy_device(None);
                }
                continue;
            }
        };
        let upload_result =
            create_av1_decode_source_buffer(instance, &device, physical_device, &upload_plan.bytes);
        let command = match build_av1_key_frame_decode_command_skeleton_from_decodes(
            &upload_plan.decodes,
            snapshot.max_dpb_slots,
        ) {
            Ok(command) => command,
            Err(err) => {
                probe_errors.push(err);
                // SAFETY: The device is no longer used after this point.
                unsafe {
                    device.destroy_device(None);
                }
                continue;
            }
        };
        let image_plan = match command.decode_image_plan(picture_format) {
            Ok(plan) => plan,
            Err(err) => {
                probe_errors.push(err);
                // SAFETY: The device is no longer used after this point.
                unsafe {
                    device.destroy_device(None);
                }
                continue;
            }
        };
        let readback_plan = match command.decode_readback_plan(picture_format) {
            Ok(plan) => plan,
            Err(err) => {
                probe_errors.push(err);
                // SAFETY: The device is no longer used after this point.
                unsafe {
                    device.destroy_device(None);
                }
                continue;
            }
        };
        let result = upload_result.and_then(|source_buffer| {
            let image =
                match create_av1_decode_image(instance, &device, physical_device, &image_plan) {
                    Ok(image) => image,
                    Err(err) => {
                        destroy_av1_decode_source_buffer(&device, source_buffer);
                        return Err(err);
                    }
                };
            let source_barrier = Av1DecodeCommandSkeleton::vk_decode_source_memory_barrier();
            if source_barrier.dst_access_mask != vk::AccessFlags2::VIDEO_DECODE_READ_KHR {
                destroy_av1_decode_image(&device, image);
                destroy_av1_decode_source_buffer(&device, source_buffer);
                return Err(
                    "AV1 decode source barrier does not target VIDEO_DECODE_READ_KHR".to_string(),
                );
            }
            let image_barrier = match command.vk_decode_image_init_barrier(image.image) {
                Ok(barrier) => barrier,
                Err(err) => {
                    destroy_av1_decode_image(&device, image);
                    destroy_av1_decode_source_buffer(&device, source_buffer);
                    return Err(err);
                }
            };
            let session = match create_av1_decode_session_with_parameters(
                instance,
                &device,
                Av1DecodeSessionParameterProbeConfig {
                    physical_device,
                    queue_family_index,
                    capability_snapshot: &snapshot,
                    picture_format,
                    coded_width,
                    coded_height,
                    std_sequence_header,
                },
            ) {
                Ok(session) => session,
                Err(err) => {
                    destroy_av1_decode_image(&device, image);
                    destroy_av1_decode_source_buffer(&device, source_buffer);
                    return Err(err);
                }
            };
            let command_buffer_record_requested = options.record_command_buffer;
            let command_buffer_submit_requested =
                command_buffer_record_requested && options.submit_command_buffer;
            let readback_requested = options.readback;
            if readback_requested
                && (!command_buffer_record_requested || !command_buffer_submit_requested)
            {
                destroy_av1_decode_session_resource(instance, &device, session);
                destroy_av1_decode_image(&device, image);
                destroy_av1_decode_source_buffer(&device, source_buffer);
                return Err(
                    "AV1 decode readback probe requires command-buffer record and submit"
                        .to_string(),
                );
            }
            let readback_buffer = if readback_requested {
                match create_av1_decode_readback_buffer(
                    instance,
                    &device,
                    physical_device,
                    readback_plan.buffer_size,
                ) {
                    Ok(buffer) => {
                        if let Err(err) = initialize_av1_decode_readback_buffer(&device, &buffer) {
                            destroy_av1_decode_readback_buffer(&device, buffer);
                            destroy_av1_decode_session_resource(instance, &device, session);
                            destroy_av1_decode_image(&device, image);
                            destroy_av1_decode_source_buffer(&device, source_buffer);
                            return Err(err);
                        }
                        Some(buffer)
                    }
                    Err(err) => {
                        destroy_av1_decode_session_resource(instance, &device, session);
                        destroy_av1_decode_image(&device, image);
                        destroy_av1_decode_source_buffer(&device, source_buffer);
                        return Err(err);
                    }
                }
            } else {
                None
            };
            let record_result = if command_buffer_record_requested {
                record_and_destroy_av1_decode_command_buffer(Av1DecodeCommandRecordConfig {
                    instance,
                    device: &device,
                    queue_family_index,
                    submit_command_buffer: command_buffer_submit_requested,
                    upload_plan: &upload_plan,
                    command: &command,
                    video_session: session.session,
                    video_session_parameters: session.parameters,
                    source_buffer: source_buffer.buffer,
                    decode_image: image.image,
                    image_view: image.view,
                    readback: readback_buffer.as_ref().map(|buffer| {
                        Av1DecodeCommandReadbackConfig {
                            buffer: buffer.buffer,
                            plan: &readback_plan,
                        }
                    }),
                })
            } else {
                upload_plan.record_decode_command_sequence(
                    &command,
                    session.session,
                    session.parameters,
                    source_buffer.buffer,
                    image.view,
                    |_| Ok(()),
                )
            };
            let summary = session.summary;
            let decode_image_barrier_layers = image_barrier.subresource_range.layer_count;
            let record_summary = match record_result {
                Ok(summary) => summary,
                Err(err) => {
                    if let Some(readback_buffer) = readback_buffer {
                        destroy_av1_decode_readback_buffer(&device, readback_buffer);
                    }
                    destroy_av1_decode_session_resource(instance, &device, session);
                    destroy_av1_decode_image(&device, image);
                    destroy_av1_decode_source_buffer(&device, source_buffer);
                    return Err(err);
                }
            };
            let readback_sample = if let Some(readback_resource) = &readback_buffer {
                match map_av1_decode_readback_buffer(&device, readback_resource) {
                    Ok(sample) => sample,
                    Err(err) => {
                        if let Some(readback_resource) = readback_buffer {
                            destroy_av1_decode_readback_buffer(&device, readback_resource);
                        }
                        destroy_av1_decode_session_resource(instance, &device, session);
                        destroy_av1_decode_image(&device, image);
                        destroy_av1_decode_source_buffer(&device, source_buffer);
                        return Err(err);
                    }
                }
            } else {
                Av1DecodeReadbackSample::default()
            };
            if let Some(readback_buffer) = readback_buffer {
                destroy_av1_decode_readback_buffer(&device, readback_buffer);
            }
            destroy_av1_decode_session_resource(instance, &device, session);
            destroy_av1_decode_image(&device, image);
            destroy_av1_decode_source_buffer(&device, source_buffer);
            Ok((
                summary,
                record_summary,
                decode_image_barrier_layers,
                readback_sample,
                command_buffer_record_requested,
                command_buffer_submit_requested,
            ))
        });
        // SAFETY: The device is no longer used after this point.
        unsafe {
            device.destroy_device(None);
        }
        match result {
            Ok((
                summary,
                record_summary,
                decode_image_barrier_layers,
                readback_sample,
                command_buffer_recorded,
                command_buffer_submitted,
            )) => {
                return Ok(Av1DecodeBitstreamSessionProbe {
                    coded_width,
                    coded_height,
                    picture_format,
                    min_bitstream_buffer_offset_alignment: snapshot
                        .min_bitstream_buffer_offset_alignment,
                    min_bitstream_buffer_size_alignment: snapshot
                        .min_bitstream_buffer_size_alignment,
                    session_memory_requirement_count: summary.memory_requirement_count,
                    session_memory_total_size: summary.memory_total_size,
                    session_memory_max_alignment: summary.memory_max_alignment,
                    session_memory_bound_count: summary.memory_bound_count,
                    bitstream_upload_bytes: upload_plan.bytes.len(),
                    decode_image_layers: image_plan.array_layers,
                    decode_image_barrier_layers,
                    readback_bytes: readback_plan.buffer_size,
                    readback_region_count: readback_plan.regions.len(),
                    readback_mapped_bytes: readback_sample.mapped_bytes,
                    readback_non_zero: readback_sample.non_zero,
                    readback_sample: readback_sample.data,
                    command_record_decode_count: record_summary.decode_count,
                    command_buffer_recorded,
                    command_buffer_submitted,
                });
            }
            Err(err) => probe_errors.push(err),
        }
    }

    if probe_errors.is_empty() {
        Err("no compatible Vulkan AV1 decode adapter found for bitstream session probe".to_string())
    } else {
        Err(format!(
            "AV1 bitstream session probe failed on all candidate adapters: {}",
            probe_errors.join("; ")
        ))
    }
}

fn create_and_destroy_av1_decode_session_parameters(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
    capability_snapshot: &Av1DecodeCapabilitySnapshot,
) -> Result<Av1DecodeSessionBootstrapSummary, String> {
    let picture_format = *capability_snapshot
        .decode_output_formats
        .first()
        .ok_or_else(|| "AV1 decode session parameter probe has no output format".to_string())?;
    let coded_width = preferred_av1_probe_extent(
        capability_snapshot.min_coded_width,
        capability_snapshot.max_coded_width,
    );
    let coded_height = preferred_av1_probe_extent(
        capability_snapshot.min_coded_height,
        capability_snapshot.max_coded_height,
    );
    let std_sequence_header = build_probe_av1_std_sequence_header(coded_width, coded_height)?;
    create_and_destroy_av1_decode_session_parameters_with_header(
        instance,
        device,
        Av1DecodeSessionParameterProbeConfig {
            physical_device,
            queue_family_index,
            capability_snapshot,
            picture_format,
            coded_width,
            coded_height,
            std_sequence_header: &std_sequence_header,
        },
    )
}

fn create_and_destroy_av1_decode_session_parameters_with_header(
    instance: &ash::Instance,
    device: &ash::Device,
    config: Av1DecodeSessionParameterProbeConfig<'_>,
) -> Result<Av1DecodeSessionBootstrapSummary, String> {
    let resource = create_av1_decode_session_with_parameters(instance, device, config)?;
    let summary = resource.summary;
    destroy_av1_decode_session_resource(instance, device, resource);
    Ok(summary)
}

fn create_av1_decode_session_with_parameters(
    instance: &ash::Instance,
    device: &ash::Device,
    config: Av1DecodeSessionParameterProbeConfig<'_>,
) -> Result<Av1DecodeSessionResource, String> {
    let mut decode_av1_profile = vk::VideoDecodeAV1ProfileInfoKHR::default()
        .std_profile(StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN)
        .film_grain_support(false);
    let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
    let profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_AV1)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut decode_av1_profile)
        .push_next(&mut decode_usage);

    let create_info = vk::VideoSessionCreateInfoKHR::default()
        .queue_family_index(config.queue_family_index)
        .video_profile(&profile)
        .picture_format(config.picture_format)
        .max_coded_extent(vk::Extent2D {
            width: config.coded_width,
            height: config.coded_height,
        })
        .reference_picture_format(config.picture_format)
        .max_dpb_slots(config.capability_snapshot.max_dpb_slots)
        .max_active_reference_pictures(config.capability_snapshot.max_active_reference_pictures)
        .std_header_version(&config.capability_snapshot.std_header_version);
    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    let mut video_session = vk::VideoSessionKHR::null();

    // SAFETY: All pointers in `create_info` live until the call returns.
    let session_result = unsafe {
        (video_queue_device.fp().create_video_session_khr)(
            device.handle(),
            &create_info,
            std::ptr::null(),
            &mut video_session,
        )
    };
    if session_result != vk::Result::SUCCESS {
        return Err(format!(
            "vkCreateVideoSessionKHR for AV1 decode failed: {session_result:?}"
        ));
    }

    let resource_result =
        query_av1_decode_session_memory_requirements(device, &video_queue_device, video_session)
            .and_then(|memory_plan| {
                let session_memories = bind_av1_decode_session_memory(
                    instance,
                    device,
                    &video_queue_device,
                    config.physical_device,
                    video_session,
                    &memory_plan.requirements,
                )?;
                let parameters = match create_av1_decode_session_parameters(
                    device,
                    &video_queue_device,
                    video_session,
                    config.std_sequence_header,
                ) {
                    Ok(parameters) => parameters,
                    Err(err) => {
                        for memory in session_memories {
                            // SAFETY: These allocations were created by this device in this scope.
                            unsafe { device.free_memory(memory, None) };
                        }
                        return Err(err);
                    }
                };
                let mut summary = memory_plan.summary;
                summary.memory_bound_count = session_memories.len();
                Ok(Av1DecodeSessionResource {
                    session: video_session,
                    parameters,
                    memories: session_memories,
                    summary,
                })
            });

    if resource_result.is_err() {
        // SAFETY: `video_session` was created by this device and is no longer used.
        unsafe {
            (video_queue_device.fp().destroy_video_session_khr)(
                device.handle(),
                video_session,
                std::ptr::null(),
            );
        }
    }
    resource_result
}

fn query_av1_decode_session_memory_requirements(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_session: vk::VideoSessionKHR,
) -> Result<Av1DecodeSessionMemoryPlan, String> {
    let mut requirement_count = 0_u32;
    // SAFETY: Count query writes only `requirement_count` for a valid video session.
    let count_result = unsafe {
        (video_queue_device
            .fp()
            .get_video_session_memory_requirements_khr)(
            device.handle(),
            video_session,
            &mut requirement_count,
            std::ptr::null_mut(),
        )
    };
    if count_result != vk::Result::SUCCESS {
        return Err(format!(
            "vkGetVideoSessionMemoryRequirementsKHR for AV1 decode count query failed: {count_result:?}"
        ));
    }
    if requirement_count == 0 {
        return Err(
            "vkGetVideoSessionMemoryRequirementsKHR for AV1 decode returned no memory requirements"
                .to_string(),
        );
    }

    let mut requirements =
        vec![vk::VideoSessionMemoryRequirementsKHR::default(); requirement_count as usize];
    // SAFETY: `requirements` has storage for the count returned above.
    let query_result = unsafe {
        (video_queue_device
            .fp()
            .get_video_session_memory_requirements_khr)(
            device.handle(),
            video_session,
            &mut requirement_count,
            requirements.as_mut_ptr(),
        )
    };
    if query_result != vk::Result::SUCCESS {
        return Err(format!(
            "vkGetVideoSessionMemoryRequirementsKHR for AV1 decode query failed: {query_result:?}"
        ));
    }
    requirements.truncate(requirement_count as usize);

    let memory_total_size = requirements
        .iter()
        .map(|requirement| requirement.memory_requirements.size)
        .sum();
    let memory_max_alignment = requirements
        .iter()
        .map(|requirement| requirement.memory_requirements.alignment)
        .max()
        .unwrap_or(0);

    Ok(Av1DecodeSessionMemoryPlan {
        summary: Av1DecodeSessionBootstrapSummary {
            memory_requirement_count: requirements.len(),
            memory_total_size,
            memory_max_alignment,
            memory_bound_count: 0,
        },
        requirements,
    })
}

fn bind_av1_decode_session_memory(
    instance: &ash::Instance,
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    physical_device: vk::PhysicalDevice,
    video_session: vk::VideoSessionKHR,
    requirements: &[vk::VideoSessionMemoryRequirementsKHR<'_>],
) -> Result<Vec<vk::DeviceMemory>, String> {
    if requirements.is_empty() {
        return Err("AV1 decode session memory bind requires at least one requirement".to_string());
    }

    // SAFETY: `physical_device` belongs to `instance`; this only reads memory metadata.
    let memory_properties =
        unsafe { instance.get_physical_device_memory_properties(physical_device) };
    let mut memories = Vec::with_capacity(requirements.len());
    let mut bindings = Vec::with_capacity(requirements.len());

    for requirement in requirements {
        let requirement_info = requirement.memory_requirements;
        let memory_type_index = select_av1_decode_memory_type_index(
            &memory_properties,
            requirement_info.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .or_else(|| {
            select_av1_decode_memory_type_index(
                &memory_properties,
                requirement_info.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
            )
        })
        .ok_or_else(|| {
            format!(
                "no compatible memory type for AV1 video session bind index {} (bits=0x{:X})",
                requirement.memory_bind_index, requirement_info.memory_type_bits
            )
        })?;
        let allocation_size = requirement_info.size.max(1);
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(allocation_size)
            .memory_type_index(memory_type_index);
        // SAFETY: Allocation info is derived from Vulkan memory requirements.
        let memory = unsafe { device.allocate_memory(&allocate_info, None) }
            .map_err(|err| format!("vkAllocateMemory for AV1 video session bind failed: {err}"))?;
        memories.push(memory);
        bindings.push(
            vk::BindVideoSessionMemoryInfoKHR::default()
                .memory_bind_index(requirement.memory_bind_index)
                .memory(memory)
                .memory_offset(0)
                .memory_size(allocation_size),
        );
    }

    // SAFETY: Bind infos reference memory allocations above and live through the call.
    let bind_result = unsafe {
        (video_queue_device.fp().bind_video_session_memory_khr)(
            device.handle(),
            video_session,
            u32::try_from(bindings.len())
                .map_err(|_| "AV1 session binding count exceeds u32 range".to_string())?,
            bindings.as_ptr(),
        )
    };
    if bind_result != vk::Result::SUCCESS {
        for memory in memories.drain(..) {
            // SAFETY: These allocations were created by this device in this scope.
            unsafe { device.free_memory(memory, None) };
        }
        return Err(format!(
            "vkBindVideoSessionMemoryKHR for AV1 decode failed: {bind_result:?}"
        ));
    }

    Ok(memories)
}

fn select_av1_decode_memory_type_index(
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    memory_type_bits: u32,
    required: vk::MemoryPropertyFlags,
) -> Option<u32> {
    let memory_type_count = usize::try_from(memory_properties.memory_type_count).ok()?;
    memory_properties
        .memory_types
        .iter()
        .take(memory_type_count)
        .enumerate()
        .find_map(|(index, memory_type)| {
            let index_u32 = u32::try_from(index).ok()?;
            let mask = 1_u32.checked_shl(index_u32)?;
            ((memory_type_bits & mask) != 0 && memory_type.property_flags.contains(required))
                .then_some(index_u32)
        })
}

fn create_av1_decode_source_buffer(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    bytes: &[u8],
) -> Result<Av1DecodeSourceBufferResource, String> {
    if bytes.is_empty() {
        return Err("AV1 decode source buffer upload requires non-empty bytes".to_string());
    }

    let buffer_size =
        u64::try_from(bytes.len()).map_err(|_| "AV1 source upload size exceeds u64")?;
    let create_info = vk::BufferCreateInfo::default()
        .size(buffer_size)
        .usage(vk::BufferUsageFlags::VIDEO_DECODE_SRC_KHR)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: Buffer create info contains only POD values.
    let buffer = unsafe { device.create_buffer(&create_info, None) }
        .map_err(|err| format!("vkCreateBuffer for AV1 decode source failed: {err}"))?;

    let upload_result = (|| -> Result<vk::DeviceMemory, String> {
        // SAFETY: `buffer` was created by this device.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        // SAFETY: `physical_device` belongs to `instance`; this only reads memory metadata.
        let memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };
        let memory_type_index = select_av1_decode_memory_type_index(
            &memory_properties,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| {
            format!(
                "no HOST_VISIBLE|HOST_COHERENT memory type for AV1 decode source buffer (bits=0x{:X})",
                requirements.memory_type_bits
            )
        })?;
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size.max(1))
            .memory_type_index(memory_type_index);
        // SAFETY: Allocation info is derived from Vulkan memory requirements.
        let memory = unsafe { device.allocate_memory(&allocate_info, None) }
            .map_err(|err| format!("vkAllocateMemory for AV1 decode source failed: {err}"))?;
        // SAFETY: Buffer and memory were created by this device and offset 0 satisfies alignment
        // by construction for a fresh allocation.
        if let Err(err) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
            // SAFETY: `memory` was allocated above and has not been bound successfully.
            unsafe { device.free_memory(memory, None) };
            return Err(format!(
                "vkBindBufferMemory for AV1 decode source failed: {err}"
            ));
        }
        // SAFETY: Memory is HOST_VISIBLE|HOST_COHERENT and the mapped range fits the allocation.
        let mapped =
            unsafe { device.map_memory(memory, 0, buffer_size, vk::MemoryMapFlags::empty()) };
        let mapped = match mapped {
            Ok(mapped) => mapped,
            Err(err) => {
                // SAFETY: `memory` was allocated above and is no longer used after map failure.
                unsafe { device.free_memory(memory, None) };
                return Err(format!("vkMapMemory for AV1 decode source failed: {err}"));
            }
        };
        // SAFETY: `mapped` points to at least `bytes.len()` writable bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len());
            device.unmap_memory(memory);
        }
        Ok(memory)
    })();

    match upload_result {
        Ok(memory) => Ok(Av1DecodeSourceBufferResource { buffer, memory }),
        Err(err) => {
            // SAFETY: The buffer was created above and is no longer used after upload failure.
            unsafe {
                device.destroy_buffer(buffer, None);
            }
            Err(err)
        }
    }
}

fn destroy_av1_decode_source_buffer(device: &ash::Device, resource: Av1DecodeSourceBufferResource) {
    // SAFETY: The resource was created by this device and is no longer used.
    unsafe {
        device.destroy_buffer(resource.buffer, None);
        device.free_memory(resource.memory, None);
    }
}

fn create_av1_decode_readback_buffer(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    size: u64,
) -> Result<Av1DecodeReadbackBufferResource, String> {
    if size == 0 {
        return Err("AV1 decode readback buffer requires non-zero size".to_string());
    }

    let create_info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: Buffer create info contains only POD values.
    let buffer = unsafe { device.create_buffer(&create_info, None) }
        .map_err(|err| format!("vkCreateBuffer for AV1 decode readback failed: {err}"))?;

    let result = (|| -> Result<vk::DeviceMemory, String> {
        // SAFETY: `buffer` was created by this device.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        // SAFETY: `physical_device` belongs to `instance`; this only reads memory metadata.
        let memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };
        let memory_type_index = select_av1_decode_memory_type_index(
            &memory_properties,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| {
            format!(
                "no HOST_VISIBLE|HOST_COHERENT memory type for AV1 decode readback buffer (bits=0x{:X})",
                requirements.memory_type_bits
            )
        })?;
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size.max(size))
            .memory_type_index(memory_type_index);
        // SAFETY: Allocation info is derived from Vulkan memory requirements.
        let memory = unsafe { device.allocate_memory(&allocate_info, None) }
            .map_err(|err| format!("vkAllocateMemory for AV1 decode readback failed: {err}"))?;
        // SAFETY: Buffer and memory were created by this device.
        if let Err(err) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
            // SAFETY: `memory` was allocated above and has not been bound successfully.
            unsafe { device.free_memory(memory, None) };
            return Err(format!(
                "vkBindBufferMemory for AV1 decode readback failed: {err}"
            ));
        }
        Ok(memory)
    })();

    match result {
        Ok(memory) => Ok(Av1DecodeReadbackBufferResource {
            buffer,
            memory,
            size,
        }),
        Err(err) => {
            // SAFETY: The buffer was created above and is no longer used after failure.
            unsafe {
                device.destroy_buffer(buffer, None);
            }
            Err(err)
        }
    }
}

fn initialize_av1_decode_readback_buffer(
    device: &ash::Device,
    resource: &Av1DecodeReadbackBufferResource,
) -> Result<(), String> {
    let mapped_len = usize::try_from(resource.size)
        .map_err(|_| "AV1 decode readback buffer size exceeds usize range".to_string())?;
    // SAFETY: The readback memory is HOST_VISIBLE|HOST_COHERENT and the requested range fits the
    // allocation used for the buffer.
    let mapped = unsafe {
        device.map_memory(
            resource.memory,
            0,
            resource.size,
            vk::MemoryMapFlags::empty(),
        )
    }
    .map_err(|err| format!("vkMapMemory for AV1 decode readback initialization failed: {err}"))?;
    // SAFETY: The mapped pointer is valid for `mapped_len` bytes until unmap.
    let mapped_slice = unsafe { std::slice::from_raw_parts_mut(mapped.cast::<u8>(), mapped_len) };
    mapped_slice.fill(0xcd);
    // SAFETY: HOST_COHERENT memory does not require an explicit flush, and the memory is no longer
    // accessed after unmap until the queued transfer writes it.
    unsafe {
        device.unmap_memory(resource.memory);
    }
    Ok(())
}

fn map_av1_decode_readback_buffer(
    device: &ash::Device,
    resource: &Av1DecodeReadbackBufferResource,
) -> Result<Av1DecodeReadbackSample, String> {
    let mapped_len = usize::try_from(resource.size)
        .map_err(|_| "AV1 decode readback buffer size exceeds usize range".to_string())?;
    // SAFETY: The readback memory is HOST_VISIBLE|HOST_COHERENT and the requested range fits the
    // allocation used for the buffer.
    let mapped = unsafe {
        device.map_memory(
            resource.memory,
            0,
            resource.size,
            vk::MemoryMapFlags::empty(),
        )
    }
    .map_err(|err| format!("vkMapMemory for AV1 decode readback failed: {err}"))?;
    // SAFETY: The mapped pointer is valid for `mapped_len` bytes until unmap.
    let mapped_slice = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), mapped_len) };
    let data = mapped_slice.to_vec();
    let non_zero = mapped_slice.iter().any(|&byte| byte != 0);
    // SAFETY: The memory was mapped above and is no longer accessed after this point.
    unsafe {
        device.unmap_memory(resource.memory);
    }
    Ok(Av1DecodeReadbackSample {
        mapped_bytes: mapped_len,
        non_zero,
        data,
    })
}

fn destroy_av1_decode_readback_buffer(
    device: &ash::Device,
    resource: Av1DecodeReadbackBufferResource,
) {
    // SAFETY: The resource was created by this device and is no longer used.
    unsafe {
        device.destroy_buffer(resource.buffer, None);
        device.free_memory(resource.memory, None);
    }
}

fn create_av1_decode_image(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    plan: &Av1DecodeImagePlan,
) -> Result<Av1DecodeImageResource, String> {
    let mut decode_av1_profile = vk::VideoDecodeAV1ProfileInfoKHR::default()
        .std_profile(StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN)
        .film_grain_support(false);
    let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
    let image_profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_AV1)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut decode_av1_profile)
        .push_next(&mut decode_usage);
    let image_profiles = [image_profile];
    let mut profile_list = vk::VideoProfileListInfoKHR::default().profiles(&image_profiles);
    let image_create_info = vk::ImageCreateInfo {
        p_next: (&mut profile_list as *mut vk::VideoProfileListInfoKHR<'_>).cast(),
        ..vk::ImageCreateInfo::default()
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
    };
    // SAFETY: Image create info references a live profile list and device is valid.
    let image = unsafe { device.create_image(&image_create_info, None) }
        .map_err(|err| format!("vkCreateImage for AV1 decode image failed: {err}"))?;

    let result = (|| -> Result<(vk::DeviceMemory, vk::ImageView), String> {
        // SAFETY: `image` was created by this device.
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        // SAFETY: `physical_device` belongs to `instance`; this only reads memory metadata.
        let memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };
        let memory_type_index = select_av1_decode_memory_type_index(
            &memory_properties,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .or_else(|| {
            select_av1_decode_memory_type_index(
                &memory_properties,
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
            )
        })
        .ok_or_else(|| {
            format!(
                "no compatible memory type for AV1 decode image (bits=0x{:X})",
                requirements.memory_type_bits
            )
        })?;
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size.max(1))
            .memory_type_index(memory_type_index);
        // SAFETY: Allocation info is derived from Vulkan image memory requirements.
        let memory = unsafe { device.allocate_memory(&allocate_info, None) }
            .map_err(|err| format!("vkAllocateMemory for AV1 decode image failed: {err}"))?;
        if let Err(err) = unsafe { device.bind_image_memory(image, memory, 0) } {
            // SAFETY: `memory` was allocated above and has not been bound successfully.
            unsafe { device.free_memory(memory, None) };
            return Err(format!(
                "vkBindImageMemory for AV1 decode image failed: {err}"
            ));
        }
        let view_create_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
            .format(plan.format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: plan.array_layers,
            });
        // SAFETY: Image view create info references a valid image and subresource range.
        let view = unsafe { device.create_image_view(&view_create_info, None) }.map_err(|err| {
            // SAFETY: `memory` was allocated and bound above and is no longer used after failure.
            unsafe { device.free_memory(memory, None) };
            format!("vkCreateImageView for AV1 decode image failed: {err}")
        })?;
        Ok((memory, view))
    })();

    match result {
        Ok((memory, view)) => Ok(Av1DecodeImageResource {
            image,
            memory,
            view,
        }),
        Err(err) => {
            // SAFETY: The image was created above and is no longer used.
            unsafe {
                device.destroy_image(image, None);
            }
            Err(err)
        }
    }
}

fn destroy_av1_decode_image(device: &ash::Device, resource: Av1DecodeImageResource) {
    // SAFETY: The resource was created by this device and is no longer used.
    unsafe {
        device.destroy_image_view(resource.view, None);
        device.destroy_image(resource.image, None);
        device.free_memory(resource.memory, None);
    }
}

fn record_and_destroy_av1_decode_command_buffer(
    config: Av1DecodeCommandRecordConfig<'_>,
) -> Result<Av1DecodeCommandRecordSummary, String> {
    let device = config.device;
    let command_pool_info =
        vk::CommandPoolCreateInfo::default().queue_family_index(config.queue_family_index);
    // SAFETY: Command pool create info references the selected decode queue family.
    let command_pool = unsafe { device.create_command_pool(&command_pool_info, None) }
        .map_err(|err| format!("vkCreateCommandPool for AV1 decode record failed: {err}"))?;

    let result = (|| -> Result<Av1DecodeCommandRecordSummary, String> {
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: Allocate info references a live command pool.
        let command_buffers = unsafe { device.allocate_command_buffers(&allocate_info) }
            .map_err(|err| format!("vkAllocateCommandBuffers for AV1 decode failed: {err}"))?;
        let command_buffer = command_buffers.first().copied().ok_or_else(|| {
            "vkAllocateCommandBuffers returned no command buffer for AV1 decode".to_string()
        })?;
        let record_mode = av1_decode_command_buffer_record_mode_from_env();
        if config.submit_command_buffer && record_mode == Av1DecodeCommandBufferRecordMode::Full {
            record_submit_av1_decode_reset_command_buffer(
                device,
                config.instance,
                command_pool,
                config.queue_family_index,
                config.video_session,
                config.video_session_parameters,
            )?;
        }

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: Command buffer is valid and not already recording.
        unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
            .map_err(|err| format!("vkBeginCommandBuffer for AV1 decode failed: {err}"))?;

        let source_barrier = Av1DecodeCommandSkeleton::vk_decode_source_memory_barrier();
        let image_barrier = config
            .command
            .vk_decode_image_init_barrier(config.decode_image)?;
        let dependency_info = vk::DependencyInfo::default()
            .memory_barriers(std::slice::from_ref(&source_barrier))
            .image_memory_barriers(std::slice::from_ref(&image_barrier));
        // SAFETY: Barriers reference live probe resources and command buffer is recording.
        unsafe {
            device.cmd_pipeline_barrier2(command_buffer, &dependency_info);
        }
        if record_mode == Av1DecodeCommandBufferRecordMode::BarrierOnly {
            // SAFETY: Command buffer is recording and only contains pipeline barriers.
            unsafe { device.end_command_buffer(command_buffer) }
                .map_err(|err| format!("vkEndCommandBuffer for AV1 decode failed: {err}"))?;
            return Ok(Av1DecodeCommandRecordSummary::default());
        }

        let video_queue_device = ash::khr::video_queue::Device::new(config.instance, device);
        let video_decode_device =
            ash::khr::video_decode_queue::Device::new(config.instance, device);
        let mut emitted_first_decode = false;
        let mut record_summary = Av1DecodeCommandRecordSummary::default();
        config.upload_plan.visit_decode_command_sequence(
            config.command,
            config.video_session,
            config.video_session_parameters,
            config.source_buffer,
            config.image_view,
            |visit| {
                match visit {
                    Av1DecodeCommandVisit::BeginCoding(info) => {
                        // SAFETY: Command buffer is recording; `info` references live resources.
                        unsafe {
                            (video_queue_device.fp().cmd_begin_video_coding_khr)(
                                command_buffer,
                                info,
                            );
                        }
                        record_summary.begin_count += 1;
                    }
                    Av1DecodeCommandVisit::ResetCoding(info) => {
                        if config.submit_command_buffer
                            && record_mode == Av1DecodeCommandBufferRecordMode::Full
                        {
                            record_summary.reset_count += 1;
                            return;
                        }
                        if record_mode == Av1DecodeCommandBufferRecordMode::BeginEnd {
                            return;
                        }
                        // SAFETY: Command buffer is inside a video coding scope.
                        unsafe {
                            (video_queue_device.fp().cmd_control_video_coding_khr)(
                                command_buffer,
                                info,
                            );
                        }
                        record_summary.reset_count += 1;
                    }
                    Av1DecodeCommandVisit::DecodeFrame { decode_info, .. } => {
                        if matches!(
                            record_mode,
                            Av1DecodeCommandBufferRecordMode::BeginEnd
                                | Av1DecodeCommandBufferRecordMode::ResetEnd
                        ) || (record_mode == Av1DecodeCommandBufferRecordMode::FirstDecode
                            && emitted_first_decode)
                        {
                            return;
                        }
                        // SAFETY: Command buffer is inside a video coding scope and decode info
                        // chains are scoped to this call.
                        unsafe {
                            (video_decode_device.fp().cmd_decode_video_khr)(
                                command_buffer,
                                decode_info,
                            );
                        }
                        emitted_first_decode = true;
                        record_summary.decode_count += 1;
                    }
                    Av1DecodeCommandVisit::EndCoding(info) => {
                        // SAFETY: Command buffer is inside a video coding scope.
                        unsafe {
                            (video_queue_device.fp().cmd_end_video_coding_khr)(
                                command_buffer,
                                info,
                            );
                        }
                        record_summary.end_count += 1;
                    }
                }
            },
        )?;

        if let Some(readback) = config.readback {
            if record_mode != Av1DecodeCommandBufferRecordMode::Full {
                return Err(
                    "AV1 decode readback requires full command-buffer record mode".to_string(),
                );
            }
            let decode_to_copy_image_barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                .src_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .old_layout(vk::ImageLayout::VIDEO_DECODE_DST_KHR)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .image(config.decode_image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            let decode_to_copy_dependency = vk::DependencyInfo::default()
                .image_memory_barriers(std::slice::from_ref(&decode_to_copy_image_barrier));
            // SAFETY: Decode image and readback buffer are live and command buffer is recording.
            unsafe {
                device.cmd_pipeline_barrier2(command_buffer, &decode_to_copy_dependency);
                device.cmd_copy_image_to_buffer(
                    command_buffer,
                    config.decode_image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    readback.buffer,
                    &readback.plan.regions,
                );
            }
            let copy_to_host_barrier = vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ);
            let copy_to_host_dependency = vk::DependencyInfo::default()
                .memory_barriers(std::slice::from_ref(&copy_to_host_barrier));
            // SAFETY: The barrier only orders transfer writes before host readback after submit.
            unsafe {
                device.cmd_pipeline_barrier2(command_buffer, &copy_to_host_dependency);
            }
        }

        // SAFETY: Command buffer is recording and all command data has been emitted.
        unsafe { device.end_command_buffer(command_buffer) }
            .map_err(|err| format!("vkEndCommandBuffer for AV1 decode failed: {err}"))?;
        if config.submit_command_buffer {
            submit_av1_decode_command_buffer(device, config.queue_family_index, command_buffer)?;
        }
        Ok(record_summary)
    })();

    // SAFETY: Command pool and allocated command buffers are no longer used.
    unsafe {
        device.destroy_command_pool(command_pool, None);
    }
    result
}

fn record_submit_av1_decode_reset_command_buffer(
    device: &ash::Device,
    instance: &ash::Instance,
    command_pool: vk::CommandPool,
    queue_family_index: u32,
    video_session: vk::VideoSessionKHR,
    video_session_parameters: vk::VideoSessionParametersKHR,
) -> Result<(), String> {
    let allocate_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: Allocate info references a live command pool.
    let reset_command_buffer = unsafe { device.allocate_command_buffers(&allocate_info) }
        .map_err(|err| format!("vkAllocateCommandBuffers for AV1 decode reset failed: {err}"))?
        .first()
        .copied()
        .ok_or_else(|| {
            "vkAllocateCommandBuffers returned no command buffer for AV1 decode reset".to_string()
        })?;
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: Command buffer is valid and not already recording.
    unsafe { device.begin_command_buffer(reset_command_buffer, &begin_info) }
        .map_err(|err| format!("vkBeginCommandBuffer for AV1 decode reset failed: {err}"))?;

    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    let begin_coding_info = vk::VideoBeginCodingInfoKHR::default()
        .video_session(video_session)
        .video_session_parameters(video_session_parameters);
    let reset_control =
        vk::VideoCodingControlInfoKHR::default().flags(vk::VideoCodingControlFlagsKHR::RESET);
    let end_coding_info = vk::VideoEndCodingInfoKHR::default();
    // SAFETY: Command buffer is recording; the reset scope references a live video session.
    unsafe {
        (video_queue_device.fp().cmd_begin_video_coding_khr)(
            reset_command_buffer,
            &begin_coding_info,
        );
        (video_queue_device.fp().cmd_control_video_coding_khr)(
            reset_command_buffer,
            &reset_control,
        );
        (video_queue_device.fp().cmd_end_video_coding_khr)(reset_command_buffer, &end_coding_info);
    }
    // SAFETY: Command buffer is recording and all reset command data has been emitted.
    unsafe { device.end_command_buffer(reset_command_buffer) }
        .map_err(|err| format!("vkEndCommandBuffer for AV1 decode reset failed: {err}"))?;
    submit_av1_decode_command_buffer(device, queue_family_index, reset_command_buffer)
}

fn submit_av1_decode_command_buffer(
    device: &ash::Device,
    queue_family_index: u32,
    command_buffer: vk::CommandBuffer,
) -> Result<(), String> {
    let fence_info = vk::FenceCreateInfo::default();
    // SAFETY: Fence create info contains only POD values.
    let fence = unsafe { device.create_fence(&fence_info, None) }
        .map_err(|err| format!("vkCreateFence for AV1 decode submit failed: {err}"))?;
    let result = (|| -> Result<(), String> {
        // SAFETY: Queue family index is the decode queue family used to create the device.
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let command_buffers = [command_buffer];
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
        // SAFETY: Submit info references a recorded command buffer that stays alive until wait.
        unsafe { device.queue_submit(queue, std::slice::from_ref(&submit_info), fence) }
            .map_err(|err| format!("vkQueueSubmit for AV1 decode failed: {err}"))?;
        // SAFETY: Fence belongs to this device and was used for the submit above.
        unsafe { device.wait_for_fences(std::slice::from_ref(&fence), true, 5_000_000_000) }
            .map_err(|err| format!("vkWaitForFences for AV1 decode failed: {err}"))?;
        Ok(())
    })();
    // SAFETY: Fence is no longer used after the wait/error path.
    unsafe {
        device.destroy_fence(fence, None);
    }
    result
}

fn av1_decode_command_buffer_record_mode_from_env() -> Av1DecodeCommandBufferRecordMode {
    match std::env::var("VIDEO_HW_VULKAN_AV1_RECORD_MODE")
        .ok()
        .as_deref()
    {
        Some("barrier_only") | Some("barrier") => Av1DecodeCommandBufferRecordMode::BarrierOnly,
        Some("begin_end") | Some("begin") => Av1DecodeCommandBufferRecordMode::BeginEnd,
        Some("reset_end") | Some("reset") => Av1DecodeCommandBufferRecordMode::ResetEnd,
        Some("first_decode") | Some("decode") => Av1DecodeCommandBufferRecordMode::FirstDecode,
        _ => Av1DecodeCommandBufferRecordMode::Full,
    }
}

fn build_av1_decode_readback_plan(
    format: vk::Format,
    coded_width: u32,
    coded_height: u32,
) -> Result<Av1DecodeReadbackPlan, String> {
    if coded_width == 0 || coded_height == 0 {
        return Err(format!(
            "invalid coded extent for AV1 decode readback: {coded_width}x{coded_height}"
        ));
    }

    let mut regions = Vec::new();
    let mut next_offset = 0_u64;
    let mut push_region = |aspect_mask: vk::ImageAspectFlags,
                           plane_width: u32,
                           plane_height: u32,
                           bytes_per_texel: u64|
     -> Result<(), String> {
        let row_bytes = u64::from(plane_width)
            .checked_mul(bytes_per_texel)
            .ok_or_else(|| "AV1 decode readback row size overflowed u64".to_string())?;
        let plane_bytes = row_bytes
            .checked_mul(u64::from(plane_height))
            .ok_or_else(|| "AV1 decode readback plane size overflowed u64".to_string())?;
        let offset = align_av1_decode_readback_offset(next_offset, 4);
        regions.push(
            vk::BufferImageCopy::default()
                .buffer_offset(offset)
                .buffer_row_length(plane_width)
                .buffer_image_height(plane_height)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_offset(vk::Offset3D::default())
                .image_extent(vk::Extent3D {
                    width: plane_width,
                    height: plane_height,
                    depth: 1,
                }),
        );
        next_offset = offset
            .checked_add(plane_bytes)
            .ok_or_else(|| "AV1 decode readback buffer size overflowed u64".to_string())?;
        Ok(())
    };

    match format {
        vk::Format::G8_B8R8_2PLANE_420_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(
                vk::ImageAspectFlags::PLANE_1,
                coded_width.div_ceil(2),
                coded_height.div_ceil(2),
                2,
            )?;
        }
        other => {
            return Err(format!(
                "AV1 decode readback is not implemented for output format {other:?}"
            ));
        }
    }

    Ok(Av1DecodeReadbackPlan {
        buffer_size: next_offset,
        regions,
    })
}

fn align_av1_decode_readback_offset(value: u64, alignment: u64) -> u64 {
    if alignment <= 1 {
        return value;
    }
    value.div_ceil(alignment) * alignment
}

fn create_av1_decode_session_parameters(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_session: vk::VideoSessionKHR,
    std_sequence_header: &StdVideoAV1SequenceHeader,
) -> Result<vk::VideoSessionParametersKHR, String> {
    let color_config = default_av1_main_420_8bit_color_config();
    let timing_info = default_av1_timing_info();
    let mut std_sequence_header = *std_sequence_header;
    std_sequence_header.pColorConfig = &color_config;
    std_sequence_header.pTimingInfo = &timing_info;
    let mut decode_av1_session_parameters =
        vk::VideoDecodeAV1SessionParametersCreateInfoKHR::default()
            .std_sequence_header(&std_sequence_header);
    let create_info = vk::VideoSessionParametersCreateInfoKHR::default()
        .video_session(video_session)
        .video_session_parameters_template(vk::VideoSessionParametersKHR::null())
        .push_next(&mut decode_av1_session_parameters);
    let mut video_session_parameters = vk::VideoSessionParametersKHR::null();

    // SAFETY: `create_info` references stack data alive for this call.
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
            "vkCreateVideoSessionParametersKHR for AV1 decode failed: {result:?}"
        ));
    }

    Ok(video_session_parameters)
}

fn default_av1_timing_info() -> StdVideoAV1TimingInfo {
    StdVideoAV1TimingInfo {
        flags: StdVideoAV1TimingInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoAV1TimingInfoFlags::new_bitfield_1(0, 0),
        },
        num_units_in_display_tick: 0,
        time_scale: 0,
        num_ticks_per_picture_minus_1: 0,
    }
}

fn default_av1_main_420_8bit_color_config() -> StdVideoAV1ColorConfig {
    StdVideoAV1ColorConfig {
        flags: StdVideoAV1ColorConfigFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoAV1ColorConfigFlags::new_bitfield_1(0, 0, 0, 0, 0),
        },
        BitDepth: 8,
        subsampling_x: 1,
        subsampling_y: 1,
        reserved1: 0,
        color_primaries: StdVideoAV1ColorPrimaries_STD_VIDEO_AV1_COLOR_PRIMARIES_BT_UNSPECIFIED,
        transfer_characteristics:
            StdVideoAV1TransferCharacteristics_STD_VIDEO_AV1_TRANSFER_CHARACTERISTICS_UNSPECIFIED,
        matrix_coefficients:
            StdVideoAV1MatrixCoefficients_STD_VIDEO_AV1_MATRIX_COEFFICIENTS_UNSPECIFIED,
        chroma_sample_position:
            StdVideoAV1ChromaSamplePosition_STD_VIDEO_AV1_CHROMA_SAMPLE_POSITION_UNKNOWN,
    }
}

fn destroy_av1_decode_session_resource(
    instance: &ash::Instance,
    device: &ash::Device,
    mut resource: Av1DecodeSessionResource,
) {
    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    // SAFETY: The handles were created by this device and are no longer used.
    unsafe {
        (video_queue_device.fp().destroy_video_session_parameters_khr)(
            device.handle(),
            resource.parameters,
            std::ptr::null(),
        );
        for memory in resource.memories.drain(..) {
            device.free_memory(memory, None);
        }
        (video_queue_device.fp().destroy_video_session_khr)(
            device.handle(),
            resource.session,
            std::ptr::null(),
        );
    }
}

fn build_probe_av1_std_sequence_header(
    coded_width: u32,
    coded_height: u32,
) -> Result<StdVideoAV1SequenceHeader, String> {
    if coded_width == 0 || coded_height == 0 {
        return Err(format!(
            "AV1 probe sequence header dimensions must be positive, got {coded_width}x{coded_height}"
        ));
    }
    let width_minus_1 = coded_width - 1;
    let height_minus_1 = coded_height - 1;
    let frame_width_bits_minus_1 = u8::try_from(31 - width_minus_1.leading_zeros())
        .map_err(|_| "AV1 probe width bit count does not fit in u8".to_string())?;
    let frame_height_bits_minus_1 = u8::try_from(31 - height_minus_1.leading_zeros())
        .map_err(|_| "AV1 probe height bit count does not fit in u8".to_string())?;
    build_av1_std_sequence_header(&ParsedAv1SequenceHeader {
        seq_profile: 0,
        still_picture: true,
        reduced_still_picture_header: true,
        frame_width_bits_minus_1,
        frame_height_bits_minus_1,
        max_frame_width_minus_1: width_minus_1,
        max_frame_height_minus_1: height_minus_1,
        use_128x128_superblock: false,
        enable_filter_intra: false,
        enable_intra_edge_filter: false,
        enable_interintra_compound: false,
        enable_masked_compound: false,
        enable_warped_motion: false,
        enable_dual_filter: false,
        enable_order_hint: false,
        enable_jnt_comp: false,
        enable_ref_frame_mvs: false,
        frame_id_numbers_present_flag: false,
        enable_superres: false,
        enable_cdef: false,
        enable_restoration: false,
        film_grain_params_present: false,
        timing_info_present_flag: false,
        initial_display_delay_present_flag: false,
        order_hint_bits_minus_1: 0,
        seq_force_screen_content_tools: AV1_SELECT_SCREEN_CONTENT_TOOLS,
        seq_force_integer_mv: AV1_SELECT_INTEGER_MV,
    })
}

fn preferred_av1_probe_extent(min: u32, max: u32) -> u32 {
    16.clamp(min, max)
}

fn create_av1_decode_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
) -> Result<ash::Device, String> {
    let priorities = [1.0_f32];
    let queue_create_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family_index)
        .queue_priorities(&priorities);
    let extension_names = [
        vk::KHR_VIDEO_QUEUE_NAME.as_ptr(),
        vk::KHR_VIDEO_DECODE_QUEUE_NAME.as_ptr(),
        vk::KHR_VIDEO_DECODE_AV1_NAME.as_ptr(),
        vk::KHR_SYNCHRONIZATION2_NAME.as_ptr(),
    ];
    let mut synchronization2_features =
        vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true);
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(std::slice::from_ref(&queue_create_info))
        .enabled_extension_names(&extension_names)
        .push_next(&mut synchronization2_features);

    // SAFETY: All pointers referenced by `create_info` live until the call returns.
    unsafe { instance.create_device(physical_device, &create_info, None) }
        .map_err(|err| format!("logical device initialization failed: {err}"))
}

fn parse_av1_low_overhead_obus(bitstream: &[u8]) -> Result<Vec<Av1ObuRecord>, String> {
    if bitstream.starts_with(b"DKIF") {
        return Err(
            "AV1 IVF container input is not a low-overhead OBU elementary stream".to_string(),
        );
    }

    let mut records = Vec::new();
    let mut cursor = 0usize;
    let mut temporal_unit_index = 0usize;
    let mut current_temporal_unit_has_payload = false;

    while cursor < bitstream.len() {
        let obu_start = cursor;
        let header = bitstream[cursor];
        cursor += 1;

        if header & 0x80 != 0 {
            return Err(format!(
                "AV1 OBU at offset {obu_start} sets obu_forbidden_bit"
            ));
        }
        if header & 0x01 != 0 {
            return Err(format!("AV1 OBU at offset {obu_start} sets reserved bit"));
        }

        let obu_type = av1_obu_type((header >> 3) & 0x0f);
        let has_extension = header & 0x04 != 0;
        let has_size_field = header & 0x02 != 0;
        if has_extension {
            if cursor >= bitstream.len() {
                return Err(format!(
                    "AV1 OBU at offset {obu_start} is truncated before extension header"
                ));
            }
            cursor += 1;
        }
        if !has_size_field {
            return Err(format!(
                "AV1 OBU at offset {obu_start} lacks the low-overhead size field"
            ));
        }

        let (payload_len, leb_len) = read_av1_leb128(&bitstream[cursor..])
            .map_err(|err| format!("AV1 OBU at offset {obu_start} has invalid size: {err}"))?;
        cursor += leb_len;
        let payload_start = cursor;
        let payload_end = payload_start
            .checked_add(payload_len)
            .ok_or_else(|| format!("AV1 OBU at offset {obu_start} size overflows usize"))?;
        if payload_end > bitstream.len() {
            return Err(format!(
                "AV1 OBU at offset {obu_start} is truncated: payload_end={payload_end}, len={}",
                bitstream.len()
            ));
        }

        if obu_type == Av1ObuType::TemporalDelimiter && current_temporal_unit_has_payload {
            temporal_unit_index = temporal_unit_index.saturating_add(1);
            current_temporal_unit_has_payload = false;
        }
        records.push(Av1ObuRecord {
            obu_type,
            obu_range: obu_start..payload_end,
            payload_range: payload_start..payload_end,
            temporal_unit_index,
        });
        if obu_type != Av1ObuType::TemporalDelimiter {
            current_temporal_unit_has_payload = true;
        }
        cursor = payload_end;
    }

    if records.is_empty() {
        return Err("AV1 bitstream contains no OBUs".to_string());
    }

    Ok(records)
}

fn parse_av1_sequence_header_payload(payload: &[u8]) -> Result<ParsedAv1SequenceHeader, String> {
    let mut bits = BitReader::new(payload);
    let seq_profile = bits.read_bits_u8(3, "seq_profile")?;
    let still_picture = bits.read_bool("still_picture")?;
    let reduced_still_picture_header = bits.read_bool("reduced_still_picture_header")?;

    let mut timing_info_present_flag = false;
    let mut initial_display_delay_present_flag = false;
    if reduced_still_picture_header {
        let _seq_level_idx_0 = bits.read_bits_u8(5, "seq_level_idx_0")?;
    } else {
        let flags = skip_av1_operating_points(&mut bits)?;
        timing_info_present_flag = flags.timing_info_present_flag;
        initial_display_delay_present_flag = flags.initial_display_delay_present_flag;
    }

    let frame_width_bits_minus_1 = bits.read_bits_u8(4, "frame_width_bits_minus_1")?;
    let frame_height_bits_minus_1 = bits.read_bits_u8(4, "frame_height_bits_minus_1")?;
    let max_frame_width_minus_1 = bits.read_bits_u32(
        usize::from(frame_width_bits_minus_1) + 1,
        "max_frame_width_minus_1",
    )?;
    let max_frame_height_minus_1 = bits.read_bits_u32(
        usize::from(frame_height_bits_minus_1) + 1,
        "max_frame_height_minus_1",
    )?;

    let mut frame_id_numbers_present_flag = false;
    if !reduced_still_picture_header {
        frame_id_numbers_present_flag = bits.read_bool("frame_id_numbers_present_flag")?;
        if frame_id_numbers_present_flag {
            let _delta_frame_id_length_minus_2 =
                bits.read_bits_u8(4, "delta_frame_id_length_minus_2")?;
            let _additional_frame_id_length_minus_1 =
                bits.read_bits_u8(3, "additional_frame_id_length_minus_1")?;
        }
    }

    let use_128x128_superblock = bits.read_bool("use_128x128_superblock")?;
    let enable_filter_intra = bits.read_bool("enable_filter_intra")?;
    let enable_intra_edge_filter = bits.read_bool("enable_intra_edge_filter")?;
    let (
        enable_interintra_compound,
        enable_masked_compound,
        enable_warped_motion,
        enable_dual_filter,
        enable_order_hint,
        enable_jnt_comp,
        enable_ref_frame_mvs,
        order_hint_bits_minus_1,
        seq_force_screen_content_tools,
        seq_force_integer_mv,
        enable_superres,
        enable_cdef,
        enable_restoration,
    ) = if reduced_still_picture_header {
        (
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            0,
            AV1_SELECT_SCREEN_CONTENT_TOOLS,
            AV1_SELECT_INTEGER_MV,
            false,
            false,
            false,
        )
    } else {
        let enable_interintra_compound = bits.read_bool("enable_interintra_compound")?;
        let enable_masked_compound = bits.read_bool("enable_masked_compound")?;
        let enable_warped_motion = bits.read_bool("enable_warped_motion")?;
        let enable_dual_filter = bits.read_bool("enable_dual_filter")?;
        let enable_order_hint = bits.read_bool("enable_order_hint")?;
        let (enable_jnt_comp, enable_ref_frame_mvs, order_hint_bits_minus_1) = if enable_order_hint
        {
            (
                bits.read_bool("enable_jnt_comp")?,
                bits.read_bool("enable_ref_frame_mvs")?,
                bits.read_bits_u8(3, "order_hint_bits_minus_1")?,
            )
        } else {
            (false, false, 0)
        };
        let seq_force_screen_content_tools = if bits.read_bool("seq_choose_screen_content_tools")? {
            AV1_SELECT_SCREEN_CONTENT_TOOLS
        } else {
            bits.read_bits_u8(1, "seq_force_screen_content_tools")?
        };
        let seq_force_integer_mv =
            if seq_force_screen_content_tools > 0 && bits.read_bool("seq_choose_integer_mv")? {
                AV1_SELECT_INTEGER_MV
            } else if seq_force_screen_content_tools > 0 {
                bits.read_bits_u8(1, "seq_force_integer_mv")?
            } else {
                0
            };
        let enable_superres = bits.read_bool("enable_superres")?;
        let enable_cdef = bits.read_bool("enable_cdef")?;
        let enable_restoration = bits.read_bool("enable_restoration")?;
        (
            enable_interintra_compound,
            enable_masked_compound,
            enable_warped_motion,
            enable_dual_filter,
            enable_order_hint,
            enable_jnt_comp,
            enable_ref_frame_mvs,
            order_hint_bits_minus_1,
            seq_force_screen_content_tools,
            seq_force_integer_mv,
            enable_superres,
            enable_cdef,
            enable_restoration,
        )
    };
    let film_grain_params_present = if !reduced_still_picture_header {
        skip_av1_color_config_and_read_film_grain_flag(&mut bits, seq_profile)?
    } else {
        false
    };

    Ok(ParsedAv1SequenceHeader {
        seq_profile,
        still_picture,
        reduced_still_picture_header,
        frame_width_bits_minus_1,
        frame_height_bits_minus_1,
        max_frame_width_minus_1,
        max_frame_height_minus_1,
        use_128x128_superblock,
        enable_filter_intra,
        enable_intra_edge_filter,
        enable_interintra_compound,
        enable_masked_compound,
        enable_warped_motion,
        enable_dual_filter,
        enable_order_hint,
        enable_jnt_comp,
        enable_ref_frame_mvs,
        frame_id_numbers_present_flag,
        enable_superres,
        enable_cdef,
        enable_restoration,
        film_grain_params_present,
        timing_info_present_flag,
        initial_display_delay_present_flag,
        order_hint_bits_minus_1,
        seq_force_screen_content_tools,
        seq_force_integer_mv,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av1OperatingPointFlags {
    timing_info_present_flag: bool,
    initial_display_delay_present_flag: bool,
}

fn skip_av1_operating_points(bits: &mut BitReader<'_>) -> Result<Av1OperatingPointFlags, String> {
    let timing_info_present_flag = bits.read_bool("timing_info_present_flag")?;
    if timing_info_present_flag {
        let _num_units_in_display_tick = bits.read_bits_u32(32, "num_units_in_display_tick")?;
        let _time_scale = bits.read_bits_u32(32, "time_scale")?;
        let equal_picture_interval = bits.read_bool("equal_picture_interval")?;
        if equal_picture_interval {
            let _num_ticks_per_picture_minus_1 = bits.read_uvlc("num_ticks_per_picture_minus_1")?;
        }
        let decoder_model_info_present_flag = bits.read_bool("decoder_model_info_present_flag")?;
        if decoder_model_info_present_flag {
            let _buffer_delay_length_minus_1 =
                bits.read_bits_u8(5, "buffer_delay_length_minus_1")?;
            let _num_units_in_decoding_tick =
                bits.read_bits_u32(32, "num_units_in_decoding_tick")?;
            let _buffer_removal_time_length_minus_1 =
                bits.read_bits_u8(5, "buffer_removal_time_length_minus_1")?;
            let _frame_presentation_time_length_minus_1 =
                bits.read_bits_u8(5, "frame_presentation_time_length_minus_1")?;
        }
    }

    let initial_display_delay_present_flag =
        bits.read_bool("initial_display_delay_present_flag")?;
    let operating_points_cnt_minus_1 = bits.read_bits_u8(5, "operating_points_cnt_minus_1")?;
    for _ in 0..=operating_points_cnt_minus_1 {
        let _operating_point_idc = bits.read_bits_u16(12, "operating_point_idc")?;
        let seq_level_idx = bits.read_bits_u8(5, "seq_level_idx")?;
        if seq_level_idx > 7 {
            let _seq_tier = bits.read_bool("seq_tier")?;
        }
        if initial_display_delay_present_flag {
            let initial_display_delay_present_for_this_op =
                bits.read_bool("initial_display_delay_present_for_this_op")?;
            if initial_display_delay_present_for_this_op {
                let _initial_display_delay_minus_1 =
                    bits.read_bits_u8(4, "initial_display_delay_minus_1")?;
            }
        }
    }

    Ok(Av1OperatingPointFlags {
        timing_info_present_flag,
        initial_display_delay_present_flag,
    })
}

fn skip_av1_color_config_and_read_film_grain_flag(
    bits: &mut BitReader<'_>,
    seq_profile: u8,
) -> Result<bool, String> {
    let high_bitdepth = bits.read_bool("high_bitdepth")?;
    if seq_profile == 2 && high_bitdepth {
        let _twelve_bit = bits.read_bool("twelve_bit")?;
    }
    let mono_chrome = if seq_profile == 1 {
        false
    } else {
        bits.read_bool("mono_chrome")?
    };
    let color_description_present_flag = bits.read_bool("color_description_present_flag")?;
    if color_description_present_flag {
        let _color_primaries = bits.read_bits_u8(8, "color_primaries")?;
        let _transfer_characteristics = bits.read_bits_u8(8, "transfer_characteristics")?;
        let _matrix_coefficients = bits.read_bits_u8(8, "matrix_coefficients")?;
    }
    let _color_range = bits.read_bool("color_range")?;
    if !mono_chrome {
        if seq_profile == 0 {
            let _chroma_sample_position = bits.read_bits_u8(2, "chroma_sample_position")?;
        }
        let _separate_uv_delta_q = bits.read_bool("separate_uv_delta_q")?;
    }
    bits.read_bool("film_grain_params_present")
}

fn read_av1_leb128(bytes: &[u8]) -> Result<(usize, usize), String> {
    let mut value = 0u64;
    for (index, byte) in bytes.iter().copied().take(8).enumerate() {
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            let value = usize::try_from(value)
                .map_err(|_| "LEB128 value does not fit in usize".to_string())?;
            return Ok((value, index + 1));
        }
    }
    if bytes.len() < 8 {
        Err("truncated LEB128 value".to_string())
    } else {
        Err("LEB128 value exceeds AV1 8-byte limit".to_string())
    }
}

fn av1_obu_type(raw: u8) -> Av1ObuType {
    match raw {
        1 => Av1ObuType::SequenceHeader,
        2 => Av1ObuType::TemporalDelimiter,
        3 => Av1ObuType::FrameHeader,
        4 => Av1ObuType::TileGroup,
        5 => Av1ObuType::Metadata,
        6 => Av1ObuType::Frame,
        7 => Av1ObuType::RedundantFrameHeader,
        8 => Av1ObuType::TileList,
        15 => Av1ObuType::Padding,
        other => Av1ObuType::Unknown(other),
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_offset: 0,
        }
    }

    fn read_bool(&mut self, field_name: &str) -> Result<bool, String> {
        Ok(self.read_bits_u8(1, field_name)? != 0)
    }

    fn read_bits_u8(&mut self, count: usize, field_name: &str) -> Result<u8, String> {
        u8::try_from(self.read_bits(count, field_name)?)
            .map_err(|_| format!("{field_name} does not fit in u8"))
    }

    fn read_bits_u16(&mut self, count: usize, field_name: &str) -> Result<u16, String> {
        u16::try_from(self.read_bits(count, field_name)?)
            .map_err(|_| format!("{field_name} does not fit in u16"))
    }

    fn read_bits_u32(&mut self, count: usize, field_name: &str) -> Result<u32, String> {
        u32::try_from(self.read_bits(count, field_name)?)
            .map_err(|_| format!("{field_name} does not fit in u32"))
    }

    fn read_bits(&mut self, count: usize, field_name: &str) -> Result<u64, String> {
        if count > 64 {
            return Err(format!("{field_name} bit count {count} exceeds 64"));
        }
        let mut value = 0u64;
        for _ in 0..count {
            let byte = self
                .data
                .get(self.bit_offset / 8)
                .ok_or_else(|| format!("sequence header ended while reading {field_name}"))?;
            let bit = (byte >> (7 - (self.bit_offset % 8))) & 1;
            value = (value << 1) | u64::from(bit);
            self.bit_offset = self
                .bit_offset
                .checked_add(1)
                .ok_or_else(|| "bit offset overflow".to_string())?;
        }
        Ok(value)
    }

    fn read_uvlc(&mut self, field_name: &str) -> Result<u64, String> {
        let mut leading_zeroes = 0usize;
        while !self.read_bool(field_name)? {
            leading_zeroes = leading_zeroes
                .checked_add(1)
                .ok_or_else(|| format!("{field_name} leading-zero count overflow"))?;
        }
        if leading_zeroes >= 63 {
            return Err(format!("{field_name} UVLC value exceeds u64 range"));
        }
        let suffix = if leading_zeroes == 0 {
            0
        } else {
            self.read_bits(leading_zeroes, field_name)?
        };
        Ok((1u64 << leading_zeroes) - 1 + suffix)
    }

    fn byte_offset(&self) -> usize {
        self.bit_offset.div_ceil(8)
    }

    fn align_to_next_byte_with_zero_bits(&mut self, field_name: &str) -> Result<(), String> {
        while !self.bit_offset.is_multiple_of(8) {
            if self.read_bool(field_name)? {
                return Err(format!("{field_name} expected zero alignment bit"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(inspection.coded_width, Some(320));
        assert_eq!(inspection.coded_height, Some(180));
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
        assert_eq!(skeleton.frame_header_offset as usize, frame_obu_start + 2);
        assert_eq!(skeleton.tile_offsets, vec![skeleton.frame_header_offset]);
        assert_eq!(skeleton.tile_sizes, vec![3]);
    }

    #[test]
    fn key_frame_obu_tile_payload_offset_skips_libaom_header() {
        let payload = [
            0x14, 0x00, 0x24, 0x00, 0x03, 0x8e, 0x69, 0xa2, 0x90, 0xae, 0xb0, 0x28, 0xdb, 0x5c,
        ];
        let header = parse_av1_key_frame_obu_header(&payload)
            .expect("libaom key-frame header should be parsed");
        assert_eq!(header.tile_payload_offset, 12);
        assert_eq!(header.base_q_idx, 128);
        assert_eq!(header.loop_filter_level, [7, 7, 13, 13]);
        assert_eq!(header.cdef_bits, 1);
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
            frame_header_obu_start + 2
        );
        assert_eq!(
            skeleton.tile_offsets,
            vec![(tile_group_obu_start + 2) as u32]
        );
        assert_eq!(skeleton.tile_sizes, vec![3]);
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
            frame_header_obu_start + 2
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
        assert_eq!(
            submits[0].frame_header_offset as usize,
            first_frame_start + 2
        );
        assert_eq!(submits[0].tile_sizes, vec![2]);
        assert_eq!(submits[1].temporal_unit_index, 1);
        assert_eq!(
            submits[1].frame_header_offset as usize,
            second_frame_start + 2
        );
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
            key_frame_header: None,
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
    fn picture_info_skeleton_rejects_empty_tiles() {
        let submit = Av1DecodeSubmitSkeleton {
            temporal_unit_index: 0,
            frame_header_offset: 12,
            tile_offsets: Vec::new(),
            tile_sizes: Vec::new(),
            key_frame_header: None,
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

        assert_eq!(decode.src_buffer_offset as usize, frame_obu_start + 2);
        assert_eq!(decode.src_buffer_range, 3);
        assert_eq!(decode.coded_width, 320);
        assert_eq!(decode.coded_height, 180);
        assert_eq!(
            u32::from(decode.setup_reference_info.frame_type),
            StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY
        );
        assert_eq!(decode.setup_reference_info.OrderHint, 0);
        assert_eq!(decode.setup_reference_info.SavedOrderHints, [0; 8]);
        assert_eq!(decode.picture_info.frame_header_offset, 0);
        assert_eq!(decode.picture_info.tile_offsets, vec![0]);
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
        let source_start = frame_header_obu_start + 2;
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
        assert_eq!(decodes[0].src_buffer_offset as usize, first_frame_start + 2);
        assert_eq!(decodes[0].src_buffer_range, 2);
        assert_eq!(decodes[0].temporal_unit_index, 0);
        assert_eq!(decodes[0].coded_width, 320);
        assert_eq!(decodes[0].coded_height, 180);
        assert_eq!(decodes[0].picture_info.frame_header_offset, 0);
        assert_eq!(
            decodes[1].src_buffer_offset as usize,
            second_frame_start + 2
        );
        assert_eq!(decodes[1].src_buffer_range, 3);
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
        assert_eq!(image_plan.array_layers, 2);
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
        assert_eq!(image_create_info.array_layers, 2);
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
            vk::ImageLayout::VIDEO_DECODE_DST_KHR
        );
        assert_eq!(
            image_barrier.dst_stage_mask,
            vk::PipelineStageFlags2::VIDEO_DECODE_KHR
        );
        assert_eq!(
            image_barrier.dst_access_mask,
            vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR
        );
        assert_eq!(image_barrier.subresource_range.layer_count, 2);
        let readback_plan = command
            .decode_readback_plan(vk::Format::G8_B8R8_2PLANE_420_UNORM)
            .expect("NV12 readback plan should be built");
        assert_eq!(readback_plan.buffer_size, 320 * 180 + 320 * 90);
        assert_eq!(readback_plan.regions.len(), 2);
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
        assert_eq!(frame_resources[2].base_array_layer, 0);
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
                    dst_base_array_layer: 0,
                    src_buffer_offset: command.frames[0].src_buffer_offset,
                    src_buffer_range: 1,
                    tile_count: 1,
                },
                Av1DecodeFrameRecordBundle {
                    frame_index: 1,
                    temporal_unit_index: 1,
                    decode_info_index: 1,
                    setup_slot_index: 1,
                    dst_base_array_layer: 1,
                    src_buffer_offset: command.frames[1].src_buffer_offset,
                    src_buffer_range: 2,
                    tile_count: 1,
                },
                Av1DecodeFrameRecordBundle {
                    frame_index: 2,
                    temporal_unit_index: 2,
                    decode_info_index: 2,
                    setup_slot_index: 0,
                    dst_base_array_layer: 0,
                    src_buffer_offset: command.frames[2].src_buffer_offset,
                    src_buffer_range: 3,
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
        let begin_dpb_info_ptrs = begin_dpb_infos
            .iter()
            .map(|info| info as *const vk::VideoDecodeAV1DpbSlotInfoKHR<'_>)
            .collect::<Vec<_>>();
        let begin_reference_slots = command
            .begin_reference_slots(&begin_resources, &mut begin_dpb_infos)
            .expect("matching begin resources and DPB infos should produce reference slots");
        assert_eq!(begin_reference_slots.len(), 2);
        assert_eq!(begin_reference_slots[0].slot_index, -1);
        assert_eq!(begin_reference_slots[1].slot_index, -1);
        assert_eq!(
            begin_reference_slots[0].p_picture_resource,
            &raw const begin_resources[0]
        );
        assert_eq!(
            begin_reference_slots[1]
                .p_next
                .cast::<vk::VideoDecodeAV1DpbSlotInfoKHR<'_>>(),
            begin_dpb_info_ptrs[1]
        );

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
        assert_eq!(begin_coding_info.reference_slot_count, 2);
        assert_eq!(
            begin_coding_info.p_reference_slots,
            begin_reference_slots.as_ptr()
        );

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
                    reference_slot_count: 2,
                },
                Av1DecodeRecordStep::ResetCoding,
                Av1DecodeRecordStep::DecodeFrame {
                    frame_index: 0,
                    temporal_unit_index: 0,
                    setup_slot_index: 0,
                    src_buffer_offset: command.frames[0].src_buffer_offset,
                    src_buffer_range: 1,
                    tile_count: 1,
                },
                Av1DecodeRecordStep::DecodeFrame {
                    frame_index: 1,
                    temporal_unit_index: 1,
                    setup_slot_index: 1,
                    src_buffer_offset: command.frames[1].src_buffer_offset,
                    src_buffer_range: 2,
                    tile_count: 1,
                },
                Av1DecodeRecordStep::DecodeFrame {
                    frame_index: 2,
                    temporal_unit_index: 2,
                    setup_slot_index: 0,
                    src_buffer_offset: command.frames[2].src_buffer_offset,
                    src_buffer_range: 3,
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

        let plan = build_av1_aligned_decode_bitstream_upload_plan(&bitstream, 8, 4)
            .expect("aligned upload plan should be built for multi-frame AV1");

        assert_eq!(plan.decodes.len(), 2);
        assert_eq!(plan.decodes[0].src_buffer_offset, 0);
        assert_eq!(plan.decodes[0].src_buffer_range, 4);
        assert_eq!(plan.decodes[1].src_buffer_offset, 8);
        assert_eq!(plan.decodes[1].src_buffer_range, 4);
        assert_eq!(&plan.bytes[0..2], &[0x11, 0x12]);
        assert_eq!(&plan.bytes[2..8], &[0; 6]);
        assert_eq!(&plan.bytes[8..11], &[0x21, 0x22, 0x23]);
        assert_eq!(plan.bytes[11], 0);
        assert_eq!(plan.decodes[0].picture_info.tile_offsets, vec![0]);
        assert_eq!(plan.decodes[1].picture_info.tile_offsets, vec![0]);
    }

    #[test]
    fn aligned_key_frame_decode_command_uses_upload_offsets() {
        let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
        bitstream.extend_from_slice(&make_obu(6, &[0x11, 0x12]));
        bitstream.extend_from_slice(&make_obu(2, &[]));
        bitstream.extend_from_slice(&make_obu(6, &[0x21, 0x22, 0x23]));

        let (plan, command) =
            build_av1_aligned_key_frame_decode_command_skeleton(&bitstream, 2, 16, 8)
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
                    dst_base_array_layer: 1,
                    src_buffer_offset: 16,
                    src_buffer_range: 8,
                    tile_count: 1,
                    upload_range: 16..24,
                },
            ]
        );

        let summary = plan
            .with_frame_decode_info(
                &command,
                1,
                vk::Buffer::null(),
                vk::ImageView::null(),
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
                true,
                true,
                [true, true, true, true, true, true, true]
            )
        );

        let loop_summaries = plan
            .with_frame_decode_infos(
                &command,
                vk::Buffer::null(),
                vk::ImageView::null(),
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
            vk::ImageView::null(),
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
                "begin:2".to_string(),
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
                vk::ImageView::null(),
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
    fn frame_record_bundles_reject_slot_base_layer_mismatch() {
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
                src_buffer_offset: 12,
                src_buffer_range: 3,
                tile_count: 1,
            }],
        };

        let err = command
            .frame_picture_resources(vk::ImageView::null())
            .expect_err("slot/base layer mismatch should be rejected");

        assert!(err.contains("setup slot/base layer mismatch"));
    }

    #[test]
    fn begin_coding_info_rejects_mismatched_reference_slot_counts() {
        let mut bitstream = make_obu(1, &av1_reduced_still_sequence_header_payload(320, 180));
        bitstream.extend_from_slice(&make_obu(6, &[0x11]));
        let command = build_av1_key_frame_decode_command_skeleton(&bitstream, 2)
            .expect("single frame should produce a command skeleton");

        let err = command
            .vk_begin_coding_info(
                vk::VideoSessionKHR::null(),
                vk::VideoSessionParametersKHR::null(),
                &[],
            )
            .expect_err("reference slot count mismatch should be rejected");

        assert!(err.contains("begin reference slot count mismatch"));
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
        assert_eq!(vk_decode.src_buffer_offset as usize, frame_obu_start + 2);
        assert_eq!(vk_decode.src_buffer_range, 3);
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
            key_frame_header: None,
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
        let plan = build_av1_decode_readback_plan(vk::Format::G8_B8R8_2PLANE_420_UNORM, 641, 479)
            .expect("odd NV12 dimensions should produce a readback plan");

        assert_eq!(plan.regions.len(), 2);
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
    fn av1_decode_readback_plan_rejects_unsupported_format() {
        let err = build_av1_decode_readback_plan(vk::Format::D32_SFLOAT, 320, 180)
            .expect_err("unsupported readback format should be rejected");
        assert!(err.contains("not implemented"));
    }

    #[test]
    #[ignore = "live Vulkan AV1 command-buffer record probe; opt in explicitly"]
    fn live_av1_decode_command_record_probe_reports_status() {
        let bitstream = if let Ok(path) = std::env::var("VIDEO_HW_VULKAN_AV1_PROBE_BITSTREAM_PATH")
        {
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
                    "AV1 Vulkan command record probe: coded={}x{}, format={:?}, upload_bytes={}, image_layers={}, barrier_layers={}, readback_bytes={}, readback_regions={}, readback_mapped_bytes={}, readback_non_zero={}, readback_sample_len={}, record_decodes={}, command_buffer_recorded={}, command_buffer_submitted={}",
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
                    probe.command_buffer_submitted
                );
                if std::env::var("VIDEO_HW_VULKAN_AV1_RECORD_COMMAND_BUFFER").as_deref() == Ok("1")
                {
                    assert!(
                        probe.command_buffer_recorded,
                        "record probe env was set but command_buffer_recorded=false"
                    );
                }
                if std::env::var("VIDEO_HW_VULKAN_AV1_SUBMIT_COMMAND_BUFFER").as_deref() == Ok("1")
                {
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
}
