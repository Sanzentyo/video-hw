use std::ffi::CStr;
use std::ops::Range;
use std::sync::OnceLock;

use ash::vk;
use ash::vk::native::{
    StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY,
    StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_SWITCHABLE,
    StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_HIGH, StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN,
    StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_PROFESSIONAL, StdVideoAV1SequenceHeader,
    StdVideoAV1SequenceHeaderFlags, StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_SELECT,
    StdVideoDecodeAV1PictureInfo, StdVideoDecodeAV1PictureInfoFlags,
    StdVideoDecodeAV1ReferenceInfo, StdVideoDecodeAV1ReferenceInfoFlags,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av1DecodeSubmitSkeleton {
    pub temporal_unit_index: usize,
    pub frame_header_offset: u32,
    pub tile_offsets: Vec<u32>,
    pub tile_sizes: Vec<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct Av1DecodePictureInfoSkeleton {
    pub std_picture_info: StdVideoDecodeAV1PictureInfo,
    pub reference_name_slot_indices: [i32; vk::MAX_VIDEO_AV1_REFERENCES_PER_FRAME_KHR],
    pub frame_header_offset: u32,
    pub tile_offsets: Vec<u32>,
    pub tile_sizes: Vec<u32>,
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
        let mut av1_picture_info = decode.picture_info.vk_picture_info();
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

impl Av1DecodeCommandSkeleton {
    pub(crate) fn coded_extent(&self) -> vk::Extent2D {
        vk::Extent2D {
            width: self.coded_width,
            height: self.coded_height,
        }
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
        for ((slot, picture_resource), dpb_slot_info) in self
            .begin_slots
            .iter()
            .zip(picture_resources.iter())
            .zip(dpb_slot_infos.iter_mut())
        {
            slots.push(
                vk::VideoReferenceSlotInfoKHR::default()
                    .slot_index(slot.slot_index)
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
        &std_sequence_header,
        coded_width,
        coded_height,
    );

    // SAFETY: The instance was created in this function and is not used afterwards.
    unsafe {
        instance.destroy_instance(None);
    }
    result
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
            Av1ObuType::Frame => submits.push(build_av1_frame_obu_submit_skeleton(record)?),
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

    let begin_slots = (0..max_dpb_slots)
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
    frame_record: &Av1ObuRecord,
) -> Result<Av1DecodeSubmitSkeleton, String> {
    let frame_header_offset = u32::try_from(frame_record.payload_range.start)
        .map_err(|_| "AV1 frame header offset exceeds u32 range".to_string())?;
    let tile_offset = frame_header_offset;
    let tile_size = u32::try_from(frame_record.payload_range.len())
        .map_err(|_| "AV1 frame OBU payload size exceeds u32 range".to_string())?;
    if tile_size == 0 {
        return Err("AV1 frame OBU payload is empty".to_string());
    }
    Ok(Av1DecodeSubmitSkeleton {
        temporal_unit_index: frame_record.temporal_unit_index,
        frame_header_offset,
        tile_offsets: vec![tile_offset],
        tile_sizes: vec![tile_size],
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
    })
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
                0,
                0,
                0,
                0,
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
        order_hint_bits_minus_1: 0,
        seq_force_integer_mv: AV1_SELECT_INTEGER_MV,
        seq_force_screen_content_tools: AV1_SELECT_SCREEN_CONTENT_TOOLS,
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

    Ok(Av1AdapterDecodeSupport {
        extensions: flags,
        decode_queue_family_index,
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
    std_sequence_header: &StdVideoAV1SequenceHeader,
    coded_width: u32,
    coded_height: u32,
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
        let Some(queue_family_index) = support.decode_queue_family_index else {
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
        let result = create_and_destroy_av1_decode_session_parameters_with_header(
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
        );
        // SAFETY: The device is no longer used after this point.
        unsafe {
            device.destroy_device(None);
        }
        match result {
            Ok(summary) => {
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

    let parameters_result =
        query_av1_decode_session_memory_requirements(device, &video_queue_device, video_session)
            .and_then(|memory_plan| {
                let mut session_memories = bind_av1_decode_session_memory(
                    instance,
                    device,
                    &video_queue_device,
                    config.physical_device,
                    video_session,
                    &memory_plan.requirements,
                )?;
                let create_result = create_av1_decode_session_parameters(
                    device,
                    &video_queue_device,
                    video_session,
                    config.std_sequence_header,
                );
                let mut summary = memory_plan.summary;
                summary.memory_bound_count = session_memories.len();
                for memory in session_memories.drain(..) {
                    // SAFETY: These allocations were created by this device in this scope and
                    // are no longer used after the bootstrap probe.
                    unsafe { device.free_memory(memory, None) };
                }
                create_result.map(|()| summary)
            });

    // SAFETY: `video_session` was created by this device and is no longer used.
    unsafe {
        (video_queue_device.fp().destroy_video_session_khr)(
            device.handle(),
            video_session,
            std::ptr::null(),
        );
    }
    parameters_result
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

fn create_av1_decode_session_parameters(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_session: vk::VideoSessionKHR,
    std_sequence_header: &StdVideoAV1SequenceHeader,
) -> Result<(), String> {
    let mut decode_av1_session_parameters =
        vk::VideoDecodeAV1SessionParametersCreateInfoKHR::default()
            .std_sequence_header(std_sequence_header);
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

    // SAFETY: `video_session_parameters` was created by this device and is no longer used.
    unsafe {
        (video_queue_device.fp().destroy_video_session_parameters_khr)(
            device.handle(),
            video_session_parameters,
            std::ptr::null(),
        );
    }
    Ok(())
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

    if reduced_still_picture_header {
        let _seq_level_idx_0 = bits.read_bits_u8(5, "seq_level_idx_0")?;
    } else {
        skip_av1_operating_points(&mut bits)?;
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

    if !reduced_still_picture_header {
        let frame_id_numbers_present_flag = bits.read_bool("frame_id_numbers_present_flag")?;
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
    })
}

fn skip_av1_operating_points(bits: &mut BitReader<'_>) -> Result<(), String> {
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

    Ok(())
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
        assert_eq!(begin_reference_slots[0].slot_index, 0);
        assert_eq!(begin_reference_slots[1].slot_index, 1);
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
                    (
                        bundle.frame_index,
                        decode_info.src_buffer_offset,
                        decode_info.src_buffer_range,
                        decode_info.dst_picture_resource.base_array_layer,
                        !decode_info.p_next.is_null(),
                        !decode_info.p_setup_reference_slot.is_null(),
                    )
                },
            )
            .expect("frame decode info should be materialized inside the callback");
        assert_eq!(summary, (1, 16, 8, 1, true, true));

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
        assert_eq!(recorded, vec!["begin", "reset", "decode", "decode", "end"]);
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
