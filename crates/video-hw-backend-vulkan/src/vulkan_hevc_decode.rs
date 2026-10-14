//! HEVC Vulkan Video decode contracts and session-bootstrap orchestration.
//!
//! Syntax parsing, reference planning, submit resources, probes, and tests live in focused
//! child modules. This facade keeps the crate-visible API and shared ownership types together.

use std::collections::HashMap;
use std::ffi::CStr;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Cursor;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

use ash::vk;
use ash::vk::native::{
    StdVideoDecodeH265PictureInfo, StdVideoDecodeH265PictureInfoFlags,
    StdVideoDecodeH265ReferenceInfo, StdVideoDecodeH265ReferenceInfoFlags,
    StdVideoH265ChromaFormatIdc, StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_420,
    StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_422,
    StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_444,
    StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_MONOCHROME,
    StdVideoH265DecPicBufMgr, StdVideoH265HrdFlags, StdVideoH265HrdParameters,
    StdVideoH265LevelIdc, StdVideoH265LongTermRefPicsSps, StdVideoH265PictureParameterSet,
    StdVideoH265PpsFlags, StdVideoH265ProfileIdc,
    StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN, StdVideoH265ProfileTierLevel,
    StdVideoH265ProfileTierLevelFlags, StdVideoH265SequenceParameterSet,
    StdVideoH265SequenceParameterSetVui, StdVideoH265ShortTermRefPicSet,
    StdVideoH265ShortTermRefPicSetFlags, StdVideoH265SpsFlags, StdVideoH265SpsVuiFlags,
    StdVideoH265VideoParameterSet, StdVideoH265VpsFlags,
};
use scuffle_h265::{AspectRatioIdc, AspectRatioInfo, NALUnitType, SpsNALUnit, SpsRbsp};

mod bootstrap;
mod model;
mod parameter_sets;
mod probe;
mod references;
mod submit;
#[cfg(test)]
mod tests;

pub(crate) use parameter_sets::build_hevc_std_parameter_set_storage;
use parameter_sets::*;
use probe::*;
use references::*;
use submit::*;

#[cfg(test)]
pub(crate) use bootstrap::{estimate_hevc_access_unit_count, probe_hevc_decode_session_bootstrap};
pub(crate) use bootstrap::{
    extract_hevc_access_unit_headers, extract_hevc_parameter_sets_annexb,
    probe_hevc_decode_prerequisites,
    probe_hevc_decode_session_bootstrap_with_access_unit_limit_and_physical_device_index,
};
pub(crate) use model::{
    HevcAccessUnitHeader, HevcDecodePrerequisiteProbe, HevcDecodeSessionBootstrap,
    HevcDecodeSubmitExecutionProbe, HevcDecodeSubmitSkeleton, HevcDecodeSubmitSkeletonProbe,
    HevcParameterSets, HevcStdParameterSetStorage, HevcVideoSessionCreateProbe,
    HevcVideoSessionParametersCreateProbe,
};

use bootstrap::*;
use model::*;
