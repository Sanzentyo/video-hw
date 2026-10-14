//! Experimental Vulkan HEVC encode facade and session-probe orchestration.
//!
//! Syntax planning, diagnostic configuration, session parameters, submission, and resource
//! ownership live in responsibility modules. Raw Vulkan lifetimes remain local to those modules.
//!
//! The complete reference-frame/GOP path remains experimental.
#![allow(dead_code)]

use std::ffi::{CStr, c_void};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use ash::vk;
use ash::vk::native::{
    StdVideoEncodeH265LongTermRefPics, StdVideoEncodeH265PictureInfo,
    StdVideoEncodeH265PictureInfoFlags, StdVideoEncodeH265ReferenceInfo,
    StdVideoEncodeH265ReferenceInfoFlags, StdVideoEncodeH265ReferenceListsInfo,
    StdVideoEncodeH265ReferenceListsInfoFlags, StdVideoEncodeH265SliceSegmentHeader,
    StdVideoEncodeH265SliceSegmentHeaderFlags, StdVideoH265LevelIdc,
    StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_I,
    StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR,
    StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_P,
    StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN, StdVideoH265ShortTermRefPicSet,
    StdVideoH265SliceType_STD_VIDEO_H265_SLICE_TYPE_I,
    StdVideoH265SliceType_STD_VIDEO_H265_SLICE_TYPE_P,
};

use crate::vulkan_hevc_decode::{
    HevcParameterSets, HevcStdParameterSetStorage, build_hevc_std_parameter_set_storage,
    extract_hevc_parameter_sets_annexb,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HevcEncodePrerequisiteProbe {
    Ready,
    MissingExtensions { missing: Vec<&'static str> },
    MissingEncodeQueueFamily,
    NoCompatibleAdapter,
    DeviceInitializationFailed(String),
    ProbeUnavailable(String),
}

#[derive(Debug, Clone)]
pub(crate) struct HevcEncodeSessionBootstrap {
    pub coded_width: u32,
    pub coded_height: u32,
    pub adapter_name: String,
    pub adapter_vendor_id: u32,
    pub adapter_device_id: u32,
    pub adapter_driver_version: u32,
    pub adapter_api_version: u32,
    pub min_coded_width: u32,
    pub min_coded_height: u32,
    pub max_coded_width: u32,
    pub max_coded_height: u32,
    pub picture_access_granularity_width: u32,
    pub picture_access_granularity_height: u32,
    pub encode_input_granularity_width: u32,
    pub encode_input_granularity_height: u32,
    pub coded_extent_input_granularity_aligned: bool,
    pub max_dpb_slots: u32,
    pub max_active_reference_pictures: u32,
    pub rate_control_modes: vk::VideoEncodeRateControlModeFlagsKHR,
    pub max_rate_control_layers: u32,
    pub max_bitrate: u64,
    pub max_quality_levels: u32,
    pub encode_capability_flags: vk::VideoEncodeCapabilityFlagsKHR,
    pub encode_h265_capability_flags: vk::VideoEncodeH265CapabilityFlagsKHR,
    pub encode_h265_std_syntax_flags: vk::VideoEncodeH265StdFlagsKHR,
    pub supported_encode_feedback_flags: vk::VideoEncodeFeedbackFlagsKHR,
    pub min_bitstream_buffer_offset_alignment: u64,
    pub min_bitstream_buffer_size_alignment: u64,
    pub max_level_idc: u32,
    pub video_maintenance1_mode: &'static str,
    pub video_maintenance1_extension_available: bool,
    pub video_maintenance1_feature_supported: bool,
    pub video_maintenance1_feature_enabled: bool,
    pub encode_input_formats: Vec<vk::Format>,
    pub encode_dpb_formats: Vec<vk::Format>,
    pub video_session_create_probe: HevcEncodeVideoSessionCreateProbe,
    pub video_session_parameters_create_probe: HevcEncodeVideoSessionParametersCreateProbe,
    pub encode_submit_execution_probe: HevcEncodeSubmitExecutionProbe,
}

#[derive(Debug, Clone)]
pub(crate) enum HevcEncodeVideoSessionCreateProbe {
    Created,
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) enum HevcEncodeVideoSessionParametersCreateProbe {
    Created,
    Failed(String),
    Skipped(String),
}

#[derive(Debug, Clone)]
pub(crate) enum HevcEncodeSubmitExecutionProbe {
    Ready {
        queue_family_index: u32,
        bitstream_buffer_offset: u32,
        dst_buffer_offset: u64,
        bitstream_memory_offset: u64,
        bytes_written: u32,
        head16: String,
        bitstream: Vec<u8>,
        bitstreams: Vec<Vec<u8>>,
    },
    Failed(String),
    Skipped(String),
}

#[derive(Debug, Clone, Copy, Default)]
struct ExtensionFlags {
    has_video_queue: bool,
    has_video_encode_queue: bool,
    has_video_encode_h265: bool,
    has_video_maintenance1: bool,
}

#[derive(Debug, Clone, Copy)]
struct EncodeQueueFamilyIndex(u32);

#[derive(Debug, Clone, Copy)]
struct AdapterEncodeSupport {
    extensions: ExtensionFlags,
    encode_queue_family_index: Option<EncodeQueueFamilyIndex>,
}

struct HevcEncodeCapabilitySnapshot {
    min_coded_extent: vk::Extent2D,
    max_coded_extent: vk::Extent2D,
    picture_access_granularity: vk::Extent2D,
    encode_input_picture_granularity: vk::Extent2D,
    max_dpb_slots: u32,
    max_active_reference_pictures: u32,
    encode_capability_flags: vk::VideoEncodeCapabilityFlagsKHR,
    encode_h265_capability_flags: vk::VideoEncodeH265CapabilityFlagsKHR,
    encode_h265_std_syntax_flags: vk::VideoEncodeH265StdFlagsKHR,
    rate_control_modes: vk::VideoEncodeRateControlModeFlagsKHR,
    max_rate_control_layers: u32,
    max_bitrate: u64,
    max_quality_levels: u32,
    supported_encode_feedback_flags: vk::VideoEncodeFeedbackFlagsKHR,
    min_bitstream_buffer_offset_alignment: vk::DeviceSize,
    min_bitstream_buffer_size_alignment: vk::DeviceSize,
    std_header_version: vk::ExtensionProperties,
    max_level_idc: StdVideoH265LevelIdc,
    video_maintenance1_extension_available: bool,
    video_maintenance1_feature_supported: bool,
}

struct HevcEncodeCapabilityCandidate {
    physical_device: vk::PhysicalDevice,
    extensions: ExtensionFlags,
    queue_family_index: EncodeQueueFamilyIndex,
    capability_snapshot: HevcEncodeCapabilitySnapshot,
    encode_input_formats: Vec<vk::Format>,
    encode_dpb_formats: Vec<vk::Format>,
}

#[derive(Debug, Clone)]
struct HevcEncodeSubmitExecutionConfig {
    physical_device: vk::PhysicalDevice,
    queue_family_index: EncodeQueueFamilyIndex,
    video_session: vk::VideoSessionKHR,
    video_session_parameters: vk::VideoSessionParametersKHR,
    parameter_set_ids: HevcEncodeParameterSetIds,
    parameter_set_coded_width: u32,
    parameter_set_coded_height: u32,
    parameter_set_pps_init_qp_minus26: i8,
    parameter_set_sample_adaptive_offset_enabled: bool,
    parameter_set_sps_temporal_mvp_enabled: bool,
    parameter_mode: HevcEncodeParameterMode,
    parameter_size_mode: HevcEncodeProbeParameterSizeMode,
    parameter_vui_safety_mode: HevcEncodeProbeParameterVuiSafetyMode,
    parameter_feedback_probe_summary: Option<String>,
    encode_h265_std_syntax_flags: vk::VideoEncodeH265StdFlagsKHR,
    coded_width: u32,
    coded_height: u32,
    picture_format: vk::Format,
    reference_picture_format: vk::Format,
    picture_access_granularity: vk::Extent2D,
    rate_control_modes: vk::VideoEncodeRateControlModeFlagsKHR,
    max_rate_control_layers: u32,
    max_quality_levels: u32,
    supported_encode_feedback_flags: vk::VideoEncodeFeedbackFlagsKHR,
    min_bitstream_buffer_offset_alignment: vk::DeviceSize,
    min_bitstream_buffer_size_alignment: vk::DeviceSize,
    maintenance1_mode: HevcEncodeProbeMaintenance1Mode,
    maintenance1_feature_enabled: bool,
    session_dpb_mode: HevcEncodeProbeSessionDpbMode,
    session_max_dpb_slots: u32,
    session_max_active_reference_pictures: u32,
    session_h265_create_info_mode: HevcEncodeProbeSessionH265CreateInfoMode,
    source_nv12: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodeVideoSessionProbeConfig<'a> {
    coded_extent: vk::Extent2D,
    maintenance1_mode: HevcEncodeProbeMaintenance1Mode,
    session_dpb_mode: HevcEncodeProbeSessionDpbMode,
    session_h265_create_info_mode: HevcEncodeProbeSessionH265CreateInfoMode,
    source_nv12_frames: &'a [Vec<u8>],
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodeSessionFormatProbeConfig {
    coded_extent: vk::Extent2D,
    picture_format: vk::Format,
    reference_picture_format: vk::Format,
    maintenance1_mode: HevcEncodeProbeMaintenance1Mode,
    session_dpb_mode: HevcEncodeProbeSessionDpbMode,
    session_h265_create_info_mode: HevcEncodeProbeSessionH265CreateInfoMode,
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodePreEncodeProbeResources {
    source_image: vk::Image,
    source_image_view: vk::ImageView,
    source_upload_buffer: vk::Buffer,
    source_upload_buffer_size: u64,
    source_upload_width: u32,
    source_upload_height: u32,
    dpb_image: vk::Image,
    dpb_image_view: vk::ImageView,
    source_picture_resource_width: u32,
    source_picture_resource_height: u32,
    picture_resource_coded_width: u32,
    picture_resource_coded_height: u32,
    dst_buffer: vk::Buffer,
    dst_buffer_offset: u64,
    dst_buffer_range: u64,
    dst_buffer_size: u64,
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodePreEncodeProbeConfig {
    command_buffer: vk::CommandBuffer,
    video_session: vk::VideoSessionKHR,
    video_session_parameters: vk::VideoSessionParametersKHR,
    control_config: HevcEncodeProbeControlConfig,
    mode: HevcEncodePreEncodeProbeMode,
    begin_session_parameters_mode: HevcEncodeProbeBeginSessionParametersMode,
    nalu_mode: HevcEncodeProbeNaluMode,
    codec_info_mode: HevcEncodeProbeCodecInfoMode,
    sample_adaptive_offset_enabled: bool,
    sps_temporal_mvp_enabled: bool,
    resources: HevcEncodePreEncodeProbeResources,
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodeProbeControlConfig {
    selected_rate_control_mode: Option<vk::VideoEncodeRateControlModeFlagsKHR>,
    selected_quality_level: u32,
    enable_quality_level_control: bool,
    control_mode: HevcEncodeProbeControlMode,
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodeProbeImageConfig {
    physical_device: vk::PhysicalDevice,
    queue_family_index: EncodeQueueFamilyIndex,
    image_width: u32,
    image_height: u32,
    image_view_mode: HevcEncodeProbeImageViewMode,
    picture_format: vk::Format,
    usage: vk::ImageUsageFlags,
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodeParameterSetIds {
    vps_id: u8,
    sps_id: u8,
    pps_id: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeParameterMode {
    Sample,
    SampleNoAddInfo,
    SampleVpsOnly,
    SampleSpsOnly,
    SampleSpsNoVui,
    SampleSpsVuiFlagOff,
    SampleSpsNoVuiFlagOn,
    SampleSpsNoRps,
    SampleSpsSafeFlags,
    SampleSpsLevel,
    SampleSpsSubLayerOrdering,
    SampleSpsLevelOrdering,
    SamplePpsOnly,
    EmptyTemplate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeReferenceListMode {
    Sentinel,
    Zero,
    NullPointers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeControlMode {
    Default,
    Ffmpeg,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeRateControlMode {
    Auto,
    Disabled,
    Cbr,
    Vbr,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeMaintenance1Mode {
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeSessionDpbMode {
    Default,
    MinimalOne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeSessionH265CreateInfoMode {
    WithMaxLevel,
    Without,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeBeginSessionParametersMode {
    With,
    Without,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeBeginPnextMode {
    None,
    RateControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeDstRangeMode {
    Full,
    FfmpegReserveAlign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeDstPrefixMode {
    None,
    Zero,
    ParameterSample,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeParameterSizeMode {
    Sample,
    Coded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeParameterVuiSafetyMode {
    Auto,
    Preserve,
    ForceOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeImageViewMode {
    YcbcrConversion,
    NoYcbcrConversion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodePreEncodeProbeMode {
    ScopeOnly,
    WithEncode,
    WithEncodeMinimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodePrimaryProbeMode {
    Submit,
    ScopeOnly,
    PreEncodeScopeOnly,
    PreEncode,
    PreEncodeMinimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeReferenceIndexMode {
    MinusOne,
    Zero,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeRpsMode {
    EmptyStruct,
    NullPointers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeBeginReferenceSlotMode {
    SlotMinusOne,
    SlotMinusOneNoCodecInfo,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeSetupReferenceSlotMode {
    SlotZero,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeDpbBarrierMode {
    With,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeEncodeReferenceSlotPointerMode {
    EmptySlice,
    FfmpegNonNull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeNaluMode {
    SingleSlice,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeCodecInfoMode {
    WithH265Info,
    WithH265InfoStdPictureOnly,
    WithH265InfoEmptyStdPicture,
    WithH265InfoMinimal,
    WithH265InfoNoStdPicture,
    WithoutH265Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbePictureFlagsMode {
    Default,
    NonReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbePictureInfoMode {
    Default,
    IntraI,
    InterP,
    Temporal1,
    Poc1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbePictureResourceExtentMode {
    Coded,
    ImageAligned,
}

#[derive(Debug, Clone)]
struct HevcEncodeSessionParameters {
    handle: vk::VideoSessionParametersKHR,
    parameter_set_ids: HevcEncodeParameterSetIds,
    coded_width: u32,
    coded_height: u32,
    pps_init_qp_minus26: i8,
    sample_adaptive_offset_enabled: bool,
    sps_temporal_mvp_enabled: bool,
    mode: HevcEncodeParameterMode,
    size_mode: HevcEncodeProbeParameterSizeMode,
    vui_safety_mode: HevcEncodeProbeParameterVuiSafetyMode,
    feedback_probe_summary: Option<String>,
}

struct HevcEncodeSessionParameterDevices<'a> {
    device: &'a ash::Device,
    video_queue_device: &'a ash::khr::video_queue::Device,
    video_encode_device: &'a ash::khr::video_encode_queue::Device,
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodeSessionParameterCreateConfig {
    video_session: vk::VideoSessionKHR,
    coded_width: u32,
    coded_height: u32,
    encode_h265_std_syntax_flags: vk::VideoEncodeH265StdFlagsKHR,
    max_quality_levels: u32,
}

const HEVC_NO_REFERENCE_PICTURE: u8 = u8::MAX;

struct HevcEncodeValidationDebugState {
    debug_utils: ash::ext::debug_utils::Instance,
    messenger: vk::DebugUtilsMessengerEXT,
}

static HEVC_ENCODE_VALIDATION_MESSAGES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

impl ExtensionFlags {
    fn supports_hevc_encode(self) -> bool {
        self.has_video_queue && self.has_video_encode_queue && self.has_video_encode_h265
    }

    fn union_assign(&mut self, other: Self) {
        self.has_video_queue |= other.has_video_queue;
        self.has_video_encode_queue |= other.has_video_encode_queue;
        self.has_video_encode_h265 |= other.has_video_encode_h265;
        self.has_video_maintenance1 |= other.has_video_maintenance1;
    }
}

mod configuration;
mod diagnostics;
mod execution;
mod feedback;
mod picture;
mod probe;
mod resources;
mod session_parameters;

use configuration::*;
use diagnostics::*;
use execution::*;
use feedback::*;
use picture::*;
use probe::*;
use resources::*;
use session_parameters::*;

/// Safe wrapper around Vulkan extension probing used by higher-level backend code.
///
/// All Vulkan FFI (`unsafe`) is confined to the private implementation modules so callers in
/// `vulkan_backend` can rely on a plain Rust enum without reasoning about raw Vulkan lifetimes.
pub(crate) fn probe_hevc_encode_prerequisites() -> HevcEncodePrerequisiteProbe {
    static CACHE: OnceLock<HevcEncodePrerequisiteProbe> = OnceLock::new();
    CACHE.get_or_init(run_hevc_encode_probe).clone()
}

pub(crate) fn probe_hevc_encode_session_bootstrap(
    coded_width: u32,
    coded_height: u32,
    fps: u32,
) -> Result<HevcEncodeSessionBootstrap, String> {
    run_hevc_encode_session_bootstrap(coded_width, coded_height, fps, Vec::new())
}

pub(crate) fn encode_hevc_idr_frame_annexb(
    coded_width: u32,
    coded_height: u32,
    fps: u32,
    source_nv12: &[u8],
) -> Result<Vec<u8>, String> {
    let mut packets =
        encode_hevc_idr_frames_annexb(coded_width, coded_height, fps, &[source_nv12.to_vec()])?;
    packets
        .pop()
        .ok_or_else(|| "HEVC encode produced no output packets".to_string())
}

pub(crate) fn encode_hevc_idr_frames_annexb(
    coded_width: u32,
    coded_height: u32,
    fps: u32,
    source_nv12_frames: &[Vec<u8>],
) -> Result<Vec<Vec<u8>>, String> {
    let expected_len = usize::try_from(u64::from(coded_width) * u64::from(coded_height) * 3 / 2)
        .map_err(|_| "HEVC encode source NV12 size exceeds usize".to_string())?;
    if source_nv12_frames.is_empty() {
        return Err("HEVC encode requires at least one source frame".to_string());
    }
    for (index, source_nv12) in source_nv12_frames.iter().enumerate() {
        if source_nv12.len() != expected_len {
            return Err(format!(
                "HEVC encode source NV12 size mismatch at frame {index}: expected {expected_len}, got {}",
                source_nv12.len()
            ));
        }
    }
    let bootstrap = run_hevc_encode_session_bootstrap(
        coded_width,
        coded_height,
        fps,
        source_nv12_frames.to_vec(),
    )?;
    let bitstreams = match bootstrap.encode_submit_execution_probe {
        HevcEncodeSubmitExecutionProbe::Ready { bitstreams, .. } => bitstreams,
        HevcEncodeSubmitExecutionProbe::Failed(err) => return Err(err),
        HevcEncodeSubmitExecutionProbe::Skipped(reason) => return Err(reason),
    };
    if bitstreams.len() != source_nv12_frames.len() {
        return Err(format!(
            "HEVC encode produced {} bitstreams for {} source frames",
            bitstreams.len(),
            source_nv12_frames.len()
        ));
    }
    let header_prefix = hevc_encode_parameter_header_prefix()?;
    bitstreams
        .into_iter()
        .enumerate()
        .map(|(index, bitstream)| {
            if bitstream.is_empty() {
                return Err(format!(
                    "HEVC encode produced an empty bitstream at frame {index}"
                ));
            }
            let mut output = header_prefix.clone();
            output.extend_from_slice(&bitstream);
            Ok(output)
        })
        .collect()
}

fn query_adapter_encode_support(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Result<AdapterEncodeSupport, vk::Result> {
    // SAFETY: We query immutable extension metadata for a valid physical device handle.
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical_device) }?;
    let mut flags = ExtensionFlags::default();
    for extension in extensions {
        // SAFETY: Vulkan guarantees `extension_name` is a null-terminated C string.
        let name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        flags.has_video_queue |= name == vk::KHR_VIDEO_QUEUE_NAME;
        flags.has_video_encode_queue |= name == vk::KHR_VIDEO_ENCODE_QUEUE_NAME;
        flags.has_video_encode_h265 |= name == vk::KHR_VIDEO_ENCODE_H265_NAME;
        flags.has_video_maintenance1 |= name == vk::KHR_VIDEO_MAINTENANCE1_NAME;
    }

    let encode_queue_family_index = query_video_codec_queue_family_index(
        instance,
        physical_device,
        vk::QueueFlags::VIDEO_ENCODE_KHR,
        vk::VideoCodecOperationFlagsKHR::ENCODE_H265,
    )
    .map(EncodeQueueFamilyIndex);

    Ok(AdapterEncodeSupport {
        extensions: flags,
        encode_queue_family_index,
    })
}

fn query_video_codec_queue_family_index(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    required_queue_flag: vk::QueueFlags,
    required_codec_operation: vk::VideoCodecOperationFlagsKHR,
) -> Option<u32> {
    // SAFETY: We only query immutable queue-family metadata for a valid physical device.
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

    // SAFETY: `queue_properties2` and chained `video_properties` live for the duration of the call.
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

fn try_initialize_hevc_encode_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: EncodeQueueFamilyIndex,
    extensions: ExtensionFlags,
    maintenance1_mode: HevcEncodeProbeMaintenance1Mode,
) -> Result<(), String> {
    let maintenance1_feature_supported = extensions.has_video_maintenance1
        && query_video_maintenance1_feature_support(instance, physical_device);
    let device = create_hevc_encode_device(
        instance,
        physical_device,
        queue_family_index,
        extensions,
        maintenance1_mode,
        maintenance1_feature_supported,
    )?;

    // SAFETY: `device` was created above and is not used after this call.
    unsafe {
        device.destroy_device(None);
    }
    Ok(())
}

fn create_hevc_encode_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: EncodeQueueFamilyIndex,
    extensions: ExtensionFlags,
    maintenance1_mode: HevcEncodeProbeMaintenance1Mode,
    maintenance1_feature_supported: bool,
) -> Result<ash::Device, String> {
    let priorities = [1.0_f32];
    let queue_create_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family_index.0)
        .queue_priorities(&priorities);
    let mut extension_names = vec![
        vk::KHR_VIDEO_QUEUE_NAME.as_ptr(),
        vk::KHR_VIDEO_ENCODE_QUEUE_NAME.as_ptr(),
        vk::KHR_VIDEO_ENCODE_H265_NAME.as_ptr(),
    ];
    if extensions.has_video_maintenance1 {
        extension_names.push(vk::KHR_VIDEO_MAINTENANCE1_NAME.as_ptr());
    }
    let enable_video_maintenance1 = resolve_hevc_encode_probe_maintenance1_feature_enabled(
        maintenance1_mode,
        extensions.has_video_maintenance1,
        maintenance1_feature_supported,
    );
    let mut synchronization2_features =
        vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true);
    let mut create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(std::slice::from_ref(&queue_create_info))
        .enabled_extension_names(&extension_names)
        .push_next(&mut synchronization2_features);
    let mut video_maintenance1_features = vk::PhysicalDeviceVideoMaintenance1FeaturesKHR::default()
        .video_maintenance1(enable_video_maintenance1);
    if enable_video_maintenance1 {
        create_info = create_info.push_next(&mut video_maintenance1_features);
    }

    // SAFETY: Pointers referenced by `create_info` remain valid through the call.
    unsafe { instance.create_device(physical_device, &create_info, None) }
        .map_err(|err| format!("logical device initialization failed: {err}"))
}

fn query_video_maintenance1_feature_support(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> bool {
    let mut maintenance1_features = vk::PhysicalDeviceVideoMaintenance1FeaturesKHR::default();
    let mut features2 =
        vk::PhysicalDeviceFeatures2::default().push_next(&mut maintenance1_features);
    // SAFETY: The feature chain points to stack-allocated structs valid for this call.
    unsafe {
        instance.get_physical_device_features2(physical_device, &mut features2);
    }
    maintenance1_features.video_maintenance1 == vk::TRUE
}

#[cfg(test)]
mod tests;
