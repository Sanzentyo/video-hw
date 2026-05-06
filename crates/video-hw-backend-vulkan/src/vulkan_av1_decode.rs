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
    max_dpb_slots: u32,
    max_active_reference_pictures: u32,
    max_level: ash::vk::native::StdVideoAV1Level,
    std_header_version: vk::ExtensionProperties,
    decode_output_formats: Vec<vk::Format>,
}

#[derive(Debug, Clone, Copy)]
struct Av1DecodeSessionParameterProbeConfig<'a> {
    queue_family_index: u32,
    capability_snapshot: &'a Av1DecodeCapabilitySnapshot,
    picture_format: vk::Format,
    coded_width: u32,
    coded_height: u32,
    std_sequence_header: &'a StdVideoAV1SequenceHeader,
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
    let records = parse_av1_low_overhead_obus(bitstream)?;
    let first_frame_record = records
        .iter()
        .find(|record| matches!(record.obu_type, Av1ObuType::Frame | Av1ObuType::TileGroup))
        .ok_or_else(|| "missing AV1 frame or tile-group OBU for decode submit".to_string())?;

    match first_frame_record.obu_type {
        Av1ObuType::Frame => build_av1_frame_obu_submit_skeleton(first_frame_record),
        Av1ObuType::TileGroup => {
            let frame_header = records.iter().rev().find(|record| {
                record.temporal_unit_index == first_frame_record.temporal_unit_index
                    && record.obu_type == Av1ObuType::FrameHeader
                    && record.obu_range.start < first_frame_record.obu_range.start
            });
            let Some(frame_header) = frame_header else {
                return Err(
                    "AV1 tile-group OBU is missing a preceding frame-header OBU".to_string()
                );
            };
            build_av1_tile_group_submit_skeleton(frame_header, first_frame_record)
        }
        _ => unreachable!("first frame record is filtered to frame payload OBUs"),
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

impl ParsedAv1SequenceHeader {
    fn coded_width(&self) -> u32 {
        self.max_frame_width_minus_1 + 1
    }

    fn coded_height(&self) -> u32 {
        self.max_frame_height_minus_1 + 1
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
    tile_group_record: &Av1ObuRecord,
) -> Result<Av1DecodeSubmitSkeleton, String> {
    let frame_header_offset = u32::try_from(frame_header_record.payload_range.start)
        .map_err(|_| "AV1 frame header offset exceeds u32 range".to_string())?;
    let tile_offset = u32::try_from(tile_group_record.payload_range.start)
        .map_err(|_| "AV1 tile offset exceeds u32 range".to_string())?;
    let tile_size = u32::try_from(tile_group_record.payload_range.len())
        .map_err(|_| "AV1 tile group payload size exceeds u32 range".to_string())?;
    if tile_size == 0 {
        return Err("AV1 tile-group OBU payload is empty".to_string());
    }
    Ok(Av1DecodeSubmitSkeleton {
        temporal_unit_index: tile_group_record.temporal_unit_index,
        frame_header_offset,
        tile_offsets: vec![tile_offset],
        tile_sizes: vec![tile_size],
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
    let max_dpb_slots = capabilities.max_dpb_slots;
    let max_active_reference_pictures = capabilities.max_active_reference_pictures;
    let std_header_version = capabilities.std_header_version;
    let max_level = decode_av1_capabilities.max_level;

    Ok(Av1DecodeCapabilitySnapshot {
        min_coded_width,
        min_coded_height,
        max_coded_width,
        max_coded_height,
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
        queue_family_index,
        capability_snapshot,
    );
    // SAFETY: The device is no longer used after this point.
    unsafe {
        device.destroy_device(None);
    }
    result
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
            Ok(()) => {
                return Ok(Av1DecodeBitstreamSessionProbe {
                    coded_width,
                    coded_height,
                    picture_format,
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
    queue_family_index: u32,
    capability_snapshot: &Av1DecodeCapabilitySnapshot,
) -> Result<(), String> {
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
) -> Result<(), String> {
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

    let parameters_result = create_av1_decode_session_parameters(
        device,
        &video_queue_device,
        video_session,
        config.std_sequence_header,
    );

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
