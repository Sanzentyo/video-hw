// Experimental Vulkan HEVC encode probe module. The encode submit path is not yet wired up;
// this code exists for capability investigation and diagnostics.
#![allow(dead_code)]

use std::ffi::{CStr, c_void};
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
        bytes_written: u32,
        head16: String,
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
    parameter_feedback_probe_summary: Option<String>,
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
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodeSessionFormatProbeConfig {
    coded_extent: vk::Extent2D,
    picture_format: vk::Format,
    reference_picture_format: vk::Format,
    maintenance1_mode: HevcEncodeProbeMaintenance1Mode,
    session_dpb_mode: HevcEncodeProbeSessionDpbMode,
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodePreEncodeProbeResources {
    source_image: vk::Image,
    source_image_view: vk::ImageView,
    dpb_image: vk::Image,
    dpb_image_view: vk::ImageView,
    picture_resource_coded_width: u32,
    picture_resource_coded_height: u32,
    dst_buffer: vk::Buffer,
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
enum HevcEncodeProbeBeginSessionParametersMode {
    With,
    Without,
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
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcEncodeProbeSetupReferenceSlotMode {
    SlotZero,
    None,
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
    feedback_probe_summary: Option<String>,
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

fn clear_hevc_encode_validation_messages() {
    if let Some(messages) = HEVC_ENCODE_VALIDATION_MESSAGES.get() {
        if let Ok(mut guard) = messages.lock() {
            guard.clear();
        }
    } else {
        let _ = HEVC_ENCODE_VALIDATION_MESSAGES.set(Mutex::new(Vec::new()));
    }
}

fn push_hevc_encode_validation_message(message: impl Into<String>) {
    let messages = HEVC_ENCODE_VALIDATION_MESSAGES.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut guard) = messages.lock() {
        guard.push(message.into());
        if guard.len() > 16 {
            let drain = guard.len().saturating_sub(16);
            guard.drain(0..drain);
        }
    }
}

fn append_hevc_encode_validation_messages(base: String) -> String {
    let Some(messages) = HEVC_ENCODE_VALIDATION_MESSAGES.get() else {
        return base;
    };
    let Ok(guard) = messages.lock() else {
        return base;
    };
    let collected = guard
        .iter()
        .rev()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    if collected.is_empty() {
        base
    } else {
        format!("{base}; validation_messages={}", collected.join(" | "))
    }
}

unsafe extern "system" fn hevc_encode_validation_callback(
    message_severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _message_types: vk::DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user_data: *mut c_void,
) -> vk::Bool32 {
    let severity = if message_severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        "error"
    } else if message_severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
        "warn"
    } else {
        "info"
    };
    let raw_message = if p_callback_data.is_null() {
        "no callback data".to_string()
    } else {
        // SAFETY: callback data pointer is provided by Vulkan for the callback duration.
        let callback_data = unsafe { &*p_callback_data };
        if callback_data.p_message.is_null() {
            "no validation message".to_string()
        } else {
            // SAFETY: Vulkan provides a valid null-terminated C string.
            unsafe { CStr::from_ptr(callback_data.p_message) }
                .to_string_lossy()
                .replace('\n', " ")
                .trim()
                .to_string()
        }
    };
    push_hevc_encode_validation_message(format!("[{severity}] {raw_message}"));
    vk::FALSE
}

fn create_hevc_encode_probe_instance(
    entry: &ash::Entry,
) -> Result<(ash::Instance, Option<HevcEncodeValidationDebugState>), String> {
    clear_hevc_encode_validation_messages();

    // SAFETY: enumerating immutable instance extension metadata requires no additional invariants.
    let available_extensions = unsafe { entry.enumerate_instance_extension_properties(None) }
        .map_err(|err| format!("failed to enumerate Vulkan instance extensions: {err}"))?;
    let has_debug_utils = available_extensions.iter().any(|extension| {
        // SAFETY: Vulkan guarantees extension_name is null-terminated.
        let name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        name == vk::EXT_DEBUG_UTILS_NAME
    });

    let validation_layer_name = c"VK_LAYER_KHRONOS_validation";
    // SAFETY: enumerating immutable instance layer metadata requires no additional invariants.
    let available_layers = unsafe { entry.enumerate_instance_layer_properties() }
        .map_err(|err| format!("failed to enumerate Vulkan instance layers: {err}"))?;
    let has_validation_layer = available_layers.iter().any(|layer| {
        // SAFETY: Vulkan guarantees layer_name is null-terminated.
        let name = unsafe { CStr::from_ptr(layer.layer_name.as_ptr()) };
        name == validation_layer_name
    });

    let mut enabled_extensions = Vec::new();
    if has_debug_utils {
        enabled_extensions.push(vk::EXT_DEBUG_UTILS_NAME.as_ptr());
    } else {
        push_hevc_encode_validation_message(
            "[info] VK_EXT_debug_utils is unavailable; validation callback disabled",
        );
    }
    let mut enabled_layers = Vec::new();
    if has_validation_layer {
        enabled_layers.push(validation_layer_name.as_ptr());
    } else {
        push_hevc_encode_validation_message(
            "[info] VK_LAYER_KHRONOS_validation is unavailable; driver validation messages may be missing",
        );
    }

    let create_info = vk::InstanceCreateInfo::default()
        .enabled_extension_names(&enabled_extensions)
        .enabled_layer_names(&enabled_layers);
    // SAFETY: instance create info references stack data valid for the call.
    let instance = unsafe { entry.create_instance(&create_info, None) }
        .map_err(|err| format!("failed to create Vulkan instance: {err}"))?;

    let debug_state = if has_debug_utils {
        let debug_utils = ash::ext::debug_utils::Instance::new(entry, &instance);
        let messenger_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
            .message_severity(
                vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                    | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
            )
            .message_type(
                vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                    | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE
                    | vk::DebugUtilsMessageTypeFlagsEXT::GENERAL,
            )
            .pfn_user_callback(Some(hevc_encode_validation_callback));
        // SAFETY: debug utils extension and instance are valid in this scope.
        match unsafe { debug_utils.create_debug_utils_messenger(&messenger_info, None) } {
            Ok(messenger) => Some(HevcEncodeValidationDebugState {
                debug_utils,
                messenger,
            }),
            Err(err) => {
                push_hevc_encode_validation_message(format!(
                    "[warn] failed to create debug utils messenger: {err}"
                ));
                None
            }
        }
    } else {
        None
    };
    Ok((instance, debug_state))
}

/// Safe wrapper around Vulkan extension probing used by higher-level backend code.
///
/// All Vulkan FFI (`unsafe`) is confined to this module so callers in `vulkan_backend`
/// can rely on a plain Rust enum and do not need to reason about raw Vulkan object
/// lifetimes or pointer management.
pub(crate) fn probe_hevc_encode_prerequisites() -> HevcEncodePrerequisiteProbe {
    static CACHE: OnceLock<HevcEncodePrerequisiteProbe> = OnceLock::new();
    CACHE.get_or_init(run_hevc_encode_probe).clone()
}

pub(crate) fn probe_hevc_encode_session_bootstrap(
    coded_width: u32,
    coded_height: u32,
    _fps: u32,
) -> Result<HevcEncodeSessionBootstrap, String> {
    if coded_width == 0 || coded_height == 0 {
        return Err(format!(
            "HEVC encode bootstrap dimensions must be > 0, got {}x{}",
            coded_width, coded_height
        ));
    }

    // SAFETY: We only load Vulkan entry points and keep the handle local to this function.
    let entry = unsafe { ash::Entry::load() }
        .map_err(|err| format!("failed to load Vulkan entry: {err}"))?;
    let (instance, validation_debug_state) = create_hevc_encode_probe_instance(&entry)?;
    let maintenance1_mode = resolve_hevc_encode_probe_maintenance1_mode();
    let session_dpb_mode = resolve_hevc_encode_probe_session_dpb_mode();

    let bootstrap_result = (|| -> Result<HevcEncodeSessionBootstrap, String> {
        let video_queue = ash::khr::video_queue::Instance::new(&entry, &instance);

        // SAFETY: `instance` is valid in this scope and we only consume opaque handles.
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;
        if physical_devices.is_empty() {
            return Err(
                "no Vulkan physical devices available for HEVC encode bootstrap".to_string(),
            );
        }

        let mut probe_errors = Vec::new();
        let mut selected_candidate: Option<(u32, HevcEncodeCapabilityCandidate)> = None;

        for physical_device in physical_devices {
            let support = query_adapter_encode_support(&instance, physical_device)
                .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
            if !support.extensions.supports_hevc_encode() {
                continue;
            }
            let Some(queue_family_index) = support.encode_queue_family_index else {
                continue;
            };

            let mut encode_h265_profile = vk::VideoEncodeH265ProfileInfoKHR::default()
                .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
            let mut encode_usage = vk::VideoEncodeUsageInfoKHR::default()
                .video_usage_hints(vk::VideoEncodeUsageFlagsKHR::DEFAULT);
            let profile = vk::VideoProfileInfoKHR::default()
                .video_codec_operation(vk::VideoCodecOperationFlagsKHR::ENCODE_H265)
                .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
                .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
                .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
                .push_next(&mut encode_h265_profile)
                .push_next(&mut encode_usage);

            let mut encode_capabilities = vk::VideoEncodeCapabilitiesKHR::default();
            let mut encode_h265_capabilities = vk::VideoEncodeH265CapabilitiesKHR::default();
            let mut capabilities = vk::VideoCapabilitiesKHR::default()
                .push_next(&mut encode_h265_capabilities)
                .push_next(&mut encode_capabilities);

            // SAFETY: All pointers reference stack values alive for the duration of the call.
            let result = unsafe {
                (video_queue.fp().get_physical_device_video_capabilities_khr)(
                    physical_device,
                    &profile,
                    &mut capabilities,
                )
            };
            if result != vk::Result::SUCCESS {
                probe_errors.push(format!("video capabilities query failed: {result:?}"));
                continue;
            }

            if coded_width < capabilities.min_coded_extent.width
                || coded_width > capabilities.max_coded_extent.width
                || coded_height < capabilities.min_coded_extent.height
                || coded_height > capabilities.max_coded_extent.height
            {
                probe_errors.push(format!(
                    "configured coded extent {}x{} is outside device-supported range {}x{}..{}x{}",
                    coded_width,
                    coded_height,
                    capabilities.min_coded_extent.width,
                    capabilities.min_coded_extent.height,
                    capabilities.max_coded_extent.width,
                    capabilities.max_coded_extent.height
                ));
                continue;
            }

            let encode_input_formats = query_hevc_encode_formats(
                &video_queue,
                physical_device,
                profile,
                vk::ImageUsageFlags::VIDEO_ENCODE_SRC_KHR,
            )?;
            if encode_input_formats.is_empty() {
                probe_errors.push(
                    "video format query returned no VIDEO_ENCODE_SRC_KHR candidates".to_string(),
                );
                continue;
            }

            let encode_dpb_formats = query_hevc_encode_formats(
                &video_queue,
                physical_device,
                profile,
                vk::ImageUsageFlags::VIDEO_ENCODE_DPB_KHR,
            )?;
            let min_coded_extent = capabilities.min_coded_extent;
            let max_coded_extent = capabilities.max_coded_extent;
            let picture_access_granularity = capabilities.picture_access_granularity;
            let max_dpb_slots = capabilities.max_dpb_slots;
            let max_active_reference_pictures = capabilities.max_active_reference_pictures;
            let min_bitstream_buffer_offset_alignment =
                capabilities.min_bitstream_buffer_offset_alignment;
            let min_bitstream_buffer_size_alignment =
                capabilities.min_bitstream_buffer_size_alignment;
            let std_header_version = capabilities.std_header_version;
            let rate_control_modes = encode_capabilities.rate_control_modes;
            let max_rate_control_layers = encode_capabilities.max_rate_control_layers;
            let max_bitrate = encode_capabilities.max_bitrate;
            let max_quality_levels = encode_capabilities.max_quality_levels;
            let encode_capability_flags = encode_capabilities.flags;
            let encode_h265_capability_flags = encode_h265_capabilities.flags;
            let supported_encode_feedback_flags =
                encode_capabilities.supported_encode_feedback_flags;
            let max_level_idc = encode_h265_capabilities.max_level_idc;
            let encode_input_picture_granularity =
                encode_capabilities.encode_input_picture_granularity;
            let video_maintenance1_extension_available = support.extensions.has_video_maintenance1;
            let video_maintenance1_feature_supported = video_maintenance1_extension_available
                && query_video_maintenance1_feature_support(&instance, physical_device);

            let capability_snapshot = HevcEncodeCapabilitySnapshot {
                min_coded_extent,
                max_coded_extent,
                picture_access_granularity,
                encode_input_picture_granularity,
                max_dpb_slots,
                max_active_reference_pictures,
                encode_capability_flags,
                encode_h265_capability_flags,
                rate_control_modes,
                max_rate_control_layers,
                max_bitrate,
                max_quality_levels,
                supported_encode_feedback_flags,
                min_bitstream_buffer_offset_alignment,
                min_bitstream_buffer_size_alignment,
                std_header_version,
                max_level_idc,
                video_maintenance1_extension_available,
                video_maintenance1_feature_supported,
            };

            let properties = unsafe { instance.get_physical_device_properties(physical_device) };
            let is_discrete = properties.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
            let selection_score = u32::from(is_discrete) * 4
                + u32::from(video_maintenance1_feature_supported) * 2
                + u32::from(video_maintenance1_extension_available);
            let candidate = HevcEncodeCapabilityCandidate {
                physical_device,
                extensions: support.extensions,
                queue_family_index,
                capability_snapshot,
                encode_input_formats,
                encode_dpb_formats,
            };
            match &selected_candidate {
                Some((best_score, _)) if *best_score >= selection_score => {}
                _ => selected_candidate = Some((selection_score, candidate)),
            }
        }

        let candidate = if let Some((_, candidate)) = selected_candidate {
            candidate
        } else if probe_errors.is_empty() {
            return Err("no Vulkan adapter passed HEVC encode bootstrap checks".to_string());
        } else {
            return Err(format!(
                "HEVC encode bootstrap checks failed on all candidate adapters: {}",
                probe_errors.join("; ")
            ));
        };

        let (
            video_session_create_probe,
            video_session_parameters_create_probe,
            encode_submit_execution_probe,
        ) = probe_hevc_encode_video_session_and_parameters_creation(
            &instance,
            &candidate,
            coded_width,
            coded_height,
            maintenance1_mode,
            session_dpb_mode,
        );
        let selected_properties =
            unsafe { instance.get_physical_device_properties(candidate.physical_device) };
        let video_maintenance1_feature_enabled =
            resolve_hevc_encode_probe_maintenance1_feature_enabled(
                maintenance1_mode,
                candidate
                    .capability_snapshot
                    .video_maintenance1_extension_available,
                candidate
                    .capability_snapshot
                    .video_maintenance1_feature_supported,
            );

        Ok(HevcEncodeSessionBootstrap {
            coded_width,
            coded_height,
            adapter_name: unsafe { CStr::from_ptr(selected_properties.device_name.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
            adapter_vendor_id: selected_properties.vendor_id,
            adapter_device_id: selected_properties.device_id,
            adapter_driver_version: selected_properties.driver_version,
            adapter_api_version: selected_properties.api_version,
            min_coded_width: candidate.capability_snapshot.min_coded_extent.width,
            min_coded_height: candidate.capability_snapshot.min_coded_extent.height,
            max_coded_width: candidate.capability_snapshot.max_coded_extent.width,
            max_coded_height: candidate.capability_snapshot.max_coded_extent.height,
            picture_access_granularity_width: candidate
                .capability_snapshot
                .picture_access_granularity
                .width,
            picture_access_granularity_height: candidate
                .capability_snapshot
                .picture_access_granularity
                .height,
            encode_input_granularity_width: candidate
                .capability_snapshot
                .encode_input_picture_granularity
                .width,
            encode_input_granularity_height: candidate
                .capability_snapshot
                .encode_input_picture_granularity
                .height,
            coded_extent_input_granularity_aligned: coded_width.is_multiple_of(
                candidate
                    .capability_snapshot
                    .encode_input_picture_granularity
                    .width
                    .max(1),
            ) && coded_height.is_multiple_of(
                candidate
                    .capability_snapshot
                    .encode_input_picture_granularity
                    .height
                    .max(1),
            ),
            max_dpb_slots: candidate.capability_snapshot.max_dpb_slots,
            max_active_reference_pictures: candidate
                .capability_snapshot
                .max_active_reference_pictures,
            rate_control_modes: candidate.capability_snapshot.rate_control_modes,
            max_rate_control_layers: candidate.capability_snapshot.max_rate_control_layers,
            max_bitrate: candidate.capability_snapshot.max_bitrate,
            max_quality_levels: candidate.capability_snapshot.max_quality_levels,
            encode_capability_flags: candidate.capability_snapshot.encode_capability_flags,
            encode_h265_capability_flags: candidate
                .capability_snapshot
                .encode_h265_capability_flags,
            supported_encode_feedback_flags: candidate
                .capability_snapshot
                .supported_encode_feedback_flags,
            min_bitstream_buffer_offset_alignment: candidate
                .capability_snapshot
                .min_bitstream_buffer_offset_alignment,
            min_bitstream_buffer_size_alignment: candidate
                .capability_snapshot
                .min_bitstream_buffer_size_alignment,
            max_level_idc: candidate.capability_snapshot.max_level_idc as u32,
            video_maintenance1_mode: hevc_encode_probe_maintenance1_mode_label(maintenance1_mode),
            video_maintenance1_extension_available: candidate
                .capability_snapshot
                .video_maintenance1_extension_available,
            video_maintenance1_feature_supported: candidate
                .capability_snapshot
                .video_maintenance1_feature_supported,
            video_maintenance1_feature_enabled,
            encode_input_formats: candidate.encode_input_formats,
            encode_dpb_formats: candidate.encode_dpb_formats,
            video_session_create_probe,
            video_session_parameters_create_probe,
            encode_submit_execution_probe,
        })
    })();

    // SAFETY: `instance` is no longer used after this point.
    unsafe {
        if let Some(debug_state) = validation_debug_state.as_ref() {
            debug_state
                .debug_utils
                .destroy_debug_utils_messenger(debug_state.messenger, None);
        }
        instance.destroy_instance(None);
    }

    bootstrap_result
}

fn run_hevc_encode_probe() -> HevcEncodePrerequisiteProbe {
    // SAFETY: We only load function pointers from the Vulkan loader and keep the returned
    // handle within this function.
    let entry = match unsafe { ash::Entry::load() } {
        Ok(entry) => entry,
        Err(err) => {
            return HevcEncodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to load Vulkan entry: {err}"
            ));
        }
    };

    // SAFETY: `vk::InstanceCreateInfo::default()` provides a valid zero-initialized
    // instance create descriptor for ash. We destroy the instance before returning.
    let instance = match unsafe { entry.create_instance(&vk::InstanceCreateInfo::default(), None) }
    {
        Ok(instance) => instance,
        Err(err) => {
            return HevcEncodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to create Vulkan instance: {err}"
            ));
        }
    };

    let probe_result = (|| -> Result<HevcEncodePrerequisiteProbe, String> {
        let maintenance1_mode = resolve_hevc_encode_probe_maintenance1_mode();
        // SAFETY: `instance` is valid for this closure and we only consume opaque handles.
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;
        if physical_devices.is_empty() {
            return Ok(HevcEncodePrerequisiteProbe::NoCompatibleAdapter);
        }

        let mut observed_extensions = ExtensionFlags::default();
        let mut observed_encode_queue = false;
        let mut candidates = Vec::new();

        for physical_device in physical_devices {
            let support = query_adapter_encode_support(&instance, physical_device)
                .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
            observed_extensions.union_assign(support.extensions);
            observed_encode_queue |= support.encode_queue_family_index.is_some();

            if support.extensions.supports_hevc_encode()
                && let Some(queue_family_index) = support.encode_queue_family_index
            {
                // Prefer discrete adapters and those exposing maintenance1.
                let properties =
                    unsafe { instance.get_physical_device_properties(physical_device) };
                let is_discrete = properties.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
                let maintenance1_feature_supported = support.extensions.has_video_maintenance1
                    && query_video_maintenance1_feature_support(&instance, physical_device);
                let selection_score = u32::from(is_discrete) * 4
                    + u32::from(maintenance1_feature_supported) * 2
                    + u32::from(support.extensions.has_video_maintenance1);
                candidates.push((
                    selection_score,
                    physical_device,
                    queue_family_index,
                    support.extensions,
                ));
            }
        }

        candidates.sort_by(|lhs, rhs| rhs.0.cmp(&lhs.0));
        let mut device_init_errors = Vec::new();
        for (_, physical_device, queue_family_index, extensions) in candidates {
            match try_initialize_hevc_encode_device(
                &instance,
                physical_device,
                queue_family_index,
                extensions,
                maintenance1_mode,
            ) {
                Ok(()) => return Ok(HevcEncodePrerequisiteProbe::Ready),
                Err(err) => device_init_errors.push(err),
            }
        }

        let mut missing = Vec::new();
        if !observed_extensions.has_video_queue {
            missing.push("VK_KHR_video_queue");
        }
        if !observed_extensions.has_video_encode_queue {
            missing.push("VK_KHR_video_encode_queue");
        }
        if !observed_extensions.has_video_encode_h265 {
            missing.push("VK_KHR_video_encode_h265");
        }
        if !missing.is_empty() {
            return Ok(HevcEncodePrerequisiteProbe::MissingExtensions { missing });
        }

        if !observed_encode_queue {
            return Ok(HevcEncodePrerequisiteProbe::MissingEncodeQueueFamily);
        }

        if !device_init_errors.is_empty() {
            return Ok(HevcEncodePrerequisiteProbe::DeviceInitializationFailed(
                device_init_errors.join("; "),
            ));
        }

        Ok(HevcEncodePrerequisiteProbe::NoCompatibleAdapter)
    })();

    // SAFETY: `instance` was created in this function and is no longer used afterwards.
    unsafe {
        instance.destroy_instance(None);
    }

    probe_result.unwrap_or_else(HevcEncodePrerequisiteProbe::ProbeUnavailable)
}

fn query_hevc_encode_formats(
    video_queue: &ash::khr::video_queue::Instance,
    physical_device: vk::PhysicalDevice,
    profile: vk::VideoProfileInfoKHR<'_>,
    image_usage: vk::ImageUsageFlags,
) -> Result<Vec<vk::Format>, String> {
    let profiles = [profile];
    let mut profile_list = vk::VideoProfileListInfoKHR::default().profiles(&profiles);
    let format_info = vk::PhysicalDeviceVideoFormatInfoKHR::default()
        .image_usage(image_usage)
        .push_next(&mut profile_list);

    let mut property_count = 0_u32;
    // SAFETY: First query asks only for the number of format properties.
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
            "video format count query failed for usage {image_usage:?}: {count_result:?}"
        ));
    }
    if property_count == 0 {
        return Ok(Vec::new());
    }

    let mut properties = vec![vk::VideoFormatPropertiesKHR::default(); property_count as usize];
    // SAFETY: `properties` points to writable storage sized by the first query.
    let properties_result = unsafe {
        (video_queue
            .fp()
            .get_physical_device_video_format_properties_khr)(
            physical_device,
            &format_info,
            &mut property_count,
            properties.as_mut_ptr(),
        )
    };
    if properties_result != vk::Result::SUCCESS {
        return Err(format!(
            "video format properties query failed for usage {image_usage:?}: {properties_result:?}"
        ));
    }

    Ok(properties
        .into_iter()
        .map(|property| property.format)
        .collect())
}

fn probe_hevc_encode_video_session_and_parameters_creation(
    instance: &ash::Instance,
    candidate: &HevcEncodeCapabilityCandidate,
    coded_width: u32,
    coded_height: u32,
    maintenance1_mode: HevcEncodeProbeMaintenance1Mode,
    session_dpb_mode: HevcEncodeProbeSessionDpbMode,
) -> (
    HevcEncodeVideoSessionCreateProbe,
    HevcEncodeVideoSessionParametersCreateProbe,
    HevcEncodeSubmitExecutionProbe,
) {
    let device = match create_hevc_encode_device(
        instance,
        candidate.physical_device,
        candidate.queue_family_index,
        candidate.extensions,
        maintenance1_mode,
        candidate
            .capability_snapshot
            .video_maintenance1_feature_supported,
    ) {
        Ok(device) => device,
        Err(err) => {
            return (
                HevcEncodeVideoSessionCreateProbe::Failed(err),
                HevcEncodeVideoSessionParametersCreateProbe::Skipped(
                    "video session parameters create probe skipped: no video session handle"
                        .to_string(),
                ),
                HevcEncodeSubmitExecutionProbe::Skipped(
                    "encode submit execution probe skipped: no video session handle".to_string(),
                ),
            );
        }
    };

    let probe_result = (|| {
        let mut session_create_errors = Vec::new();
        let mut session_parameters_errors = Vec::new();

        let reference_formats = if candidate.encode_dpb_formats.is_empty() {
            candidate.encode_input_formats.clone()
        } else {
            candidate.encode_dpb_formats.clone()
        };

        for picture_format in candidate.encode_input_formats.iter().copied() {
            for reference_picture_format in reference_formats.iter().copied() {
                match probe_single_hevc_encode_session_format(
                    &device,
                    instance,
                    candidate,
                    HevcEncodeSessionFormatProbeConfig {
                        coded_extent: vk::Extent2D {
                            width: coded_width,
                            height: coded_height,
                        },
                        picture_format,
                        reference_picture_format,
                        maintenance1_mode,
                        session_dpb_mode,
                    },
                ) {
                    Ok((
                        HevcEncodeVideoSessionParametersCreateProbe::Created,
                        submit_execution_probe,
                    )) => {
                        return (
                            HevcEncodeVideoSessionCreateProbe::Created,
                            HevcEncodeVideoSessionParametersCreateProbe::Created,
                            submit_execution_probe,
                        );
                    }
                    Ok((HevcEncodeVideoSessionParametersCreateProbe::Failed(err), _)) => {
                        session_parameters_errors.push(format!(
                            "picture={picture_format:?}, reference={reference_picture_format:?}: {err}"
                        ));
                    }
                    Ok((HevcEncodeVideoSessionParametersCreateProbe::Skipped(reason), _)) => {
                        session_parameters_errors.push(format!(
                            "picture={picture_format:?}, reference={reference_picture_format:?}: {reason}"
                        ));
                    }
                    Err(err) => {
                        session_create_errors.push(format!(
                            "picture={picture_format:?}, reference={reference_picture_format:?}: {err}"
                        ));
                    }
                }
            }
        }

        if !session_parameters_errors.is_empty() {
            return (
                HevcEncodeVideoSessionCreateProbe::Created,
                HevcEncodeVideoSessionParametersCreateProbe::Failed(format!(
                    "vkCreateVideoSessionParametersKHR failed on all candidate format pairs: {}",
                    session_parameters_errors.join("; ")
                )),
                HevcEncodeSubmitExecutionProbe::Skipped(
                    "encode submit execution probe skipped: session parameters creation failed"
                        .to_string(),
                ),
            );
        }

        let create_details = if session_create_errors.is_empty() {
            "no encode input formats were reported".to_string()
        } else {
            session_create_errors.join("; ")
        };
        (
            HevcEncodeVideoSessionCreateProbe::Failed(format!(
                "vkCreateVideoSessionKHR failed on all candidate format pairs: {create_details}"
            )),
            HevcEncodeVideoSessionParametersCreateProbe::Skipped(
                "video session parameters create probe skipped: video session creation failed"
                    .to_string(),
            ),
            HevcEncodeSubmitExecutionProbe::Skipped(
                "encode submit execution probe skipped: video session creation failed".to_string(),
            ),
        )
    })();

    // SAFETY: `device` is no longer used after this point.
    unsafe {
        device.destroy_device(None);
    }

    probe_result
}

fn probe_single_hevc_encode_session_format(
    device: &ash::Device,
    instance: &ash::Instance,
    candidate: &HevcEncodeCapabilityCandidate,
    config: HevcEncodeSessionFormatProbeConfig,
) -> Result<
    (
        HevcEncodeVideoSessionParametersCreateProbe,
        HevcEncodeSubmitExecutionProbe,
    ),
    String,
> {
    let mut encode_h265_profile = vk::VideoEncodeH265ProfileInfoKHR::default()
        .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
    let mut encode_usage = vk::VideoEncodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoEncodeUsageFlagsKHR::DEFAULT);
    let profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::ENCODE_H265)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut encode_h265_profile)
        .push_next(&mut encode_usage);

    let mut encode_h265_session_create = vk::VideoEncodeH265SessionCreateInfoKHR::default()
        .use_max_level_idc(true)
        .max_level_idc(candidate.capability_snapshot.max_level_idc);
    let (session_max_dpb_slots, session_max_active_reference_pictures) =
        resolve_hevc_encode_probe_session_dpb_limits(
            config.session_dpb_mode,
            candidate.capability_snapshot.max_dpb_slots,
            candidate.capability_snapshot.max_active_reference_pictures,
        );
    let create_info = vk::VideoSessionCreateInfoKHR::default()
        .queue_family_index(candidate.queue_family_index.0)
        .video_profile(&profile)
        .picture_format(config.picture_format)
        .max_coded_extent(config.coded_extent)
        .reference_picture_format(config.reference_picture_format)
        .max_dpb_slots(session_max_dpb_slots)
        .max_active_reference_pictures(session_max_active_reference_pictures)
        .std_header_version(&candidate.capability_snapshot.std_header_version)
        .push_next(&mut encode_h265_session_create);
    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    let video_encode_device = ash::khr::video_encode_queue::Device::new(instance, device);
    let mut video_session = vk::VideoSessionKHR::null();

    // SAFETY: `create_info` references stack data that stays valid for the call.
    let result = unsafe {
        (video_queue_device.fp().create_video_session_khr)(
            device.handle(),
            &create_info,
            std::ptr::null(),
            &mut video_session,
        )
    };
    if result != vk::Result::SUCCESS {
        return Err(format!("vkCreateVideoSessionKHR failed: {result:?}"));
    }
    let session_memories = bind_hevc_encode_video_session_memory(
        device,
        instance,
        &video_queue_device,
        candidate.physical_device,
        video_session,
    )?;

    let (session_parameters_probe, submit_execution_probe) =
        match create_hevc_encode_video_session_parameters(
            device,
            &video_queue_device,
            &video_encode_device,
            video_session,
            config.coded_extent.width,
            config.coded_extent.height,
        ) {
            Ok(session_parameters) => {
                let submit_execution_probe = probe_hevc_encode_submit_execution(
                    device,
                    instance,
                    HevcEncodeSubmitExecutionConfig {
                        physical_device: candidate.physical_device,
                        queue_family_index: candidate.queue_family_index,
                        video_session,
                        video_session_parameters: session_parameters.handle,
                        parameter_set_ids: session_parameters.parameter_set_ids,
                        parameter_set_coded_width: session_parameters.coded_width,
                        parameter_set_coded_height: session_parameters.coded_height,
                        parameter_set_pps_init_qp_minus26: session_parameters.pps_init_qp_minus26,
                        parameter_set_sample_adaptive_offset_enabled: session_parameters
                            .sample_adaptive_offset_enabled,
                        parameter_set_sps_temporal_mvp_enabled: session_parameters
                            .sps_temporal_mvp_enabled,
                        parameter_mode: session_parameters.mode,
                        parameter_feedback_probe_summary: session_parameters
                            .feedback_probe_summary
                            .clone(),
                        coded_width: config.coded_extent.width,
                        coded_height: config.coded_extent.height,
                        picture_format: config.picture_format,
                        reference_picture_format: config.reference_picture_format,
                        picture_access_granularity: candidate
                            .capability_snapshot
                            .picture_access_granularity,
                        rate_control_modes: candidate.capability_snapshot.rate_control_modes,
                        max_rate_control_layers: candidate
                            .capability_snapshot
                            .max_rate_control_layers,
                        max_quality_levels: candidate.capability_snapshot.max_quality_levels,
                        supported_encode_feedback_flags: candidate
                            .capability_snapshot
                            .supported_encode_feedback_flags,
                        min_bitstream_buffer_offset_alignment: candidate
                            .capability_snapshot
                            .min_bitstream_buffer_offset_alignment,
                        min_bitstream_buffer_size_alignment: candidate
                            .capability_snapshot
                            .min_bitstream_buffer_size_alignment,
                        maintenance1_mode: config.maintenance1_mode,
                        maintenance1_feature_enabled:
                            resolve_hevc_encode_probe_maintenance1_feature_enabled(
                                config.maintenance1_mode,
                                candidate
                                    .capability_snapshot
                                    .video_maintenance1_extension_available,
                                candidate
                                    .capability_snapshot
                                    .video_maintenance1_feature_supported,
                            ),
                        session_dpb_mode: config.session_dpb_mode,
                        session_max_dpb_slots,
                        session_max_active_reference_pictures,
                    },
                );
                // SAFETY: handle was created from this device and is not reused afterwards.
                unsafe {
                    (video_queue_device.fp().destroy_video_session_parameters_khr)(
                        device.handle(),
                        session_parameters.handle,
                        std::ptr::null(),
                    );
                }
                (
                    HevcEncodeVideoSessionParametersCreateProbe::Created,
                    submit_execution_probe,
                )
            }
            Err(err) => (
                HevcEncodeVideoSessionParametersCreateProbe::Failed(err),
                HevcEncodeSubmitExecutionProbe::Skipped(
                    "encode submit execution probe skipped: session parameters creation failed"
                        .to_string(),
                ),
            ),
        };

    // SAFETY: session bind allocations were created by this device and are no longer used.
    unsafe {
        for memory in session_memories {
            device.free_memory(memory, None);
        }
    }

    // SAFETY: `video_session` was created from this `device` and is not reused.
    unsafe {
        (video_queue_device.fp().destroy_video_session_khr)(
            device.handle(),
            video_session,
            std::ptr::null(),
        );
    }
    Ok((session_parameters_probe, submit_execution_probe))
}

fn bind_hevc_encode_video_session_memory(
    device: &ash::Device,
    instance: &ash::Instance,
    video_queue_device: &ash::khr::video_queue::Device,
    physical_device: vk::PhysicalDevice,
    video_session: vk::VideoSessionKHR,
) -> Result<Vec<vk::DeviceMemory>, String> {
    let mut requirement_count = 0_u32;
    // SAFETY: first query asks only for requirement count for a valid video session handle.
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
            "vkGetVideoSessionMemoryRequirementsKHR count query failed: {count_result:?}"
        ));
    }
    if requirement_count == 0 {
        return Err(
            "vkGetVideoSessionMemoryRequirementsKHR returned no memory requirements".to_string(),
        );
    }

    let mut requirements =
        vec![vk::VideoSessionMemoryRequirementsKHR::default(); requirement_count as usize];
    // SAFETY: `requirements` storage is sized from the count query above.
    let requirements_result = unsafe {
        (video_queue_device
            .fp()
            .get_video_session_memory_requirements_khr)(
            device.handle(),
            video_session,
            &mut requirement_count,
            requirements.as_mut_ptr(),
        )
    };
    if requirements_result != vk::Result::SUCCESS {
        return Err(format!(
            "vkGetVideoSessionMemoryRequirementsKHR query failed: {requirements_result:?}"
        ));
    }

    let mut session_memories = Vec::with_capacity(requirements.len());
    let mut bindings = Vec::with_capacity(requirements.len());
    for requirement in &requirements {
        let memory_requirements = requirement.memory_requirements;
        let memory_type_index = find_memory_type_index(
            physical_device,
            instance,
            memory_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .or_else(|| {
            find_memory_type_index(
                physical_device,
                instance,
                memory_requirements.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
            )
        })
        .ok_or_else(|| {
            format!(
                "no compatible memory type for encode video session bind index {} (bits=0x{:X})",
                requirement.memory_bind_index, memory_requirements.memory_type_bits
            )
        })?;
        let allocation_size = memory_requirements.size.max(1);
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(allocation_size)
            .memory_type_index(memory_type_index);
        // SAFETY: allocation info references only POD values and `device` is valid.
        let memory = unsafe { device.allocate_memory(&allocate_info, None) }.map_err(|err| {
            format!("vkAllocateMemory for encode video session bind failed: {err}")
        })?;
        session_memories.push(memory);
        bindings.push(
            vk::BindVideoSessionMemoryInfoKHR::default()
                .memory_bind_index(requirement.memory_bind_index)
                .memory(memory)
                .memory_offset(0)
                .memory_size(allocation_size),
        );
    }

    // SAFETY: bind infos reference allocations that stay alive for this call.
    let bind_result = unsafe {
        (video_queue_device.fp().bind_video_session_memory_khr)(
            device.handle(),
            video_session,
            u32::try_from(bindings.len())
                .map_err(|_| "encode session binding count exceeds u32 range".to_string())?,
            bindings.as_ptr(),
        )
    };
    if bind_result != vk::Result::SUCCESS {
        // SAFETY: allocations were created from this device and are not bound on failure path.
        unsafe {
            for memory in &session_memories {
                device.free_memory(*memory, None);
            }
        }
        return Err(format!(
            "vkBindVideoSessionMemoryKHR failed: {bind_result:?}"
        ));
    }

    Ok(session_memories)
}

fn create_hevc_encode_video_session_parameters(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_encode_device: &ash::khr::video_encode_queue::Device,
    video_session: vk::VideoSessionKHR,
    coded_width: u32,
    coded_height: u32,
) -> Result<HevcEncodeSessionParameters, String> {
    let parameter_mode = resolve_hevc_encode_parameter_mode();
    match parameter_mode {
        HevcEncodeParameterMode::Sample
        | HevcEncodeParameterMode::SampleNoAddInfo
        | HevcEncodeParameterMode::SampleVpsOnly
        | HevcEncodeParameterMode::SampleSpsOnly
        | HevcEncodeParameterMode::SampleSpsNoVui
        | HevcEncodeParameterMode::SampleSpsVuiFlagOff
        | HevcEncodeParameterMode::SampleSpsNoVuiFlagOn
        | HevcEncodeParameterMode::SampleSpsNoRps
        | HevcEncodeParameterMode::SampleSpsSafeFlags
        | HevcEncodeParameterMode::SampleSpsLevel
        | HevcEncodeParameterMode::SampleSpsSubLayerOrdering
        | HevcEncodeParameterMode::SampleSpsLevelOrdering
        | HevcEncodeParameterMode::SamplePpsOnly => {
            create_hevc_encode_video_session_parameters_from_sample(
                device,
                video_queue_device,
                video_encode_device,
                video_session,
                parameter_mode,
            )
        }
        HevcEncodeParameterMode::EmptyTemplate => {
            let mut encode_h265_session_parameters =
                vk::VideoEncodeH265SessionParametersCreateInfoKHR::default()
                    .max_std_vps_count(1)
                    .max_std_sps_count(1)
                    .max_std_pps_count(1);
            let create_info = vk::VideoSessionParametersCreateInfoKHR::default()
                .video_session(video_session)
                .video_session_parameters_template(vk::VideoSessionParametersKHR::null())
                .push_next(&mut encode_h265_session_parameters);
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
                    "vkCreateVideoSessionParametersKHR failed: {result:?}"
                ));
            }
            Ok(HevcEncodeSessionParameters {
                handle: video_session_parameters,
                parameter_set_ids: HevcEncodeParameterSetIds {
                    vps_id: 0,
                    sps_id: 0,
                    pps_id: 0,
                },
                coded_width,
                coded_height,
                pps_init_qp_minus26: 0,
                sample_adaptive_offset_enabled: false,
                sps_temporal_mvp_enabled: false,
                mode: parameter_mode,
                feedback_probe_summary: Some("skipped(mode=empty-template)".to_string()),
            })
        }
    }
}

fn create_hevc_encode_video_session_parameters_from_sample(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_encode_device: &ash::khr::video_encode_queue::Device,
    video_session: vk::VideoSessionKHR,
    parameter_mode: HevcEncodeParameterMode,
) -> Result<HevcEncodeSessionParameters, String> {
    debug_assert!(matches!(
        parameter_mode,
        HevcEncodeParameterMode::Sample
            | HevcEncodeParameterMode::SampleNoAddInfo
            | HevcEncodeParameterMode::SampleVpsOnly
            | HevcEncodeParameterMode::SampleSpsOnly
            | HevcEncodeParameterMode::SampleSpsNoVui
            | HevcEncodeParameterMode::SampleSpsVuiFlagOff
            | HevcEncodeParameterMode::SampleSpsNoVuiFlagOn
            | HevcEncodeParameterMode::SampleSpsNoRps
            | HevcEncodeParameterMode::SampleSpsSafeFlags
            | HevcEncodeParameterMode::SampleSpsLevel
            | HevcEncodeParameterMode::SampleSpsSubLayerOrdering
            | HevcEncodeParameterMode::SampleSpsLevelOrdering
            | HevcEncodeParameterMode::SamplePpsOnly
    ));
    let parameter_sample = load_hevc_encode_probe_parameter_sample()?;
    let mut parameter_sets = extract_hevc_parameter_sets_annexb(&parameter_sample)
        .map_err(|err| format!("failed to extract HEVC parameter sets for encode probe: {err}"))?;
    validate_hevc_encode_probe_parameter_profile(&parameter_sets)?;
    let effective_parameter_mode = resolve_hevc_encode_parameter_mode_with_vui_safety(
        parameter_mode,
        hevc_encode_probe_parameter_sample_override_path().is_some(),
        parameter_sets.parsed_sps.vui_parameters.is_some(),
    );
    apply_hevc_encode_parameter_mode_sps_overrides(&mut parameter_sets, effective_parameter_mode);
    let mut std_parameter_storage =
        build_hevc_std_parameter_set_storage(&parameter_sets).map_err(|err| {
            format!("failed to build HEVC std parameter storage for encode probe: {err}")
        })?;
    apply_hevc_encode_parameter_mode_std_overrides(
        &mut std_parameter_storage,
        effective_parameter_mode,
    );
    let (vps_id, sps_id, pps_id) = std_parameter_storage.encode_parameter_set_ids();
    let pps_init_qp_minus26 = std_parameter_storage.encode_pps_init_qp_minus26();
    let sample_adaptive_offset_enabled = parameter_sets
        .parsed_sps
        .sample_adaptive_offset_enabled_flag;
    let sps_temporal_mvp_enabled = parameter_sets.parsed_sps.sps_temporal_mvp_enabled_flag;
    let encode_h265_session_parameters_add = hevc_encode_parameter_mode_add_info_filter(
        effective_parameter_mode,
    )
    .map(|(include_vps, include_sps, include_pps)| {
        std_parameter_storage.encode_add_info_with_filter(include_vps, include_sps, include_pps)
    });
    let mut encode_h265_session_parameters =
        vk::VideoEncodeH265SessionParametersCreateInfoKHR::default()
            .max_std_vps_count(1)
            .max_std_sps_count(1)
            .max_std_pps_count(1);
    if let Some(add_info) = encode_h265_session_parameters_add.as_ref() {
        encode_h265_session_parameters =
            encode_h265_session_parameters.parameters_add_info(add_info);
    }
    let create_info = vk::VideoSessionParametersCreateInfoKHR::default()
        .video_session(video_session)
        .video_session_parameters_template(vk::VideoSessionParametersKHR::null())
        .push_next(&mut encode_h265_session_parameters);
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
            "vkCreateVideoSessionParametersKHR failed: {result:?}"
        ));
    }
    let feedback_probe_summary = match probe_hevc_encode_parameter_feedback(
        device,
        video_encode_device,
        video_session_parameters,
        HevcEncodeParameterSetIds {
            vps_id,
            sps_id,
            pps_id,
        },
    ) {
        Ok(summary) => Some(summary),
        Err(err) => Some(format!("failed({err})")),
    };
    Ok(HevcEncodeSessionParameters {
        handle: video_session_parameters,
        parameter_set_ids: HevcEncodeParameterSetIds {
            vps_id,
            sps_id,
            pps_id,
        },
        coded_width: parameter_sets.coded_width,
        coded_height: parameter_sets.coded_height,
        pps_init_qp_minus26,
        sample_adaptive_offset_enabled,
        sps_temporal_mvp_enabled,
        mode: effective_parameter_mode,
        feedback_probe_summary,
    })
}

fn resolve_hevc_encode_parameter_mode() -> HevcEncodeParameterMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_parameter_mode(mode.as_deref())
}

fn parse_hevc_encode_parameter_mode(mode: Option<&str>) -> HevcEncodeParameterMode {
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

fn hevc_encode_parameter_mode_add_info_filter(
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

fn apply_hevc_encode_parameter_mode_std_overrides(
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

fn apply_hevc_encode_parameter_mode_sps_overrides(
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

fn hevc_encode_parameter_mode_label(mode: HevcEncodeParameterMode) -> &'static str {
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

fn resolve_hevc_encode_probe_reference_list_mode() -> HevcEncodeProbeReferenceListMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_LIST_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_reference_list_mode(mode.as_deref())
}

fn parse_hevc_encode_probe_reference_list_mode(
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

fn hevc_encode_probe_reference_list_mode_label(
    mode: HevcEncodeProbeReferenceListMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeReferenceListMode::Sentinel => "sentinel",
        HevcEncodeProbeReferenceListMode::Zero => "zero",
        HevcEncodeProbeReferenceListMode::NullPointers => "null-pointers",
    }
}

fn resolve_hevc_encode_probe_reference_index_mode() -> HevcEncodeProbeReferenceIndexMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_IDX_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_reference_index_mode(mode.as_deref())
}

fn resolve_hevc_encode_probe_rps_mode() -> HevcEncodeProbeRpsMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_RPS_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_rps_mode(mode.as_deref())
}

fn parse_hevc_encode_probe_rps_mode(mode: Option<&str>) -> HevcEncodeProbeRpsMode {
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

fn hevc_encode_probe_rps_mode_label(mode: HevcEncodeProbeRpsMode) -> &'static str {
    match mode {
        HevcEncodeProbeRpsMode::EmptyStruct => "empty-struct",
        HevcEncodeProbeRpsMode::NullPointers => "null-pointers",
    }
}

fn resolve_hevc_encode_probe_begin_reference_slot_mode() -> HevcEncodeProbeBeginReferenceSlotMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_REFERENCE_SLOT_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_begin_reference_slot_mode(mode.as_deref())
}

fn parse_hevc_encode_probe_begin_reference_slot_mode(
    mode: Option<&str>,
) -> HevcEncodeProbeBeginReferenceSlotMode {
    let normalized = mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        Some("none") | Some("off") | Some("disable") | Some("disabled") => {
            HevcEncodeProbeBeginReferenceSlotMode::None
        }
        _ => HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOne,
    }
}

fn hevc_encode_probe_begin_reference_slot_mode_label(
    mode: HevcEncodeProbeBeginReferenceSlotMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOne => "slot-minus-one",
        HevcEncodeProbeBeginReferenceSlotMode::None => "none",
    }
}

fn resolve_hevc_encode_probe_setup_reference_slot_mode() -> HevcEncodeProbeSetupReferenceSlotMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_SETUP_REFERENCE_SLOT_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_setup_reference_slot_mode(mode.as_deref())
}

fn parse_hevc_encode_probe_setup_reference_slot_mode(
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

fn hevc_encode_probe_setup_reference_slot_mode_label(
    mode: HevcEncodeProbeSetupReferenceSlotMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeSetupReferenceSlotMode::SlotZero => "slot-zero",
        HevcEncodeProbeSetupReferenceSlotMode::None => "none",
    }
}

fn resolve_hevc_encode_probe_control_mode() -> HevcEncodeProbeControlMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_CONTROL_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_control_mode(mode.as_deref())
}

fn resolve_hevc_encode_probe_nalu_mode() -> HevcEncodeProbeNaluMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_NALU_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_nalu_mode(mode.as_deref())
}

fn resolve_hevc_encode_probe_codec_info_mode() -> HevcEncodeProbeCodecInfoMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_CODEC_INFO_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_codec_info_mode(mode.as_deref())
}

fn resolve_hevc_encode_probe_picture_flags_mode() -> HevcEncodeProbePictureFlagsMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_FLAGS_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_picture_flags_mode(mode.as_deref())
}

fn resolve_hevc_encode_probe_picture_info_mode() -> HevcEncodeProbePictureInfoMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_INFO_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_picture_info_mode(mode.as_deref())
}

fn resolve_hevc_encode_probe_rate_control_mode() -> HevcEncodeProbeRateControlMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_RATE_CONTROL_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_rate_control_mode(mode.as_deref())
}

fn resolve_hevc_encode_probe_maintenance1_mode() -> HevcEncodeProbeMaintenance1Mode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_MAINTENANCE1_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_maintenance1_mode(mode.as_deref())
}

fn resolve_hevc_encode_probe_session_dpb_mode() -> HevcEncodeProbeSessionDpbMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_SESSION_DPB_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_session_dpb_mode(mode.as_deref())
}

fn resolve_hevc_encode_probe_begin_session_parameters_mode()
-> HevcEncodeProbeBeginSessionParametersMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_SESSION_PARAMETERS_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_begin_session_parameters_mode(mode.as_deref())
}

fn resolve_hevc_encode_probe_picture_resource_extent_mode()
-> HevcEncodeProbePictureResourceExtentMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PICTURE_RESOURCE_EXTENT_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_picture_resource_extent_mode(mode.as_deref())
}

fn resolve_hevc_encode_probe_primary_mode() -> HevcEncodePrimaryProbeMode {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PRIMARY_MODE";
    let mode = std::env::var(ENV_VAR).ok();
    parse_hevc_encode_probe_primary_mode(mode.as_deref())
}

fn parse_hevc_encode_probe_maintenance1_mode(
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

fn parse_hevc_encode_probe_session_dpb_mode(mode: Option<&str>) -> HevcEncodeProbeSessionDpbMode {
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

fn parse_hevc_encode_probe_begin_session_parameters_mode(
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

fn parse_hevc_encode_probe_picture_resource_extent_mode(
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

fn parse_hevc_encode_probe_primary_mode(mode: Option<&str>) -> HevcEncodePrimaryProbeMode {
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

fn hevc_encode_probe_maintenance1_mode_label(
    mode: HevcEncodeProbeMaintenance1Mode,
) -> &'static str {
    match mode {
        HevcEncodeProbeMaintenance1Mode::Auto => "auto",
        HevcEncodeProbeMaintenance1Mode::On => "on",
        HevcEncodeProbeMaintenance1Mode::Off => "off",
    }
}

fn hevc_encode_probe_session_dpb_mode_label(mode: HevcEncodeProbeSessionDpbMode) -> &'static str {
    match mode {
        HevcEncodeProbeSessionDpbMode::Default => "default",
        HevcEncodeProbeSessionDpbMode::MinimalOne => "minimal-one",
    }
}

fn hevc_encode_probe_begin_session_parameters_mode_label(
    mode: HevcEncodeProbeBeginSessionParametersMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeBeginSessionParametersMode::With => "with",
        HevcEncodeProbeBeginSessionParametersMode::Without => "without",
    }
}

fn hevc_encode_probe_picture_resource_extent_mode_label(
    mode: HevcEncodeProbePictureResourceExtentMode,
) -> &'static str {
    match mode {
        HevcEncodeProbePictureResourceExtentMode::Coded => "coded",
        HevcEncodeProbePictureResourceExtentMode::ImageAligned => "image-aligned",
    }
}

fn resolve_hevc_encode_probe_session_dpb_limits(
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

fn resolve_hevc_encode_probe_maintenance1_feature_enabled(
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

fn parse_hevc_encode_probe_control_mode(mode: Option<&str>) -> HevcEncodeProbeControlMode {
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

fn hevc_encode_probe_control_mode_label(mode: HevcEncodeProbeControlMode) -> &'static str {
    match mode {
        HevcEncodeProbeControlMode::Default => "default",
        HevcEncodeProbeControlMode::Ffmpeg => "ffmpeg",
        HevcEncodeProbeControlMode::None => "none",
    }
}

fn parse_hevc_encode_probe_nalu_mode(mode: Option<&str>) -> HevcEncodeProbeNaluMode {
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

fn hevc_encode_probe_nalu_mode_label(mode: HevcEncodeProbeNaluMode) -> &'static str {
    match mode {
        HevcEncodeProbeNaluMode::SingleSlice => "single-slice",
        HevcEncodeProbeNaluMode::Empty => "empty",
    }
}

fn parse_hevc_encode_probe_codec_info_mode(mode: Option<&str>) -> HevcEncodeProbeCodecInfoMode {
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

fn hevc_encode_probe_codec_info_mode_label(mode: HevcEncodeProbeCodecInfoMode) -> &'static str {
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

fn parse_hevc_encode_probe_picture_flags_mode(
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

fn hevc_encode_probe_picture_flags_mode_label(
    mode: HevcEncodeProbePictureFlagsMode,
) -> &'static str {
    match mode {
        HevcEncodeProbePictureFlagsMode::Default => "default",
        HevcEncodeProbePictureFlagsMode::NonReference => "non-reference",
    }
}

fn parse_hevc_encode_probe_picture_info_mode(mode: Option<&str>) -> HevcEncodeProbePictureInfoMode {
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

fn hevc_encode_probe_picture_info_mode_label(mode: HevcEncodeProbePictureInfoMode) -> &'static str {
    match mode {
        HevcEncodeProbePictureInfoMode::Default => "default",
        HevcEncodeProbePictureInfoMode::IntraI => "intra-i",
        HevcEncodeProbePictureInfoMode::InterP => "inter-p",
        HevcEncodeProbePictureInfoMode::Temporal1 => "temporal-1",
        HevcEncodeProbePictureInfoMode::Poc1 => "poc-1",
    }
}

fn parse_hevc_encode_probe_rate_control_mode(mode: Option<&str>) -> HevcEncodeProbeRateControlMode {
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

fn hevc_encode_probe_rate_control_mode_label(mode: HevcEncodeProbeRateControlMode) -> &'static str {
    match mode {
        HevcEncodeProbeRateControlMode::Auto => "auto",
        HevcEncodeProbeRateControlMode::Disabled => "disabled",
        HevcEncodeProbeRateControlMode::Cbr => "cbr",
        HevcEncodeProbeRateControlMode::Vbr => "vbr",
        HevcEncodeProbeRateControlMode::None => "none",
    }
}

fn resolve_hevc_encode_probe_selected_rate_control_mode(
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

fn hevc_encode_probe_picture_type(
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

fn hevc_encode_probe_slice_type(
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

fn hevc_encode_probe_temporal_id(mode: HevcEncodeProbePictureInfoMode) -> u8 {
    match mode {
        HevcEncodeProbePictureInfoMode::Temporal1 => 1,
        _ => 0,
    }
}

fn hevc_encode_probe_pic_order_cnt_val(mode: HevcEncodeProbePictureInfoMode) -> i32 {
    match mode {
        HevcEncodeProbePictureInfoMode::Poc1 => 1,
        _ => 0,
    }
}

fn hevc_encode_probe_constant_qp(
    selected_rate_control_mode: Option<vk::VideoEncodeRateControlModeFlagsKHR>,
) -> i32 {
    if selected_rate_control_mode
        .is_some_and(|mode| mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED)
    {
        0
    } else {
        26
    }
}

fn hevc_encode_probe_slice_qp_delta(
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

fn hevc_encode_std_slice_segment_header_flags(
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

fn hevc_encode_h265_rate_control_info() -> vk::VideoEncodeH265RateControlInfoKHR<'static> {
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

fn hevc_encode_pre_encode_probe_mode_label(mode: HevcEncodePreEncodeProbeMode) -> &'static str {
    match mode {
        HevcEncodePreEncodeProbeMode::ScopeOnly => "scope-only",
        HevcEncodePreEncodeProbeMode::WithEncode => "with-encode",
        HevcEncodePreEncodeProbeMode::WithEncodeMinimal => "with-encode-minimal",
    }
}

fn hevc_encode_primary_probe_mode_label(mode: HevcEncodePrimaryProbeMode) -> &'static str {
    match mode {
        HevcEncodePrimaryProbeMode::Submit => "submit",
        HevcEncodePrimaryProbeMode::ScopeOnly => "scope-only",
        HevcEncodePrimaryProbeMode::PreEncodeScopeOnly => "pre-encode-scope-only",
        HevcEncodePrimaryProbeMode::PreEncode => "pre-encode",
        HevcEncodePrimaryProbeMode::PreEncodeMinimal => "pre-encode-minimal",
    }
}

fn parse_hevc_encode_probe_reference_index_mode(
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

fn hevc_encode_probe_reference_index_mode_label(
    mode: HevcEncodeProbeReferenceIndexMode,
) -> &'static str {
    match mode {
        HevcEncodeProbeReferenceIndexMode::MinusOne => "minus-one",
        HevcEncodeProbeReferenceIndexMode::Zero => "zero",
    }
}

fn hevc_encode_probe_reference_idx_minus1(mode: HevcEncodeProbeReferenceIndexMode) -> u8 {
    match mode {
        HevcEncodeProbeReferenceIndexMode::MinusOne => u8::MAX,
        HevcEncodeProbeReferenceIndexMode::Zero => 0,
    }
}

fn resolve_hevc_encode_parameter_mode_with_vui_safety(
    requested_mode: HevcEncodeParameterMode,
    sample_override_enabled: bool,
    sample_has_vui_parameters: bool,
) -> HevcEncodeParameterMode {
    if sample_override_enabled
        && sample_has_vui_parameters
        && matches!(requested_mode, HevcEncodeParameterMode::Sample)
    {
        HevcEncodeParameterMode::SampleSpsVuiFlagOff
    } else {
        requested_mode
    }
}

fn hevc_encode_probe_parameter_sample_override_path() -> Option<String> {
    const ENV_VAR: &str = "VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SAMPLE_PATH";
    std::env::var(ENV_VAR)
        .ok()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
}

fn load_hevc_encode_probe_parameter_sample() -> Result<Vec<u8>, String> {
    let override_path = hevc_encode_probe_parameter_sample_override_path();
    load_hevc_encode_probe_parameter_sample_from_path(override_path.as_deref())
}

fn load_hevc_encode_probe_parameter_sample_from_path(
    override_path: Option<&str>,
) -> Result<Vec<u8>, String> {
    if let Some(path) = override_path.map(str::trim).filter(|path| !path.is_empty()) {
        return std::fs::read(path).map_err(|err| {
            format!("failed to read HEVC encode probe parameter sample at '{path}': {err}")
        });
    }
    Ok(include_bytes!("../../../sample-videos/sample-10s.h265").to_vec())
}

fn probe_hevc_encode_parameter_feedback(
    device: &ash::Device,
    video_encode_device: &ash::khr::video_encode_queue::Device,
    video_session_parameters: vk::VideoSessionParametersKHR,
    parameter_set_ids: HevcEncodeParameterSetIds,
) -> Result<String, String> {
    let mut h265_get_info = vk::VideoEncodeH265SessionParametersGetInfoKHR::default()
        .write_std_vps(true)
        .write_std_sps(true)
        .write_std_pps(true)
        .std_vps_id(u32::from(parameter_set_ids.vps_id))
        .std_sps_id(u32::from(parameter_set_ids.sps_id))
        .std_pps_id(u32::from(parameter_set_ids.pps_id));
    let get_info = vk::VideoEncodeSessionParametersGetInfoKHR::default()
        .video_session_parameters(video_session_parameters)
        .push_next(&mut h265_get_info);

    let mut h265_feedback_info = vk::VideoEncodeH265SessionParametersFeedbackInfoKHR::default();
    let mut feedback_info = vk::VideoEncodeSessionParametersFeedbackInfoKHR::default()
        .push_next(&mut h265_feedback_info);
    let mut data_size: usize = 0;
    // SAFETY: all structs are initialized and point to stack memory valid for this call.
    let first_result = unsafe {
        (video_encode_device
            .fp()
            .get_encoded_video_session_parameters_khr)(
            device.handle(),
            &get_info,
            &mut feedback_info,
            &mut data_size,
            std::ptr::null_mut(),
        )
    };
    if first_result != vk::Result::SUCCESS && first_result != vk::Result::INCOMPLETE {
        return Err(format!(
            "vkGetEncodedVideoSessionParametersKHR(size-query) failed: {first_result:?}"
        ));
    }

    let mut data = vec![0_u8; data_size];
    // SAFETY: output buffer is valid for `data_size` bytes and pointers live for the call.
    let second_result = unsafe {
        (video_encode_device
            .fp()
            .get_encoded_video_session_parameters_khr)(
            device.handle(),
            &get_info,
            &mut feedback_info,
            &mut data_size,
            data.as_mut_ptr().cast(),
        )
    };
    if second_result != vk::Result::SUCCESS && second_result != vk::Result::INCOMPLETE {
        return Err(format!(
            "vkGetEncodedVideoSessionParametersKHR(data) failed: {second_result:?}"
        ));
    }
    let used_size = data_size.min(data.len());
    let preview_bytes = data
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("");
    Ok(format!(
        "result={second_result:?}, size={used_size}, has_overrides={}, vps_overrides={}, sps_overrides={}, pps_overrides={}, head16={preview_bytes}",
        feedback_info.has_overrides != 0,
        h265_feedback_info.has_std_vps_overrides != 0,
        h265_feedback_info.has_std_sps_overrides != 0,
        h265_feedback_info.has_std_pps_overrides != 0,
    ))
}

fn validate_hevc_encode_probe_parameter_profile(
    parameter_sets: &HevcParameterSets,
) -> Result<(), String> {
    const HEVC_MAIN_PROFILE_IDC: u8 = 1;
    let profile_idc = parameter_sets
        .parsed_sps
        .profile_tier_level
        .general_profile
        .profile_idc;
    if profile_idc == HEVC_MAIN_PROFILE_IDC {
        return Ok(());
    }
    Err(format!(
        "unsupported HEVC encode probe parameter sample profile_idc={profile_idc}; only Main profile (profile_idc=1) is currently supported for Vulkan HEVC encode probe session parameters"
    ))
}

fn probe_hevc_encode_submit_execution(
    device: &ash::Device,
    instance: &ash::Instance,
    config: HevcEncodeSubmitExecutionConfig,
) -> HevcEncodeSubmitExecutionProbe {
    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    let video_encode_device = ash::khr::video_encode_queue::Device::new(instance, device);
    let mut command_pool = vk::CommandPool::null();
    let mut fence = vk::Fence::null();
    let mut source_image = vk::Image::null();
    let mut source_image_memory = vk::DeviceMemory::null();
    let mut source_image_view = vk::ImageView::null();
    let mut dpb_image = vk::Image::null();
    let mut dpb_image_memory = vk::DeviceMemory::null();
    let mut dpb_image_view = vk::ImageView::null();
    let mut dst_buffer = vk::Buffer::null();
    let mut dst_buffer_memory = vk::DeviceMemory::null();
    let mut query_pool = vk::QueryPool::null();
    let mut query_result = HevcEncodeFeedbackQueryResult::default();
    let mut dst_head16 = String::new();

    let probe_result = (|| -> Result<(), String> {
        if !config
            .supported_encode_feedback_flags
            .contains(vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BYTES_WRITTEN)
        {
            return Err(
                "encode submit feedback does not support BITSTREAM_BYTES_WRITTEN".to_string(),
            );
        }

        // SAFETY: queue family index and queue 0 were requested during device creation.
        let queue = unsafe { device.get_device_queue(config.queue_family_index.0, 0) };
        if queue == vk::Queue::null() {
            return Err(format!(
                "vkGetDeviceQueue returned null for queue family {}",
                config.queue_family_index.0
            ));
        }

        let command_pool_info =
            vk::CommandPoolCreateInfo::default().queue_family_index(config.queue_family_index.0);
        // SAFETY: command pool create info references only stack data.
        command_pool = unsafe { device.create_command_pool(&command_pool_info, None) }
            .map_err(|err| format!("vkCreateCommandPool failed: {err}"))?;

        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(5);
        // SAFETY: allocation info references valid command pool handle.
        let command_buffers = unsafe { device.allocate_command_buffers(&allocate_info) }
            .map_err(|err| format!("vkAllocateCommandBuffers failed: {err}"))?;
        let command_buffer = *command_buffers.first().ok_or_else(|| {
            "vkAllocateCommandBuffers returned no command buffers for encode submit probe"
                .to_string()
        })?;
        let coding_scope_probe_command_buffer = *command_buffers.get(1).ok_or_else(|| {
            "vkAllocateCommandBuffers returned no coding-scope probe command buffer".to_string()
        })?;
        let pre_encode_scope_probe_command_buffer = *command_buffers.get(2).ok_or_else(|| {
            "vkAllocateCommandBuffers returned no pre-encode scope-only probe command buffer"
                .to_string()
        })?;
        let pre_encode_probe_command_buffer = *command_buffers.get(3).ok_or_else(|| {
            "vkAllocateCommandBuffers returned no pre-encode probe command buffer".to_string()
        })?;
        let pre_encode_minimal_probe_command_buffer = *command_buffers.get(4).ok_or_else(|| {
            "vkAllocateCommandBuffers returned no pre-encode minimal probe command buffer"
                .to_string()
        })?;
        let image_width = u32::try_from(align_up(
            u64::from(config.coded_width),
            u64::from(config.picture_access_granularity.width.max(1)),
        ))
        .map_err(|_| "aligned encode source image width exceeds u32 range".to_string())?;
        let image_height = u32::try_from(align_up(
            u64::from(config.coded_height),
            u64::from(config.picture_access_granularity.height.max(1)),
        ))
        .map_err(|_| "aligned encode source image height exceeds u32 range".to_string())?;
        let (created_source_image, created_source_image_memory, created_source_image_view) =
            create_hevc_encode_probe_image(
                device,
                instance,
                HevcEncodeProbeImageConfig {
                    physical_device: config.physical_device,
                    queue_family_index: config.queue_family_index,
                    image_width,
                    image_height,
                    picture_format: config.picture_format,
                    usage: vk::ImageUsageFlags::VIDEO_ENCODE_SRC_KHR
                        | vk::ImageUsageFlags::TRANSFER_DST,
                },
            )?;
        source_image = created_source_image;
        source_image_memory = created_source_image_memory;
        source_image_view = created_source_image_view;
        let (created_dpb_image, created_dpb_image_memory, created_dpb_image_view) =
            create_hevc_encode_probe_image(
                device,
                instance,
                HevcEncodeProbeImageConfig {
                    physical_device: config.physical_device,
                    queue_family_index: config.queue_family_index,
                    image_width,
                    image_height,
                    picture_format: config.reference_picture_format,
                    usage: vk::ImageUsageFlags::VIDEO_ENCODE_DPB_KHR,
                },
            )?;
        dpb_image = created_dpb_image;
        dpb_image_memory = created_dpb_image_memory;
        dpb_image_view = created_dpb_image_view;
        let base_dst_size = u64::from(config.coded_width)
            .saturating_mul(u64::from(config.coded_height))
            .saturating_mul(4)
            .max(1_048_576);
        let dst_offset_alignment = config.min_bitstream_buffer_offset_alignment.max(1);
        let synthetic_prefix_bytes = 0_u64;
        let dst_buffer_offset = 0_u64;
        if !dst_buffer_offset.is_multiple_of(dst_offset_alignment) {
            return Err(format!(
                "encode dst buffer offset {dst_buffer_offset} is not aligned to {dst_offset_alignment}"
            ));
        }
        let dst_buffer_range = align_up(
            base_dst_size,
            config.min_bitstream_buffer_size_alignment.max(1),
        );
        if dst_buffer_range == 0 {
            return Err("encode dst buffer range resolved to zero".to_string());
        }
        let dst_buffer_size = dst_buffer_offset
            .checked_add(dst_buffer_range)
            .ok_or_else(|| "encode dst buffer size overflow".to_string())?;
        let (created_dst_buffer, created_dst_buffer_memory) = create_hevc_encode_probe_dst_buffer(
            device,
            instance,
            config.physical_device,
            dst_buffer_size,
        )?;
        dst_buffer = created_dst_buffer;
        dst_buffer_memory = created_dst_buffer_memory;
        query_pool =
            create_hevc_encode_feedback_query_pool(device, config.supported_encode_feedback_flags)?;
        let requested_rate_control_mode = resolve_hevc_encode_probe_rate_control_mode();
        let selected_rate_control_mode = resolve_hevc_encode_probe_selected_rate_control_mode(
            requested_rate_control_mode,
            config.rate_control_modes,
            config.max_rate_control_layers,
        );
        let reference_list_mode = resolve_hevc_encode_probe_reference_list_mode();
        let reference_index_mode = resolve_hevc_encode_probe_reference_index_mode();
        let rps_mode = resolve_hevc_encode_probe_rps_mode();
        let begin_reference_slot_mode = resolve_hevc_encode_probe_begin_reference_slot_mode();
        let setup_reference_slot_mode = resolve_hevc_encode_probe_setup_reference_slot_mode();
        let control_mode = resolve_hevc_encode_probe_control_mode();
        let nalu_mode = resolve_hevc_encode_probe_nalu_mode();
        let codec_info_mode = resolve_hevc_encode_probe_codec_info_mode();
        let picture_flags_mode = resolve_hevc_encode_probe_picture_flags_mode();
        let picture_info_mode = resolve_hevc_encode_probe_picture_info_mode();
        let picture_type = hevc_encode_probe_picture_type(picture_info_mode);
        let slice_type = hevc_encode_probe_slice_type(picture_info_mode);
        let temporal_id = hevc_encode_probe_temporal_id(picture_info_mode);
        let pic_order_cnt_val = hevc_encode_probe_pic_order_cnt_val(picture_info_mode);
        let reference_idx_minus1 = hevc_encode_probe_reference_idx_minus1(reference_index_mode);
        let selected_quality_level = config.max_quality_levels.saturating_sub(1);
        let control_config = HevcEncodeProbeControlConfig {
            selected_rate_control_mode,
            selected_quality_level,
            enable_quality_level_control: config.max_quality_levels > 0,
            control_mode,
        };
        let selected_rate_control_mode_label = selected_rate_control_mode
            .map(|mode| format!("{mode:?}"))
            .unwrap_or_else(|| "none".to_string());
        let requested_rate_control_mode_label =
            hevc_encode_probe_rate_control_mode_label(requested_rate_control_mode);
        let parameter_mode_label = hevc_encode_parameter_mode_label(config.parameter_mode);
        let constant_qp = hevc_encode_probe_constant_qp(selected_rate_control_mode);
        let slice_qp_delta =
            hevc_encode_probe_slice_qp_delta(constant_qp, config.parameter_set_pps_init_qp_minus26)
                .map_err(|err| format!("{err} for submit probe"))?;
        let parameter_feedback_probe = config
            .parameter_feedback_probe_summary
            .clone()
            .unwrap_or_else(|| "none".to_string());
        let reference_list_mode_label =
            hevc_encode_probe_reference_list_mode_label(reference_list_mode);
        let reference_index_mode_label =
            hevc_encode_probe_reference_index_mode_label(reference_index_mode);
        let rps_mode_label = hevc_encode_probe_rps_mode_label(rps_mode);
        let begin_reference_slot_mode_label =
            hevc_encode_probe_begin_reference_slot_mode_label(begin_reference_slot_mode);
        let setup_reference_slot_mode_label =
            hevc_encode_probe_setup_reference_slot_mode_label(setup_reference_slot_mode);
        let begin_session_parameters_mode =
            resolve_hevc_encode_probe_begin_session_parameters_mode();
        let begin_session_parameters_mode_label =
            hevc_encode_probe_begin_session_parameters_mode_label(begin_session_parameters_mode);
        let control_mode_label = hevc_encode_probe_control_mode_label(control_mode);
        let nalu_mode_label = hevc_encode_probe_nalu_mode_label(nalu_mode);
        let codec_info_mode_label = hevc_encode_probe_codec_info_mode_label(codec_info_mode);
        let primary_probe_mode = resolve_hevc_encode_probe_primary_mode();
        let primary_probe_mode_label = hevc_encode_primary_probe_mode_label(primary_probe_mode);
        let picture_flags_mode_label =
            hevc_encode_probe_picture_flags_mode_label(picture_flags_mode);
        let picture_info_mode_label = hevc_encode_probe_picture_info_mode_label(picture_info_mode);
        let session_dpb_mode_label =
            hevc_encode_probe_session_dpb_mode_label(config.session_dpb_mode);
        let picture_resource_extent_mode = resolve_hevc_encode_probe_picture_resource_extent_mode();
        let (picture_resource_coded_width, picture_resource_coded_height) =
            match picture_resource_extent_mode {
                HevcEncodeProbePictureResourceExtentMode::Coded => {
                    (config.coded_width, config.coded_height)
                }
                HevcEncodeProbePictureResourceExtentMode::ImageAligned => {
                    (image_width, image_height)
                }
            };
        let picture_resource_extent_mode_label =
            hevc_encode_probe_picture_resource_extent_mode_label(picture_resource_extent_mode);
        let encode_probe_context = format!(
            "encode_probe_inputs(coded={}x{}, image={}x{}, picture_resource_coded={}x{}, picture_resource_extent_mode={}, picture_format={:?}, reference_picture_format={:?}, dst_offset={}, dst_range={}, dst_prefix={}, dst_offset_align={}, dst_size_align={}, parameter_mode={}, parameter_set_ids=vps:{}|sps:{}|pps:{}, parameter_set_coded={}x{}, parameter_set_coded_match={}, parameter_set_pps_init_qp_minus26={}, parameter_set_sao={}, parameter_set_temporal_mvp={}, parameter_feedback_probe={}, reference_list_mode={}, reference_idx_mode={}, reference_idx_minus1={}, rps_mode={}, begin_reference_slot_mode={}, setup_reference_slot_mode={}, begin_session_parameters_mode={}, control_mode={}, nalu_mode={}, codec_info_mode={}, primary_mode={}, picture_flags_mode={}, picture_info_mode={}, pic_order_cnt_val={}, temporal_id={}, constant_qp={}, slice_qp_delta={}, requested_rate_control_mode={}, rate_control_mode={}, max_rate_control_layers={}, quality_level={}, quality_level_control={}, maintenance1_mode={}, maintenance1_feature_enabled={}, session_dpb_mode={}, session_max_dpb_slots={}, session_max_active_refs={})",
            config.coded_width,
            config.coded_height,
            image_width,
            image_height,
            picture_resource_coded_width,
            picture_resource_coded_height,
            picture_resource_extent_mode_label,
            config.picture_format,
            config.reference_picture_format,
            dst_buffer_offset,
            dst_buffer_range,
            synthetic_prefix_bytes,
            config.min_bitstream_buffer_offset_alignment.max(1),
            config.min_bitstream_buffer_size_alignment.max(1),
            parameter_mode_label,
            config.parameter_set_ids.vps_id,
            config.parameter_set_ids.sps_id,
            config.parameter_set_ids.pps_id,
            config.parameter_set_coded_width,
            config.parameter_set_coded_height,
            config.parameter_set_coded_width == config.coded_width
                && config.parameter_set_coded_height == config.coded_height,
            config.parameter_set_pps_init_qp_minus26,
            config.parameter_set_sample_adaptive_offset_enabled,
            config.parameter_set_sps_temporal_mvp_enabled,
            parameter_feedback_probe,
            reference_list_mode_label,
            reference_index_mode_label,
            reference_idx_minus1,
            rps_mode_label,
            begin_reference_slot_mode_label,
            setup_reference_slot_mode_label,
            begin_session_parameters_mode_label,
            control_mode_label,
            nalu_mode_label,
            codec_info_mode_label,
            primary_probe_mode_label,
            picture_flags_mode_label,
            picture_info_mode_label,
            pic_order_cnt_val,
            temporal_id,
            constant_qp,
            slice_qp_delta,
            requested_rate_control_mode_label,
            selected_rate_control_mode_label,
            config.max_rate_control_layers,
            selected_quality_level,
            control_config.enable_quality_level_control,
            hevc_encode_probe_maintenance1_mode_label(config.maintenance1_mode),
            config.maintenance1_feature_enabled,
            session_dpb_mode_label,
            config.session_max_dpb_slots,
            config.session_max_active_reference_pictures,
        );

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: command buffer is valid and not recording.
        unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
            .map_err(|err| format!("vkBeginCommandBuffer failed: {err}"))?;

        let source_image_prepare_barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .src_access_mask(vk::AccessFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(source_image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let source_prepare_dependency = vk::DependencyInfo::default()
            .image_memory_barriers(std::slice::from_ref(&source_image_prepare_barrier));
        let clear_color = vk::ClearColorValue { uint32: [0; 4] };
        let clear_range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let source_image_encode_barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
            .dst_access_mask(vk::AccessFlags2::VIDEO_ENCODE_READ_KHR)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::VIDEO_ENCODE_SRC_KHR)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(source_image)
            .subresource_range(clear_range);
        let dpb_image_encode_barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .src_access_mask(vk::AccessFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
            .dst_access_mask(
                vk::AccessFlags2::VIDEO_ENCODE_READ_KHR | vk::AccessFlags2::VIDEO_ENCODE_WRITE_KHR,
            )
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::VIDEO_ENCODE_DPB_KHR)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(dpb_image)
            .subresource_range(clear_range);
        let dst_buffer_barrier = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .src_access_mask(vk::AccessFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
            .dst_access_mask(vk::AccessFlags2::VIDEO_ENCODE_WRITE_KHR)
            .buffer(dst_buffer)
            .offset(0)
            .size(dst_buffer_size);
        let encode_image_barriers = [source_image_encode_barrier, dpb_image_encode_barrier];
        let dependency_info = vk::DependencyInfo::default()
            .image_memory_barriers(&encode_image_barriers)
            .buffer_memory_barriers(std::slice::from_ref(&dst_buffer_barrier));
        // SAFETY: barriers reference resources created above and command buffer is recording.
        unsafe {
            device.cmd_reset_query_pool(command_buffer, query_pool, 0, 1);
            device.cmd_pipeline_barrier2(command_buffer, &source_prepare_dependency);
            device.cmd_clear_color_image(
                command_buffer,
                source_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &clear_color,
                std::slice::from_ref(&clear_range),
            );
            device.cmd_pipeline_barrier2(command_buffer, &dependency_info);
        }

        let reconstructed_picture_resource = vk::VideoPictureResourceInfoKHR::default()
            .coded_offset(vk::Offset2D::default())
            .coded_extent(vk::Extent2D {
                width: picture_resource_coded_width,
                height: picture_resource_coded_height,
            })
            .base_array_layer(0)
            .image_view_binding(dpb_image_view);
        let setup_reference_info = StdVideoEncodeH265ReferenceInfo {
            flags: StdVideoEncodeH265ReferenceInfoFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
            },
            pic_type: StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR,
            PicOrderCntVal: 0,
            TemporalId: 0,
        };
        let mut setup_dpb_slot_info =
            vk::VideoEncodeH265DpbSlotInfoKHR::default().std_reference_info(&setup_reference_info);
        let setup_reference_slot = vk::VideoReferenceSlotInfoKHR::default()
            .slot_index(0)
            .picture_resource(&reconstructed_picture_resource)
            .push_next(&mut setup_dpb_slot_info);
        let begin_reference_info = StdVideoEncodeH265ReferenceInfo {
            flags: StdVideoEncodeH265ReferenceInfoFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
            },
            pic_type: StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR,
            PicOrderCntVal: 0,
            TemporalId: 0,
        };
        let mut begin_dpb_slot_info =
            vk::VideoEncodeH265DpbSlotInfoKHR::default().std_reference_info(&begin_reference_info);
        let begin_reference_slots = [vk::VideoReferenceSlotInfoKHR::default()
            .slot_index(-1)
            .picture_resource(&reconstructed_picture_resource)
            .push_next(&mut begin_dpb_slot_info)];

        let begin_coding_info_base = vk::VideoBeginCodingInfoKHR::default()
            .video_session(config.video_session)
            .video_session_parameters(config.video_session_parameters);
        let begin_coding_info_with_ref_slots = match begin_reference_slot_mode {
            HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOne => {
                begin_coding_info_base.reference_slots(&begin_reference_slots)
            }
            HevcEncodeProbeBeginReferenceSlotMode::None => begin_coding_info_base,
        };
        let begin_coding_info_without_session_parameters =
            vk::VideoBeginCodingInfoKHR::default().video_session(config.video_session);
        let begin_coding_info = match begin_session_parameters_mode {
            HevcEncodeProbeBeginSessionParametersMode::With => begin_coding_info_with_ref_slots,
            HevcEncodeProbeBeginSessionParametersMode::Without => match begin_reference_slot_mode {
                HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOne => {
                    begin_coding_info_without_session_parameters
                        .reference_slots(&begin_reference_slots)
                }
                HevcEncodeProbeBeginReferenceSlotMode::None => {
                    begin_coding_info_without_session_parameters
                }
            },
        };
        if matches!(primary_probe_mode, HevcEncodePrimaryProbeMode::ScopeOnly) {
            let scope_only_result = run_hevc_encode_coding_scope_probe(
                device,
                &video_queue_device,
                command_buffer,
                config.video_session,
                config.video_session_parameters,
                begin_session_parameters_mode,
                control_config,
            )
            .map_err(|err| {
                format!("scope-only primary probe failed: {err}; {encode_probe_context}")
            });
            return scope_only_result;
        }

        let pre_encode_resources = HevcEncodePreEncodeProbeResources {
            source_image,
            source_image_view,
            dpb_image,
            dpb_image_view,
            picture_resource_coded_width,
            picture_resource_coded_height,
            dst_buffer,
            dst_buffer_size,
        };
        match primary_probe_mode {
            HevcEncodePrimaryProbeMode::PreEncodeScopeOnly => {
                let pre_encode_scope_result = run_hevc_encode_pre_encode_probe(
                    device,
                    &video_queue_device,
                    &video_encode_device,
                    HevcEncodePreEncodeProbeConfig {
                        command_buffer,
                        video_session: config.video_session,
                        video_session_parameters: config.video_session_parameters,
                        control_config,
                        mode: HevcEncodePreEncodeProbeMode::ScopeOnly,
                        begin_session_parameters_mode,
                        nalu_mode,
                        codec_info_mode,
                        sample_adaptive_offset_enabled: config
                            .parameter_set_sample_adaptive_offset_enabled,
                        sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                        resources: pre_encode_resources,
                    },
                )
                .map_err(|err| {
                    format!(
                        "pre-encode-scope-only primary probe failed: {err}; {encode_probe_context}"
                    )
                });
                return pre_encode_scope_result;
            }
            HevcEncodePrimaryProbeMode::PreEncode => {
                let pre_encode_result = run_hevc_encode_pre_encode_probe(
                    device,
                    &video_queue_device,
                    &video_encode_device,
                    HevcEncodePreEncodeProbeConfig {
                        command_buffer,
                        video_session: config.video_session,
                        video_session_parameters: config.video_session_parameters,
                        control_config,
                        mode: HevcEncodePreEncodeProbeMode::WithEncode,
                        begin_session_parameters_mode,
                        nalu_mode,
                        codec_info_mode,
                        sample_adaptive_offset_enabled: config
                            .parameter_set_sample_adaptive_offset_enabled,
                        sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                        resources: pre_encode_resources,
                    },
                )
                .map_err(|err| {
                    format!("pre-encode primary probe failed: {err}; {encode_probe_context}")
                });
                return pre_encode_result;
            }
            HevcEncodePrimaryProbeMode::PreEncodeMinimal => {
                let pre_encode_minimal_result = run_hevc_encode_pre_encode_probe(
                    device,
                    &video_queue_device,
                    &video_encode_device,
                    HevcEncodePreEncodeProbeConfig {
                        command_buffer,
                        video_session: config.video_session,
                        video_session_parameters: config.video_session_parameters,
                        control_config,
                        mode: HevcEncodePreEncodeProbeMode::WithEncodeMinimal,
                        begin_session_parameters_mode,
                        nalu_mode,
                        codec_info_mode,
                        sample_adaptive_offset_enabled: config
                            .parameter_set_sample_adaptive_offset_enabled,
                        sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                        resources: pre_encode_resources,
                    },
                )
                .map_err(|err| {
                    format!(
                        "pre-encode-minimal primary probe failed: {err}; {encode_probe_context}"
                    )
                });
                return pre_encode_minimal_result;
            }
            HevcEncodePrimaryProbeMode::Submit => {}
            HevcEncodePrimaryProbeMode::ScopeOnly => unreachable!(),
        }

        let rate_control_mode = control_config
            .selected_rate_control_mode
            .unwrap_or(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED);
        let mut rate_control_layer_h265 = vk::VideoEncodeH265RateControlLayerInfoKHR::default();
        let rate_control_layer = vk::VideoEncodeRateControlLayerInfoKHR::default()
            .average_bitrate(8_000_000)
            .max_bitrate(8_000_000)
            .frame_rate_numerator(30)
            .frame_rate_denominator(1)
            .push_next(&mut rate_control_layer_h265);
        let mut rate_control_info =
            vk::VideoEncodeRateControlInfoKHR::default().rate_control_mode(rate_control_mode);
        let mut coding_control_info =
            vk::VideoCodingControlInfoKHR::default().flags(vk::VideoCodingControlFlagsKHR::RESET);
        let mut quality_level_info = vk::VideoEncodeQualityLevelInfoKHR::default()
            .quality_level(control_config.selected_quality_level);
        let mut issue_coding_control = true;
        let h265_rate_control_info = hevc_encode_h265_rate_control_info();
        match control_config.control_mode {
            HevcEncodeProbeControlMode::Default => {
                if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
                    rate_control_info.p_next = std::ptr::from_ref(&h265_rate_control_info).cast();
                    rate_control_info = rate_control_info
                        .layers(std::slice::from_ref(&rate_control_layer))
                        .virtual_buffer_size_in_ms(1000)
                        .initial_virtual_buffer_size_in_ms(1000);
                    coding_control_info = coding_control_info
                        .flags(
                            vk::VideoCodingControlFlagsKHR::RESET
                                | vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL,
                        )
                        .push_next(&mut rate_control_info);
                }
                if control_config.enable_quality_level_control {
                    coding_control_info = coding_control_info
                        .flags(
                            coding_control_info.flags
                                | vk::VideoCodingControlFlagsKHR::ENCODE_QUALITY_LEVEL,
                        )
                        .push_next(&mut quality_level_info);
                }
            }
            HevcEncodeProbeControlMode::Ffmpeg => {
                rate_control_info.p_next = std::ptr::from_ref(&h265_rate_control_info).cast();
                if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
                    rate_control_info = rate_control_info
                        .layers(std::slice::from_ref(&rate_control_layer))
                        .virtual_buffer_size_in_ms(1000)
                        .initial_virtual_buffer_size_in_ms(1000);
                }
                coding_control_info = coding_control_info
                    .flags(
                        vk::VideoCodingControlFlagsKHR::RESET
                            | vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL,
                    )
                    .push_next(&mut rate_control_info);
                if control_config.enable_quality_level_control {
                    coding_control_info = coding_control_info
                        .flags(
                            coding_control_info.flags
                                | vk::VideoCodingControlFlagsKHR::ENCODE_QUALITY_LEVEL,
                        )
                        .push_next(&mut quality_level_info);
                }
            }
            HevcEncodeProbeControlMode::None => {
                issue_coding_control = false;
            }
        }
        // SAFETY: command buffer is recording and session handles are valid.
        unsafe {
            (video_queue_device.fp().cmd_begin_video_coding_khr)(
                command_buffer,
                &begin_coding_info,
            );
            if issue_coding_control {
                (video_queue_device.fp().cmd_control_video_coding_khr)(
                    command_buffer,
                    &coding_control_info,
                );
            }
        }

        let std_slice_flags = hevc_encode_std_slice_segment_header_flags(
            config.parameter_set_sample_adaptive_offset_enabled,
        );
        let std_slice_header = StdVideoEncodeH265SliceSegmentHeader {
            flags: std_slice_flags,
            slice_type,
            slice_segment_address: 0,
            collocated_ref_idx: 0,
            MaxNumMergeCand: 5,
            slice_cb_qp_offset: 0,
            slice_cr_qp_offset: 0,
            slice_beta_offset_div2: 0,
            slice_tc_offset_div2: 0,
            slice_act_y_qp_offset: 0,
            slice_act_cb_qp_offset: 0,
            slice_act_cr_qp_offset: 0,
            slice_qp_delta,
            reserved1: 0,
            pWeightTable: std::ptr::null(),
        };
        let nalu_slice_entries = [vk::VideoEncodeH265NaluSliceSegmentInfoKHR::default()
            .constant_qp(constant_qp)
            .std_slice_segment_header(&std_slice_header)];
        let nalu_entries: &[vk::VideoEncodeH265NaluSliceSegmentInfoKHR] = match nalu_mode {
            HevcEncodeProbeNaluMode::SingleSlice => &nalu_slice_entries,
            HevcEncodeProbeNaluMode::Empty => &[],
        };
        let mut std_reference_list_flags =
            ash::vk::native::StdVideoEncodeH265ReferenceListsInfoFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
            };
        std_reference_list_flags.set_ref_pic_list_modification_flag_l0(0);
        std_reference_list_flags.set_ref_pic_list_modification_flag_l1(0);
        let reference_list_sentinel = match reference_list_mode {
            HevcEncodeProbeReferenceListMode::Sentinel => HEVC_NO_REFERENCE_PICTURE,
            HevcEncodeProbeReferenceListMode::Zero => 0,
            HevcEncodeProbeReferenceListMode::NullPointers => HEVC_NO_REFERENCE_PICTURE,
        };
        let std_reference_lists = StdVideoEncodeH265ReferenceListsInfo {
            flags: std_reference_list_flags,
            num_ref_idx_l0_active_minus1: reference_idx_minus1,
            num_ref_idx_l1_active_minus1: reference_idx_minus1,
            RefPicList0: [reference_list_sentinel; 15],
            RefPicList1: [reference_list_sentinel; 15],
            list_entry_l0: [0; 15],
            list_entry_l1: [0; 15],
        };
        let reference_lists_ptr = match reference_list_mode {
            HevcEncodeProbeReferenceListMode::NullPointers => std::ptr::null(),
            HevcEncodeProbeReferenceListMode::Sentinel | HevcEncodeProbeReferenceListMode::Zero => {
                &std_reference_lists as *const StdVideoEncodeH265ReferenceListsInfo
            }
        };
        let std_short_term_ref_pic_set = StdVideoH265ShortTermRefPicSet {
            flags: ash::vk::native::StdVideoH265ShortTermRefPicSetFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
                __bindgen_padding_0: [0; 3],
            },
            delta_idx_minus1: 0,
            use_delta_flag: 0,
            abs_delta_rps_minus1: 0,
            used_by_curr_pic_flag: 0,
            used_by_curr_pic_s0_flag: 0,
            used_by_curr_pic_s1_flag: 0,
            reserved1: 0,
            reserved2: 0,
            reserved3: 0,
            num_negative_pics: 0,
            num_positive_pics: 0,
            delta_poc_s0_minus1: [0; 16],
            delta_poc_s1_minus1: [0; 16],
        };
        let std_long_term_ref_pics = StdVideoEncodeH265LongTermRefPics {
            num_long_term_sps: 0,
            num_long_term_pics: 0,
            lt_idx_sps: [0; 32],
            poc_lsb_lt: [0; 16],
            used_by_curr_pic_lt_flag: 0,
            delta_poc_msb_present_flag: [0; 48],
            delta_poc_msb_cycle_lt: [0; 48],
        };
        let (short_term_ref_pic_set_ptr, long_term_ref_pics_ptr) = match rps_mode {
            HevcEncodeProbeRpsMode::EmptyStruct => (
                &std_short_term_ref_pic_set as *const StdVideoH265ShortTermRefPicSet,
                &std_long_term_ref_pics as *const StdVideoEncodeH265LongTermRefPics,
            ),
            HevcEncodeProbeRpsMode::NullPointers => (std::ptr::null(), std::ptr::null()),
        };
        let mut std_picture_flags = StdVideoEncodeH265PictureInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: Default::default(),
        };
        let is_reference = matches!(picture_flags_mode, HevcEncodeProbePictureFlagsMode::Default);
        std_picture_flags.set_is_reference(u32::from(is_reference));
        std_picture_flags.set_IrapPicFlag(u32::from(is_reference));
        std_picture_flags.set_pic_output_flag(1);
        std_picture_flags.set_short_term_ref_pic_set_sps_flag(0);
        std_picture_flags.set_slice_temporal_mvp_enabled_flag(u32::from(
            config.parameter_set_sps_temporal_mvp_enabled,
        ));
        std_picture_flags.set_used_for_long_term_reference(0);
        std_picture_flags.set_discardable_flag(0);
        std_picture_flags.set_cross_layer_bla_flag(0);
        std_picture_flags.set_no_output_of_prior_pics_flag(0);
        let std_picture_info = StdVideoEncodeH265PictureInfo {
            flags: std_picture_flags,
            pic_type: picture_type,
            sps_video_parameter_set_id: config.parameter_set_ids.vps_id,
            pps_seq_parameter_set_id: config.parameter_set_ids.sps_id,
            pps_pic_parameter_set_id: config.parameter_set_ids.pps_id,
            short_term_ref_pic_set_idx: 0,
            PicOrderCntVal: pic_order_cnt_val,
            TemporalId: temporal_id,
            reserved1: [0; 7],
            pRefLists: reference_lists_ptr,
            pShortTermRefPicSet: short_term_ref_pic_set_ptr,
            pLongTermRefPics: long_term_ref_pics_ptr,
        };
        let mut h265_encode_info = vk::VideoEncodeH265PictureInfoKHR::default()
            .nalu_slice_segment_entries(nalu_entries)
            .std_picture_info(&std_picture_info);
        let src_picture_resource = vk::VideoPictureResourceInfoKHR::default()
            .coded_offset(vk::Offset2D::default())
            .coded_extent(vk::Extent2D {
                width: picture_resource_coded_width,
                height: picture_resource_coded_height,
            })
            .base_array_layer(0)
            .image_view_binding(source_image_view);
        let encode_info = vk::VideoEncodeInfoKHR::default()
            .dst_buffer(dst_buffer)
            .dst_buffer_offset(dst_buffer_offset)
            .dst_buffer_range(dst_buffer_range)
            .src_picture_resource(src_picture_resource)
            .push_next(&mut h265_encode_info);
        let encode_info = match setup_reference_slot_mode {
            HevcEncodeProbeSetupReferenceSlotMode::SlotZero => {
                encode_info.setup_reference_slot(&setup_reference_slot)
            }
            HevcEncodeProbeSetupReferenceSlotMode::None => encode_info,
        };
        // SAFETY: encode info references local data that remains valid through the call.
        unsafe {
            device.cmd_begin_query(
                command_buffer,
                query_pool,
                0,
                vk::QueryControlFlags::empty(),
            );
            (video_encode_device.fp().cmd_encode_video_khr)(command_buffer, &encode_info);
            device.cmd_end_query(command_buffer, query_pool, 0);
        }

        let end_coding_info = vk::VideoEndCodingInfoKHR::default();
        // SAFETY: command buffer is recording and video coding scope was opened.
        unsafe {
            (video_queue_device.fp().cmd_end_video_coding_khr)(command_buffer, &end_coding_info);
        }

        // SAFETY: command buffer is valid and currently recording.
        if let Err(err) = unsafe { device.end_command_buffer(command_buffer) } {
            let coding_scope_probe = run_hevc_encode_coding_scope_probe(
                device,
                &video_queue_device,
                coding_scope_probe_command_buffer,
                config.video_session,
                config.video_session_parameters,
                begin_session_parameters_mode,
                control_config,
            )
            .map(|()| "ok".to_string())
            .unwrap_or_else(|probe_err| format!("failed ({probe_err})"));
            let pre_encode_scope_probe = run_hevc_encode_pre_encode_probe(
                device,
                &video_queue_device,
                &video_encode_device,
                HevcEncodePreEncodeProbeConfig {
                    command_buffer: pre_encode_scope_probe_command_buffer,
                    video_session: config.video_session,
                    video_session_parameters: config.video_session_parameters,
                    control_config,
                    mode: HevcEncodePreEncodeProbeMode::ScopeOnly,
                    begin_session_parameters_mode,
                    nalu_mode,
                    codec_info_mode,
                    sample_adaptive_offset_enabled: config
                        .parameter_set_sample_adaptive_offset_enabled,
                    sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                    resources: HevcEncodePreEncodeProbeResources {
                        source_image,
                        source_image_view,
                        dpb_image,
                        dpb_image_view,
                        picture_resource_coded_width,
                        picture_resource_coded_height,
                        dst_buffer,
                        dst_buffer_size,
                    },
                },
            )
            .map(|()| "ok".to_string())
            .unwrap_or_else(|probe_err| format!("failed ({probe_err})"));
            let pre_encode_probe = run_hevc_encode_pre_encode_probe(
                device,
                &video_queue_device,
                &video_encode_device,
                HevcEncodePreEncodeProbeConfig {
                    command_buffer: pre_encode_probe_command_buffer,
                    video_session: config.video_session,
                    video_session_parameters: config.video_session_parameters,
                    control_config,
                    mode: HevcEncodePreEncodeProbeMode::WithEncode,
                    begin_session_parameters_mode,
                    nalu_mode,
                    codec_info_mode,
                    sample_adaptive_offset_enabled: config
                        .parameter_set_sample_adaptive_offset_enabled,
                    sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                    resources: HevcEncodePreEncodeProbeResources {
                        source_image,
                        source_image_view,
                        dpb_image,
                        dpb_image_view,
                        picture_resource_coded_width,
                        picture_resource_coded_height,
                        dst_buffer,
                        dst_buffer_size,
                    },
                },
            )
            .map(|()| "ok".to_string())
            .unwrap_or_else(|probe_err| format!("failed ({probe_err})"));
            let pre_encode_minimal_probe = run_hevc_encode_pre_encode_probe(
                device,
                &video_queue_device,
                &video_encode_device,
                HevcEncodePreEncodeProbeConfig {
                    command_buffer: pre_encode_minimal_probe_command_buffer,
                    video_session: config.video_session,
                    video_session_parameters: config.video_session_parameters,
                    control_config,
                    mode: HevcEncodePreEncodeProbeMode::WithEncodeMinimal,
                    begin_session_parameters_mode,
                    nalu_mode,
                    codec_info_mode,
                    sample_adaptive_offset_enabled: config
                        .parameter_set_sample_adaptive_offset_enabled,
                    sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                    resources: HevcEncodePreEncodeProbeResources {
                        source_image,
                        source_image_view,
                        dpb_image,
                        dpb_image_view,
                        picture_resource_coded_width,
                        picture_resource_coded_height,
                        dst_buffer,
                        dst_buffer_size,
                    },
                },
            )
            .map(|()| "ok".to_string())
            .unwrap_or_else(|probe_err| format!("failed ({probe_err})"));
            return Err(format!(
                "vkEndCommandBuffer failed: {err}; {encode_probe_context}; coding_scope_probe={coding_scope_probe}; pre_encode_scope_probe={pre_encode_scope_probe}; pre_encode_probe={pre_encode_probe}; pre_encode_minimal_probe={pre_encode_minimal_probe}"
            ));
        }

        // SAFETY: default fence create info is valid.
        fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
            .map_err(|err| format!("vkCreateFence failed: {err}"))?;

        let command_buffers = [command_buffer];
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
        // SAFETY: submit info references command buffers that stay alive through the call.
        unsafe { device.queue_submit(queue, std::slice::from_ref(&submit_info), fence) }
            .map_err(|err| format!("vkQueueSubmit failed: {err}"))?;
        // SAFETY: fence is valid and associated with this queue submit.
        unsafe { device.wait_for_fences(std::slice::from_ref(&fence), true, 1_000_000_000) }
            .map_err(|err| format!("vkWaitForFences failed: {err}"))?;
        query_result = read_hevc_encode_feedback_query(device, query_pool)?;
        if query_result.status != vk::QueryResultStatusKHR::COMPLETE {
            return Err(format!(
                "encode feedback status was not COMPLETE: {:?}",
                query_result.status
            ));
        }
        dst_head16 = read_hevc_encode_dst_head(device, dst_buffer_memory, query_result)
            .unwrap_or_else(|err| format!("unreadable:{err}"));
        Ok(())
    })();

    // SAFETY: synchronization primitives are valid if they were created and are no longer used.
    unsafe {
        if source_image_view != vk::ImageView::null() {
            device.destroy_image_view(source_image_view, None);
        }
        if source_image != vk::Image::null() {
            device.destroy_image(source_image, None);
        }
        if source_image_memory != vk::DeviceMemory::null() {
            device.free_memory(source_image_memory, None);
        }
        if dpb_image_view != vk::ImageView::null() {
            device.destroy_image_view(dpb_image_view, None);
        }
        if dpb_image != vk::Image::null() {
            device.destroy_image(dpb_image, None);
        }
        if dpb_image_memory != vk::DeviceMemory::null() {
            device.free_memory(dpb_image_memory, None);
        }
        if dst_buffer != vk::Buffer::null() {
            device.destroy_buffer(dst_buffer, None);
        }
        if dst_buffer_memory != vk::DeviceMemory::null() {
            device.free_memory(dst_buffer_memory, None);
        }
        if query_pool != vk::QueryPool::null() {
            device.destroy_query_pool(query_pool, None);
        }
        if fence != vk::Fence::null() {
            device.destroy_fence(fence, None);
        }
        if command_pool != vk::CommandPool::null() {
            device.destroy_command_pool(command_pool, None);
        }
    }

    match probe_result {
        Ok(()) => HevcEncodeSubmitExecutionProbe::Ready {
            queue_family_index: config.queue_family_index.0,
            bitstream_buffer_offset: query_result.offset,
            bytes_written: query_result.bytes_written,
            head16: dst_head16,
        },
        Err(err) => {
            HevcEncodeSubmitExecutionProbe::Failed(append_hevc_encode_validation_messages(err))
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct HevcEncodeFeedbackQueryResult {
    offset: u32,
    bytes_written: u32,
    status: vk::QueryResultStatusKHR,
}

impl Default for HevcEncodeFeedbackQueryResult {
    fn default() -> Self {
        Self {
            offset: 0,
            bytes_written: 0,
            status: vk::QueryResultStatusKHR::NOT_READY,
        }
    }
}

fn create_hevc_encode_feedback_query_pool(
    device: &ash::Device,
    supported_flags: vk::VideoEncodeFeedbackFlagsKHR,
) -> Result<vk::QueryPool, String> {
    let flags = vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BYTES_WRITTEN
        | if supported_flags.contains(vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BUFFER_OFFSET) {
            vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BUFFER_OFFSET
        } else {
            vk::VideoEncodeFeedbackFlagsKHR::empty()
        };
    let mut feedback_create_info =
        vk::QueryPoolVideoEncodeFeedbackCreateInfoKHR::default().encode_feedback_flags(flags);
    let mut h265_profile = vk::VideoEncodeH265ProfileInfoKHR::default()
        .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
    let mut encode_usage = vk::VideoEncodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoEncodeUsageFlagsKHR::DEFAULT);
    let mut profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::ENCODE_H265)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut h265_profile)
        .push_next(&mut encode_usage);
    let create_info = vk::QueryPoolCreateInfo::default()
        .query_type(vk::QueryType::VIDEO_ENCODE_FEEDBACK_KHR)
        .query_count(1)
        .push_next(&mut profile)
        .push_next(&mut feedback_create_info);
    // SAFETY: query pool create info references stack data that lives through the call.
    unsafe { device.create_query_pool(&create_info, None) }
        .map_err(|err| format!("vkCreateQueryPool for HEVC encode feedback failed: {err}"))
}

fn read_hevc_encode_feedback_query(
    device: &ash::Device,
    query_pool: vk::QueryPool,
) -> Result<HevcEncodeFeedbackQueryResult, String> {
    let mut result = [HevcEncodeFeedbackQueryResult::default()];
    // SAFETY: query pool is valid, result buffer matches the requested query count.
    unsafe {
        device.get_query_pool_results(
            query_pool,
            0,
            &mut result,
            vk::QueryResultFlags::WAIT | vk::QueryResultFlags::WITH_STATUS_KHR,
        )
    }
    .map_err(|err| format!("vkGetQueryPoolResults for HEVC encode feedback failed: {err}"))?;
    Ok(result[0])
}

fn read_hevc_encode_dst_head(
    device: &ash::Device,
    dst_buffer_memory: vk::DeviceMemory,
    query_result: HevcEncodeFeedbackQueryResult,
) -> Result<String, String> {
    let bytes_to_read = query_result.bytes_written.min(16);
    if bytes_to_read == 0 {
        return Ok(String::new());
    }
    let offset = u64::from(query_result.offset);
    let size = u64::from(bytes_to_read);
    // SAFETY: memory is owned by this device and the requested range is limited to the
    // implementation-reported encoded byte count.
    let ptr =
        unsafe { device.map_memory(dst_buffer_memory, offset, size, vk::MemoryMapFlags::empty()) }
            .map_err(|err| format!("vkMapMemory for HEVC encode dst head failed: {err}"))?;
    // SAFETY: mapped range is valid for `bytes_to_read` bytes until unmap.
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), bytes_to_read as usize) };
    let head = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("");
    // SAFETY: pointer was returned by `vkMapMemory` above.
    unsafe {
        device.unmap_memory(dst_buffer_memory);
    }
    Ok(head)
}

fn create_hevc_encode_probe_image(
    device: &ash::Device,
    instance: &ash::Instance,
    config: HevcEncodeProbeImageConfig,
) -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView), String> {
    let mut encode_h265_profile = vk::VideoEncodeH265ProfileInfoKHR::default()
        .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
    let mut encode_usage = vk::VideoEncodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoEncodeUsageFlagsKHR::DEFAULT);
    let profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::ENCODE_H265)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut encode_h265_profile)
        .push_next(&mut encode_usage);
    let profiles = [profile];
    let mut profile_list = vk::VideoProfileListInfoKHR::default().profiles(&profiles);
    let create_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(config.picture_format)
        .extent(vk::Extent3D {
            width: config.image_width,
            height: config.image_height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(config.usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .queue_family_indices(std::slice::from_ref(&config.queue_family_index.0))
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut profile_list);
    // SAFETY: image create info references local data valid for this call.
    let image = unsafe { device.create_image(&create_info, None) }
        .map_err(|err| format!("vkCreateImage for encode submit probe failed: {err}"))?;
    // SAFETY: image handle is valid and owned by `device`.
    let memory_requirements = unsafe { device.get_image_memory_requirements(image) };
    let memory_type_index = find_memory_type_index(
        config.physical_device,
        instance,
        memory_requirements.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .or_else(|| {
        find_memory_type_index(
            config.physical_device,
            instance,
            memory_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::empty(),
        )
    })
    .ok_or_else(|| "no compatible memory type for encode submit source image".to_string())?;
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(memory_requirements.size)
        .memory_type_index(memory_type_index);
    // SAFETY: allocation info references local data and selected memory type index is valid.
    let image_memory = unsafe { device.allocate_memory(&allocate_info, None) }
        .map_err(|err| format!("vkAllocateMemory for encode submit source image failed: {err}"))?;
    // SAFETY: image and memory were created from this device.
    unsafe { device.bind_image_memory(image, image_memory, 0) }
        .map_err(|err| format!("vkBindImageMemory for encode submit source image failed: {err}"))?;

    let view_usage = if config
        .usage
        .contains(vk::ImageUsageFlags::VIDEO_ENCODE_DPB_KHR)
    {
        vk::ImageUsageFlags::VIDEO_ENCODE_DPB_KHR
    } else if config
        .usage
        .contains(vk::ImageUsageFlags::VIDEO_ENCODE_SRC_KHR)
    {
        vk::ImageUsageFlags::VIDEO_ENCODE_SRC_KHR
    } else {
        config.usage
    };
    let mut image_view_usage = vk::ImageViewUsageCreateInfo::default().usage(view_usage);
    let view_create_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(config.picture_format)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
        .push_next(&mut image_view_usage);
    // SAFETY: image view create info references valid image and stack data.
    let image_view = unsafe { device.create_image_view(&view_create_info, None) }
        .map_err(|err| format!("vkCreateImageView for encode submit source image failed: {err}"))?;
    Ok((image, image_memory, image_view))
}

fn run_hevc_encode_coding_scope_probe(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    command_buffer: vk::CommandBuffer,
    video_session: vk::VideoSessionKHR,
    video_session_parameters: vk::VideoSessionParametersKHR,
    begin_session_parameters_mode: HevcEncodeProbeBeginSessionParametersMode,
    control_config: HevcEncodeProbeControlConfig,
) -> Result<(), String> {
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: command buffer is valid and not recording.
    unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
        .map_err(|err| format!("coding-scope probe vkBeginCommandBuffer failed: {err}"))?;

    let begin_coding_info = match begin_session_parameters_mode {
        HevcEncodeProbeBeginSessionParametersMode::With => vk::VideoBeginCodingInfoKHR::default()
            .video_session(video_session)
            .video_session_parameters(video_session_parameters),
        HevcEncodeProbeBeginSessionParametersMode::Without => {
            vk::VideoBeginCodingInfoKHR::default().video_session(video_session)
        }
    };
    let rate_control_mode = control_config
        .selected_rate_control_mode
        .unwrap_or(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED);
    let mut rate_control_layer_h265 = vk::VideoEncodeH265RateControlLayerInfoKHR::default();
    let rate_control_layer = vk::VideoEncodeRateControlLayerInfoKHR::default()
        .average_bitrate(8_000_000)
        .max_bitrate(8_000_000)
        .frame_rate_numerator(30)
        .frame_rate_denominator(1)
        .push_next(&mut rate_control_layer_h265);
    let mut rate_control_info =
        vk::VideoEncodeRateControlInfoKHR::default().rate_control_mode(rate_control_mode);
    let mut quality_level_info = vk::VideoEncodeQualityLevelInfoKHR::default()
        .quality_level(control_config.selected_quality_level);
    let mut coding_control_info =
        vk::VideoCodingControlInfoKHR::default().flags(vk::VideoCodingControlFlagsKHR::RESET);
    let mut issue_coding_control = true;
    let h265_rate_control_info = hevc_encode_h265_rate_control_info();
    match control_config.control_mode {
        HevcEncodeProbeControlMode::Default => {
            if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
                rate_control_info.p_next = std::ptr::from_ref(&h265_rate_control_info).cast();
                rate_control_info = rate_control_info
                    .layers(std::slice::from_ref(&rate_control_layer))
                    .virtual_buffer_size_in_ms(1000)
                    .initial_virtual_buffer_size_in_ms(1000);
                coding_control_info = coding_control_info
                    .flags(
                        vk::VideoCodingControlFlagsKHR::RESET
                            | vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL,
                    )
                    .push_next(&mut rate_control_info);
            }
            if control_config.enable_quality_level_control {
                coding_control_info = coding_control_info
                    .flags(
                        coding_control_info.flags
                            | vk::VideoCodingControlFlagsKHR::ENCODE_QUALITY_LEVEL,
                    )
                    .push_next(&mut quality_level_info);
            }
        }
        HevcEncodeProbeControlMode::Ffmpeg => {
            rate_control_info.p_next = std::ptr::from_ref(&h265_rate_control_info).cast();
            if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
                rate_control_info = rate_control_info
                    .layers(std::slice::from_ref(&rate_control_layer))
                    .virtual_buffer_size_in_ms(1000)
                    .initial_virtual_buffer_size_in_ms(1000);
            }
            coding_control_info = coding_control_info
                .flags(
                    vk::VideoCodingControlFlagsKHR::RESET
                        | vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL,
                )
                .push_next(&mut rate_control_info);
            if control_config.enable_quality_level_control {
                coding_control_info = coding_control_info
                    .flags(
                        coding_control_info.flags
                            | vk::VideoCodingControlFlagsKHR::ENCODE_QUALITY_LEVEL,
                    )
                    .push_next(&mut quality_level_info);
            }
        }
        HevcEncodeProbeControlMode::None => {
            issue_coding_control = false;
        }
    }
    // SAFETY: command buffer is recording and video session handles are valid.
    unsafe {
        (video_queue_device.fp().cmd_begin_video_coding_khr)(command_buffer, &begin_coding_info);
        if issue_coding_control {
            (video_queue_device.fp().cmd_control_video_coding_khr)(
                command_buffer,
                &coding_control_info,
            );
        }
    }
    let end_coding_info = vk::VideoEndCodingInfoKHR::default();
    // SAFETY: command buffer is recording and coding scope was opened above.
    unsafe {
        (video_queue_device.fp().cmd_end_video_coding_khr)(command_buffer, &end_coding_info);
    }
    // SAFETY: command buffer is valid and recording.
    unsafe { device.end_command_buffer(command_buffer) }
        .map_err(|err| format!("coding-scope probe vkEndCommandBuffer failed: {err}"))?;
    Ok(())
}

fn run_hevc_encode_pre_encode_probe(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_encode_device: &ash::khr::video_encode_queue::Device,
    config: HevcEncodePreEncodeProbeConfig,
) -> Result<(), String> {
    let command_buffer = config.command_buffer;
    let video_session = config.video_session;
    let video_session_parameters = config.video_session_parameters;
    let begin_session_parameters_mode = config.begin_session_parameters_mode;
    let control_config = config.control_config;
    let mode = config.mode;
    let mode_label = hevc_encode_pre_encode_probe_mode_label(mode);
    let nalu_mode = config.nalu_mode;
    let codec_info_mode = config.codec_info_mode;
    let picture_flags_mode = resolve_hevc_encode_probe_picture_flags_mode();
    let picture_info_mode = resolve_hevc_encode_probe_picture_info_mode();
    let picture_type = hevc_encode_probe_picture_type(picture_info_mode);
    let slice_type = hevc_encode_probe_slice_type(picture_info_mode);
    let temporal_id = hevc_encode_probe_temporal_id(picture_info_mode);
    let pic_order_cnt_val = hevc_encode_probe_pic_order_cnt_val(picture_info_mode);
    let resources = config.resources;
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: command buffer is valid and not recording.
    unsafe { device.begin_command_buffer(command_buffer, &begin_info) }.map_err(|err| {
        format!("pre-encode probe ({mode_label}) vkBeginCommandBuffer failed: {err}")
    })?;

    let source_image_prepare_barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(resources.source_image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    let source_prepare_dependency = vk::DependencyInfo::default()
        .image_memory_barriers(std::slice::from_ref(&source_image_prepare_barrier));
    let clear_color = vk::ClearColorValue { uint32: [0; 4] };
    let clear_range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    };
    let source_image_encode_barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
        .dst_access_mask(vk::AccessFlags2::VIDEO_ENCODE_READ_KHR)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::VIDEO_ENCODE_SRC_KHR)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(resources.source_image)
        .subresource_range(clear_range);
    let dpb_image_encode_barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
        .dst_access_mask(vk::AccessFlags2::VIDEO_ENCODE_WRITE_KHR)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::VIDEO_ENCODE_DPB_KHR)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(resources.dpb_image)
        .subresource_range(clear_range);
    let dst_buffer_barrier = vk::BufferMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
        .dst_access_mask(vk::AccessFlags2::VIDEO_ENCODE_WRITE_KHR)
        .buffer(resources.dst_buffer)
        .offset(0)
        .size(resources.dst_buffer_size);
    let pre_encode_image_barriers = [source_image_encode_barrier, dpb_image_encode_barrier];
    let dependency_info = vk::DependencyInfo::default()
        .image_memory_barriers(&pre_encode_image_barriers)
        .buffer_memory_barriers(std::slice::from_ref(&dst_buffer_barrier));
    // SAFETY: barriers reference resources passed by the caller and command buffer is recording.
    unsafe {
        device.cmd_pipeline_barrier2(command_buffer, &source_prepare_dependency);
        device.cmd_clear_color_image(
            command_buffer,
            resources.source_image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &clear_color,
            std::slice::from_ref(&clear_range),
        );
        device.cmd_pipeline_barrier2(command_buffer, &dependency_info);
    }

    let begin_coding_info = match begin_session_parameters_mode {
        HevcEncodeProbeBeginSessionParametersMode::With => vk::VideoBeginCodingInfoKHR::default()
            .video_session(video_session)
            .video_session_parameters(video_session_parameters),
        HevcEncodeProbeBeginSessionParametersMode::Without => {
            vk::VideoBeginCodingInfoKHR::default().video_session(video_session)
        }
    };
    let rate_control_mode = control_config
        .selected_rate_control_mode
        .unwrap_or(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED);
    let mut rate_control_layer_h265 = vk::VideoEncodeH265RateControlLayerInfoKHR::default();
    let rate_control_layer = vk::VideoEncodeRateControlLayerInfoKHR::default()
        .average_bitrate(8_000_000)
        .max_bitrate(8_000_000)
        .frame_rate_numerator(30)
        .frame_rate_denominator(1)
        .push_next(&mut rate_control_layer_h265);
    let mut rate_control_info =
        vk::VideoEncodeRateControlInfoKHR::default().rate_control_mode(rate_control_mode);
    let mut quality_level_info = vk::VideoEncodeQualityLevelInfoKHR::default()
        .quality_level(control_config.selected_quality_level);
    let mut coding_control_info =
        vk::VideoCodingControlInfoKHR::default().flags(vk::VideoCodingControlFlagsKHR::RESET);
    let mut issue_coding_control = true;
    match control_config.control_mode {
        HevcEncodeProbeControlMode::Default => {
            if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
                rate_control_info = rate_control_info
                    .layers(std::slice::from_ref(&rate_control_layer))
                    .virtual_buffer_size_in_ms(1000)
                    .initial_virtual_buffer_size_in_ms(1000);
                coding_control_info = coding_control_info
                    .flags(
                        vk::VideoCodingControlFlagsKHR::RESET
                            | vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL,
                    )
                    .push_next(&mut rate_control_info);
            }
            if control_config.enable_quality_level_control {
                coding_control_info = coding_control_info
                    .flags(
                        coding_control_info.flags
                            | vk::VideoCodingControlFlagsKHR::ENCODE_QUALITY_LEVEL,
                    )
                    .push_next(&mut quality_level_info);
            }
        }
        HevcEncodeProbeControlMode::Ffmpeg => {
            if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
                rate_control_info = rate_control_info
                    .layers(std::slice::from_ref(&rate_control_layer))
                    .virtual_buffer_size_in_ms(1000)
                    .initial_virtual_buffer_size_in_ms(1000);
            }
            coding_control_info = coding_control_info
                .flags(
                    vk::VideoCodingControlFlagsKHR::RESET
                        | vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL,
                )
                .push_next(&mut rate_control_info);
            if control_config.enable_quality_level_control {
                coding_control_info = coding_control_info
                    .flags(
                        coding_control_info.flags
                            | vk::VideoCodingControlFlagsKHR::ENCODE_QUALITY_LEVEL,
                    )
                    .push_next(&mut quality_level_info);
            }
        }
        HevcEncodeProbeControlMode::None => {
            issue_coding_control = false;
        }
    }
    // SAFETY: command buffer is recording and video session handles are valid.
    unsafe {
        (video_queue_device.fp().cmd_begin_video_coding_khr)(command_buffer, &begin_coding_info);
        if issue_coding_control {
            (video_queue_device.fp().cmd_control_video_coding_khr)(
                command_buffer,
                &coding_control_info,
            );
        }
    }
    if matches!(
        mode,
        HevcEncodePreEncodeProbeMode::WithEncode | HevcEncodePreEncodeProbeMode::WithEncodeMinimal
    ) {
        let reference_index_mode = resolve_hevc_encode_probe_reference_index_mode();
        let rps_mode = resolve_hevc_encode_probe_rps_mode();
        let setup_reference_slot_mode = resolve_hevc_encode_probe_setup_reference_slot_mode();
        let reference_idx_minus1 = hevc_encode_probe_reference_idx_minus1(reference_index_mode);
        let reconstructed_picture_resource = vk::VideoPictureResourceInfoKHR::default()
            .coded_offset(vk::Offset2D::default())
            .coded_extent(vk::Extent2D {
                width: resources.picture_resource_coded_width,
                height: resources.picture_resource_coded_height,
            })
            .base_array_layer(0)
            .image_view_binding(resources.dpb_image_view);
        let setup_reference_info = StdVideoEncodeH265ReferenceInfo {
            flags: StdVideoEncodeH265ReferenceInfoFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
            },
            pic_type: StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR,
            PicOrderCntVal: 0,
            TemporalId: 0,
        };
        let mut setup_dpb_slot_info =
            vk::VideoEncodeH265DpbSlotInfoKHR::default().std_reference_info(&setup_reference_info);
        let setup_reference_slot = vk::VideoReferenceSlotInfoKHR::default()
            .slot_index(0)
            .picture_resource(&reconstructed_picture_resource)
            .push_next(&mut setup_dpb_slot_info);
        let mut std_reference_list_flags = StdVideoEncodeH265ReferenceListsInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: Default::default(),
        };
        std_reference_list_flags.set_ref_pic_list_modification_flag_l0(0);
        std_reference_list_flags.set_ref_pic_list_modification_flag_l1(0);
        let std_reference_lists = StdVideoEncodeH265ReferenceListsInfo {
            flags: std_reference_list_flags,
            num_ref_idx_l0_active_minus1: reference_idx_minus1,
            num_ref_idx_l1_active_minus1: reference_idx_minus1,
            RefPicList0: [HEVC_NO_REFERENCE_PICTURE; 15],
            RefPicList1: [HEVC_NO_REFERENCE_PICTURE; 15],
            list_entry_l0: [0; 15],
            list_entry_l1: [0; 15],
        };
        let std_short_term_ref_pic_set = StdVideoH265ShortTermRefPicSet {
            flags: ash::vk::native::StdVideoH265ShortTermRefPicSetFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
                __bindgen_padding_0: [0; 3],
            },
            delta_idx_minus1: 0,
            use_delta_flag: 0,
            abs_delta_rps_minus1: 0,
            used_by_curr_pic_flag: 0,
            used_by_curr_pic_s0_flag: 0,
            used_by_curr_pic_s1_flag: 0,
            reserved1: 0,
            reserved2: 0,
            reserved3: 0,
            num_negative_pics: 0,
            num_positive_pics: 0,
            delta_poc_s0_minus1: [0; 16],
            delta_poc_s1_minus1: [0; 16],
        };
        let std_long_term_ref_pics = StdVideoEncodeH265LongTermRefPics {
            num_long_term_sps: 0,
            num_long_term_pics: 0,
            lt_idx_sps: [0; 32],
            poc_lsb_lt: [0; 16],
            used_by_curr_pic_lt_flag: 0,
            delta_poc_msb_present_flag: [0; 48],
            delta_poc_msb_cycle_lt: [0; 48],
        };
        let (short_term_ref_pic_set_ptr, long_term_ref_pics_ptr) = match rps_mode {
            HevcEncodeProbeRpsMode::EmptyStruct => (
                &std_short_term_ref_pic_set as *const StdVideoH265ShortTermRefPicSet,
                &std_long_term_ref_pics as *const StdVideoEncodeH265LongTermRefPics,
            ),
            HevcEncodeProbeRpsMode::NullPointers => (std::ptr::null(), std::ptr::null()),
        };
        let mut std_picture_flags = StdVideoEncodeH265PictureInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: Default::default(),
        };
        let is_reference = matches!(picture_flags_mode, HevcEncodeProbePictureFlagsMode::Default);
        std_picture_flags.set_is_reference(u32::from(is_reference));
        std_picture_flags.set_IrapPicFlag(u32::from(is_reference));
        std_picture_flags.set_pic_output_flag(1);
        std_picture_flags.set_short_term_ref_pic_set_sps_flag(0);
        std_picture_flags
            .set_slice_temporal_mvp_enabled_flag(u32::from(config.sps_temporal_mvp_enabled));
        std_picture_flags.set_used_for_long_term_reference(0);
        std_picture_flags.set_discardable_flag(0);
        std_picture_flags.set_cross_layer_bla_flag(0);
        std_picture_flags.set_no_output_of_prior_pics_flag(0);
        let (reference_lists_ptr, short_term_ref_pic_set_ptr, long_term_ref_pics_ptr) = match mode {
            HevcEncodePreEncodeProbeMode::WithEncodeMinimal => {
                (std::ptr::null(), std::ptr::null(), std::ptr::null())
            }
            HevcEncodePreEncodeProbeMode::WithEncode | HevcEncodePreEncodeProbeMode::ScopeOnly => (
                &std_reference_lists as *const StdVideoEncodeH265ReferenceListsInfo,
                short_term_ref_pic_set_ptr,
                long_term_ref_pics_ptr,
            ),
        };
        let std_picture_info = StdVideoEncodeH265PictureInfo {
            flags: std_picture_flags,
            pic_type: picture_type,
            sps_video_parameter_set_id: 0,
            pps_seq_parameter_set_id: 0,
            pps_pic_parameter_set_id: 0,
            short_term_ref_pic_set_idx: 0,
            PicOrderCntVal: pic_order_cnt_val,
            TemporalId: temporal_id,
            reserved1: [0; 7],
            pRefLists: reference_lists_ptr,
            pShortTermRefPicSet: short_term_ref_pic_set_ptr,
            pLongTermRefPics: long_term_ref_pics_ptr,
        };
        let std_picture_info_empty = StdVideoEncodeH265PictureInfo {
            flags: StdVideoEncodeH265PictureInfoFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
            },
            pic_type: StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR,
            sps_video_parameter_set_id: 0,
            pps_seq_parameter_set_id: 0,
            pps_pic_parameter_set_id: 0,
            short_term_ref_pic_set_idx: 0,
            PicOrderCntVal: 0,
            TemporalId: 0,
            reserved1: [0; 7],
            pRefLists: std::ptr::null(),
            pShortTermRefPicSet: std::ptr::null(),
            pLongTermRefPics: std::ptr::null(),
        };
        let std_slice_flags =
            hevc_encode_std_slice_segment_header_flags(config.sample_adaptive_offset_enabled);
        let constant_qp = hevc_encode_probe_constant_qp(control_config.selected_rate_control_mode);
        let slice_qp_delta = hevc_encode_probe_slice_qp_delta(constant_qp, 0)
            .map_err(|err| format!("{err} for pre-encode probe"))?;
        let std_slice_header = StdVideoEncodeH265SliceSegmentHeader {
            flags: std_slice_flags,
            slice_type,
            slice_segment_address: 0,
            collocated_ref_idx: 0,
            MaxNumMergeCand: 5,
            slice_cb_qp_offset: 0,
            slice_cr_qp_offset: 0,
            slice_beta_offset_div2: 0,
            slice_tc_offset_div2: 0,
            slice_act_y_qp_offset: 0,
            slice_act_cb_qp_offset: 0,
            slice_act_cr_qp_offset: 0,
            slice_qp_delta,
            reserved1: 0,
            pWeightTable: std::ptr::null(),
        };
        let nalu_slice_entries = [vk::VideoEncodeH265NaluSliceSegmentInfoKHR::default()
            .constant_qp(constant_qp)
            .std_slice_segment_header(&std_slice_header)];
        let nalu_entries: &[vk::VideoEncodeH265NaluSliceSegmentInfoKHR] = match nalu_mode {
            HevcEncodeProbeNaluMode::SingleSlice => &nalu_slice_entries,
            HevcEncodeProbeNaluMode::Empty => &[],
        };
        let mut h265_encode_info = vk::VideoEncodeH265PictureInfoKHR::default()
            .nalu_slice_segment_entries(nalu_entries)
            .std_picture_info(&std_picture_info);
        let mut h265_encode_info_std_picture_only =
            vk::VideoEncodeH265PictureInfoKHR::default().std_picture_info(&std_picture_info);
        let mut h265_encode_info_empty_std_picture = vk::VideoEncodeH265PictureInfoKHR::default()
            .nalu_slice_segment_entries(nalu_entries)
            .std_picture_info(&std_picture_info_empty);
        let mut h265_encode_info_minimal = vk::VideoEncodeH265PictureInfoKHR::default();
        let mut h265_encode_info_no_std_picture =
            vk::VideoEncodeH265PictureInfoKHR::default().nalu_slice_segment_entries(nalu_entries);
        let src_picture_resource = vk::VideoPictureResourceInfoKHR::default()
            .coded_offset(vk::Offset2D::default())
            .coded_extent(vk::Extent2D {
                width: resources.picture_resource_coded_width,
                height: resources.picture_resource_coded_height,
            })
            .base_array_layer(0)
            .image_view_binding(resources.source_image_view);
        let encode_info = vk::VideoEncodeInfoKHR::default()
            .dst_buffer(resources.dst_buffer)
            .dst_buffer_offset(0)
            .dst_buffer_range(resources.dst_buffer_size)
            .src_picture_resource(src_picture_resource);
        let encode_info = match codec_info_mode {
            HevcEncodeProbeCodecInfoMode::WithH265Info => {
                encode_info.push_next(&mut h265_encode_info)
            }
            HevcEncodeProbeCodecInfoMode::WithH265InfoStdPictureOnly => {
                encode_info.push_next(&mut h265_encode_info_std_picture_only)
            }
            HevcEncodeProbeCodecInfoMode::WithH265InfoEmptyStdPicture => {
                encode_info.push_next(&mut h265_encode_info_empty_std_picture)
            }
            HevcEncodeProbeCodecInfoMode::WithH265InfoMinimal => {
                encode_info.push_next(&mut h265_encode_info_minimal)
            }
            HevcEncodeProbeCodecInfoMode::WithH265InfoNoStdPicture => {
                encode_info.push_next(&mut h265_encode_info_no_std_picture)
            }
            HevcEncodeProbeCodecInfoMode::WithoutH265Info => encode_info,
        };
        let encode_info = match mode {
            HevcEncodePreEncodeProbeMode::WithEncodeMinimal => encode_info,
            HevcEncodePreEncodeProbeMode::WithEncode | HevcEncodePreEncodeProbeMode::ScopeOnly => {
                match setup_reference_slot_mode {
                    HevcEncodeProbeSetupReferenceSlotMode::SlotZero => {
                        encode_info.setup_reference_slot(&setup_reference_slot)
                    }
                    HevcEncodeProbeSetupReferenceSlotMode::None => encode_info,
                }
            }
        };
        // SAFETY: command buffer is recording and structures are valid for this call.
        unsafe {
            (video_encode_device.fp().cmd_encode_video_khr)(command_buffer, &encode_info);
        }
    }
    let end_coding_info = vk::VideoEndCodingInfoKHR::default();
    // SAFETY: command buffer is recording and coding scope was opened above.
    unsafe {
        (video_queue_device.fp().cmd_end_video_coding_khr)(command_buffer, &end_coding_info);
    }
    // SAFETY: command buffer is valid and recording.
    unsafe { device.end_command_buffer(command_buffer) }.map_err(|err| {
        let picture_info_mode_label = hevc_encode_probe_picture_info_mode_label(picture_info_mode);
        format!(
            "pre-encode probe ({mode_label}, picture_info_mode={picture_info_mode_label}, pic_order_cnt_val={pic_order_cnt_val}, temporal_id={temporal_id}) vkEndCommandBuffer failed: {err}"
        )
    })?;
    Ok(())
}

fn create_hevc_encode_probe_dst_buffer(
    device: &ash::Device,
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    buffer_size: u64,
) -> Result<(vk::Buffer, vk::DeviceMemory), String> {
    let mut encode_h265_profile = vk::VideoEncodeH265ProfileInfoKHR::default()
        .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
    let mut encode_usage = vk::VideoEncodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoEncodeUsageFlagsKHR::DEFAULT);
    let profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::ENCODE_H265)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut encode_h265_profile)
        .push_next(&mut encode_usage);
    let profiles = [profile];
    let mut profile_list = vk::VideoProfileListInfoKHR::default().profiles(&profiles);
    let create_info = vk::BufferCreateInfo::default()
        .size(buffer_size)
        .usage(vk::BufferUsageFlags::VIDEO_ENCODE_DST_KHR)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .push_next(&mut profile_list);
    // SAFETY: buffer create info references local stack data.
    let buffer = unsafe { device.create_buffer(&create_info, None) }
        .map_err(|err| format!("vkCreateBuffer for encode submit probe failed: {err}"))?;
    // SAFETY: buffer handle is valid and owned by this device.
    let memory_requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let memory_type_index = find_memory_type_index(
        physical_device,
        instance,
        memory_requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )
    .or_else(|| {
        find_memory_type_index(
            physical_device,
            instance,
            memory_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE,
        )
    })
    .or_else(|| {
        find_memory_type_index(
            physical_device,
            instance,
            memory_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
    })
    .or_else(|| {
        find_memory_type_index(
            physical_device,
            instance,
            memory_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::empty(),
        )
    })
    .ok_or_else(|| "no compatible memory type for encode submit destination buffer".to_string())?;
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(memory_requirements.size)
        .memory_type_index(memory_type_index);
    // SAFETY: allocation info is valid for this device.
    let buffer_memory = unsafe { device.allocate_memory(&allocate_info, None) }.map_err(|err| {
        format!("vkAllocateMemory for encode submit destination buffer failed: {err}")
    })?;
    // SAFETY: buffer and memory were created from this device.
    unsafe { device.bind_buffer_memory(buffer, buffer_memory, 0) }.map_err(|err| {
        format!("vkBindBufferMemory for encode submit destination buffer failed: {err}")
    })?;
    Ok((buffer, buffer_memory))
}

fn select_hevc_encode_rate_control_mode(
    modes: vk::VideoEncodeRateControlModeFlagsKHR,
    max_rate_control_layers: u32,
) -> Option<vk::VideoEncodeRateControlModeFlagsKHR> {
    if modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED) {
        return Some(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED);
    }
    if max_rate_control_layers == 0 {
        return None;
    }
    if modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::CBR) {
        return Some(vk::VideoEncodeRateControlModeFlagsKHR::CBR);
    }
    if modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::VBR) {
        return Some(vk::VideoEncodeRateControlModeFlagsKHR::VBR);
    }
    None
}

fn align_up(value: u64, alignment: u64) -> u64 {
    if alignment <= 1 {
        value
    } else {
        let remainder = value % alignment;
        if remainder == 0 {
            value
        } else {
            value.saturating_add(alignment - remainder)
        }
    }
}

fn find_memory_type_index(
    physical_device: vk::PhysicalDevice,
    instance: &ash::Instance,
    memory_type_bits: u32,
    required_properties: vk::MemoryPropertyFlags,
) -> Option<u32> {
    // SAFETY: physical device handle is valid for this instance.
    let memory_properties =
        unsafe { instance.get_physical_device_memory_properties(physical_device) };
    memory_properties
        .memory_types
        .iter()
        .take(usize::try_from(memory_properties.memory_type_count).ok()?)
        .enumerate()
        .find_map(|(index, memory_type)| {
            let index_u32 = u32::try_from(index).ok()?;
            let supported = (memory_type_bits & (1_u32 << index_u32)) != 0;
            let properties_match = memory_type.property_flags.contains(required_properties);
            (supported && properties_match).then_some(index_u32)
        })
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
mod tests {
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
        assert_eq!(hevc_encode_probe_constant_qp(None), 26);
        assert_eq!(
            hevc_encode_probe_constant_qp(Some(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED)),
            26
        );
        assert_eq!(
            hevc_encode_probe_constant_qp(Some(vk::VideoEncodeRateControlModeFlagsKHR::CBR)),
            0
        );
        assert_eq!(
            hevc_encode_probe_constant_qp(Some(vk::VideoEncodeRateControlModeFlagsKHR::VBR)),
            0
        );
    }

    #[test]
    fn hevc_encode_slice_qp_delta_matches_ffmpeg_formula() {
        assert_eq!(hevc_encode_probe_slice_qp_delta(26, 0).unwrap(), 0);
        assert_eq!(hevc_encode_probe_slice_qp_delta(0, 0).unwrap(), -26);
        assert_eq!(hevc_encode_probe_slice_qp_delta(26, -2).unwrap(), 2);
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
                true
            ),
            HevcEncodeParameterMode::SampleSpsVuiFlagOff
        );
        assert_eq!(
            resolve_hevc_encode_parameter_mode_with_vui_safety(
                HevcEncodeParameterMode::Sample,
                false,
                true
            ),
            HevcEncodeParameterMode::Sample
        );
        assert_eq!(
            resolve_hevc_encode_parameter_mode_with_vui_safety(
                HevcEncodeParameterMode::Sample,
                true,
                false
            ),
            HevcEncodeParameterMode::Sample
        );
        assert_eq!(
            resolve_hevc_encode_parameter_mode_with_vui_safety(
                HevcEncodeParameterMode::SampleSpsNoVuiFlagOn,
                true,
                true
            ),
            HevcEncodeParameterMode::SampleSpsNoVuiFlagOn
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
            resolve_hevc_encode_probe_session_dpb_limits(
                HevcEncodeProbeSessionDpbMode::Default,
                0,
                0,
            ),
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
}
