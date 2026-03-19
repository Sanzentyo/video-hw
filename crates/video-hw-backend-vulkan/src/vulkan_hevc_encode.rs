use std::ffi::{CStr, c_void};
use std::sync::{Mutex, OnceLock};

use ash::vk;
use ash::vk::native::{
    StdVideoEncodeH265PictureInfo, StdVideoEncodeH265PictureInfoFlags,
    StdVideoEncodeH265ReferenceInfo, StdVideoEncodeH265ReferenceInfoFlags,
    StdVideoEncodeH265SliceSegmentHeader, StdVideoEncodeH265SliceSegmentHeaderFlags,
    StdVideoH265LevelIdc, StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR,
    StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN,
    StdVideoH265SliceType_STD_VIDEO_H265_SLICE_TYPE_I,
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
    Ready { queue_family_index: u32 },
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
}

struct HevcEncodeCapabilityCandidate {
    physical_device: vk::PhysicalDevice,
    extensions: ExtensionFlags,
    queue_family_index: EncodeQueueFamilyIndex,
    capability_snapshot: HevcEncodeCapabilitySnapshot,
    encode_input_formats: Vec<vk::Format>,
    encode_dpb_formats: Vec<vk::Format>,
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodeSubmitExecutionConfig {
    physical_device: vk::PhysicalDevice,
    queue_family_index: EncodeQueueFamilyIndex,
    video_session: vk::VideoSessionKHR,
    video_session_parameters: vk::VideoSessionParametersKHR,
    parameter_set_ids: HevcEncodeParameterSetIds,
    parameter_set_coded_width: u32,
    parameter_set_coded_height: u32,
    parameter_set_pps_init_qp_minus26: i8,
    parameter_mode: HevcEncodeParameterMode,
    coded_width: u32,
    coded_height: u32,
    picture_format: vk::Format,
    picture_access_granularity: vk::Extent2D,
    rate_control_modes: vk::VideoEncodeRateControlModeFlagsKHR,
    max_rate_control_layers: u32,
    max_quality_levels: u32,
    min_bitstream_buffer_offset_alignment: vk::DeviceSize,
    min_bitstream_buffer_size_alignment: vk::DeviceSize,
}

#[derive(Debug, Clone, Copy)]
struct HevcEncodePreEncodeProbeResources {
    source_image: vk::Image,
    dst_buffer: vk::Buffer,
    dst_buffer_size: u64,
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

#[derive(Debug, Clone, Copy)]
struct HevcEncodeSessionParameters {
    handle: vk::VideoSessionParametersKHR,
    parameter_set_ids: HevcEncodeParameterSetIds,
    coded_width: u32,
    coded_height: u32,
    pps_init_qp_minus26: i8,
    mode: HevcEncodeParameterMode,
}

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
            };

            let properties = unsafe { instance.get_physical_device_properties(physical_device) };
            let is_discrete = properties.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
            let selection_score =
                u32::from(is_discrete) * 2 + u32::from(support.extensions.has_video_maintenance1);
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
        );
        let selected_properties =
            unsafe { instance.get_physical_device_properties(candidate.physical_device) };

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
                let selection_score = u32::from(is_discrete) * 2
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
                    coded_width,
                    coded_height,
                    picture_format,
                    reference_picture_format,
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
    coded_width: u32,
    coded_height: u32,
    picture_format: vk::Format,
    reference_picture_format: vk::Format,
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
    let create_info = vk::VideoSessionCreateInfoKHR::default()
        .queue_family_index(candidate.queue_family_index.0)
        .video_profile(&profile)
        .picture_format(picture_format)
        .max_coded_extent(vk::Extent2D {
            width: coded_width,
            height: coded_height,
        })
        .reference_picture_format(reference_picture_format)
        .max_dpb_slots(candidate.capability_snapshot.max_dpb_slots.max(1))
        .max_active_reference_pictures(
            candidate
                .capability_snapshot
                .max_active_reference_pictures
                .max(1),
        )
        .std_header_version(&candidate.capability_snapshot.std_header_version)
        .push_next(&mut encode_h265_session_create);
    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
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
            video_session,
            coded_width,
            coded_height,
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
                        parameter_mode: session_parameters.mode,
                        coded_width,
                        coded_height,
                        picture_format,
                        picture_access_granularity: candidate
                            .capability_snapshot
                            .picture_access_granularity,
                        rate_control_modes: candidate.capability_snapshot.rate_control_modes,
                        max_rate_control_layers: candidate
                            .capability_snapshot
                            .max_rate_control_layers,
                        max_quality_levels: candidate.capability_snapshot.max_quality_levels,
                        min_bitstream_buffer_offset_alignment: candidate
                            .capability_snapshot
                            .min_bitstream_buffer_offset_alignment,
                        min_bitstream_buffer_size_alignment: candidate
                            .capability_snapshot
                            .min_bitstream_buffer_size_alignment,
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
                mode: parameter_mode,
            })
        }
    }
}

fn create_hevc_encode_video_session_parameters_from_sample(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
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
        mode: effective_parameter_mode,
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

    let probe_result = (|| -> Result<(), String> {
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
            .command_buffer_count(3);
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
        let pre_encode_probe_command_buffer = *command_buffers.get(2).ok_or_else(|| {
            "vkAllocateCommandBuffers returned no pre-encode probe command buffer".to_string()
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
                    picture_format: config.picture_format,
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
        let synthetic_prefix_bytes = align_up(128, dst_offset_alignment);
        let dst_buffer_offset = synthetic_prefix_bytes;
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
        let selected_rate_control_mode = select_hevc_encode_rate_control_mode(
            config.rate_control_modes,
            config.max_rate_control_layers,
        );
        let selected_quality_level = config.max_quality_levels.saturating_sub(1);
        let selected_rate_control_mode_label = selected_rate_control_mode
            .map(|mode| format!("{mode:?}"))
            .unwrap_or_else(|| "none".to_string());
        let parameter_mode_label = hevc_encode_parameter_mode_label(config.parameter_mode);
        let constant_qp = 26_i32;
        let slice_qp_delta =
            i8::try_from(26_i16 - (i16::from(config.parameter_set_pps_init_qp_minus26) + 26_i16))
                .map_err(|_| {
                format!(
                    "slice_qp_delta is out of range for parameter-set pps_init_qp_minus26={}",
                    config.parameter_set_pps_init_qp_minus26
                )
            })?;
        let encode_probe_context = format!(
            "encode_probe_inputs(coded={}x{}, image={}x{}, dst_offset={}, dst_range={}, dst_prefix={}, dst_offset_align={}, dst_size_align={}, parameter_mode={}, parameter_set_ids=vps:{}|sps:{}|pps:{}, parameter_set_coded={}x{}, parameter_set_coded_match={}, parameter_set_pps_init_qp_minus26={}, constant_qp={}, slice_qp_delta={}, rate_control_mode={}, max_rate_control_layers={}, quality_level={})",
            config.coded_width,
            config.coded_height,
            image_width,
            image_height,
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
            constant_qp,
            slice_qp_delta,
            selected_rate_control_mode_label,
            config.max_rate_control_layers,
            selected_quality_level,
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

        let setup_picture_resource = vk::VideoPictureResourceInfoKHR::default()
            .coded_offset(vk::Offset2D::default())
            .coded_extent(vk::Extent2D {
                width: config.coded_width,
                height: config.coded_height,
            })
            .base_array_layer(0)
            .image_view_binding(dpb_image_view);
        let mut std_setup_reference_info_flags = StdVideoEncodeH265ReferenceInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: Default::default(),
        };
        std_setup_reference_info_flags.set_used_for_long_term_reference(0);
        std_setup_reference_info_flags.set_unused_for_reference(0);
        let std_setup_reference_info = StdVideoEncodeH265ReferenceInfo {
            flags: std_setup_reference_info_flags,
            pic_type: StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR,
            PicOrderCntVal: 0,
            TemporalId: 0,
        };
        let mut setup_reference_dpb_info_for_begin = vk::VideoEncodeH265DpbSlotInfoKHR::default()
            .std_reference_info(&std_setup_reference_info);
        let mut setup_reference_dpb_info_for_encode = vk::VideoEncodeH265DpbSlotInfoKHR::default()
            .std_reference_info(&std_setup_reference_info);
        let begin_reference_slots = [vk::VideoReferenceSlotInfoKHR::default()
            .slot_index(-1)
            .picture_resource(&setup_picture_resource)
            .push_next(&mut setup_reference_dpb_info_for_begin)];
        let setup_reference_slot = vk::VideoReferenceSlotInfoKHR::default()
            .slot_index(0)
            .picture_resource(&setup_picture_resource)
            .push_next(&mut setup_reference_dpb_info_for_encode);
        let begin_coding_info = vk::VideoBeginCodingInfoKHR::default()
            .video_session(config.video_session)
            .video_session_parameters(config.video_session_parameters)
            .reference_slots(&begin_reference_slots);
        let rate_control_mode =
            selected_rate_control_mode.unwrap_or(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED);
        let mut rate_control_layer_h265 = vk::VideoEncodeH265RateControlLayerInfoKHR::default();
        let rate_control_layer = vk::VideoEncodeRateControlLayerInfoKHR::default()
            .average_bitrate(8_000_000)
            .max_bitrate(8_000_000)
            .frame_rate_numerator(30)
            .frame_rate_denominator(1)
            .push_next(&mut rate_control_layer_h265);
        let mut rate_control_info =
            vk::VideoEncodeRateControlInfoKHR::default().rate_control_mode(rate_control_mode);
        if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
            rate_control_info = rate_control_info
                .layers(std::slice::from_ref(&rate_control_layer))
                .virtual_buffer_size_in_ms(1000)
                .initial_virtual_buffer_size_in_ms(1000);
        }
        let coding_control_info = vk::VideoCodingControlInfoKHR::default()
            .flags(
                vk::VideoCodingControlFlagsKHR::RESET
                    | vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL,
            )
            .push_next(&mut rate_control_info);
        // SAFETY: command buffer is recording and session handles are valid.
        unsafe {
            (video_queue_device.fp().cmd_begin_video_coding_khr)(
                command_buffer,
                &begin_coding_info,
            );
            (video_queue_device.fp().cmd_control_video_coding_khr)(
                command_buffer,
                &coding_control_info,
            );
        }

        let mut std_slice_flags = StdVideoEncodeH265SliceSegmentHeaderFlags {
            _bitfield_align_1: [],
            _bitfield_1: Default::default(),
        };
        std_slice_flags.set_first_slice_segment_in_pic_flag(1);
        let std_slice_header = StdVideoEncodeH265SliceSegmentHeader {
            flags: std_slice_flags,
            slice_type: StdVideoH265SliceType_STD_VIDEO_H265_SLICE_TYPE_I,
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
        let mut std_picture_flags = StdVideoEncodeH265PictureInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: Default::default(),
        };
        std_picture_flags.set_is_reference(1);
        std_picture_flags.set_IrapPicFlag(1);
        std_picture_flags.set_pic_output_flag(1);
        let mut std_reference_lists_flags = vk::native::StdVideoEncodeH265ReferenceListsInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: Default::default(),
        };
        std_reference_lists_flags.set_ref_pic_list_modification_flag_l0(0);
        std_reference_lists_flags.set_ref_pic_list_modification_flag_l1(0);
        let std_reference_lists_info = vk::native::StdVideoEncodeH265ReferenceListsInfo {
            flags: std_reference_lists_flags,
            num_ref_idx_l0_active_minus1: 0,
            num_ref_idx_l1_active_minus1: 0,
            RefPicList0: std::array::from_fn(|_| 0_u8),
            RefPicList1: std::array::from_fn(|_| 0_u8),
            list_entry_l0: std::array::from_fn(|_| 0_u8),
            list_entry_l1: std::array::from_fn(|_| 0_u8),
        };
        let mut std_short_term_ref_pic_set_flags =
            vk::native::StdVideoH265ShortTermRefPicSetFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
                __bindgen_padding_0: [0; 3],
            };
        std_short_term_ref_pic_set_flags.set_inter_ref_pic_set_prediction_flag(0);
        std_short_term_ref_pic_set_flags.set_delta_rps_sign(0);
        let std_short_term_ref_pic_set = vk::native::StdVideoH265ShortTermRefPicSet {
            flags: std_short_term_ref_pic_set_flags,
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
            delta_poc_s0_minus1: std::array::from_fn(|_| 0_u16),
            delta_poc_s1_minus1: std::array::from_fn(|_| 0_u16),
        };
        let std_long_term_ref_pics = vk::native::StdVideoEncodeH265LongTermRefPics {
            num_long_term_sps: 0,
            num_long_term_pics: 0,
            lt_idx_sps: std::array::from_fn(|_| 0_u8),
            poc_lsb_lt: std::array::from_fn(|_| 0_u8),
            used_by_curr_pic_lt_flag: 0,
            delta_poc_msb_present_flag: std::array::from_fn(|_| 0_u8),
            delta_poc_msb_cycle_lt: std::array::from_fn(|_| 0_u8),
        };
        let std_picture_info = StdVideoEncodeH265PictureInfo {
            flags: std_picture_flags,
            pic_type: StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR,
            sps_video_parameter_set_id: config.parameter_set_ids.vps_id,
            pps_seq_parameter_set_id: config.parameter_set_ids.sps_id,
            pps_pic_parameter_set_id: config.parameter_set_ids.pps_id,
            short_term_ref_pic_set_idx: 0,
            PicOrderCntVal: 0,
            TemporalId: 0,
            reserved1: [0; 7],
            pRefLists: &std_reference_lists_info,
            pShortTermRefPicSet: &std_short_term_ref_pic_set,
            pLongTermRefPics: &std_long_term_ref_pics,
        };
        let mut h265_encode_info = vk::VideoEncodeH265PictureInfoKHR::default()
            .nalu_slice_segment_entries(&nalu_slice_entries)
            .std_picture_info(&std_picture_info);
        let src_picture_resource = vk::VideoPictureResourceInfoKHR::default()
            .coded_offset(vk::Offset2D::default())
            .coded_extent(vk::Extent2D {
                width: config.coded_width,
                height: config.coded_height,
            })
            .base_array_layer(0)
            .image_view_binding(source_image_view);
        let encode_info = vk::VideoEncodeInfoKHR::default()
            .dst_buffer(dst_buffer)
            .dst_buffer_offset(dst_buffer_offset)
            .dst_buffer_range(dst_buffer_range)
            .src_picture_resource(src_picture_resource)
            .setup_reference_slot(&setup_reference_slot)
            .push_next(&mut h265_encode_info);
        // SAFETY: encode info references local data that remains valid through the call.
        unsafe {
            (video_encode_device.fp().cmd_encode_video_khr)(command_buffer, &encode_info);
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
                selected_rate_control_mode,
            )
            .map(|()| "ok".to_string())
            .unwrap_or_else(|probe_err| format!("failed ({probe_err})"));
            let pre_encode_probe = run_hevc_encode_pre_encode_probe(
                device,
                &video_queue_device,
                pre_encode_probe_command_buffer,
                config.video_session,
                config.video_session_parameters,
                selected_rate_control_mode,
                HevcEncodePreEncodeProbeResources {
                    source_image,
                    dst_buffer,
                    dst_buffer_size,
                },
            )
            .map(|()| "ok".to_string())
            .unwrap_or_else(|probe_err| format!("failed ({probe_err})"));
            return Err(format!(
                "vkEndCommandBuffer failed: {err}; {encode_probe_context}; coding_scope_probe={coding_scope_probe}; pre_encode_probe={pre_encode_probe}"
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
        },
        Err(err) => {
            HevcEncodeSubmitExecutionProbe::Failed(append_hevc_encode_validation_messages(err))
        }
    }
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
        });
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
    selected_rate_control_mode: Option<vk::VideoEncodeRateControlModeFlagsKHR>,
) -> Result<(), String> {
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: command buffer is valid and not recording.
    unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
        .map_err(|err| format!("coding-scope probe vkBeginCommandBuffer failed: {err}"))?;

    let mut begin_coding_info = vk::VideoBeginCodingInfoKHR::default()
        .video_session(video_session)
        .video_session_parameters(video_session_parameters);
    let mut rate_control_layer_h265 = vk::VideoEncodeH265RateControlLayerInfoKHR::default();
    let rate_control_layer = vk::VideoEncodeRateControlLayerInfoKHR::default()
        .average_bitrate(8_000_000)
        .max_bitrate(8_000_000)
        .frame_rate_numerator(30)
        .frame_rate_denominator(1)
        .push_next(&mut rate_control_layer_h265);
    let mut rate_control_info = vk::VideoEncodeRateControlInfoKHR::default();
    if let Some(rate_control_mode) = selected_rate_control_mode {
        rate_control_info = rate_control_info.rate_control_mode(rate_control_mode);
        if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
            rate_control_info = rate_control_info
                .layers(std::slice::from_ref(&rate_control_layer))
                .virtual_buffer_size_in_ms(1000)
                .initial_virtual_buffer_size_in_ms(1000);
        }
        begin_coding_info = begin_coding_info.push_next(&mut rate_control_info);
    }
    // SAFETY: command buffer is recording and video session handles are valid.
    unsafe {
        (video_queue_device.fp().cmd_begin_video_coding_khr)(command_buffer, &begin_coding_info);
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
    command_buffer: vk::CommandBuffer,
    video_session: vk::VideoSessionKHR,
    video_session_parameters: vk::VideoSessionParametersKHR,
    selected_rate_control_mode: Option<vk::VideoEncodeRateControlModeFlagsKHR>,
    resources: HevcEncodePreEncodeProbeResources,
) -> Result<(), String> {
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: command buffer is valid and not recording.
    unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
        .map_err(|err| format!("pre-encode probe vkBeginCommandBuffer failed: {err}"))?;

    let source_image_prepare_barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
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
        .image(resources.source_image)
        .subresource_range(clear_range);
    let dst_buffer_barrier = vk::BufferMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
        .dst_access_mask(vk::AccessFlags2::VIDEO_ENCODE_WRITE_KHR)
        .buffer(resources.dst_buffer)
        .offset(0)
        .size(resources.dst_buffer_size);
    let dependency_info = vk::DependencyInfo::default()
        .image_memory_barriers(std::slice::from_ref(&source_image_encode_barrier))
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

    let mut begin_coding_info = vk::VideoBeginCodingInfoKHR::default()
        .video_session(video_session)
        .video_session_parameters(video_session_parameters);
    let mut rate_control_layer_h265 = vk::VideoEncodeH265RateControlLayerInfoKHR::default();
    let rate_control_layer = vk::VideoEncodeRateControlLayerInfoKHR::default()
        .average_bitrate(8_000_000)
        .max_bitrate(8_000_000)
        .frame_rate_numerator(30)
        .frame_rate_denominator(1)
        .push_next(&mut rate_control_layer_h265);
    let mut rate_control_info = vk::VideoEncodeRateControlInfoKHR::default();
    if let Some(rate_control_mode) = selected_rate_control_mode {
        rate_control_info = rate_control_info.rate_control_mode(rate_control_mode);
        if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
            rate_control_info = rate_control_info
                .layers(std::slice::from_ref(&rate_control_layer))
                .virtual_buffer_size_in_ms(1000)
                .initial_virtual_buffer_size_in_ms(1000);
        }
        begin_coding_info = begin_coding_info.push_next(&mut rate_control_info);
    }
    // SAFETY: command buffer is recording and video session handles are valid.
    unsafe {
        (video_queue_device.fp().cmd_begin_video_coding_khr)(command_buffer, &begin_coding_info);
    }
    let end_coding_info = vk::VideoEndCodingInfoKHR::default();
    // SAFETY: command buffer is recording and coding scope was opened above.
    unsafe {
        (video_queue_device.fp().cmd_end_video_coding_khr)(command_buffer, &end_coding_info);
    }
    // SAFETY: command buffer is valid and recording.
    unsafe { device.end_command_buffer(command_buffer) }
        .map_err(|err| format!("pre-encode probe vkEndCommandBuffer failed: {err}"))?;
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

    // SAFETY: We only query queue-family properties for a valid physical device.
    let queue_family_properties =
        unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    let encode_queue_family_index =
        queue_family_properties
            .iter()
            .enumerate()
            .find_map(|(index, queue)| {
                (queue.queue_count > 0
                    && queue.queue_flags.contains(vk::QueueFlags::VIDEO_ENCODE_KHR))
                .then(|| u32::try_from(index).ok().map(EncodeQueueFamilyIndex))
                .flatten()
            });

    Ok(AdapterEncodeSupport {
        extensions: flags,
        encode_queue_family_index,
    })
}

fn try_initialize_hevc_encode_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: EncodeQueueFamilyIndex,
    extensions: ExtensionFlags,
) -> Result<(), String> {
    let device =
        create_hevc_encode_device(instance, physical_device, queue_family_index, extensions)?;

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
    let mut synchronization2_features =
        vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true);
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(std::slice::from_ref(&queue_create_info))
        .enabled_extension_names(&extension_names)
        .push_next(&mut synchronization2_features);

    // SAFETY: Pointers referenced by `create_info` remain valid through the call.
    unsafe { instance.create_device(physical_device, &create_info, None) }
        .map_err(|err| format!("logical device initialization failed: {err}"))
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
