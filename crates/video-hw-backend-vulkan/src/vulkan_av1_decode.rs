//! AV1 syntax inspection, decode planning, Vulkan session ownership, and capability probes.
//!
//! Private child modules divide parser, planning, command-recording, and Vulkan resource
//! lifetimes while this module preserves the existing crate-visible API.

use std::ffi::CStr;
use std::ops::Range;
use std::sync::OnceLock;

use ash::vk;
use ash::vk::native::{
    StdVideoAV1CDEF, StdVideoAV1ChromaSamplePosition_STD_VIDEO_AV1_CHROMA_SAMPLE_POSITION_UNKNOWN,
    StdVideoAV1ColorConfig, StdVideoAV1ColorConfigFlags,
    StdVideoAV1ColorPrimaries_STD_VIDEO_AV1_COLOR_PRIMARIES_BT_UNSPECIFIED,
    StdVideoAV1FrameRestorationType_STD_VIDEO_AV1_FRAME_RESTORATION_TYPE_NONE,
    StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_INTER,
    StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_INTRA_ONLY,
    StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY,
    StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_SWITCH, StdVideoAV1GlobalMotion,
    StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_BILINEAR,
    StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_EIGHTTAP,
    StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_EIGHTTAP_SHARP,
    StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_EIGHTTAP_SMOOTH,
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
    StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_ONLY_4X4,
    StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_SELECT, StdVideoDecodeAV1PictureInfo,
    StdVideoDecodeAV1PictureInfoFlags, StdVideoDecodeAV1ReferenceInfo,
    StdVideoDecodeAV1ReferenceInfoFlags,
};
use oxideav_av1::{
    FrameHeader as OxideAv1FrameHeader, FrameType as OxideAv1FrameType,
    SequenceHeader as OxideAv1SequenceHeader,
    dpb::Dpb as OxideAv1Dpb,
    frame_header::{parse_frame_header_with_dpb, parse_frame_obu_with_dpb},
    frame_header_tail::{
        GmType as OxideAv1GmType, LoopRestorationParams as OxideAv1LoopRestorationParams,
        RESTORATION_SGR, RESTORATION_SWITCHABLE, RESTORATION_WIENER, TxMode as OxideAv1TxMode,
    },
};

mod command;
mod planning;
mod probe;
mod session;
mod syntax;

#[cfg(test)]
mod tests;

use command::*;
use planning::*;
use probe::*;
use session::*;
use syntax::*;

pub(crate) use command::{
    Av1BeginCodingSlotSkeleton, Av1BitstreamInspection, Av1DecodeBitstreamSessionProbe,
    Av1DecodeBitstreamUploadPlan, Av1DecodeCommandRecordSummary, Av1DecodeCommandSkeleton,
    Av1DecodeCommandVisit, Av1DecodeFrameCommandSkeleton, Av1DecodeImagePlan,
    Av1DecodeInfoSkeleton, Av1DecodePictureInfoSkeleton, Av1DecodePrerequisiteProbe,
    Av1DecodeReadbackFrame, Av1DecodeReadbackPlan, Av1DecodeSubmitSkeleton,
    Av1ParsedFrameHeaderSummary, Av1ParsedKeyFrameHeader, Av1StdPictureOverrides,
};
pub(crate) use planning::{
    build_av1_aligned_decode_bitstream_upload_plan,
    build_av1_aligned_key_frame_decode_command_skeleton, build_av1_decode_info_skeleton,
    build_av1_decode_info_skeletons, build_av1_decode_picture_info_skeleton,
    build_av1_decode_submit_skeleton, build_av1_key_frame_decode_command_skeleton,
};
pub(crate) use probe::{
    av1_display_readback_indices, count_av1_display_frames, decode_av1_bitstream_to_nv12_frames,
    extract_av1_std_sequence_header, inspect_av1_low_overhead_obus, probe_av1_decode_prerequisites,
    probe_av1_decode_session_parameters_for_bitstream, submit_av1_bitstream_without_readback,
};
pub(crate) use session::Av1DecodePictureViews;
pub(crate) use syntax::ParsedAv1SequenceHeader;
