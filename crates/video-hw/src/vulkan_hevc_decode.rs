use std::ffi::CStr;
use std::io::Cursor;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::OnceLock;

use ash::vk;
use ash::vk::native::{
    StdVideoDecodeH265PictureInfo, StdVideoDecodeH265PictureInfoFlags,
    StdVideoDecodeH265ReferenceInfo, StdVideoDecodeH265ReferenceInfoFlags,
    StdVideoH265ChromaFormatIdc, StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_420,
    StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_422,
    StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_444,
    StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_MONOCHROME,
    StdVideoH265DecPicBufMgr, StdVideoH265LevelIdc, StdVideoH265LongTermRefPicsSps,
    StdVideoH265PictureParameterSet, StdVideoH265PpsFlags, StdVideoH265ProfileIdc,
    StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN, StdVideoH265ProfileTierLevel,
    StdVideoH265ProfileTierLevelFlags, StdVideoH265SequenceParameterSet,
    StdVideoH265ShortTermRefPicSet, StdVideoH265ShortTermRefPicSetFlags, StdVideoH265SpsFlags,
    StdVideoH265VideoParameterSet, StdVideoH265VpsFlags,
};
use scuffle_h265::{NALUnitType, SpsNALUnit, SpsRbsp};

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
struct ExtensionFlags {
    has_video_queue: bool,
    has_video_decode_queue: bool,
    has_video_decode_h265: bool,
}

#[derive(Debug, Clone, Copy)]
struct AdapterDecodeSupport {
    extensions: ExtensionFlags,
    decode_queue_family_index: Option<DecodeQueueFamilyIndex>,
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
}

#[derive(Debug, Clone)]
struct ParsedHevcParameterSets(HevcParameterSets);

#[derive(Debug, Clone, Copy)]
struct ParsedHevcVps {
    vps_video_parameter_set_id: u8,
    vps_max_sub_layers_minus1: u8,
    vps_temporal_id_nesting_flag: bool,
}

#[derive(Debug, Clone)]
struct ParsedHevcPps {
    pps_pic_parameter_set_id: u8,
    pps_seq_parameter_set_id: u8,
    num_extra_slice_header_bits: u8,
    num_ref_idx_l0_default_active_minus1: u8,
    num_ref_idx_l1_default_active_minus1: u8,
    init_qp_minus26: i8,
    diff_cu_qp_delta_depth: u8,
    pps_cb_qp_offset: i8,
    pps_cr_qp_offset: i8,
    pps_beta_offset_div2: i8,
    pps_tc_offset_div2: i8,
    log2_parallel_merge_level_minus2: u8,
    num_tile_columns_minus1: u8,
    num_tile_rows_minus1: u8,
    column_width_minus1: [u16; 19],
    row_height_minus1: [u16; 21],
    dependent_slice_segments_enabled_flag: bool,
    output_flag_present_flag: bool,
    sign_data_hiding_enabled_flag: bool,
    cabac_init_present_flag: bool,
    constrained_intra_pred_flag: bool,
    transform_skip_enabled_flag: bool,
    cu_qp_delta_enabled_flag: bool,
    pps_slice_chroma_qp_offsets_present_flag: bool,
    weighted_pred_flag: bool,
    weighted_bipred_flag: bool,
    transquant_bypass_enabled_flag: bool,
    tiles_enabled_flag: bool,
    entropy_coding_sync_enabled_flag: bool,
    uniform_spacing_flag: bool,
    loop_filter_across_tiles_enabled_flag: bool,
    pps_loop_filter_across_slices_enabled_flag: bool,
    deblocking_filter_control_present_flag: bool,
    deblocking_filter_override_enabled_flag: bool,
    pps_deblocking_filter_disabled_flag: bool,
    lists_modification_present_flag: bool,
    slice_segment_header_extension_present_flag: bool,
}

#[derive(Debug, Clone, Copy)]
struct ParsedHevcSliceHeader {
    nal_unit_type: u8,
    pps_id: u8,
    is_first_slice_segment: bool,
    pic_order_cnt_lsb: Option<u16>,
}

const MAX_HEVC_SUBMIT_PROBE_ACCESS_UNITS: usize = 16;
const HEVC_REF_PIC_SET_LIST_SIZE: usize = 8;
const HEVC_NO_REFERENCE_PICTURE: u8 = u8::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcExperimentalDpbMode {
    Off,
    Auto,
    On,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HevcExperimentalDpbDecision {
    DisabledOff,
    EnabledOn,
    EnabledAuto,
    DisabledAutoMarkerPresent,
    DisabledAutoMarkerWriteFailed,
}

#[derive(Debug, Clone)]
struct HevcExperimentalDpbConfiguration {
    enabled: bool,
    mode: HevcExperimentalDpbMode,
    status: String,
    marker_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
struct HevcSubmitProbeAccessUnit {
    header: HevcAccessUnitHeader,
    slice_segment_offset: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HevcActiveReferenceSlot {
    slot: usize,
    pic_order_cnt_val: i32,
}

#[derive(Debug, Clone)]
struct HevcSubmitProbeBitstreamPayload {
    bytes: Vec<u8>,
    access_units: Vec<HevcSubmitProbeAccessUnit>,
    parsed_pps: ParsedHevcPps,
}

#[derive(Debug, Clone)]
struct HevcStdParameterSetStorage {
    vps: [StdVideoH265VideoParameterSet; 1],
    sps: [StdVideoH265SequenceParameterSet; 1],
    pps: [StdVideoH265PictureParameterSet; 1],
    profile_tier_level: StdVideoH265ProfileTierLevel,
    dec_pic_buf_mgr: StdVideoH265DecPicBufMgr,
    short_term_ref_pic_sets: Vec<StdVideoH265ShortTermRefPicSet>,
    long_term_ref_pics_sps: Option<StdVideoH265LongTermRefPicsSps>,
}

impl HevcStdParameterSetStorage {
    fn add_info(&self) -> vk::VideoDecodeH265SessionParametersAddInfoKHR<'_> {
        vk::VideoDecodeH265SessionParametersAddInfoKHR::default()
            .std_vp_ss(&self.vps)
            .std_sp_ss(&self.sps)
            .std_pp_ss(&self.pps)
    }
}

#[derive(Debug, Clone, Copy)]
struct RbspBitReader<'a> {
    data: &'a [u8],
    bit_offset: usize,
}

impl<'a> RbspBitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_offset: 0,
        }
    }

    fn read_flag(&mut self) -> Result<bool, String> {
        Ok(self.read_bits(1)? != 0)
    }

    fn read_bits(&mut self, bit_count: usize) -> Result<u32, String> {
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

    fn read_ue(&mut self) -> Result<u32, String> {
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

    fn read_se(&mut self) -> Result<i32, String> {
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
struct DecodeQueueFamilyIndex(u32);

#[derive(Debug, Clone)]
struct AwaitingCapabilityProbe;

#[derive(Debug, Clone)]
struct CapabilityProbeComplete {
    physical_device: vk::PhysicalDevice,
    queue_family_index: DecodeQueueFamilyIndex,
    capability_snapshot: HevcCapabilitySnapshot,
    decode_output_formats: Vec<vk::Format>,
}

#[derive(Debug, Clone)]
struct HevcCapabilitySnapshot {
    min_bitstream_buffer_offset_alignment: vk::DeviceSize,
    min_bitstream_buffer_size_alignment: vk::DeviceSize,
    picture_access_granularity: vk::Extent2D,
    min_coded_extent: vk::Extent2D,
    max_coded_extent: vk::Extent2D,
    max_dpb_slots: u32,
    max_active_reference_pictures: u32,
    max_level_idc: u32,
    std_header_version: vk::ExtensionProperties,
}

#[derive(Clone, Copy)]
struct HevcDecodeSubmitExecutionContext<'a> {
    instance: &'a ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: DecodeQueueFamilyIndex,
    output_format: vk::Format,
    capability_snapshot: &'a HevcCapabilitySnapshot,
    parameter_sets: &'a HevcParameterSets,
    bitstream: &'a [u8],
    submit_probe_access_unit_limit: usize,
}

#[derive(Debug, Clone)]
struct HevcSessionBootstrapMachine<State> {
    parsed: ParsedHevcParameterSets,
    state: State,
    _marker: PhantomData<State>,
}

impl ExtensionFlags {
    fn supports_hevc_decode(self) -> bool {
        self.has_video_queue && self.has_video_decode_queue && self.has_video_decode_h265
    }

    fn union_assign(&mut self, other: Self) {
        self.has_video_queue |= other.has_video_queue;
        self.has_video_decode_queue |= other.has_video_decode_queue;
        self.has_video_decode_h265 |= other.has_video_decode_h265;
    }
}

/// Safe wrapper around Vulkan extension probing used by higher-level backend code.
///
/// All Vulkan FFI (`unsafe`) is confined to this module so callers in `video-hw`
/// only interact with a plain Rust enum and do not need to reason about raw Vulkan
/// object lifetimes, pointers, or destruction ordering.
pub(crate) fn probe_hevc_decode_prerequisites() -> HevcDecodePrerequisiteProbe {
    static CACHE: OnceLock<HevcDecodePrerequisiteProbe> = OnceLock::new();
    CACHE.get_or_init(run_hevc_decode_probe).clone()
}

pub(crate) fn extract_hevc_parameter_sets_annexb(
    bitstream: &[u8],
) -> Result<HevcParameterSets, String> {
    let nalus = split_annexb_nalus(bitstream);
    let mut vps = None;
    let mut sps = None;
    let mut pps = None;
    let mut first_vcl_nalus = Vec::new();

    for nalu in nalus {
        if hevc_nal_type_raw(nalu).is_some_and(|nal_type| nal_type <= 31)
            && first_vcl_nalus.len() < 8
        {
            first_vcl_nalus.push(nalu.to_vec());
        }
        let Some(nal_type) = hevc_nal_type(nalu) else {
            continue;
        };
        match nal_type {
            NALUnitType::VpsNut if vps.is_none() => vps = Some(nalu.to_vec()),
            NALUnitType::SpsNut if sps.is_none() => sps = Some(nalu.to_vec()),
            NALUnitType::PpsNut if pps.is_none() => pps = Some(nalu.to_vec()),
            _ => {}
        }
    }

    let vps = vps.ok_or_else(|| "missing VPS in HEVC Annex-B bitstream".to_string())?;
    let sps = sps.ok_or_else(|| "missing SPS in HEVC Annex-B bitstream".to_string())?;
    let pps = pps.ok_or_else(|| "missing PPS in HEVC Annex-B bitstream".to_string())?;

    let parsed_sps = SpsNALUnit::parse(Cursor::new(&sps))
        .map_err(|err| format!("failed to parse SPS: {err}"))?;
    let parsed_sps_rbsp = parsed_sps.rbsp;
    let coded_width = u32::try_from(parsed_sps_rbsp.cropped_width())
        .map_err(|_| "SPS cropped_width does not fit into u32".to_string())?;
    let coded_height = u32::try_from(parsed_sps_rbsp.cropped_height())
        .map_err(|_| "SPS cropped_height does not fit into u32".to_string())?;

    Ok(HevcParameterSets {
        vps,
        sps,
        pps,
        first_vcl_nalus,
        parsed_sps: parsed_sps_rbsp,
        coded_width,
        coded_height,
    })
}

pub(crate) fn extract_hevc_access_unit_headers(
    bitstream: &[u8],
) -> Result<Vec<HevcAccessUnitHeader>, String> {
    let parameter_sets = extract_hevc_parameter_sets_annexb(bitstream)?;
    let parsed_pps = parse_hevc_pps(&parameter_sets.pps)?;
    let mut headers = Vec::new();

    for nalu in split_annexb_nalus(bitstream) {
        let Some(nal_unit_type) = hevc_nal_type_raw(nalu) else {
            continue;
        };
        if nal_unit_type > 31 {
            continue;
        }
        let slice_header = parse_hevc_slice_header(nalu, &parsed_pps, &parameter_sets.parsed_sps)
            .map_err(|err| {
            format!("failed to parse HEVC slice header while extracting access units: {err}")
        })?;
        if slice_header.is_first_slice_segment {
            headers.push(HevcAccessUnitHeader {
                nal_unit_type: slice_header.nal_unit_type,
                pps_id: slice_header.pps_id,
                pic_order_cnt_lsb: slice_header.pic_order_cnt_lsb,
            });
        }
    }

    if headers.is_empty() {
        return Err(
            "HEVC bitstream does not contain any access-unit-leading VCL NAL units".to_string(),
        );
    }

    Ok(headers)
}

#[cfg(test)]
pub(crate) fn estimate_hevc_access_unit_count(bitstream: &[u8]) -> Result<usize, String> {
    Ok(extract_hevc_access_unit_headers(bitstream)?.len())
}

pub(crate) fn probe_hevc_decode_session_bootstrap(
    bitstream: &[u8],
) -> Result<HevcDecodeSessionBootstrap, String> {
    probe_hevc_decode_session_bootstrap_with_access_unit_limit(bitstream, None)
}

pub(crate) fn probe_hevc_decode_session_bootstrap_with_access_unit_limit(
    bitstream: &[u8],
    submit_probe_access_unit_limit: Option<usize>,
) -> Result<HevcDecodeSessionBootstrap, String> {
    let machine = HevcSessionBootstrapMachine::<AwaitingCapabilityProbe>::parse(bitstream)?;
    let submit_probe_access_unit_limit = submit_probe_access_unit_limit
        .unwrap_or(MAX_HEVC_SUBMIT_PROBE_ACCESS_UNITS)
        .max(1);

    // SAFETY: We only load Vulkan entry points and keep the handle local to this function.
    let entry = unsafe { ash::Entry::load() }
        .map_err(|err| format!("failed to load Vulkan entry: {err}"))?;

    // SAFETY: Default create info is valid for instance creation. We destroy the instance
    // before returning and no raw handle escapes this function.
    let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default(), None) }
        .map_err(|err| format!("failed to create Vulkan instance: {err}"))?;

    let bootstrap_result = (|| -> Result<HevcDecodeSessionBootstrap, String> {
        let machine = machine.probe_capabilities(&entry, &instance)?;
        let (
            session_probe,
            session_parameters_probe,
            submit_skeleton_probe,
            submit_execution_probe,
        ) = machine.probe_video_session_and_parameters_creation(
            &instance,
            bitstream,
            submit_probe_access_unit_limit,
        );
        Ok(machine.into_bootstrap(
            session_probe,
            session_parameters_probe,
            submit_skeleton_probe,
            submit_execution_probe,
        ))
    })();

    // SAFETY: `instance` was created in this function and is not used after this point.
    unsafe {
        instance.destroy_instance(None);
    }

    bootstrap_result
}

impl HevcSessionBootstrapMachine<AwaitingCapabilityProbe> {
    fn parse(bitstream: &[u8]) -> Result<Self, String> {
        let parsed = ParsedHevcParameterSets(extract_hevc_parameter_sets_annexb(bitstream)?);
        Ok(Self {
            parsed,
            state: AwaitingCapabilityProbe,
            _marker: PhantomData,
        })
    }

    fn probe_capabilities(
        self,
        entry: &ash::Entry,
        instance: &ash::Instance,
    ) -> Result<HevcSessionBootstrapMachine<CapabilityProbeComplete>, String> {
        let video_queue = ash::khr::video_queue::Instance::new(entry, instance);

        // SAFETY: `instance` is valid here; we only enumerate physical device handles.
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;
        if physical_devices.is_empty() {
            return Err("no Vulkan physical devices available for HEVC bootstrap".to_string());
        }

        let mut probe_errors = Vec::new();
        for physical_device in physical_devices {
            let support = query_adapter_decode_support(instance, physical_device)
                .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
            if !support.extensions.supports_hevc_decode() {
                continue;
            }
            let Some(queue_family_index) = support.decode_queue_family_index else {
                continue;
            };

            let mut decode_h265_profile = vk::VideoDecodeH265ProfileInfoKHR::default()
                .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
            let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
                .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
            let profile = vk::VideoProfileInfoKHR::default()
                .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_H265)
                .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
                .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
                .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
                .push_next(&mut decode_h265_profile)
                .push_next(&mut decode_usage);

            let mut decode_capabilities = vk::VideoDecodeCapabilitiesKHR::default();
            let mut decode_h265_capabilities = vk::VideoDecodeH265CapabilitiesKHR::default();
            let mut capabilities = vk::VideoCapabilitiesKHR::default()
                .push_next(&mut decode_h265_capabilities)
                .push_next(&mut decode_capabilities);

            // SAFETY: All pointers passed into the Vulkan call refer to stack-allocated
            // structs that live for the duration of the call.
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

            let coded_width = self.parsed.0.coded_width;
            let coded_height = self.parsed.0.coded_height;
            if coded_width < capabilities.min_coded_extent.width
                || coded_width > capabilities.max_coded_extent.width
                || coded_height < capabilities.min_coded_extent.height
                || coded_height > capabilities.max_coded_extent.height
            {
                probe_errors.push(format!(
                    "SPS coded extent {}x{} is outside device-supported range {}x{}..{}x{}",
                    coded_width,
                    coded_height,
                    capabilities.min_coded_extent.width,
                    capabilities.min_coded_extent.height,
                    capabilities.max_coded_extent.width,
                    capabilities.max_coded_extent.height
                ));
                continue;
            }

            let decode_output_formats =
                query_hevc_decode_output_formats(&video_queue, physical_device, profile)?;
            let min_coded_extent = capabilities.min_coded_extent;
            let max_coded_extent = capabilities.max_coded_extent;
            let max_dpb_slots = capabilities.max_dpb_slots;
            let max_active_reference_pictures = capabilities.max_active_reference_pictures;
            let std_header_version = capabilities.std_header_version;
            let capability_snapshot = HevcCapabilitySnapshot {
                min_bitstream_buffer_offset_alignment: capabilities
                    .min_bitstream_buffer_offset_alignment,
                min_bitstream_buffer_size_alignment: capabilities
                    .min_bitstream_buffer_size_alignment,
                picture_access_granularity: capabilities.picture_access_granularity,
                min_coded_extent,
                max_coded_extent,
                max_dpb_slots,
                max_active_reference_pictures,
                max_level_idc: decode_h265_capabilities.max_level_idc,
                std_header_version,
            };

            return Ok(HevcSessionBootstrapMachine {
                parsed: self.parsed,
                state: CapabilityProbeComplete {
                    physical_device,
                    queue_family_index,
                    capability_snapshot,
                    decode_output_formats,
                },
                _marker: PhantomData,
            });
        }

        if probe_errors.is_empty() {
            Err("no Vulkan adapter passed HEVC decode bootstrap checks".to_string())
        } else {
            Err(format!(
                "HEVC decode bootstrap checks failed on all candidate adapters: {}",
                probe_errors.join("; ")
            ))
        }
    }
}

impl HevcSessionBootstrapMachine<CapabilityProbeComplete> {
    fn into_bootstrap(
        self,
        video_session_create_probe: HevcVideoSessionCreateProbe,
        video_session_parameters_create_probe: HevcVideoSessionParametersCreateProbe,
        decode_submit_skeleton_probe: HevcDecodeSubmitSkeletonProbe,
        decode_submit_execution_probe: HevcDecodeSubmitExecutionProbe,
    ) -> HevcDecodeSessionBootstrap {
        HevcDecodeSessionBootstrap {
            coded_width: self.parsed.0.coded_width,
            coded_height: self.parsed.0.coded_height,
            min_coded_width: self.state.capability_snapshot.min_coded_extent.width,
            min_coded_height: self.state.capability_snapshot.min_coded_extent.height,
            max_coded_width: self.state.capability_snapshot.max_coded_extent.width,
            max_coded_height: self.state.capability_snapshot.max_coded_extent.height,
            max_dpb_slots: self.state.capability_snapshot.max_dpb_slots,
            max_active_reference_pictures: self
                .state
                .capability_snapshot
                .max_active_reference_pictures,
            max_level_idc: self.state.capability_snapshot.max_level_idc,
            decode_output_formats: self.state.decode_output_formats,
            video_session_create_probe,
            video_session_parameters_create_probe,
            decode_submit_skeleton_probe,
            decode_submit_execution_probe,
        }
    }

    fn probe_video_session_and_parameters_creation(
        &self,
        instance: &ash::Instance,
        bitstream: &[u8],
        submit_probe_access_unit_limit: usize,
    ) -> (
        HevcVideoSessionCreateProbe,
        HevcVideoSessionParametersCreateProbe,
        HevcDecodeSubmitSkeletonProbe,
        HevcDecodeSubmitExecutionProbe,
    ) {
        if self.state.decode_output_formats.is_empty() {
            return (
                HevcVideoSessionCreateProbe::Failed(
                    "video session create probe skipped: no decode output format reported"
                        .to_string(),
                ),
                HevcVideoSessionParametersCreateProbe::Skipped(
                    "video session parameters create probe skipped: no video session handle"
                        .to_string(),
                ),
                HevcDecodeSubmitSkeletonProbe::Skipped(
                    "decode submit skeleton skipped: no video session handle".to_string(),
                ),
                HevcDecodeSubmitExecutionProbe::Skipped(
                    "decode submit execution probe skipped: no video session handle".to_string(),
                ),
            );
        }

        let device = match create_hevc_decode_device(
            instance,
            self.state.physical_device,
            self.state.queue_family_index,
        ) {
            Ok(device) => device,
            Err(err) => {
                return (
                    HevcVideoSessionCreateProbe::Failed(err),
                    HevcVideoSessionParametersCreateProbe::Skipped(
                        "video session parameters create probe skipped: no video session handle"
                            .to_string(),
                    ),
                    HevcDecodeSubmitSkeletonProbe::Skipped(
                        "decode submit skeleton skipped: no video session handle".to_string(),
                    ),
                    HevcDecodeSubmitExecutionProbe::Skipped(
                        "decode submit execution probe skipped: no video session handle"
                            .to_string(),
                    ),
                );
            }
        };

        let probe_result = (|| {
            let mut session_create_errors = Vec::new();
            let mut session_parameters_errors = Vec::new();

            for picture_format in self.state.decode_output_formats.iter().copied() {
                match self.probe_single_video_session_format(&device, instance, picture_format) {
                    Ok(HevcVideoSessionParametersCreateProbe::Created) => {
                        let submit_skeleton_probe = build_hevc_decode_submit_skeleton_probe(
                            &self.parsed.0,
                            &self.state.capability_snapshot,
                        );
                        let submit_execution_probe = probe_hevc_decode_submit_execution(
                            &device,
                            HevcDecodeSubmitExecutionContext {
                                instance,
                                physical_device: self.state.physical_device,
                                queue_family_index: self.state.queue_family_index,
                                output_format: picture_format,
                                capability_snapshot: &self.state.capability_snapshot,
                                parameter_sets: &self.parsed.0,
                                bitstream,
                                submit_probe_access_unit_limit,
                            },
                        );
                        return (
                            HevcVideoSessionCreateProbe::Created,
                            HevcVideoSessionParametersCreateProbe::Created,
                            submit_skeleton_probe,
                            submit_execution_probe,
                        );
                    }
                    Ok(HevcVideoSessionParametersCreateProbe::Failed(err)) => {
                        session_parameters_errors.push(format!("{picture_format:?}: {err}"));
                    }
                    Ok(HevcVideoSessionParametersCreateProbe::Skipped(reason)) => {
                        session_parameters_errors.push(format!("{picture_format:?}: {reason}"));
                    }
                    Err(err) => {
                        session_create_errors.push(format!("{picture_format:?}: {err}"));
                    }
                }
            }

            if !session_parameters_errors.is_empty() {
                return (
                    HevcVideoSessionCreateProbe::Created,
                    HevcVideoSessionParametersCreateProbe::Failed(format!(
                        "vkCreateVideoSessionParametersKHR failed on all candidate output formats: {}",
                        session_parameters_errors.join("; ")
                    )),
                    HevcDecodeSubmitSkeletonProbe::Skipped(
                        "decode submit skeleton skipped: video session parameters creation failed"
                            .to_string(),
                    ),
                    HevcDecodeSubmitExecutionProbe::Skipped(
                        "decode submit execution probe skipped: video session parameters creation failed"
                            .to_string(),
                    ),
                );
            }

            let create_details = if session_create_errors.is_empty() {
                "no decode output formats were reported".to_string()
            } else {
                session_create_errors.join("; ")
            };
            (
                HevcVideoSessionCreateProbe::Failed(format!(
                    "vkCreateVideoSessionKHR failed on all candidate output formats: {create_details}"
                )),
                HevcVideoSessionParametersCreateProbe::Skipped(
                    "video session parameters create probe skipped: video session creation failed"
                        .to_string(),
                ),
                HevcDecodeSubmitSkeletonProbe::Skipped(
                    "decode submit skeleton skipped: video session creation failed".to_string(),
                ),
                HevcDecodeSubmitExecutionProbe::Skipped(
                    "decode submit execution probe skipped: video session creation failed"
                        .to_string(),
                ),
            )
        })();

        // SAFETY: `device` is no longer used after this point.
        unsafe {
            device.destroy_device(None);
        }

        probe_result
    }

    fn probe_single_video_session_format(
        &self,
        device: &ash::Device,
        instance: &ash::Instance,
        picture_format: vk::Format,
    ) -> Result<HevcVideoSessionParametersCreateProbe, String> {
        let mut decode_h265_profile = vk::VideoDecodeH265ProfileInfoKHR::default()
            .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
        let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
            .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
        let profile = vk::VideoProfileInfoKHR::default()
            .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_H265)
            .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
            .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
            .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
            .push_next(&mut decode_h265_profile)
            .push_next(&mut decode_usage);

        let create_info = vk::VideoSessionCreateInfoKHR::default()
            .queue_family_index(self.state.queue_family_index.0)
            .video_profile(&profile)
            .picture_format(picture_format)
            .max_coded_extent(vk::Extent2D {
                width: self.parsed.0.coded_width,
                height: self.parsed.0.coded_height,
            })
            .reference_picture_format(picture_format)
            .max_dpb_slots(self.state.capability_snapshot.max_dpb_slots.max(1))
            .max_active_reference_pictures(
                self.state
                    .capability_snapshot
                    .max_active_reference_pictures
                    .max(1),
            )
            .std_header_version(&self.state.capability_snapshot.std_header_version);
        let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
        let mut video_session = vk::VideoSessionKHR::null();

        // SAFETY: All pointers in `create_info` reference stack values that remain valid
        // during the call, and `device` is a valid logical device created above.
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

        let session_parameters_probe = probe_video_session_parameters_creation(
            device,
            &video_queue_device,
            video_session,
            &self.parsed.0,
        );

        // SAFETY: `video_session` was created by the same `device` and is not reused.
        unsafe {
            (video_queue_device.fp().destroy_video_session_khr)(
                device.handle(),
                video_session,
                std::ptr::null(),
            );
        }
        Ok(session_parameters_probe)
    }
}

fn create_video_session_parameters(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_session: vk::VideoSessionKHR,
    parameter_sets: &HevcParameterSets,
) -> Result<vk::VideoSessionParametersKHR, String> {
    let std_parameter_sets = match build_hevc_std_parameter_set_storage(parameter_sets) {
        Ok(std_parameter_sets) => std_parameter_sets,
        Err(err) => {
            return Err(format!(
                "failed to map HEVC VPS/SPS/PPS into StdVideo structs: {err}"
            ));
        }
    };
    let parameters_add_info = std_parameter_sets.add_info();
    let mut decode_h265_session_parameters =
        vk::VideoDecodeH265SessionParametersCreateInfoKHR::default()
            .max_std_vps_count(1)
            .max_std_sps_count(1)
            .max_std_pps_count(1)
            .parameters_add_info(&parameters_add_info);
    let create_info = vk::VideoSessionParametersCreateInfoKHR::default()
        .video_session(video_session)
        .video_session_parameters_template(vk::VideoSessionParametersKHR::null())
        .push_next(&mut decode_h265_session_parameters);
    let mut video_session_parameters = vk::VideoSessionParametersKHR::null();

    // SAFETY: `create_info` references stack data alive for the call and `device` is valid.
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
    Ok(video_session_parameters)
}

fn probe_video_session_parameters_creation(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_session: vk::VideoSessionKHR,
    parameter_sets: &HevcParameterSets,
) -> HevcVideoSessionParametersCreateProbe {
    let video_session_parameters = match create_video_session_parameters(
        device,
        video_queue_device,
        video_session,
        parameter_sets,
    ) {
        Ok(video_session_parameters) => video_session_parameters,
        Err(err) => return HevcVideoSessionParametersCreateProbe::Failed(err),
    };

    // SAFETY: `video_session_parameters` was created from this device and is no longer used.
    unsafe {
        (video_queue_device.fp().destroy_video_session_parameters_khr)(
            device.handle(),
            video_session_parameters,
            std::ptr::null(),
        );
    }
    HevcVideoSessionParametersCreateProbe::Created
}

fn build_hevc_decode_submit_skeleton_probe(
    parameter_sets: &HevcParameterSets,
    capability_snapshot: &HevcCapabilitySnapshot,
) -> HevcDecodeSubmitSkeletonProbe {
    let parsed_vps = match parse_hevc_vps(&parameter_sets.vps) {
        Ok(parsed_vps) => parsed_vps,
        Err(err) => {
            return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                "failed to parse VPS for decode submit skeleton: {err}"
            ));
        }
    };
    let parsed_pps = match parse_hevc_pps(&parameter_sets.pps) {
        Ok(parsed_pps) => parsed_pps,
        Err(err) => {
            return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                "failed to parse PPS for decode submit skeleton: {err}"
            ));
        }
    };
    let sps_id = match narrow_u64_to_u8(
        parameter_sets.parsed_sps.sps_seq_parameter_set_id,
        "sps_seq_parameter_set_id",
    ) {
        Ok(sps_id) => sps_id,
        Err(err) => {
            return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                "failed to map SPS id for decode submit skeleton: {err}"
            ));
        }
    };

    let dpb_slot_count = capability_snapshot.max_dpb_slots.clamp(1, 16);
    let mut planned_dpb_slots = Vec::new();
    for slot in 0..dpb_slot_count {
        match u8::try_from(slot) {
            Ok(slot_index) => planned_dpb_slots.push(slot_index),
            Err(_) => {
                return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                    "DPB slot index {slot} does not fit in u8"
                ));
            }
        }
    }

    let requested_references = parameter_sets
        .parsed_sps
        .short_term_ref_pic_sets
        .num_delta_pocs
        .first()
        .copied()
        .unwrap_or(0);
    let reference_capacity = u64::from(capability_snapshot.max_active_reference_pictures.max(1));
    let max_plannable_references = u64::from(dpb_slot_count.saturating_sub(1));
    let planned_reference_count = requested_references
        .min(reference_capacity)
        .min(max_plannable_references);
    let mut planned_reference_slots = Vec::new();
    for slot in 1..=planned_reference_count {
        match u8::try_from(slot) {
            Ok(slot_index) => planned_reference_slots.push(slot_index),
            Err(_) => {
                return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                    "reference slot index {slot} does not fit in u8"
                ));
            }
        }
    }

    let (first_slice_nal_type, first_slice_pps_id, first_slice_pic_order_cnt_lsb) =
        if let Some(first_vcl_nalu) = parameter_sets.first_vcl_nalus.first() {
            match parse_hevc_slice_header(first_vcl_nalu, &parsed_pps, &parameter_sets.parsed_sps) {
                Ok(slice_header) => (
                    Some(slice_header.nal_unit_type),
                    Some(slice_header.pps_id),
                    slice_header.pic_order_cnt_lsb,
                ),
                Err(err) => {
                    return HevcDecodeSubmitSkeletonProbe::Failed(format!(
                        "failed to parse first VCL slice header for decode submit skeleton: {err}"
                    ));
                }
            }
        } else {
            (None, None, None)
        };

    HevcDecodeSubmitSkeletonProbe::Ready(HevcDecodeSubmitSkeleton {
        vps_id: parsed_vps.vps_video_parameter_set_id,
        sps_id,
        pps_id: parsed_pps.pps_pic_parameter_set_id,
        vcl_nalu_count: parameter_sets.first_vcl_nalus.len(),
        first_slice_nal_type,
        first_slice_pps_id,
        first_slice_pic_order_cnt_lsb,
        planned_dpb_slots,
        planned_reference_slots,
    })
}

fn probe_hevc_decode_submit_execution(
    device: &ash::Device,
    context: HevcDecodeSubmitExecutionContext<'_>,
) -> HevcDecodeSubmitExecutionProbe {
    let HevcDecodeSubmitExecutionContext {
        instance,
        physical_device,
        queue_family_index,
        output_format,
        capability_snapshot,
        parameter_sets,
        bitstream,
        submit_probe_access_unit_limit,
    } = context;
    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    let video_decode_device = ash::khr::video_decode_queue::Device::new(instance, device);
    let mut video_session = vk::VideoSessionKHR::null();
    let mut video_session_parameters = vk::VideoSessionParametersKHR::null();
    let mut session_memories = Vec::new();
    let mut src_buffer = vk::Buffer::null();
    let mut src_buffer_memory = vk::DeviceMemory::null();
    let mut readback_buffer = vk::Buffer::null();
    let mut readback_buffer_memory = vk::DeviceMemory::null();
    let mut decode_image = vk::Image::null();
    let mut decode_image_memory = vk::DeviceMemory::null();
    let mut decode_image_views = Vec::new();
    let mut command_pool = vk::CommandPool::null();
    let mut fence = vk::Fence::null();
    let mut readback_non_zero = false;
    let mut readback_bytes = 0_usize;
    let mut readback_planes = 0_u32;
    let mut readback_sample_stride = 0_usize;
    let mut readback_sample_count = 0_u32;
    let mut readback_sample = Vec::new();
    let mut submitted_access_units = 0_u32;
    let mut experimental_dpb_enabled = false;
    let mut experimental_dpb_mode = HevcExperimentalDpbMode::Off;
    let mut experimental_dpb_status = "mode=off (experimental DPB disabled)".to_string();
    let mut experimental_dpb_marker_path = None;

    let submit_result = (|| -> Result<(), String> {
        let mut decode_h265_profile = vk::VideoDecodeH265ProfileInfoKHR::default()
            .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
        let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
            .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
        let profile = vk::VideoProfileInfoKHR::default()
            .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_H265)
            .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
            .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
            .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
            .push_next(&mut decode_h265_profile)
            .push_next(&mut decode_usage);
        let create_info = vk::VideoSessionCreateInfoKHR::default()
            .queue_family_index(queue_family_index.0)
            .video_profile(&profile)
            .picture_format(output_format)
            .max_coded_extent(vk::Extent2D {
                width: parameter_sets.coded_width,
                height: parameter_sets.coded_height,
            })
            .reference_picture_format(output_format)
            .max_dpb_slots(capability_snapshot.max_dpb_slots.max(1))
            .max_active_reference_pictures(capability_snapshot.max_active_reference_pictures.max(1))
            .std_header_version(&capability_snapshot.std_header_version);
        // SAFETY: All pointers in `create_info` reference stack data valid for this call.
        let session_create_result = unsafe {
            (video_queue_device.fp().create_video_session_khr)(
                device.handle(),
                &create_info,
                std::ptr::null(),
                &mut video_session,
            )
        };
        if session_create_result != vk::Result::SUCCESS {
            return Err(format!(
                "vkCreateVideoSessionKHR for submit execution probe failed: {session_create_result:?}"
            ));
        }

        video_session_parameters = create_video_session_parameters(
            device,
            &video_queue_device,
            video_session,
            parameter_sets,
        )?;

        // SAFETY: `physical_device` belongs to `instance`; we only read immutable memory properties.
        let memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };

        let mut session_requirement_count = 0_u32;
        // SAFETY: First query requests only the count for a valid video session handle.
        let session_requirement_count_result = unsafe {
            (video_queue_device
                .fp()
                .get_video_session_memory_requirements_khr)(
                device.handle(),
                video_session,
                &mut session_requirement_count,
                std::ptr::null_mut(),
            )
        };
        if session_requirement_count_result != vk::Result::SUCCESS {
            return Err(format!(
                "vkGetVideoSessionMemoryRequirementsKHR count query failed: {session_requirement_count_result:?}"
            ));
        }
        if session_requirement_count == 0 {
            return Err(
                "vkGetVideoSessionMemoryRequirementsKHR returned no memory requirements"
                    .to_string(),
            );
        }

        let mut session_requirements = vec![
            vk::VideoSessionMemoryRequirementsKHR::default();
            session_requirement_count as usize
        ];
        // SAFETY: `session_requirements` storage is sized from the count query above.
        let session_requirements_result = unsafe {
            (video_queue_device
                .fp()
                .get_video_session_memory_requirements_khr)(
                device.handle(),
                video_session,
                &mut session_requirement_count,
                session_requirements.as_mut_ptr(),
            )
        };
        if session_requirements_result != vk::Result::SUCCESS {
            return Err(format!(
                "vkGetVideoSessionMemoryRequirementsKHR query failed: {session_requirements_result:?}"
            ));
        }

        let mut session_bindings = Vec::with_capacity(session_requirements.len());
        for requirement in &session_requirements {
            let requirement_info = requirement.memory_requirements;
            let memory_type_index = select_memory_type_index(
                &memory_properties,
                requirement_info.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .or_else(|| {
                select_memory_type_index(
                    &memory_properties,
                    requirement_info.memory_type_bits,
                    vk::MemoryPropertyFlags::empty(),
                )
            })
            .ok_or_else(|| {
                format!(
                    "no compatible memory type for video session bind index {} (bits=0x{:X})",
                    requirement.memory_bind_index, requirement_info.memory_type_bits
                )
            })?;
            let allocation_size = requirement_info.size.max(1);
            let allocate_info = vk::MemoryAllocateInfo::default()
                .allocation_size(allocation_size)
                .memory_type_index(memory_type_index);
            // SAFETY: Allocation info references only POD values and `device` is valid.
            let memory = unsafe { device.allocate_memory(&allocate_info, None) }
                .map_err(|err| format!("vkAllocateMemory for video session bind failed: {err}"))?;
            session_memories.push(memory);
            session_bindings.push(
                vk::BindVideoSessionMemoryInfoKHR::default()
                    .memory_bind_index(requirement.memory_bind_index)
                    .memory(memory)
                    .memory_offset(0)
                    .memory_size(allocation_size),
            );
        }
        // SAFETY: bind infos reference memory allocations created above and live through the call.
        let bind_session_memory_result = unsafe {
            (video_queue_device.fp().bind_video_session_memory_khr)(
                device.handle(),
                video_session,
                u32::try_from(session_bindings.len())
                    .map_err(|_| "session binding count exceeds u32 range".to_string())?,
                session_bindings.as_ptr(),
            )
        };
        if bind_session_memory_result != vk::Result::SUCCESS {
            return Err(format!(
                "vkBindVideoSessionMemoryKHR failed: {bind_session_memory_result:?}"
            ));
        }

        let payload = build_hevc_submit_probe_bitstream_payload(
            parameter_sets,
            bitstream,
            submit_probe_access_unit_limit,
        )?;
        let queue = unsafe { device.get_device_queue(queue_family_index.0, 0) };
        if queue == vk::Queue::null() {
            return Err(format!(
                "vkGetDeviceQueue returned null queue for family {}",
                queue_family_index.0
            ));
        }

        let src_buffer_offset = 0_u64;
        let src_offset_alignment = capability_snapshot
            .min_bitstream_buffer_offset_alignment
            .max(1);
        if !src_buffer_offset.is_multiple_of(src_offset_alignment) {
            return Err(format!(
                "src buffer offset {src_buffer_offset} is not aligned to {}",
                src_offset_alignment
            ));
        }
        let src_buffer_range = align_up(
            u64::try_from(payload.bytes.len())
                .map_err(|_| "submit probe payload size exceeds u64 range".to_string())?,
            capability_snapshot
                .min_bitstream_buffer_size_alignment
                .max(1),
        );
        if src_buffer_range == 0 {
            return Err("submit probe payload produced an empty bitstream buffer".to_string());
        }
        let src_buffer_create_info = vk::BufferCreateInfo::default()
            .size(src_buffer_range)
            .usage(vk::BufferUsageFlags::VIDEO_DECODE_SRC_KHR)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: `src_buffer_create_info` is fully initialized and `device` is valid.
        src_buffer = unsafe { device.create_buffer(&src_buffer_create_info, None) }
            .map_err(|err| format!("vkCreateBuffer for decode source failed: {err}"))?;
        // SAFETY: source buffer was created from the same logical device.
        let src_buffer_requirements = unsafe { device.get_buffer_memory_requirements(src_buffer) };
        let src_memory_type_index = select_memory_type_index(
            &memory_properties,
            src_buffer_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| {
            format!(
                "no HOST_VISIBLE|HOST_COHERENT memory type for decode source buffer (bits=0x{:X})",
                src_buffer_requirements.memory_type_bits
            )
        })?;
        let src_allocation_size = src_buffer_requirements.size.max(src_buffer_range);
        let src_allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(src_allocation_size)
            .memory_type_index(src_memory_type_index);
        // SAFETY: Allocation info references only POD values and `device` is valid.
        src_buffer_memory = unsafe { device.allocate_memory(&src_allocate_info, None) }
            .map_err(|err| format!("vkAllocateMemory for decode source buffer failed: {err}"))?;
        // SAFETY: Buffer and memory were created from the same logical device.
        unsafe { device.bind_buffer_memory(src_buffer, src_buffer_memory, 0) }
            .map_err(|err| format!("vkBindBufferMemory for decode source buffer failed: {err}"))?;
        // SAFETY: Mapping range is within the allocated memory bound to `src_buffer_memory`.
        let mapped = unsafe {
            device.map_memory(
                src_buffer_memory,
                0,
                src_buffer_range,
                vk::MemoryMapFlags::empty(),
            )
        }
        .map_err(|err| format!("vkMapMemory for decode source buffer failed: {err}"))?;
        let mapped_len = usize::try_from(src_buffer_range)
            .map_err(|_| "decode source buffer range exceeds usize range".to_string())?;
        // SAFETY: destination pointer is valid for `mapped_len` bytes from `vkMapMemory`.
        unsafe {
            std::ptr::copy_nonoverlapping(
                payload.bytes.as_ptr(),
                mapped.cast::<u8>(),
                payload.bytes.len(),
            );
            if mapped_len > payload.bytes.len() {
                std::ptr::write_bytes(
                    mapped.cast::<u8>().add(payload.bytes.len()),
                    0,
                    mapped_len - payload.bytes.len(),
                );
            }
            device.unmap_memory(src_buffer_memory);
        }

        let coded_extent = vk::Extent2D {
            width: parameter_sets.coded_width,
            height: parameter_sets.coded_height,
        };
        let (single_readback_buffer_size, readback_regions_template) =
            build_decode_readback_regions(output_format, coded_extent.width, coded_extent.height)?;
        readback_planes = u32::try_from(readback_regions_template.len()).unwrap_or(u32::MAX);
        readback_sample_stride = usize::try_from(single_readback_buffer_size)
            .map_err(|_| "decode readback sample size exceeds usize range".to_string())?;
        readback_sample_count = u32::try_from(payload.access_units.len()).unwrap_or(u32::MAX);
        let readback_buffer_size = single_readback_buffer_size
            .checked_mul(u64::from(readback_sample_count))
            .ok_or_else(|| "decode readback buffer size overflow".to_string())?;
        let dpb_configuration = configure_hevc_experimental_dpb();
        experimental_dpb_enabled = dpb_configuration.enabled;
        experimental_dpb_mode = dpb_configuration.mode;
        experimental_dpb_status = dpb_configuration.status;
        experimental_dpb_marker_path = dpb_configuration.marker_path;
        let dpb_slot_count = if experimental_dpb_enabled {
            usize::try_from(capability_snapshot.max_dpb_slots.max(1))
                .ok()
                .unwrap_or(1)
        } else {
            1
        };
        let image_array_layers = u32::try_from(dpb_slot_count)
            .map_err(|_| "decode DPB slot count exceeds u32 range".to_string())?;
        let image_extent_width = u32::try_from(align_up(
            u64::from(coded_extent.width),
            u64::from(capability_snapshot.picture_access_granularity.width.max(1)),
        ))
        .map_err(|_| "aligned decode image width exceeds u32 range".to_string())?;
        let image_extent_height = u32::try_from(align_up(
            u64::from(coded_extent.height),
            u64::from(capability_snapshot.picture_access_granularity.height.max(1)),
        ))
        .map_err(|_| "aligned decode image height exceeds u32 range".to_string())?;
        let decode_image_create_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(output_format)
            .extent(vk::Extent3D {
                width: image_extent_width,
                height: image_extent_height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(image_array_layers)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(
                vk::ImageUsageFlags::VIDEO_DECODE_DST_KHR
                    | vk::ImageUsageFlags::VIDEO_DECODE_DPB_KHR
                    | vk::ImageUsageFlags::TRANSFER_SRC,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: image create info is fully initialized and device is valid.
        decode_image = unsafe { device.create_image(&decode_image_create_info, None) }
            .map_err(|err| format!("vkCreateImage for decode destination failed: {err}"))?;
        // SAFETY: image handle was created from `device`.
        let decode_image_requirements =
            unsafe { device.get_image_memory_requirements(decode_image) };
        let decode_image_memory_type = select_memory_type_index(
            &memory_properties,
            decode_image_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .or_else(|| {
            select_memory_type_index(
                &memory_properties,
                decode_image_requirements.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
            )
        })
        .ok_or_else(|| {
            format!(
                "no compatible memory type for decode destination image (bits=0x{:X})",
                decode_image_requirements.memory_type_bits
            )
        })?;
        let decode_image_allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(decode_image_requirements.size.max(1))
            .memory_type_index(decode_image_memory_type);
        // SAFETY: allocation parameters are provided by Vulkan memory requirements.
        decode_image_memory = unsafe { device.allocate_memory(&decode_image_allocate_info, None) }
            .map_err(|err| {
                format!("vkAllocateMemory for decode destination image failed: {err}")
            })?;
        // SAFETY: image and memory were created by the same logical device.
        unsafe { device.bind_image_memory(decode_image, decode_image_memory, 0) }.map_err(
            |err| format!("vkBindImageMemory for decode destination image failed: {err}"),
        )?;
        for layer_index in 0..image_array_layers {
            let decode_image_view_create_info = vk::ImageViewCreateInfo::default()
                .image(decode_image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(output_format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: layer_index,
                    layer_count: 1,
                });
            // SAFETY: image view create info references a valid image and subresource range.
            let decode_image_view =
                unsafe { device.create_image_view(&decode_image_view_create_info, None) }.map_err(
                    |err| format!("vkCreateImageView for decode destination failed: {err}"),
                )?;
            decode_image_views.push(decode_image_view);
        }
        if decode_image_views.is_empty() {
            return Err(
                "submit execution probe could not create any decode image views".to_string(),
            );
        }

        let readback_buffer_create_info = vk::BufferCreateInfo::default()
            .size(readback_buffer_size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: `readback_buffer_create_info` is fully initialized and `device` is valid.
        readback_buffer = unsafe { device.create_buffer(&readback_buffer_create_info, None) }
            .map_err(|err| format!("vkCreateBuffer for decode readback failed: {err}"))?;
        // SAFETY: readback buffer was created from the same logical device.
        let readback_buffer_requirements =
            unsafe { device.get_buffer_memory_requirements(readback_buffer) };
        let readback_memory_type_index = select_memory_type_index(
            &memory_properties,
            readback_buffer_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| {
            format!(
                "no HOST_VISIBLE|HOST_COHERENT memory type for decode readback buffer (bits=0x{:X})",
                readback_buffer_requirements.memory_type_bits
            )
        })?;
        let readback_allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(readback_buffer_requirements.size.max(readback_buffer_size))
            .memory_type_index(readback_memory_type_index);
        // SAFETY: allocation info references only POD values and `device` is valid.
        readback_buffer_memory = unsafe { device.allocate_memory(&readback_allocate_info, None) }
            .map_err(|err| {
            format!("vkAllocateMemory for decode readback buffer failed: {err}")
        })?;
        // SAFETY: buffer and memory were created by the same logical device.
        unsafe { device.bind_buffer_memory(readback_buffer, readback_buffer_memory, 0) }.map_err(
            |err| format!("vkBindBufferMemory for decode readback buffer failed: {err}"),
        )?;

        let command_pool_info =
            vk::CommandPoolCreateInfo::default().queue_family_index(queue_family_index.0);
        // SAFETY: command pool create info is valid for the selected queue family.
        command_pool = unsafe { device.create_command_pool(&command_pool_info, None) }
            .map_err(|err| format!("vkCreateCommandPool failed: {err}"))?;
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: allocate info references a valid command pool.
        let command_buffers = unsafe { device.allocate_command_buffers(&allocate_info) }
            .map_err(|err| format!("vkAllocateCommandBuffers failed: {err}"))?;
        let command_buffer = command_buffers.first().copied().ok_or_else(|| {
            "vkAllocateCommandBuffers returned no command buffers for submit execution probe"
                .to_string()
        })?;

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: command buffer is valid and not already recording.
        unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
            .map_err(|err| format!("vkBeginCommandBuffer failed: {err}"))?;

        let source_buffer_barrier = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::HOST)
            .src_access_mask(vk::AccessFlags2::HOST_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
            .dst_access_mask(vk::AccessFlags2::VIDEO_DECODE_READ_KHR)
            .buffer(src_buffer)
            .offset(0)
            .size(src_buffer_range);
        let destination_image_barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .src_access_mask(vk::AccessFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
            .dst_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::VIDEO_DECODE_DST_KHR)
            .image(decode_image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: image_array_layers,
            });
        let dependency_info = vk::DependencyInfo::default()
            .buffer_memory_barriers(std::slice::from_ref(&source_buffer_barrier))
            .image_memory_barriers(std::slice::from_ref(&destination_image_barrier));
        // SAFETY: barriers reference resources created above and the command buffer is recording.
        unsafe {
            device.cmd_pipeline_barrier2(command_buffer, &dependency_info);
        }

        let begin_coding_info = vk::VideoBeginCodingInfoKHR::default()
            .video_session(video_session)
            .video_session_parameters(video_session_parameters);
        let mut next_reference_slot = 0_usize;
        let mut active_reference_slots = Vec::new();
        let max_active_reference_slots = if experimental_dpb_enabled {
            usize::try_from(capability_snapshot.max_active_reference_pictures.max(1))
                .ok()
                .unwrap_or(1)
                .min(dpb_slot_count.max(1))
        } else {
            0
        };
        // SAFETY: command buffer is recording and session/session-parameters are valid.
        unsafe {
            (video_queue_device.fp().cmd_begin_video_coding_khr)(
                command_buffer,
                &begin_coding_info,
            );
        }
        for (submitted_index, access_unit) in payload.access_units.iter().enumerate() {
            let is_irap = (16..=23).contains(&access_unit.header.nal_unit_type);
            if experimental_dpb_enabled && is_irap {
                active_reference_slots.clear();
            }
            let is_reference = experimental_dpb_enabled
                && is_hevc_reference_nal_type(access_unit.header.nal_unit_type);
            let slot = select_hevc_decode_dpb_slot(
                experimental_dpb_enabled,
                is_reference,
                dpb_slot_count,
                &mut next_reference_slot,
                &active_reference_slots,
            );
            if experimental_dpb_enabled {
                active_reference_slots.retain(|entry| entry.slot != slot);
            }
            let destination_picture_resource = vk::VideoPictureResourceInfoKHR::default()
                .coded_offset(vk::Offset2D { x: 0, y: 0 })
                .coded_extent(coded_extent)
                .base_array_layer(0)
                .image_view_binding(decode_image_views[slot]);
            let mut std_picture_info_flags = empty_decode_h265_picture_info_flags();
            std_picture_info_flags.set_IrapPicFlag(bool_to_u32(is_irap));
            std_picture_info_flags.set_IdrPicFlag(bool_to_u32(is_hevc_idr_nal_type(
                access_unit.header.nal_unit_type,
            )));
            std_picture_info_flags.set_IsReference(bool_to_u32(is_reference));
            std_picture_info_flags.set_short_term_ref_pic_set_sps_flag(1);
            let current_pic_order_cnt_val = access_unit
                .header
                .pic_order_cnt_lsb
                .map(i32::from)
                .unwrap_or(0);
            let selected_references = if experimental_dpb_enabled && !is_irap {
                active_reference_slots
                    .iter()
                    .rev()
                    .take(HEVC_REF_PIC_SET_LIST_SIZE)
                    .copied()
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let selected_reference_picture_order_cnt_values = selected_references
                .iter()
                .map(|reference| reference.pic_order_cnt_val)
                .collect::<Vec<_>>();
            let (
                reference_pic_set_st_curr_before,
                reference_pic_set_st_curr_after,
                num_delta_pocs_of_ref_rps_idx,
            ) = build_hevc_ref_pic_set_lists(
                &selected_reference_picture_order_cnt_values,
                current_pic_order_cnt_val,
            );
            let std_picture_info = StdVideoDecodeH265PictureInfo {
                flags: std_picture_info_flags,
                sps_video_parameter_set_id: parameter_sets.parsed_sps.sps_video_parameter_set_id,
                pps_seq_parameter_set_id: payload.parsed_pps.pps_seq_parameter_set_id,
                pps_pic_parameter_set_id: access_unit.header.pps_id,
                NumDeltaPocsOfRefRpsIdx: num_delta_pocs_of_ref_rps_idx,
                PicOrderCntVal: current_pic_order_cnt_val,
                NumBitsForSTRefPicSetInSlice: 0,
                reserved: 0,
                RefPicSetStCurrBefore: reference_pic_set_st_curr_before,
                RefPicSetStCurrAfter: reference_pic_set_st_curr_after,
                RefPicSetLtCurr: [HEVC_NO_REFERENCE_PICTURE; HEVC_REF_PIC_SET_LIST_SIZE],
            };
            let slice_segment_offsets = [access_unit.slice_segment_offset];
            let mut h265_picture_info = vk::VideoDecodeH265PictureInfoKHR::default()
                .std_picture_info(&std_picture_info)
                .slice_segment_offsets(&slice_segment_offsets);
            let mut reference_picture_resources = Vec::new();
            let mut reference_info_values = Vec::new();
            let mut reference_info_flags = empty_decode_h265_reference_info_flags();
            reference_info_flags.set_used_for_long_term_reference(0);
            reference_info_flags.set_unused_for_reference(0);
            let mut reference_slots = Vec::new();
            for reference in &selected_references {
                reference_picture_resources.push(
                    vk::VideoPictureResourceInfoKHR::default()
                        .coded_offset(vk::Offset2D { x: 0, y: 0 })
                        .coded_extent(coded_extent)
                        .base_array_layer(0)
                        .image_view_binding(decode_image_views[reference.slot]),
                );
                reference_info_values.push(StdVideoDecodeH265ReferenceInfo {
                    flags: reference_info_flags,
                    PicOrderCntVal: reference.pic_order_cnt_val,
                });
            }
            let mut reference_dpb_slot_infos = reference_info_values
                .iter()
                .map(|reference_info| {
                    vk::VideoDecodeH265DpbSlotInfoKHR::default().std_reference_info(reference_info)
                })
                .collect::<Vec<_>>();
            for ((reference, reference_picture_resource), reference_dpb_slot_info) in
                selected_references
                    .iter()
                    .zip(reference_picture_resources.iter())
                    .zip(reference_dpb_slot_infos.iter_mut())
            {
                let reference_slot_index = i32::try_from(reference.slot)
                    .map_err(|_| "decode reference slot index exceeds i32 range".to_string())?;
                reference_slots.push(
                    vk::VideoReferenceSlotInfoKHR::default()
                        .slot_index(reference_slot_index)
                        .picture_resource(reference_picture_resource)
                        .push_next(reference_dpb_slot_info),
                );
            }
            let decode_info_base = vk::VideoDecodeInfoKHR::default()
                .src_buffer(src_buffer)
                .src_buffer_offset(src_buffer_offset)
                .src_buffer_range(src_buffer_range)
                .dst_picture_resource(destination_picture_resource);
            let mut decode_info_builder = decode_info_base;
            if !reference_slots.is_empty() {
                decode_info_builder = decode_info_builder.reference_slots(&reference_slots);
            }
            if is_reference {
                let mut setup_reference_info_flags = empty_decode_h265_reference_info_flags();
                setup_reference_info_flags.set_used_for_long_term_reference(0);
                setup_reference_info_flags.set_unused_for_reference(0);
                let setup_reference_info_value = StdVideoDecodeH265ReferenceInfo {
                    flags: setup_reference_info_flags,
                    PicOrderCntVal: current_pic_order_cnt_val,
                };
                let mut setup_reference_info = vk::VideoDecodeH265DpbSlotInfoKHR::default()
                    .std_reference_info(&setup_reference_info_value);
                let setup_picture_resource = vk::VideoPictureResourceInfoKHR::default()
                    .coded_offset(vk::Offset2D { x: 0, y: 0 })
                    .coded_extent(coded_extent)
                    .base_array_layer(0)
                    .image_view_binding(decode_image_views[slot]);
                let slot_index = i32::try_from(slot).map_err(|_| {
                    "decode setup reference slot index exceeds i32 range".to_string()
                })?;
                let setup_reference_slot = vk::VideoReferenceSlotInfoKHR::default()
                    .slot_index(slot_index)
                    .picture_resource(&setup_picture_resource)
                    .push_next(&mut setup_reference_info);
                let decode_info = decode_info_builder
                    .setup_reference_slot(&setup_reference_slot)
                    .push_next(&mut h265_picture_info);
                // SAFETY: command buffer is recording and decode info references live local data.
                unsafe {
                    (video_decode_device.fp().cmd_decode_video_khr)(command_buffer, &decode_info);
                }
            } else {
                let decode_info = decode_info_builder.push_next(&mut h265_picture_info);
                // SAFETY: command buffer is recording and decode info references live local data.
                unsafe {
                    (video_decode_device.fp().cmd_decode_video_khr)(command_buffer, &decode_info);
                }
            }
            let submitted_layer = u32::try_from(slot)
                .map_err(|_| "submitted DPB slot index exceeds u32 range".to_string())?;
            let sample_offset = single_readback_buffer_size
                .checked_mul(
                    u64::try_from(submitted_index)
                        .map_err(|_| "submitted access-unit index exceeds u64 range".to_string())?,
                )
                .ok_or_else(|| "decode readback sample offset overflow".to_string())?;
            let mut readback_regions = readback_regions_template.clone();
            for region in &mut readback_regions {
                region.image_subresource.base_array_layer = submitted_layer;
                region.buffer_offset = region
                    .buffer_offset
                    .checked_add(sample_offset)
                    .ok_or_else(|| "decode readback region offset overflow".to_string())?;
            }

            let decode_to_copy_image_barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                .src_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .old_layout(vk::ImageLayout::VIDEO_DECODE_DST_KHR)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .image(decode_image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: submitted_layer,
                    layer_count: 1,
                });
            let decode_to_copy_dependency = vk::DependencyInfo::default()
                .image_memory_barriers(std::slice::from_ref(&decode_to_copy_image_barrier));
            // SAFETY: decode output image and readback buffer are valid resources in this command buffer.
            unsafe {
                device.cmd_pipeline_barrier2(command_buffer, &decode_to_copy_dependency);
                device.cmd_copy_image_to_buffer(
                    command_buffer,
                    decode_image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    readback_buffer,
                    &readback_regions,
                );
            }

            let copy_to_decode_image_barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                .dst_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(vk::ImageLayout::VIDEO_DECODE_DST_KHR)
                .image(decode_image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: submitted_layer,
                    layer_count: 1,
                });
            let copy_to_decode_dependency = vk::DependencyInfo::default()
                .image_memory_barriers(std::slice::from_ref(&copy_to_decode_image_barrier));
            // SAFETY: image barrier transitions the same subresource back to decode destination layout.
            unsafe {
                device.cmd_pipeline_barrier2(command_buffer, &copy_to_decode_dependency);
            }
            if experimental_dpb_enabled && is_reference {
                active_reference_slots.push(HevcActiveReferenceSlot {
                    slot,
                    pic_order_cnt_val: current_pic_order_cnt_val,
                });
                let evicted = active_reference_slots
                    .len()
                    .saturating_sub(max_active_reference_slots);
                if evicted > 0 {
                    active_reference_slots.drain(0..evicted);
                }
            }
        }
        submitted_access_units = u32::try_from(payload.access_units.len()).unwrap_or(u32::MAX);
        // SAFETY: command buffer is recording and video coding has started.
        unsafe {
            (video_queue_device.fp().cmd_end_video_coding_khr)(
                command_buffer,
                &vk::VideoEndCodingInfoKHR::default(),
            );
        }
        // SAFETY: command buffer is in recording state.
        unsafe { device.end_command_buffer(command_buffer) }
            .map_err(|err| format!("vkEndCommandBuffer failed: {err}"))?;

        // SAFETY: fence create info is valid and the device is alive.
        fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
            .map_err(|err| format!("vkCreateFence failed: {err}"))?;
        let submit_info =
            vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&command_buffer));
        // SAFETY: queue and command buffer are valid; fence belongs to the same device.
        unsafe { device.queue_submit(queue, std::slice::from_ref(&submit_info), fence) }
            .map_err(|err| format!("vkQueueSubmit failed: {err}"))?;
        // SAFETY: waiting for a fence created by the same device is valid.
        unsafe { device.wait_for_fences(std::slice::from_ref(&fence), true, 1_000_000_000) }
            .map_err(|err| format!("vkWaitForFences failed: {err}"))?;

        // SAFETY: the full readback buffer range is within the allocated host-visible memory.
        let readback_ptr = unsafe {
            device.map_memory(
                readback_buffer_memory,
                0,
                readback_buffer_size,
                vk::MemoryMapFlags::empty(),
            )
        }
        .map_err(|err| format!("vkMapMemory for decode readback buffer failed: {err}"))?;
        let readback_len = usize::try_from(readback_buffer_size)
            .map_err(|_| "decode readback buffer size exceeds usize range".to_string())?;
        readback_bytes = readback_sample_stride;
        // SAFETY: pointer returned by `vkMapMemory` is valid for `readback_len` bytes.
        let readback_slice =
            unsafe { std::slice::from_raw_parts(readback_ptr.cast::<u8>(), readback_len) };
        readback_sample = readback_slice.to_vec();
        // SAFETY: pointer returned by `vkMapMemory` is valid for `readback_len` bytes.
        readback_non_zero = readback_slice.iter().any(|&byte| byte != 0);
        // SAFETY: readback buffer memory was mapped in this scope and must be unmapped once.
        unsafe {
            device.unmap_memory(readback_buffer_memory);
        }
        Ok(())
    })();

    // SAFETY: every handle below was created by `device` and is no longer used afterward.
    unsafe {
        if fence != vk::Fence::null() {
            device.destroy_fence(fence, None);
        }
        if command_pool != vk::CommandPool::null() {
            device.destroy_command_pool(command_pool, None);
        }
        for decode_image_view in decode_image_views {
            if decode_image_view != vk::ImageView::null() {
                device.destroy_image_view(decode_image_view, None);
            }
        }
        if decode_image != vk::Image::null() {
            device.destroy_image(decode_image, None);
        }
        if decode_image_memory != vk::DeviceMemory::null() {
            device.free_memory(decode_image_memory, None);
        }
        if src_buffer != vk::Buffer::null() {
            device.destroy_buffer(src_buffer, None);
        }
        if src_buffer_memory != vk::DeviceMemory::null() {
            device.free_memory(src_buffer_memory, None);
        }
        if readback_buffer != vk::Buffer::null() {
            device.destroy_buffer(readback_buffer, None);
        }
        if readback_buffer_memory != vk::DeviceMemory::null() {
            device.free_memory(readback_buffer_memory, None);
        }
        if video_session_parameters != vk::VideoSessionParametersKHR::null() {
            (video_queue_device.fp().destroy_video_session_parameters_khr)(
                device.handle(),
                video_session_parameters,
                std::ptr::null(),
            );
        }
        if video_session != vk::VideoSessionKHR::null() {
            (video_queue_device.fp().destroy_video_session_khr)(
                device.handle(),
                video_session,
                std::ptr::null(),
            );
        }
        for memory in session_memories {
            device.free_memory(memory, None);
        }
    }

    if let Some(marker_path) = experimental_dpb_marker_path.take() {
        let _ = std::fs::remove_file(marker_path);
    }

    if let Err(err) = submit_result {
        return HevcDecodeSubmitExecutionProbe::Failed(format!(
            "{err}; experimental_dpb_mode={}, experimental_dpb_status={experimental_dpb_status:?}",
            experimental_dpb_mode.as_str()
        ));
    }

    HevcDecodeSubmitExecutionProbe::Ready {
        queue_family_index: queue_family_index.0,
        output_format,
        coded_width: parameter_sets.coded_width,
        coded_height: parameter_sets.coded_height,
        readback_non_zero,
        readback_bytes,
        readback_planes,
        readback_sample_stride,
        readback_sample_count,
        readback_sample,
        submitted_access_units,
        experimental_dpb_enabled,
        experimental_dpb_mode: experimental_dpb_mode.as_str(),
        experimental_dpb_status,
    }
}

fn build_hevc_submit_probe_bitstream_payload(
    parameter_sets: &HevcParameterSets,
    bitstream: &[u8],
    access_unit_limit: usize,
) -> Result<HevcSubmitProbeBitstreamPayload, String> {
    let parsed_pps = parse_hevc_pps(&parameter_sets.pps)?;
    let mut bytes = Vec::new();
    let mut access_units = Vec::new();
    for nalu in split_annexb_nalus(bitstream) {
        let Some(nal_unit_type) = hevc_nal_type_raw(nalu) else {
            continue;
        };
        if nal_unit_type > 31 {
            continue;
        }
        let slice_header = parse_hevc_slice_header(nalu, &parsed_pps, &parameter_sets.parsed_sps)
            .map_err(|err| {
            format!("failed to parse HEVC slice header while building submit payload: {err}")
        })?;
        if !slice_header.is_first_slice_segment {
            continue;
        }
        append_annexb_nalu(&mut bytes, &parameter_sets.vps);
        append_annexb_nalu(&mut bytes, &parameter_sets.sps);
        append_annexb_nalu(&mut bytes, &parameter_sets.pps);
        let slice_segment_offset = u32::try_from(bytes.len())
            .map_err(|_| "submit execution probe payload offset exceeds u32 range".to_string())?;
        append_annexb_nalu(&mut bytes, nalu);
        access_units.push(HevcSubmitProbeAccessUnit {
            header: HevcAccessUnitHeader {
                nal_unit_type: slice_header.nal_unit_type,
                pps_id: slice_header.pps_id,
                pic_order_cnt_lsb: slice_header.pic_order_cnt_lsb,
            },
            slice_segment_offset,
        });
        if access_units.len() >= access_unit_limit {
            break;
        }
    }
    if access_units.is_empty() {
        return Err(
            "submit execution probe requires at least one access-unit-leading VCL NALU".to_string(),
        );
    }

    Ok(HevcSubmitProbeBitstreamPayload {
        bytes,
        access_units,
        parsed_pps,
    })
}

fn append_annexb_nalu(buffer: &mut Vec<u8>, nalu: &[u8]) {
    buffer.extend_from_slice(&[0, 0, 0, 1]);
    buffer.extend_from_slice(nalu);
}

fn select_memory_type_index(
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

fn align_up(value: u64, alignment: u64) -> u64 {
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

fn hevc_experimental_dpb_mode() -> HevcExperimentalDpbMode {
    match std::env::var("VIDEO_HW_VULKAN_HEVC_EXPERIMENTAL_DPB")
        .ok()
        .as_deref()
    {
        Some("1" | "on" | "true" | "full") => HevcExperimentalDpbMode::On,
        Some("auto") => HevcExperimentalDpbMode::Auto,
        _ => HevcExperimentalDpbMode::Off,
    }
}

fn hevc_experimental_dpb_marker_path() -> PathBuf {
    std::env::temp_dir().join("video-hw-vulkan-hevc-dpb-inflight.flag")
}

impl HevcExperimentalDpbMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::On => "on",
        }
    }
}

impl HevcExperimentalDpbDecision {
    fn enabled(self) -> bool {
        matches!(self, Self::EnabledOn | Self::EnabledAuto)
    }
}

fn decide_hevc_experimental_dpb(
    mode: HevcExperimentalDpbMode,
    marker_present: bool,
    marker_write_failed: bool,
) -> HevcExperimentalDpbDecision {
    match mode {
        HevcExperimentalDpbMode::Off => HevcExperimentalDpbDecision::DisabledOff,
        HevcExperimentalDpbMode::On => HevcExperimentalDpbDecision::EnabledOn,
        HevcExperimentalDpbMode::Auto => {
            if marker_present {
                HevcExperimentalDpbDecision::DisabledAutoMarkerPresent
            } else if marker_write_failed {
                HevcExperimentalDpbDecision::DisabledAutoMarkerWriteFailed
            } else {
                HevcExperimentalDpbDecision::EnabledAuto
            }
        }
    }
}

fn format_hevc_experimental_dpb_status(
    decision: HevcExperimentalDpbDecision,
    marker_path: &std::path::Path,
    marker_write_error: Option<&str>,
) -> String {
    let marker_display = marker_path.display();
    match decision {
        HevcExperimentalDpbDecision::DisabledOff => {
            "mode=off (experimental DPB disabled)".to_string()
        }
        HevcExperimentalDpbDecision::EnabledOn => {
            if let Some(err) = marker_write_error {
                format!("mode=on (experimental DPB enabled; inflight marker write failed: {err})")
            } else {
                format!(
                    "mode=on (experimental DPB enabled; inflight marker armed at {marker_display})"
                )
            }
        }
        HevcExperimentalDpbDecision::EnabledAuto => format!(
            "mode=auto (experimental DPB enabled; no stale marker; inflight marker armed at {marker_display})"
        ),
        HevcExperimentalDpbDecision::DisabledAutoMarkerPresent => format!(
            "mode=auto (experimental DPB disabled; stale inflight marker detected at {marker_display})"
        ),
        HevcExperimentalDpbDecision::DisabledAutoMarkerWriteFailed => {
            let detail = marker_write_error.unwrap_or("unknown error");
            format!("mode=auto (experimental DPB disabled; inflight marker write failed: {detail})")
        }
    }
}

fn configure_hevc_experimental_dpb() -> HevcExperimentalDpbConfiguration {
    let mode = hevc_experimental_dpb_mode();
    let marker_path = hevc_experimental_dpb_marker_path();
    let marker_present = marker_path.exists();
    let should_attempt_marker_write = match mode {
        HevcExperimentalDpbMode::Off => false,
        HevcExperimentalDpbMode::On => true,
        HevcExperimentalDpbMode::Auto => !marker_present,
    };

    let mut marker_write_error = None;
    let mut armed_marker_path = None;
    if should_attempt_marker_write {
        match std::fs::write(&marker_path, b"inflight") {
            Ok(()) => {
                armed_marker_path = Some(marker_path.clone());
            }
            Err(err) => {
                marker_write_error = Some(err.to_string());
            }
        }
    }

    let decision = decide_hevc_experimental_dpb(
        mode,
        marker_present,
        should_attempt_marker_write && armed_marker_path.is_none(),
    );
    let status =
        format_hevc_experimental_dpb_status(decision, &marker_path, marker_write_error.as_deref());
    let enabled = decision.enabled();
    if !enabled {
        armed_marker_path = None;
    }

    HevcExperimentalDpbConfiguration {
        enabled,
        mode,
        status,
        marker_path: armed_marker_path,
    }
}

fn build_decode_readback_regions(
    output_format: vk::Format,
    coded_width: u32,
    coded_height: u32,
) -> Result<(u64, Vec<vk::BufferImageCopy>), String> {
    if coded_width == 0 || coded_height == 0 {
        return Err(format!(
            "invalid coded extent for decode readback: {}x{}",
            coded_width, coded_height
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
            .ok_or_else(|| "decode readback row size overflowed u64".to_string())?;
        let plane_bytes = row_bytes
            .checked_mul(u64::from(plane_height))
            .ok_or_else(|| "decode readback plane size overflowed u64".to_string())?;
        let offset = align_up(next_offset, 4);
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
            .ok_or_else(|| "decode readback buffer size overflowed u64".to_string())?;
        Ok(())
    };

    match output_format {
        vk::Format::G8_B8R8_2PLANE_420_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(
                vk::ImageAspectFlags::PLANE_1,
                coded_width.div_ceil(2),
                coded_height.div_ceil(2),
                2,
            )?;
        }
        vk::Format::G8_B8R8_2PLANE_422_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(
                vk::ImageAspectFlags::PLANE_1,
                coded_width.div_ceil(2),
                coded_height,
                2,
            )?;
        }
        vk::Format::G8_B8R8_2PLANE_444_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(vk::ImageAspectFlags::PLANE_1, coded_width, coded_height, 2)?;
        }
        vk::Format::G8_B8_R8_3PLANE_420_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(
                vk::ImageAspectFlags::PLANE_1,
                coded_width.div_ceil(2),
                coded_height.div_ceil(2),
                1,
            )?;
            push_region(
                vk::ImageAspectFlags::PLANE_2,
                coded_width.div_ceil(2),
                coded_height.div_ceil(2),
                1,
            )?;
        }
        vk::Format::G8_B8_R8_3PLANE_422_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(
                vk::ImageAspectFlags::PLANE_1,
                coded_width.div_ceil(2),
                coded_height,
                1,
            )?;
            push_region(
                vk::ImageAspectFlags::PLANE_2,
                coded_width.div_ceil(2),
                coded_height,
                1,
            )?;
        }
        vk::Format::G8_B8_R8_3PLANE_444_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(vk::ImageAspectFlags::PLANE_1, coded_width, coded_height, 1)?;
            push_region(vk::ImageAspectFlags::PLANE_2, coded_width, coded_height, 1)?;
        }
        vk::Format::B8G8R8A8_UNORM | vk::Format::R8G8B8A8_UNORM => {
            push_region(vk::ImageAspectFlags::COLOR, coded_width, coded_height, 4)?;
        }
        _ => {
            return Err(format!(
                "decode readback is not implemented for output format {output_format:?}"
            ));
        }
    }

    Ok((next_offset, regions))
}

fn build_hevc_std_parameter_set_storage(
    parameter_sets: &HevcParameterSets,
) -> Result<HevcStdParameterSetStorage, String> {
    let parsed_vps = parse_hevc_vps(&parameter_sets.vps)?;
    let parsed_pps = parse_hevc_pps(&parameter_sets.pps)?;
    let sps = &parameter_sets.parsed_sps;

    let profile = &sps.profile_tier_level.general_profile;
    let mut profile_tier_flags = empty_profile_tier_level_flags();
    profile_tier_flags.set_general_tier_flag(bool_to_u32(profile.tier_flag));
    profile_tier_flags
        .set_general_progressive_source_flag(bool_to_u32(profile.progressive_source_flag));
    profile_tier_flags
        .set_general_interlaced_source_flag(bool_to_u32(profile.interlaced_source_flag));
    profile_tier_flags
        .set_general_non_packed_constraint_flag(bool_to_u32(profile.non_packed_constraint_flag));
    profile_tier_flags
        .set_general_frame_only_constraint_flag(bool_to_u32(profile.frame_only_constraint_flag));

    let profile_tier_level = StdVideoH265ProfileTierLevel {
        flags: profile_tier_flags,
        general_profile_idc: StdVideoH265ProfileIdc::from(profile.profile_idc),
        general_level_idc: StdVideoH265LevelIdc::from(profile.level_idc.unwrap_or_default()),
    };

    let mut dec_pic_buf_mgr = StdVideoH265DecPicBufMgr {
        max_latency_increase_plus1: [0; 7],
        max_dec_pic_buffering_minus1: [0; 7],
        max_num_reorder_pics: [0; 7],
    };
    for (index, &value) in sps
        .sub_layer_ordering_info
        .sps_max_latency_increase_plus1
        .iter()
        .enumerate()
        .take(7)
    {
        dec_pic_buf_mgr.max_latency_increase_plus1[index] = value;
    }
    for (index, &value) in sps
        .sub_layer_ordering_info
        .sps_max_dec_pic_buffering_minus1
        .iter()
        .enumerate()
        .take(7)
    {
        dec_pic_buf_mgr.max_dec_pic_buffering_minus1[index] =
            narrow_u64_to_u8(value, "sps_max_dec_pic_buffering_minus1")?;
    }
    for (index, &value) in sps
        .sub_layer_ordering_info
        .sps_max_num_reorder_pics
        .iter()
        .enumerate()
        .take(7)
    {
        dec_pic_buf_mgr.max_num_reorder_pics[index] =
            narrow_u64_to_u8(value, "sps_max_num_reorder_pics")?;
    }

    let short_term_ref_pic_sets = build_std_short_term_ref_pic_sets(sps)?;
    let long_term_ref_pics_sps = build_std_long_term_ref_pics_sps(sps)?;

    let has_conformance_window = sps.conformance_window.conf_win_left_offset != 0
        || sps.conformance_window.conf_win_right_offset != 0
        || sps.conformance_window.conf_win_top_offset != 0
        || sps.conformance_window.conf_win_bottom_offset != 0;
    let inferred_sub_layer_ordering_present = infer_sps_sub_layer_ordering_info_present_flag(sps);

    let mut sps_flags = empty_sps_flags();
    sps_flags.set_sps_temporal_id_nesting_flag(bool_to_u32(sps.sps_temporal_id_nesting_flag));
    sps_flags.set_separate_colour_plane_flag(bool_to_u32(sps.separate_colour_plane_flag));
    sps_flags.set_conformance_window_flag(bool_to_u32(has_conformance_window));
    sps_flags.set_sps_sub_layer_ordering_info_present_flag(bool_to_u32(
        inferred_sub_layer_ordering_present,
    ));
    sps_flags.set_scaling_list_enabled_flag(bool_to_u32(sps.scaling_list_data.is_some()));
    sps_flags.set_sps_scaling_list_data_present_flag(bool_to_u32(sps.scaling_list_data.is_some()));
    sps_flags.set_amp_enabled_flag(bool_to_u32(sps.amp_enabled_flag));
    sps_flags.set_sample_adaptive_offset_enabled_flag(bool_to_u32(
        sps.sample_adaptive_offset_enabled_flag,
    ));
    sps_flags.set_pcm_enabled_flag(bool_to_u32(sps.pcm.is_some()));
    sps_flags.set_pcm_loop_filter_disabled_flag(bool_to_u32(
        sps.pcm
            .as_ref()
            .is_some_and(|pcm| pcm.pcm_loop_filter_disabled_flag),
    ));
    sps_flags.set_long_term_ref_pics_present_flag(bool_to_u32(sps.long_term_ref_pics.is_some()));
    sps_flags.set_sps_temporal_mvp_enabled_flag(bool_to_u32(sps.sps_temporal_mvp_enabled_flag));
    sps_flags.set_strong_intra_smoothing_enabled_flag(bool_to_u32(
        sps.strong_intra_smoothing_enabled_flag,
    ));
    sps_flags.set_vui_parameters_present_flag(bool_to_u32(sps.vui_parameters.is_some()));
    let sps_extension_present = sps.range_extension.is_some()
        || sps.multilayer_extension.is_some()
        || sps.sps_3d_extension.is_some()
        || sps.scc_extension.is_some();
    sps_flags.set_sps_extension_present_flag(bool_to_u32(sps_extension_present));
    sps_flags.set_sps_range_extension_flag(bool_to_u32(sps.range_extension.is_some()));
    if let Some(range_extension) = &sps.range_extension {
        sps_flags.set_transform_skip_rotation_enabled_flag(bool_to_u32(
            range_extension.transform_skip_rotation_enabled_flag,
        ));
        sps_flags.set_transform_skip_context_enabled_flag(bool_to_u32(
            range_extension.transform_skip_context_enabled_flag,
        ));
        sps_flags.set_implicit_rdpcm_enabled_flag(bool_to_u32(
            range_extension.implicit_rdpcm_enabled_flag,
        ));
        sps_flags.set_explicit_rdpcm_enabled_flag(bool_to_u32(
            range_extension.explicit_rdpcm_enabled_flag,
        ));
        sps_flags.set_extended_precision_processing_flag(bool_to_u32(
            range_extension.extended_precision_processing_flag,
        ));
        sps_flags.set_intra_smoothing_disabled_flag(bool_to_u32(
            range_extension.intra_smoothing_disabled_flag,
        ));
        sps_flags.set_high_precision_offsets_enabled_flag(bool_to_u32(
            range_extension.high_precision_offsets_enabled_flag,
        ));
        sps_flags.set_persistent_rice_adaptation_enabled_flag(bool_to_u32(
            range_extension.persistent_rice_adaptation_enabled_flag,
        ));
        sps_flags.set_cabac_bypass_alignment_enabled_flag(bool_to_u32(
            range_extension.cabac_bypass_alignment_enabled_flag,
        ));
    }

    let chroma_format_idc = map_h265_chroma_format_idc(sps.chroma_format_idc)?;
    let short_term_count =
        narrow_usize_to_u8(short_term_ref_pic_sets.len(), "num_short_term_ref_pic_sets")?;
    let long_term_count = if let Some(long_term_ref_pics) = &sps.long_term_ref_pics {
        narrow_usize_to_u8(
            long_term_ref_pics.lt_ref_pic_poc_lsb_sps.len(),
            "num_long_term_ref_pics_sps",
        )?
    } else {
        0
    };

    let std_sps = StdVideoH265SequenceParameterSet {
        flags: sps_flags,
        chroma_format_idc,
        pic_width_in_luma_samples: narrow_u64_to_u32(
            sps.pic_width_in_luma_samples.get(),
            "pic_width_in_luma_samples",
        )?,
        pic_height_in_luma_samples: narrow_u64_to_u32(
            sps.pic_height_in_luma_samples.get(),
            "pic_height_in_luma_samples",
        )?,
        sps_video_parameter_set_id: sps.sps_video_parameter_set_id,
        sps_max_sub_layers_minus1: sps.sps_max_sub_layers_minus1,
        sps_seq_parameter_set_id: narrow_u64_to_u8(
            sps.sps_seq_parameter_set_id,
            "sps_seq_parameter_set_id",
        )?,
        bit_depth_luma_minus8: sps.bit_depth_luma_minus8,
        bit_depth_chroma_minus8: sps.bit_depth_chroma_minus8,
        log2_max_pic_order_cnt_lsb_minus4: sps.log2_max_pic_order_cnt_lsb_minus4,
        log2_min_luma_coding_block_size_minus3: narrow_u64_to_u8(
            sps.log2_min_luma_coding_block_size_minus3,
            "log2_min_luma_coding_block_size_minus3",
        )?,
        log2_diff_max_min_luma_coding_block_size: narrow_u64_to_u8(
            sps.log2_diff_max_min_luma_coding_block_size,
            "log2_diff_max_min_luma_coding_block_size",
        )?,
        log2_min_luma_transform_block_size_minus2: narrow_u64_to_u8(
            sps.log2_min_luma_transform_block_size_minus2,
            "log2_min_luma_transform_block_size_minus2",
        )?,
        log2_diff_max_min_luma_transform_block_size: narrow_u64_to_u8(
            sps.log2_diff_max_min_luma_transform_block_size,
            "log2_diff_max_min_luma_transform_block_size",
        )?,
        max_transform_hierarchy_depth_inter: narrow_u64_to_u8(
            sps.max_transform_hierarchy_depth_inter,
            "max_transform_hierarchy_depth_inter",
        )?,
        max_transform_hierarchy_depth_intra: narrow_u64_to_u8(
            sps.max_transform_hierarchy_depth_intra,
            "max_transform_hierarchy_depth_intra",
        )?,
        num_short_term_ref_pic_sets: short_term_count,
        num_long_term_ref_pics_sps: long_term_count,
        pcm_sample_bit_depth_luma_minus1: sps
            .pcm
            .as_ref()
            .map_or(0, |pcm| pcm.pcm_sample_bit_depth_luma_minus1),
        pcm_sample_bit_depth_chroma_minus1: sps
            .pcm
            .as_ref()
            .map_or(0, |pcm| pcm.pcm_sample_bit_depth_chroma_minus1),
        log2_min_pcm_luma_coding_block_size_minus3: if let Some(pcm) = &sps.pcm {
            narrow_u64_to_u8(
                pcm.log2_min_pcm_luma_coding_block_size_minus3,
                "log2_min_pcm_luma_coding_block_size_minus3",
            )?
        } else {
            0
        },
        log2_diff_max_min_pcm_luma_coding_block_size: if let Some(pcm) = &sps.pcm {
            narrow_u64_to_u8(
                pcm.log2_diff_max_min_pcm_luma_coding_block_size,
                "log2_diff_max_min_pcm_luma_coding_block_size",
            )?
        } else {
            0
        },
        reserved1: 0,
        reserved2: 0,
        palette_max_size: 0,
        delta_palette_max_predictor_size: 0,
        motion_vector_resolution_control_idc: 0,
        sps_num_palette_predictor_initializers_minus1: 0,
        conf_win_left_offset: narrow_u64_to_u32(
            sps.conformance_window.conf_win_left_offset,
            "conf_win_left_offset",
        )?,
        conf_win_right_offset: narrow_u64_to_u32(
            sps.conformance_window.conf_win_right_offset,
            "conf_win_right_offset",
        )?,
        conf_win_top_offset: narrow_u64_to_u32(
            sps.conformance_window.conf_win_top_offset,
            "conf_win_top_offset",
        )?,
        conf_win_bottom_offset: narrow_u64_to_u32(
            sps.conformance_window.conf_win_bottom_offset,
            "conf_win_bottom_offset",
        )?,
        pProfileTierLevel: std::ptr::null(),
        pDecPicBufMgr: std::ptr::null(),
        pScalingLists: std::ptr::null(),
        pShortTermRefPicSet: std::ptr::null(),
        pLongTermRefPicsSps: std::ptr::null(),
        pSequenceParameterSetVui: std::ptr::null(),
        pPredictorPaletteEntries: std::ptr::null(),
    };

    let mut vps_flags = empty_vps_flags();
    vps_flags
        .set_vps_temporal_id_nesting_flag(bool_to_u32(parsed_vps.vps_temporal_id_nesting_flag));

    let std_vps = StdVideoH265VideoParameterSet {
        flags: vps_flags,
        vps_video_parameter_set_id: parsed_vps.vps_video_parameter_set_id,
        vps_max_sub_layers_minus1: parsed_vps.vps_max_sub_layers_minus1,
        reserved1: 0,
        reserved2: 0,
        vps_num_units_in_tick: 0,
        vps_time_scale: 0,
        vps_num_ticks_poc_diff_one_minus1: 0,
        reserved3: 0,
        pDecPicBufMgr: std::ptr::null(),
        pHrdParameters: std::ptr::null(),
        pProfileTierLevel: std::ptr::null(),
    };

    let mut pps_flags = empty_pps_flags();
    pps_flags.set_dependent_slice_segments_enabled_flag(bool_to_u32(
        parsed_pps.dependent_slice_segments_enabled_flag,
    ));
    pps_flags.set_output_flag_present_flag(bool_to_u32(parsed_pps.output_flag_present_flag));
    pps_flags
        .set_sign_data_hiding_enabled_flag(bool_to_u32(parsed_pps.sign_data_hiding_enabled_flag));
    pps_flags.set_cabac_init_present_flag(bool_to_u32(parsed_pps.cabac_init_present_flag));
    pps_flags.set_constrained_intra_pred_flag(bool_to_u32(parsed_pps.constrained_intra_pred_flag));
    pps_flags.set_transform_skip_enabled_flag(bool_to_u32(parsed_pps.transform_skip_enabled_flag));
    pps_flags.set_cu_qp_delta_enabled_flag(bool_to_u32(parsed_pps.cu_qp_delta_enabled_flag));
    pps_flags.set_pps_slice_chroma_qp_offsets_present_flag(bool_to_u32(
        parsed_pps.pps_slice_chroma_qp_offsets_present_flag,
    ));
    pps_flags.set_weighted_pred_flag(bool_to_u32(parsed_pps.weighted_pred_flag));
    pps_flags.set_weighted_bipred_flag(bool_to_u32(parsed_pps.weighted_bipred_flag));
    pps_flags
        .set_transquant_bypass_enabled_flag(bool_to_u32(parsed_pps.transquant_bypass_enabled_flag));
    pps_flags.set_tiles_enabled_flag(bool_to_u32(parsed_pps.tiles_enabled_flag));
    pps_flags.set_entropy_coding_sync_enabled_flag(bool_to_u32(
        parsed_pps.entropy_coding_sync_enabled_flag,
    ));
    pps_flags.set_uniform_spacing_flag(bool_to_u32(parsed_pps.uniform_spacing_flag));
    pps_flags.set_loop_filter_across_tiles_enabled_flag(bool_to_u32(
        parsed_pps.loop_filter_across_tiles_enabled_flag,
    ));
    pps_flags.set_pps_loop_filter_across_slices_enabled_flag(bool_to_u32(
        parsed_pps.pps_loop_filter_across_slices_enabled_flag,
    ));
    pps_flags.set_deblocking_filter_control_present_flag(bool_to_u32(
        parsed_pps.deblocking_filter_control_present_flag,
    ));
    pps_flags.set_deblocking_filter_override_enabled_flag(bool_to_u32(
        parsed_pps.deblocking_filter_override_enabled_flag,
    ));
    pps_flags.set_pps_deblocking_filter_disabled_flag(bool_to_u32(
        parsed_pps.pps_deblocking_filter_disabled_flag,
    ));
    pps_flags.set_lists_modification_present_flag(bool_to_u32(
        parsed_pps.lists_modification_present_flag,
    ));
    pps_flags.set_slice_segment_header_extension_present_flag(bool_to_u32(
        parsed_pps.slice_segment_header_extension_present_flag,
    ));

    let std_pps = StdVideoH265PictureParameterSet {
        flags: pps_flags,
        pps_pic_parameter_set_id: parsed_pps.pps_pic_parameter_set_id,
        pps_seq_parameter_set_id: parsed_pps.pps_seq_parameter_set_id,
        sps_video_parameter_set_id: sps.sps_video_parameter_set_id,
        num_extra_slice_header_bits: parsed_pps.num_extra_slice_header_bits,
        num_ref_idx_l0_default_active_minus1: parsed_pps.num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1: parsed_pps.num_ref_idx_l1_default_active_minus1,
        init_qp_minus26: parsed_pps.init_qp_minus26,
        diff_cu_qp_delta_depth: parsed_pps.diff_cu_qp_delta_depth,
        pps_cb_qp_offset: parsed_pps.pps_cb_qp_offset,
        pps_cr_qp_offset: parsed_pps.pps_cr_qp_offset,
        pps_beta_offset_div2: parsed_pps.pps_beta_offset_div2,
        pps_tc_offset_div2: parsed_pps.pps_tc_offset_div2,
        log2_parallel_merge_level_minus2: parsed_pps.log2_parallel_merge_level_minus2,
        log2_max_transform_skip_block_size_minus2: 0,
        diff_cu_chroma_qp_offset_depth: 0,
        chroma_qp_offset_list_len_minus1: 0,
        cb_qp_offset_list: [0; 6],
        cr_qp_offset_list: [0; 6],
        log2_sao_offset_scale_luma: 0,
        log2_sao_offset_scale_chroma: 0,
        pps_act_y_qp_offset_plus5: 0,
        pps_act_cb_qp_offset_plus5: 0,
        pps_act_cr_qp_offset_plus3: 0,
        pps_num_palette_predictor_initializers: 0,
        luma_bit_depth_entry_minus8: 0,
        chroma_bit_depth_entry_minus8: 0,
        num_tile_columns_minus1: parsed_pps.num_tile_columns_minus1,
        num_tile_rows_minus1: parsed_pps.num_tile_rows_minus1,
        reserved1: 0,
        reserved2: 0,
        column_width_minus1: parsed_pps.column_width_minus1,
        row_height_minus1: parsed_pps.row_height_minus1,
        reserved3: 0,
        pScalingLists: std::ptr::null(),
        pPredictorPaletteEntries: std::ptr::null(),
    };

    let mut storage = HevcStdParameterSetStorage {
        vps: [std_vps],
        sps: [std_sps],
        pps: [std_pps],
        profile_tier_level,
        dec_pic_buf_mgr,
        short_term_ref_pic_sets,
        long_term_ref_pics_sps,
    };

    let profile_ptr = &storage.profile_tier_level as *const StdVideoH265ProfileTierLevel;
    let dec_pic_buf_mgr_ptr = &storage.dec_pic_buf_mgr as *const StdVideoH265DecPicBufMgr;
    storage.vps[0].pProfileTierLevel = profile_ptr;
    storage.vps[0].pDecPicBufMgr = dec_pic_buf_mgr_ptr;
    storage.sps[0].pProfileTierLevel = profile_ptr;
    storage.sps[0].pDecPicBufMgr = dec_pic_buf_mgr_ptr;

    storage.sps[0].pShortTermRefPicSet = if storage.short_term_ref_pic_sets.is_empty() {
        std::ptr::null()
    } else {
        storage.short_term_ref_pic_sets.as_ptr()
    };
    storage.sps[0].pLongTermRefPicsSps = storage
        .long_term_ref_pics_sps
        .as_ref()
        .map_or(std::ptr::null(), |long_term| {
            long_term as *const StdVideoH265LongTermRefPicsSps
        });

    Ok(storage)
}

fn build_std_short_term_ref_pic_sets(
    sps: &SpsRbsp,
) -> Result<Vec<StdVideoH265ShortTermRefPicSet>, String> {
    let short_term = &sps.short_term_ref_pic_sets;
    let set_count = short_term.num_delta_pocs.len();
    let mut sets = Vec::with_capacity(set_count);

    for set_index in 0..set_count {
        let delta_s0 = short_term
            .delta_poc_s0
            .get(set_index)
            .ok_or_else(|| format!("short_term_ref_pic_sets[{set_index}] missing delta_poc_s0"))?;
        let delta_s1 = short_term
            .delta_poc_s1
            .get(set_index)
            .ok_or_else(|| format!("short_term_ref_pic_sets[{set_index}] missing delta_poc_s1"))?;
        if delta_s0.len() > 16 {
            return Err(format!(
                "short_term_ref_pic_sets[{set_index}] has {} negative pics (max 16)",
                delta_s0.len()
            ));
        }
        if delta_s1.len() > 16 {
            return Err(format!(
                "short_term_ref_pic_sets[{set_index}] has {} positive pics (max 16)",
                delta_s1.len()
            ));
        }

        let mut delta_poc_s0_minus1 = [0_u16; 16];
        let mut delta_poc_s1_minus1 = [0_u16; 16];
        for (index, &delta) in delta_s0.iter().enumerate() {
            if delta >= 0 {
                return Err(format!(
                    "short_term_ref_pic_sets[{set_index}].delta_poc_s0[{index}] must be negative (got {delta})"
                ));
            }
            let minus1 = (-delta)
                .checked_sub(1)
                .ok_or_else(|| {
                    format!(
                        "short_term_ref_pic_sets[{set_index}].delta_poc_s0[{index}] conversion underflow"
                    )
                })?;
            delta_poc_s0_minus1[index] = narrow_i64_to_u16(minus1, "delta_poc_s0_minus1")?;
        }
        for (index, &delta) in delta_s1.iter().enumerate() {
            if delta <= 0 {
                return Err(format!(
                    "short_term_ref_pic_sets[{set_index}].delta_poc_s1[{index}] must be positive (got {delta})"
                ));
            }
            let minus1 = delta.checked_sub(1).ok_or_else(|| {
                format!(
                    "short_term_ref_pic_sets[{set_index}].delta_poc_s1[{index}] conversion underflow"
                )
            })?;
            delta_poc_s1_minus1[index] = narrow_i64_to_u16(minus1, "delta_poc_s1_minus1")?;
        }

        let used_s0 = short_term
            .used_by_curr_pic_s0
            .get(set_index)
            .ok_or_else(|| {
                format!("short_term_ref_pic_sets[{set_index}] missing used_by_curr_pic_s0")
            })?;
        let used_s1 = short_term
            .used_by_curr_pic_s1
            .get(set_index)
            .ok_or_else(|| {
                format!("short_term_ref_pic_sets[{set_index}] missing used_by_curr_pic_s1")
            })?;
        let used_by_curr_pic_s0_flag = bools_to_u16_mask(used_s0, "used_by_curr_pic_s0")?;
        let used_by_curr_pic_s1_flag = bools_to_u16_mask(used_s1, "used_by_curr_pic_s1")?;

        let mut flags = empty_short_term_ref_pic_set_flags();
        flags.set_inter_ref_pic_set_prediction_flag(0);
        flags.set_delta_rps_sign(0);

        sets.push(StdVideoH265ShortTermRefPicSet {
            flags,
            delta_idx_minus1: 0,
            use_delta_flag: 0,
            abs_delta_rps_minus1: 0,
            used_by_curr_pic_flag: 0,
            used_by_curr_pic_s0_flag,
            used_by_curr_pic_s1_flag,
            reserved1: 0,
            reserved2: 0,
            reserved3: 0,
            num_negative_pics: narrow_usize_to_u8(delta_s0.len(), "num_negative_pics")?,
            num_positive_pics: narrow_usize_to_u8(delta_s1.len(), "num_positive_pics")?,
            delta_poc_s0_minus1,
            delta_poc_s1_minus1,
        });
    }

    Ok(sets)
}

fn build_std_long_term_ref_pics_sps(
    sps: &SpsRbsp,
) -> Result<Option<StdVideoH265LongTermRefPicsSps>, String> {
    let Some(long_term_ref_pics) = &sps.long_term_ref_pics else {
        return Ok(None);
    };
    if long_term_ref_pics.lt_ref_pic_poc_lsb_sps.len() > 32 {
        return Err(format!(
            "long_term_ref_pics has {} entries (max 32)",
            long_term_ref_pics.lt_ref_pic_poc_lsb_sps.len()
        ));
    }
    let mut used_by_curr_pic_lt_sps_flag = 0_u32;
    let mut lt_ref_pic_poc_lsb_sps = [0_u32; 32];

    for (index, &poc_lsb) in long_term_ref_pics.lt_ref_pic_poc_lsb_sps.iter().enumerate() {
        lt_ref_pic_poc_lsb_sps[index] = narrow_u64_to_u32(poc_lsb, "lt_ref_pic_poc_lsb_sps")?;
    }
    for (index, &used) in long_term_ref_pics
        .used_by_curr_pic_lt_sps_flag
        .iter()
        .enumerate()
    {
        if used {
            used_by_curr_pic_lt_sps_flag |= 1_u32 << index;
        }
    }

    Ok(Some(StdVideoH265LongTermRefPicsSps {
        used_by_curr_pic_lt_sps_flag,
        lt_ref_pic_poc_lsb_sps,
    }))
}

fn infer_sps_sub_layer_ordering_info_present_flag(sps: &SpsRbsp) -> bool {
    if sps.sps_max_sub_layers_minus1 == 0 {
        return true;
    }
    let dec_pic_buf = &sps.sub_layer_ordering_info.sps_max_dec_pic_buffering_minus1;
    let reorder_pics = &sps.sub_layer_ordering_info.sps_max_num_reorder_pics;
    let latency = &sps.sub_layer_ordering_info.sps_max_latency_increase_plus1;
    !all_values_equal(dec_pic_buf) || !all_values_equal(reorder_pics) || !all_values_equal(latency)
}

fn all_values_equal<T: PartialEq>(values: &[T]) -> bool {
    match values.split_first() {
        None => true,
        Some((head, tail)) => tail.iter().all(|value| value == head),
    }
}

fn parse_hevc_vps(vps_nalu: &[u8]) -> Result<ParsedHevcVps, String> {
    match hevc_nal_type(vps_nalu) {
        Some(NALUnitType::VpsNut) => {}
        Some(other) => return Err(format!("expected VPS NALU but found {other:?}")),
        None => return Err("truncated VPS NALU header".to_string()),
    }

    let rbsp = nalu_payload_to_rbsp(vps_nalu)?;
    let mut reader = RbspBitReader::new(&rbsp);
    let vps_video_parameter_set_id =
        narrow_u32_to_u8(reader.read_bits(4)?, "vps_video_parameter_set_id")?;
    let _vps_base_layer_internal_flag = reader.read_flag()?;
    let _vps_base_layer_available_flag = reader.read_flag()?;
    let _vps_max_layers_minus1 = reader.read_bits(6)?;
    let vps_max_sub_layers_minus1 =
        narrow_u32_to_u8(reader.read_bits(3)?, "vps_max_sub_layers_minus1")?;
    let vps_temporal_id_nesting_flag = reader.read_flag()?;
    let _vps_reserved_0xffff_16bits = reader.read_bits(16)?;

    Ok(ParsedHevcVps {
        vps_video_parameter_set_id,
        vps_max_sub_layers_minus1,
        vps_temporal_id_nesting_flag,
    })
}

fn parse_hevc_pps(pps_nalu: &[u8]) -> Result<ParsedHevcPps, String> {
    match hevc_nal_type(pps_nalu) {
        Some(NALUnitType::PpsNut) => {}
        Some(other) => return Err(format!("expected PPS NALU but found {other:?}")),
        None => return Err("truncated PPS NALU header".to_string()),
    }

    let rbsp = nalu_payload_to_rbsp(pps_nalu)?;
    let mut reader = RbspBitReader::new(&rbsp);

    let pps_pic_parameter_set_id = narrow_u32_to_u8(reader.read_ue()?, "pps_pic_parameter_set_id")?;
    let pps_seq_parameter_set_id = narrow_u32_to_u8(reader.read_ue()?, "pps_seq_parameter_set_id")?;
    let dependent_slice_segments_enabled_flag = reader.read_flag()?;
    let output_flag_present_flag = reader.read_flag()?;
    let num_extra_slice_header_bits =
        narrow_u32_to_u8(reader.read_bits(3)?, "num_extra_slice_header_bits")?;
    let sign_data_hiding_enabled_flag = reader.read_flag()?;
    let cabac_init_present_flag = reader.read_flag()?;
    let num_ref_idx_l0_default_active_minus1 =
        narrow_u32_to_u8(reader.read_ue()?, "num_ref_idx_l0_default_active_minus1")?;
    let num_ref_idx_l1_default_active_minus1 =
        narrow_u32_to_u8(reader.read_ue()?, "num_ref_idx_l1_default_active_minus1")?;
    let init_qp_minus26 = narrow_i32_to_i8(reader.read_se()?, "init_qp_minus26")?;
    let constrained_intra_pred_flag = reader.read_flag()?;
    let transform_skip_enabled_flag = reader.read_flag()?;
    let cu_qp_delta_enabled_flag = reader.read_flag()?;
    let diff_cu_qp_delta_depth = if cu_qp_delta_enabled_flag {
        narrow_u32_to_u8(reader.read_ue()?, "diff_cu_qp_delta_depth")?
    } else {
        0
    };
    let pps_cb_qp_offset = narrow_i32_to_i8(reader.read_se()?, "pps_cb_qp_offset")?;
    let pps_cr_qp_offset = narrow_i32_to_i8(reader.read_se()?, "pps_cr_qp_offset")?;
    let pps_slice_chroma_qp_offsets_present_flag = reader.read_flag()?;
    let weighted_pred_flag = reader.read_flag()?;
    let weighted_bipred_flag = reader.read_flag()?;
    let transquant_bypass_enabled_flag = reader.read_flag()?;
    let tiles_enabled_flag = reader.read_flag()?;
    let entropy_coding_sync_enabled_flag = reader.read_flag()?;

    let mut num_tile_columns_minus1 = 0_u8;
    let mut num_tile_rows_minus1 = 0_u8;
    let mut uniform_spacing_flag = false;
    let mut loop_filter_across_tiles_enabled_flag = false;
    let mut column_width_minus1 = [0_u16; 19];
    let mut row_height_minus1 = [0_u16; 21];

    if tiles_enabled_flag {
        let num_tile_columns = reader
            .read_ue()?
            .checked_add(1)
            .ok_or_else(|| "num_tile_columns overflows u32".to_string())?;
        let num_tile_rows = reader
            .read_ue()?
            .checked_add(1)
            .ok_or_else(|| "num_tile_rows overflows u32".to_string())?;
        if num_tile_columns == 0 || num_tile_columns > 19 {
            return Err(format!(
                "num_tile_columns={num_tile_columns} is outside supported range 1..=19"
            ));
        }
        if num_tile_rows == 0 || num_tile_rows > 21 {
            return Err(format!(
                "num_tile_rows={num_tile_rows} is outside supported range 1..=21"
            ));
        }

        num_tile_columns_minus1 =
            narrow_u32_to_u8(num_tile_columns - 1, "num_tile_columns_minus1")?;
        num_tile_rows_minus1 = narrow_u32_to_u8(num_tile_rows - 1, "num_tile_rows_minus1")?;
        uniform_spacing_flag = reader.read_flag()?;
        if !uniform_spacing_flag {
            for width in column_width_minus1
                .iter_mut()
                .take(usize::from(num_tile_columns_minus1))
            {
                *width = narrow_u32_to_u16(reader.read_ue()?, "column_width_minus1")?;
            }
            for height in row_height_minus1
                .iter_mut()
                .take(usize::from(num_tile_rows_minus1))
            {
                *height = narrow_u32_to_u16(reader.read_ue()?, "row_height_minus1")?;
            }
        }
        loop_filter_across_tiles_enabled_flag = reader.read_flag()?;
    }

    let pps_loop_filter_across_slices_enabled_flag = reader.read_flag()?;
    let deblocking_filter_control_present_flag = reader.read_flag()?;
    let mut deblocking_filter_override_enabled_flag = false;
    let mut pps_deblocking_filter_disabled_flag = false;
    let mut pps_beta_offset_div2 = 0_i8;
    let mut pps_tc_offset_div2 = 0_i8;
    if deblocking_filter_control_present_flag {
        deblocking_filter_override_enabled_flag = reader.read_flag()?;
        pps_deblocking_filter_disabled_flag = reader.read_flag()?;
        if !pps_deblocking_filter_disabled_flag {
            pps_beta_offset_div2 = narrow_i32_to_i8(reader.read_se()?, "pps_beta_offset_div2")?;
            pps_tc_offset_div2 = narrow_i32_to_i8(reader.read_se()?, "pps_tc_offset_div2")?;
        }
    }

    let pps_scaling_list_data_present_flag = reader.read_flag()?;
    if pps_scaling_list_data_present_flag {
        return Err("pps_scaling_list_data_present_flag=1 is not yet supported".to_string());
    }
    let lists_modification_present_flag = reader.read_flag()?;
    let log2_parallel_merge_level_minus2 =
        narrow_u32_to_u8(reader.read_ue()?, "log2_parallel_merge_level_minus2")?;
    let slice_segment_header_extension_present_flag = reader.read_flag()?;
    let pps_extension_present_flag = reader.read_flag()?;
    if pps_extension_present_flag {
        return Err("pps_extension_present_flag=1 is not yet supported".to_string());
    }

    Ok(ParsedHevcPps {
        pps_pic_parameter_set_id,
        pps_seq_parameter_set_id,
        num_extra_slice_header_bits,
        num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1,
        init_qp_minus26,
        diff_cu_qp_delta_depth,
        pps_cb_qp_offset,
        pps_cr_qp_offset,
        pps_beta_offset_div2,
        pps_tc_offset_div2,
        log2_parallel_merge_level_minus2,
        num_tile_columns_minus1,
        num_tile_rows_minus1,
        column_width_minus1,
        row_height_minus1,
        dependent_slice_segments_enabled_flag,
        output_flag_present_flag,
        sign_data_hiding_enabled_flag,
        cabac_init_present_flag,
        constrained_intra_pred_flag,
        transform_skip_enabled_flag,
        cu_qp_delta_enabled_flag,
        pps_slice_chroma_qp_offsets_present_flag,
        weighted_pred_flag,
        weighted_bipred_flag,
        transquant_bypass_enabled_flag,
        tiles_enabled_flag,
        entropy_coding_sync_enabled_flag,
        uniform_spacing_flag,
        loop_filter_across_tiles_enabled_flag,
        pps_loop_filter_across_slices_enabled_flag,
        deblocking_filter_control_present_flag,
        deblocking_filter_override_enabled_flag,
        pps_deblocking_filter_disabled_flag,
        lists_modification_present_flag,
        slice_segment_header_extension_present_flag,
    })
}

fn parse_hevc_slice_header(
    slice_nalu: &[u8],
    parsed_pps: &ParsedHevcPps,
    sps: &SpsRbsp,
) -> Result<ParsedHevcSliceHeader, String> {
    let nal_unit_type =
        hevc_nal_type_raw(slice_nalu).ok_or_else(|| "truncated slice NALU header".to_string())?;
    if nal_unit_type > 31 {
        return Err(format!(
            "NALU type {nal_unit_type} is not a VCL slice NAL unit"
        ));
    }

    let rbsp = nalu_payload_to_rbsp(slice_nalu)?;
    let mut reader = RbspBitReader::new(&rbsp);
    let first_slice_segment_in_pic_flag = reader.read_flag()?;
    if (16..=23).contains(&nal_unit_type) {
        let _no_output_of_prior_pics_flag = reader.read_flag()?;
    }
    let pps_id = narrow_u32_to_u8(reader.read_ue()?, "slice_pic_parameter_set_id")?;

    if !first_slice_segment_in_pic_flag {
        return Ok(ParsedHevcSliceHeader {
            nal_unit_type,
            pps_id,
            is_first_slice_segment: false,
            pic_order_cnt_lsb: None,
        });
    }

    for _ in 0..parsed_pps.num_extra_slice_header_bits {
        let _ = reader.read_flag()?;
    }
    let _slice_type = reader.read_ue()?;
    if parsed_pps.output_flag_present_flag {
        let _pic_output_flag = reader.read_flag()?;
    }
    if sps.separate_colour_plane_flag {
        let _colour_plane_id = reader.read_bits(2)?;
    }

    let pic_order_cnt_lsb = if is_hevc_idr_nal_type(nal_unit_type) {
        None
    } else {
        let poc_bits = usize::from(sps.log2_max_pic_order_cnt_lsb_minus4) + 4;
        Some(narrow_u32_to_u16(
            reader.read_bits(poc_bits)?,
            "slice_pic_order_cnt_lsb",
        )?)
    };

    Ok(ParsedHevcSliceHeader {
        nal_unit_type,
        pps_id,
        is_first_slice_segment: true,
        pic_order_cnt_lsb,
    })
}

fn is_hevc_idr_nal_type(nal_unit_type: u8) -> bool {
    nal_unit_type == 19 || nal_unit_type == 20
}

fn is_hevc_reference_nal_type(nal_unit_type: u8) -> bool {
    matches!(
        nal_unit_type,
        1 | 3 | 5 | 7 | 9 | 11 | 13 | 15 | 16 | 17 | 18 | 19 | 20 | 21
    )
}

fn select_hevc_decode_dpb_slot(
    experimental_dpb_enabled: bool,
    is_reference: bool,
    dpb_slot_count: usize,
    next_reference_slot: &mut usize,
    active_reference_slots: &[HevcActiveReferenceSlot],
) -> usize {
    if !experimental_dpb_enabled || dpb_slot_count == 0 {
        return 0;
    }
    if is_reference {
        if dpb_slot_count <= 1 {
            return 0;
        }
        let reference_slot_count = dpb_slot_count - 1;
        let slot = 1 + (*next_reference_slot % reference_slot_count);
        *next_reference_slot = next_reference_slot.saturating_add(1);
        return slot;
    }

    let non_reference_slot = 0_usize;
    if !active_reference_slots
        .iter()
        .any(|entry| entry.slot == non_reference_slot)
    {
        return non_reference_slot;
    }
    (0..dpb_slot_count)
        .find(|candidate| {
            !active_reference_slots
                .iter()
                .any(|entry| entry.slot == *candidate)
        })
        .unwrap_or(non_reference_slot)
}

fn build_hevc_ref_pic_set_lists(
    reference_picture_order_cnt_values: &[i32],
    current_pic_order_cnt_val: i32,
) -> (
    [u8; HEVC_REF_PIC_SET_LIST_SIZE],
    [u8; HEVC_REF_PIC_SET_LIST_SIZE],
    u8,
) {
    let mut short_term_before = [HEVC_NO_REFERENCE_PICTURE; HEVC_REF_PIC_SET_LIST_SIZE];
    let mut short_term_after = [HEVC_NO_REFERENCE_PICTURE; HEVC_REF_PIC_SET_LIST_SIZE];
    let mut before_count = 0_usize;
    let mut after_count = 0_usize;
    for (reference_list_index, reference_pic_order_cnt_val) in
        reference_picture_order_cnt_values.iter().enumerate()
    {
        let Ok(reference_list_index_u8) = u8::try_from(reference_list_index) else {
            break;
        };
        if *reference_pic_order_cnt_val < current_pic_order_cnt_val {
            if before_count < HEVC_REF_PIC_SET_LIST_SIZE {
                short_term_before[before_count] = reference_list_index_u8;
                before_count = before_count.saturating_add(1);
            }
        } else if after_count < HEVC_REF_PIC_SET_LIST_SIZE {
            short_term_after[after_count] = reference_list_index_u8;
            after_count = after_count.saturating_add(1);
        }
    }
    let total_references = before_count
        .saturating_add(after_count)
        .min(usize::from(u8::MAX));
    (
        short_term_before,
        short_term_after,
        u8::try_from(total_references).unwrap_or(u8::MAX),
    )
}

fn nalu_payload_to_rbsp(nalu: &[u8]) -> Result<Vec<u8>, String> {
    if nalu.len() < 2 {
        return Err("HEVC NALU is truncated before RBSP payload".to_string());
    }
    let payload = &nalu[2..];
    let mut rbsp = Vec::with_capacity(payload.len());
    let mut index = 0_usize;
    while index < payload.len() {
        if index + 2 < payload.len()
            && payload[index] == 0
            && payload[index + 1] == 0
            && payload[index + 2] == 0x03
        {
            rbsp.push(0);
            rbsp.push(0);
            index += 3;
            continue;
        }
        rbsp.push(payload[index]);
        index += 1;
    }
    Ok(rbsp)
}

const fn bool_to_u32(value: bool) -> u32 {
    if value { 1 } else { 0 }
}

fn map_h265_chroma_format_idc(
    chroma_format_idc: u8,
) -> Result<StdVideoH265ChromaFormatIdc, String> {
    match chroma_format_idc {
        0 => Ok(StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_MONOCHROME),
        1 => Ok(StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_420),
        2 => Ok(StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_422),
        3 => Ok(StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_444),
        _ => Err(format!(
            "unsupported SPS chroma_format_idc={chroma_format_idc} (expected 0..=3)"
        )),
    }
}

fn bools_to_u16_mask(values: &[bool], field_name: &str) -> Result<u16, String> {
    if values.len() > 16 {
        return Err(format!(
            "{field_name} length {} exceeds 16-bit mask capacity",
            values.len()
        ));
    }
    let mut mask = 0_u16;
    for (index, &value) in values.iter().enumerate() {
        if value {
            mask |= 1_u16 << index;
        }
    }
    Ok(mask)
}

fn empty_vps_flags() -> StdVideoH265VpsFlags {
    StdVideoH265VpsFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
        __bindgen_padding_0: [0; 3],
    }
}

fn empty_profile_tier_level_flags() -> StdVideoH265ProfileTierLevelFlags {
    StdVideoH265ProfileTierLevelFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
        __bindgen_padding_0: [0; 3],
    }
}

fn empty_sps_flags() -> StdVideoH265SpsFlags {
    StdVideoH265SpsFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
    }
}

fn empty_pps_flags() -> StdVideoH265PpsFlags {
    StdVideoH265PpsFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
    }
}

fn empty_short_term_ref_pic_set_flags() -> StdVideoH265ShortTermRefPicSetFlags {
    StdVideoH265ShortTermRefPicSetFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
        __bindgen_padding_0: [0; 3],
    }
}

fn empty_decode_h265_picture_info_flags() -> StdVideoDecodeH265PictureInfoFlags {
    StdVideoDecodeH265PictureInfoFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
        __bindgen_padding_0: [0; 3],
    }
}

fn empty_decode_h265_reference_info_flags() -> StdVideoDecodeH265ReferenceInfoFlags {
    StdVideoDecodeH265ReferenceInfoFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
        __bindgen_padding_0: [0; 3],
    }
}

fn narrow_u64_to_u8(value: u64, field_name: &str) -> Result<u8, String> {
    u8::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u8 range"))
}

fn narrow_u64_to_u32(value: u64, field_name: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u32 range"))
}

fn narrow_u32_to_u8(value: u32, field_name: &str) -> Result<u8, String> {
    u8::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u8 range"))
}

fn narrow_u32_to_u16(value: u32, field_name: &str) -> Result<u16, String> {
    u16::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u16 range"))
}

fn narrow_i32_to_i8(value: i32, field_name: &str) -> Result<i8, String> {
    i8::try_from(value).map_err(|_| format!("{field_name}={value} exceeds i8 range"))
}

fn narrow_i64_to_u16(value: i64, field_name: &str) -> Result<u16, String> {
    u16::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u16 range"))
}

fn narrow_usize_to_u8(value: usize, field_name: &str) -> Result<u8, String> {
    u8::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u8 range"))
}

fn run_hevc_decode_probe() -> HevcDecodePrerequisiteProbe {
    // SAFETY: We only load function pointers from the Vulkan loader and keep the returned
    // handle within this function. No borrowed pointers escape this module.
    let entry = match unsafe { ash::Entry::load() } {
        Ok(entry) => entry,
        Err(err) => {
            return HevcDecodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to load Vulkan entry: {err}"
            ));
        }
    };

    // SAFETY: `vk::InstanceCreateInfo::default()` produces a valid zero-initialized struct
    // for instance creation in ash. We destroy the created instance before returning.
    let instance = match unsafe { entry.create_instance(&vk::InstanceCreateInfo::default(), None) }
    {
        Ok(instance) => instance,
        Err(err) => {
            return HevcDecodePrerequisiteProbe::ProbeUnavailable(format!(
                "failed to create Vulkan instance: {err}"
            ));
        }
    };

    let probe_result = (|| -> Result<HevcDecodePrerequisiteProbe, String> {
        // SAFETY: `instance` is valid for the duration of this closure; we only read
        // physical-device handles returned by Vulkan and do not dereference raw pointers.
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;

        if physical_devices.is_empty() {
            return Ok(HevcDecodePrerequisiteProbe::NoCompatibleAdapter);
        }

        let mut observed_extensions = ExtensionFlags::default();
        let mut observed_decode_queue = false;
        let mut device_init_errors = Vec::new();
        for physical_device in physical_devices {
            let support = query_adapter_decode_support(&instance, physical_device)
                .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
            observed_extensions.union_assign(support.extensions);
            observed_decode_queue |= support.decode_queue_family_index.is_some();

            if support.extensions.supports_hevc_decode()
                && let Some(queue_family_index) = support.decode_queue_family_index
            {
                match try_initialize_hevc_decode_device(
                    &instance,
                    physical_device,
                    queue_family_index,
                ) {
                    Ok(()) => return Ok(HevcDecodePrerequisiteProbe::Ready),
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
        if !observed_extensions.has_video_decode_h265 {
            missing.push("VK_KHR_video_decode_h265");
        }

        if !missing.is_empty() {
            return Ok(HevcDecodePrerequisiteProbe::MissingExtensions { missing });
        }

        if !observed_decode_queue {
            return Ok(HevcDecodePrerequisiteProbe::MissingDecodeQueueFamily);
        }

        if !device_init_errors.is_empty() {
            let joined = device_init_errors.join("; ");
            return Ok(HevcDecodePrerequisiteProbe::DeviceInitializationFailed(
                joined,
            ));
        }

        Ok(HevcDecodePrerequisiteProbe::NoCompatibleAdapter)
    })();

    // SAFETY: `instance` was created in this function and is no longer used after this call.
    unsafe {
        instance.destroy_instance(None);
    }

    probe_result.unwrap_or_else(HevcDecodePrerequisiteProbe::ProbeUnavailable)
}

fn split_annexb_nalus(bitstream: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let mut cursor = 0usize;
    while let Some((start, prefix_len)) = find_annexb_start_code(bitstream, cursor) {
        let nalu_start = start.saturating_add(prefix_len);
        if nalu_start >= bitstream.len() {
            break;
        }
        let next_start = find_annexb_start_code(bitstream, nalu_start)
            .map(|(offset, _)| offset)
            .unwrap_or(bitstream.len());
        if next_start > nalu_start {
            nalus.push(&bitstream[nalu_start..next_start]);
        }
        cursor = next_start;
    }
    nalus
}

fn find_annexb_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    if from >= data.len() {
        return None;
    }
    let mut index = from;
    while index + 3 <= data.len() {
        if data[index] == 0 && data[index + 1] == 0 {
            if data.get(index + 2) == Some(&1) {
                return Some((index, 3));
            }
            if index + 3 < data.len() && data[index + 2] == 0 && data[index + 3] == 1 {
                return Some((index, 4));
            }
        }
        index += 1;
    }
    None
}

fn hevc_nal_type_raw(nalu: &[u8]) -> Option<u8> {
    let header = *nalu.first()?;
    Some((header >> 1) & 0b0011_1111)
}

fn hevc_nal_type(nalu: &[u8]) -> Option<NALUnitType> {
    Some(NALUnitType::from(hevc_nal_type_raw(nalu)?))
}

fn query_hevc_decode_output_formats(
    video_queue: &ash::khr::video_queue::Instance,
    physical_device: vk::PhysicalDevice,
    profile: vk::VideoProfileInfoKHR<'_>,
) -> Result<Vec<vk::Format>, String> {
    let profiles = [profile];
    let mut profile_list = vk::VideoProfileListInfoKHR::default().profiles(&profiles);
    let format_info = vk::PhysicalDeviceVideoFormatInfoKHR::default()
        .image_usage(vk::ImageUsageFlags::VIDEO_DECODE_DST_KHR)
        .push_next(&mut profile_list);

    let mut property_count = 0_u32;
    // SAFETY: We pass valid pointers and request only the count in this first query.
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
        return Err(format!("video format count query failed: {count_result:?}"));
    }

    if property_count == 0 {
        return Ok(Vec::new());
    }

    let mut properties = vec![vk::VideoFormatPropertiesKHR::default(); property_count as usize];
    // SAFETY: `properties` points to writable storage sized from the prior count query.
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
            "video format properties query failed: {properties_result:?}"
        ));
    }

    Ok(properties
        .into_iter()
        .map(|property| property.format)
        .collect())
}

fn query_adapter_decode_support(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Result<AdapterDecodeSupport, vk::Result> {
    // SAFETY: We call Vulkan with a valid `physical_device` handle returned from
    // `enumerate_physical_devices` for this same `instance`.
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical_device) }?;

    let mut flags = ExtensionFlags::default();
    for extension in extensions {
        // SAFETY: Vulkan guarantees `extension_name` is a null-terminated C string.
        let name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        flags.has_video_queue |= name == vk::KHR_VIDEO_QUEUE_NAME;
        flags.has_video_decode_queue |= name == vk::KHR_VIDEO_DECODE_QUEUE_NAME;
        flags.has_video_decode_h265 |= name == vk::KHR_VIDEO_DECODE_H265_NAME;
    }

    // SAFETY: We only query immutable queue-family properties for a valid physical device.
    let queue_family_properties =
        unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    let decode_queue_family_index =
        queue_family_properties
            .iter()
            .enumerate()
            .find_map(|(index, queue)| {
                (queue.queue_count > 0
                    && queue.queue_flags.contains(vk::QueueFlags::VIDEO_DECODE_KHR))
                .then(|| u32::try_from(index).ok().map(DecodeQueueFamilyIndex))
                .flatten()
            });

    Ok(AdapterDecodeSupport {
        extensions: flags,
        decode_queue_family_index,
    })
}

fn try_initialize_hevc_decode_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: DecodeQueueFamilyIndex,
) -> Result<(), String> {
    let device = create_hevc_decode_device(instance, physical_device, queue_family_index)?;

    // SAFETY: `device` was created above and is no longer used after this point.
    unsafe {
        device.destroy_device(None);
    }
    Ok(())
}

fn create_hevc_decode_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: DecodeQueueFamilyIndex,
) -> Result<ash::Device, String> {
    let priorities = [1.0_f32];
    let queue_create_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family_index.0)
        .queue_priorities(&priorities);
    let extension_names = [
        vk::KHR_VIDEO_QUEUE_NAME.as_ptr(),
        vk::KHR_VIDEO_DECODE_QUEUE_NAME.as_ptr(),
        vk::KHR_VIDEO_DECODE_H265_NAME.as_ptr(),
        vk::KHR_VIDEO_MAINTENANCE1_NAME.as_ptr(),
    ];
    let mut synchronization2_features =
        vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true);
    let mut video_maintenance1_features =
        vk::PhysicalDeviceVideoMaintenance1FeaturesKHR::default().video_maintenance1(true);
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(std::slice::from_ref(&queue_create_info))
        .enabled_extension_names(&extension_names)
        .push_next(&mut synchronization2_features)
        .push_next(&mut video_maintenance1_features);

    // SAFETY: All pointers referenced by `create_info` live until the call returns.
    unsafe { instance.create_device(physical_device, &create_info, None) }
        .map_err(|err| format!("logical device initialization failed: {err}"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn extension_flags_require_all_three_hevc_decode_extensions() {
        let mut flags = ExtensionFlags::default();
        assert!(!flags.supports_hevc_decode());
        flags.has_video_queue = true;
        assert!(!flags.supports_hevc_decode());
        flags.has_video_decode_queue = true;
        assert!(!flags.supports_hevc_decode());
        flags.has_video_decode_h265 = true;
        assert!(flags.supports_hevc_decode());
    }

    #[test]
    fn hevc_decode_probe_returns_stable_enum() {
        let status = probe_hevc_decode_prerequisites();
        match status {
            HevcDecodePrerequisiteProbe::Ready
            | HevcDecodePrerequisiteProbe::MissingExtensions { .. }
            | HevcDecodePrerequisiteProbe::MissingDecodeQueueFamily
            | HevcDecodePrerequisiteProbe::NoCompatibleAdapter
            | HevcDecodePrerequisiteProbe::DeviceInitializationFailed(_)
            | HevcDecodePrerequisiteProbe::ProbeUnavailable(_) => {}
        }
    }

    #[test]
    fn extract_hevc_parameter_sets_reports_missing_sets() {
        let err = extract_hevc_parameter_sets_annexb(&[0, 0, 1, 0x26, 0x01])
            .expect_err("non-parameter-set stream should fail");
        assert!(
            err.contains("missing VPS")
                || err.contains("missing SPS")
                || err.contains("missing PPS")
        );
    }

    #[test]
    fn split_annexb_nalus_handles_three_and_four_byte_start_codes() {
        let stream = [
            0_u8, 0, 0, 1, 0x40, 0x01, 0x02, 0, 0, 1, 0x42, 0x01, 0x03, 0, 0, 0, 1, 0x44, 0x01,
            0x04,
        ];
        let nalus = split_annexb_nalus(&stream);
        assert_eq!(nalus.len(), 3);
        assert_eq!(nalus[0], &[0x40, 0x01, 0x02]);
        assert_eq!(nalus[1], &[0x42, 0x01, 0x03]);
        assert_eq!(nalus[2], &[0x44, 0x01, 0x04]);
    }

    #[test]
    fn extract_hevc_parameter_sets_parses_repository_sample() {
        let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.h265");
        let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
        let parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
            .expect("repository HEVC sample should contain VPS/SPS/PPS");

        assert!(!parameter_sets.vps.is_empty());
        assert!(!parameter_sets.sps.is_empty());
        assert!(!parameter_sets.pps.is_empty());
        assert!(parameter_sets.coded_width > 0);
        assert!(parameter_sets.coded_height > 0);

        let std_parameter_sets = build_hevc_std_parameter_set_storage(&parameter_sets)
            .expect("sample parameter sets should map to StdVideo structs");
        assert_eq!(std_parameter_sets.vps.len(), 1);
        assert_eq!(std_parameter_sets.sps.len(), 1);
        assert_eq!(std_parameter_sets.pps.len(), 1);
        assert!(!std_parameter_sets.sps[0].pProfileTierLevel.is_null());
        assert_eq!(
            std_parameter_sets.sps[0].sps_video_parameter_set_id,
            parameter_sets.parsed_sps.sps_video_parameter_set_id
        );
    }

    #[test]
    fn estimate_hevc_access_unit_count_matches_repository_sample() {
        let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.h265");
        let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
        let count = estimate_hevc_access_unit_count(&bitstream)
            .expect("HEVC access-unit count should be derivable from repository sample");
        assert_eq!(count, 303);
    }

    #[test]
    fn extract_hevc_access_unit_headers_matches_repository_sample() {
        let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.h265");
        let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
        let headers = extract_hevc_access_unit_headers(&bitstream)
            .expect("HEVC access-unit headers should be derivable from repository sample");
        assert_eq!(headers.len(), 303);
        assert!(headers.iter().all(|header| header.nal_unit_type <= 31));
    }

    #[test]
    fn decode_submit_skeleton_probe_maps_repository_sample() {
        let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.h265");
        let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
        let parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
            .expect("repository HEVC sample should contain VPS/SPS/PPS");
        let capability_snapshot = HevcCapabilitySnapshot {
            min_bitstream_buffer_offset_alignment: 1,
            min_bitstream_buffer_size_alignment: 1,
            picture_access_granularity: vk::Extent2D {
                width: 1,
                height: 1,
            },
            min_coded_extent: vk::Extent2D {
                width: 64,
                height: 64,
            },
            max_coded_extent: vk::Extent2D {
                width: 8192,
                height: 8192,
            },
            max_dpb_slots: 8,
            max_active_reference_pictures: 4,
            max_level_idc: 0,
            std_header_version: vk::ExtensionProperties::default(),
        };

        match build_hevc_decode_submit_skeleton_probe(&parameter_sets, &capability_snapshot) {
            HevcDecodeSubmitSkeletonProbe::Ready(skeleton) => {
                assert_eq!(
                    skeleton.sps_id,
                    u8::try_from(parameter_sets.parsed_sps.sps_seq_parameter_set_id)
                        .expect("sample sps id must fit u8")
                );
                assert!(skeleton.vcl_nalu_count > 0);
                assert_eq!(skeleton.first_slice_pps_id, Some(skeleton.pps_id));
                assert!(!skeleton.planned_dpb_slots.is_empty());
            }
            other => panic!("expected decode submit skeleton to be ready, got {other:?}"),
        }
    }

    #[test]
    fn build_submit_probe_bitstream_payload_maps_first_slice() {
        let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.h265");
        let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
        let parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
            .expect("repository HEVC sample should contain VPS/SPS/PPS");

        let payload = build_hevc_submit_probe_bitstream_payload(
            &parameter_sets,
            &bitstream,
            MAX_HEVC_SUBMIT_PROBE_ACCESS_UNITS,
        )
        .expect("submit probe payload should be built from parsed parameter sets");
        assert!(payload.bytes.starts_with(&[0, 0, 0, 1]));
        assert_eq!(
            payload.access_units.len(),
            MAX_HEVC_SUBMIT_PROBE_ACCESS_UNITS
        );
        assert!(
            usize::try_from(payload.access_units[0].slice_segment_offset)
                .is_ok_and(|offset| offset < payload.bytes.len())
        );
        assert_eq!(
            payload.access_units[0].header.pps_id,
            payload.parsed_pps.pps_pic_parameter_set_id
        );
    }

    #[test]
    fn build_submit_probe_bitstream_payload_allows_extended_access_unit_limit() {
        let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.h265");
        let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
        let parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
            .expect("repository HEVC sample should contain VPS/SPS/PPS");
        let access_unit_headers =
            extract_hevc_access_unit_headers(&bitstream).expect("sample must expose access units");
        let payload = build_hevc_submit_probe_bitstream_payload(
            &parameter_sets,
            &bitstream,
            access_unit_headers.len(),
        )
        .expect("extended submit payload should be built");
        assert_eq!(payload.access_units.len(), access_unit_headers.len());
        assert!(payload.access_units.len() > MAX_HEVC_SUBMIT_PROBE_ACCESS_UNITS);
    }

    #[test]
    fn build_decode_readback_regions_nv12_handles_odd_dimensions() {
        let (buffer_size, regions) =
            build_decode_readback_regions(vk::Format::G8_B8R8_2PLANE_420_UNORM, 641, 479)
                .expect("NV12 readback regions should be computed");
        assert_eq!(regions.len(), 2);
        assert_eq!(buffer_size, 461_120);

        let y_region = regions[0];
        assert_eq!(y_region.buffer_offset, 0);
        assert_eq!(y_region.buffer_row_length, 641);
        assert_eq!(y_region.buffer_image_height, 479);
        assert_eq!(
            y_region.image_subresource.aspect_mask,
            vk::ImageAspectFlags::PLANE_0
        );
        assert_eq!(y_region.image_extent.width, 641);
        assert_eq!(y_region.image_extent.height, 479);

        let uv_region = regions[1];
        assert_eq!(uv_region.buffer_offset, 307_040);
        assert_eq!(uv_region.buffer_row_length, 321);
        assert_eq!(uv_region.buffer_image_height, 240);
        assert_eq!(
            uv_region.image_subresource.aspect_mask,
            vk::ImageAspectFlags::PLANE_1
        );
        assert_eq!(uv_region.image_extent.width, 321);
        assert_eq!(uv_region.image_extent.height, 240);
    }

    #[test]
    fn build_decode_readback_regions_rejects_unsupported_format() {
        let err = build_decode_readback_regions(vk::Format::D32_SFLOAT, 640, 360)
            .expect_err("unsupported decode output format should be rejected");
        assert!(
            err.contains("not implemented"),
            "expected unsupported format details, got: {err}"
        );
    }

    #[test]
    fn decide_hevc_experimental_dpb_auto_disables_when_marker_exists() {
        let decision = decide_hevc_experimental_dpb(HevcExperimentalDpbMode::Auto, true, false);
        assert_eq!(
            decision,
            HevcExperimentalDpbDecision::DisabledAutoMarkerPresent
        );
    }

    #[test]
    fn decide_hevc_experimental_dpb_auto_disables_when_marker_write_fails() {
        let decision = decide_hevc_experimental_dpb(HevcExperimentalDpbMode::Auto, false, true);
        assert_eq!(
            decision,
            HevcExperimentalDpbDecision::DisabledAutoMarkerWriteFailed
        );
    }

    #[test]
    fn decide_hevc_experimental_dpb_on_keeps_path_enabled_when_marker_write_fails() {
        let decision = decide_hevc_experimental_dpb(HevcExperimentalDpbMode::On, false, true);
        assert_eq!(decision, HevcExperimentalDpbDecision::EnabledOn);
        assert!(decision.enabled());
    }

    #[test]
    fn format_hevc_experimental_dpb_status_surfaces_auto_marker_reason() {
        let marker_path = PathBuf::from("C:\\temp\\video-hw-vulkan-hevc-dpb-inflight.flag");
        let status = format_hevc_experimental_dpb_status(
            HevcExperimentalDpbDecision::DisabledAutoMarkerPresent,
            &marker_path,
            None,
        );
        assert!(status.contains("mode=auto"));
        assert!(status.contains("disabled"));
        assert!(status.contains("stale inflight marker"));
    }

    #[test]
    fn hevc_reference_nal_type_identifies_reference_and_non_reference_types() {
        assert!(
            is_hevc_reference_nal_type(19),
            "IDR should be reference-capable"
        );
        assert!(
            is_hevc_reference_nal_type(1),
            "TRAIL_R should be reference-capable"
        );
        assert!(
            !is_hevc_reference_nal_type(0),
            "TRAIL_N should be treated as non-reference"
        );
    }

    #[test]
    fn select_hevc_decode_dpb_slot_rotates_reference_slots() {
        let mut next_reference_slot = 0_usize;
        let active = Vec::new();
        let first = select_hevc_decode_dpb_slot(true, true, 4, &mut next_reference_slot, &active);
        let second = select_hevc_decode_dpb_slot(true, true, 4, &mut next_reference_slot, &active);
        let third = select_hevc_decode_dpb_slot(true, true, 4, &mut next_reference_slot, &active);
        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert_eq!(third, 3);
    }

    #[test]
    fn select_hevc_decode_dpb_slot_uses_free_slot_for_non_reference_when_slot_zero_is_active() {
        let mut next_reference_slot = 0_usize;
        let active = vec![HevcActiveReferenceSlot {
            slot: 0,
            pic_order_cnt_val: 0,
        }];
        let slot = select_hevc_decode_dpb_slot(true, false, 4, &mut next_reference_slot, &active);
        assert_eq!(slot, 1);
    }

    #[test]
    fn build_hevc_ref_pic_set_lists_assigns_before_and_after_indices() {
        let references = vec![12_i32, 8, 16];
        let (before, after, count) = build_hevc_ref_pic_set_lists(&references, 10);
        assert_eq!(before[0], 1);
        assert_eq!(before[1], HEVC_NO_REFERENCE_PICTURE);
        assert_eq!(after[0], 0);
        assert_eq!(after[1], 2);
        assert_eq!(count, 3);
    }

    #[test]
    fn build_hevc_ref_pic_set_lists_marks_empty_entries_as_no_reference() {
        let references = Vec::<i32>::new();
        let (before, after, count) = build_hevc_ref_pic_set_lists(&references, 0);
        assert_eq!(
            before,
            [HEVC_NO_REFERENCE_PICTURE; HEVC_REF_PIC_SET_LIST_SIZE]
        );
        assert_eq!(
            after,
            [HEVC_NO_REFERENCE_PICTURE; HEVC_REF_PIC_SET_LIST_SIZE]
        );
        assert_eq!(count, 0);
    }

    #[test]
    fn decode_submit_skeleton_probe_surfaces_pps_parse_errors() {
        let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.h265");
        let bitstream = std::fs::read(sample_path).expect("sample-10s.h265 should be readable");
        let mut parameter_sets = extract_hevc_parameter_sets_annexb(&bitstream)
            .expect("repository HEVC sample should contain VPS/SPS/PPS");
        parameter_sets.pps = vec![0x44, 0x01];
        let capability_snapshot = HevcCapabilitySnapshot {
            min_bitstream_buffer_offset_alignment: 1,
            min_bitstream_buffer_size_alignment: 1,
            picture_access_granularity: vk::Extent2D {
                width: 1,
                height: 1,
            },
            min_coded_extent: vk::Extent2D {
                width: 64,
                height: 64,
            },
            max_coded_extent: vk::Extent2D {
                width: 8192,
                height: 8192,
            },
            max_dpb_slots: 8,
            max_active_reference_pictures: 4,
            max_level_idc: 0,
            std_header_version: vk::ExtensionProperties::default(),
        };

        match build_hevc_decode_submit_skeleton_probe(&parameter_sets, &capability_snapshot) {
            HevcDecodeSubmitSkeletonProbe::Failed(err) => {
                assert!(
                    err.contains("parse PPS"),
                    "expected parse failure details in error: {err}"
                );
            }
            other => panic!("expected decode submit skeleton failure, got {other:?}"),
        }
    }

    #[test]
    fn hevc_decode_session_bootstrap_rejects_invalid_bitstream() {
        let err = probe_hevc_decode_session_bootstrap(&[1, 2, 3])
            .expect_err("invalid stream should fail before Vulkan bootstrap");
        assert!(
            err.contains("missing VPS")
                || err.contains("missing SPS")
                || err.contains("missing PPS")
        );
    }
}
