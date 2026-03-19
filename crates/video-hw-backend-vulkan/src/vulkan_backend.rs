use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use ash::vk;
use vk_video::parameters::{DecoderParameters, RateControl, VideoParameters};
use vk_video::{
    BytesDecoder, BytesEncoder, EncodedInputChunk, Frame as VkFrame, RawFrameData, VulkanDevice,
    VulkanInstance,
};

use crate::{
    BackendDecoderOptions, BackendEncoderOptions, BackendError, CapabilityReport, Codec,
    DecodeOutputMode, DecodeSummary, DecoderConfig, EncodedPacket, Frame, Nv12Frame, VideoDecoder,
    VideoEncoder, VulkanDecoderOptions, VulkanEncoderOptions, argb_to_nv12, nv12_to_rgb24,
    vulkan_hevc_decode::{
        HevcDecodePrerequisiteProbe, HevcDecodeSubmitExecutionProbe, HevcDecodeSubmitSkeletonProbe,
        HevcVideoSessionCreateProbe, HevcVideoSessionParametersCreateProbe,
        extract_hevc_access_unit_headers, extract_hevc_parameter_sets_annexb,
        probe_hevc_decode_prerequisites, probe_hevc_decode_session_bootstrap,
        probe_hevc_decode_session_bootstrap_with_access_unit_limit,
    },
    vulkan_hevc_encode::{
        HevcEncodePrerequisiteProbe, HevcEncodeSessionBootstrap, HevcEncodeSubmitExecutionProbe,
        HevcEncodeVideoSessionCreateProbe, HevcEncodeVideoSessionParametersCreateProbe,
        probe_hevc_encode_prerequisites, probe_hevc_encode_session_bootstrap,
    },
};

const HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG: u32 = 1;
const HEVC_DECODE_INFO_SLOT_SHIFT: u32 = 1;
const HEVC_DECODE_INFO_PROBE_COVERED_FLAG: u32 = 1 << 31;

pub struct VulkanDecoderAdapter {
    config: DecoderConfig,
    options: VulkanDecoderOptions,
    pending_bitstream: Vec<u8>,
    next_pts_90k: i64,
    last_summary: DecodeSummary,
}

impl VulkanDecoderAdapter {
    pub fn new(config: DecoderConfig) -> Self {
        let options = match &config.backend_options {
            BackendDecoderOptions::Vulkan(options) => options.clone(),
            BackendDecoderOptions::Default
            | BackendDecoderOptions::Nvidia(_)
            | BackendDecoderOptions::Intel(_) => VulkanDecoderOptions::default(),
        };
        Self {
            config,
            options,
            pending_bitstream: Vec::new(),
            next_pts_90k: 0,
            last_summary: DecodeSummary {
                decoded_frames: 0,
                width: None,
                height: None,
                pixel_format: None,
            },
        }
    }

    fn apply_decoded_summary(&mut self, decoded: &[Frame]) {
        self.last_summary.decoded_frames = self
            .last_summary
            .decoded_frames
            .saturating_add(decoded.len());
        if let Some(last) = decoded.last() {
            self.last_summary.width = Some(last.width);
            self.last_summary.height = Some(last.height);
            self.last_summary.pixel_format = last.pixel_format;
        }
    }

    fn decode_pending_bitstream(&self, bitstream: &[u8]) -> Result<Vec<Frame>, BackendError> {
        if matches!(self.config.codec, Codec::Hevc) {
            return self.decode_pending_hevc_bitstream(bitstream);
        }
        ensure_vk_codec_supported(self.config.codec, "decode")?;
        let mut decoder = create_vk_bytes_decoder().map_err(|err| {
            if !self.config.require_hardware && self.options.allow_software_fallback.unwrap_or(true)
            {
                BackendError::UnsupportedConfig(format!(
                    "{err}; software fallback is not available in direct Vulkan backend"
                ))
            } else {
                err
            }
        })?;
        let mut decoded = decoder
            .decode(EncodedInputChunk {
                data: bitstream,
                pts: None,
            })
            .map_err(|err| {
                BackendError::UnsupportedConfig(format!("Vulkan decode failed: {err}"))
            })?;
        decoded.extend(decoder.flush().map_err(|err| {
            BackendError::UnsupportedConfig(format!("Vulkan decode flush failed: {err}"))
        })?);
        build_backend_frames(
            decoded,
            self.config.output_mode,
            self.next_pts_90k,
            self.config.fps,
        )
    }

    fn decode_pending_hevc_bitstream(&self, bitstream: &[u8]) -> Result<Vec<Frame>, BackendError> {
        let metadata_only = matches!(self.config.output_mode, DecodeOutputMode::Metadata);
        let access_unit_headers = extract_hevc_access_unit_headers(bitstream).map_err(|err| {
            BackendError::UnsupportedConfig(format!(
                "Vulkan HEVC decode could not extract access units: {err}"
            ))
        })?;
        let submit_probe_access_unit_limit = (!metadata_only).then_some(access_unit_headers.len());
        let bootstrap = probe_hevc_decode_session_bootstrap_with_access_unit_limit(
            bitstream,
            submit_probe_access_unit_limit,
        )
        .map_err(|_| {
            BackendError::UnsupportedConfig(hevc_decode_blocker_message_with_bitstream(bitstream))
        })?;
        let (
            output_format,
            coded_width,
            coded_height,
            readback_non_zero,
            readback_bytes,
            readback_planes,
            readback_sample_stride,
            readback_sample_count,
            readback_sample,
            submitted_access_units,
        ) = match &bootstrap.decode_submit_execution_probe {
            HevcDecodeSubmitExecutionProbe::Ready {
                output_format,
                coded_width,
                coded_height,
                readback_non_zero,
                readback_bytes,
                readback_planes,
                readback_sample_stride,
                readback_sample_count,
                readback_sample,
                submitted_access_units,
                ..
            } => (
                *output_format,
                *coded_width,
                *coded_height,
                *readback_non_zero,
                *readback_bytes,
                *readback_planes,
                *readback_sample_stride,
                *readback_sample_count,
                readback_sample.clone(),
                *submitted_access_units,
            ),
            HevcDecodeSubmitExecutionProbe::Failed(err) => {
                return Err(BackendError::UnsupportedConfig(format!(
                    "Vulkan HEVC decode submit execution failed: {err}; {}",
                    hevc_decode_blocker_message_with_bitstream(bitstream)
                )));
            }
            HevcDecodeSubmitExecutionProbe::Skipped(reason) => {
                return Err(BackendError::UnsupportedConfig(format!(
                    "Vulkan HEVC decode submit execution was skipped: {reason}; {}",
                    hevc_decode_blocker_message_with_bitstream(bitstream)
                )));
            }
        };
        if submitted_access_units == 0 {
            return Err(BackendError::UnsupportedConfig(
                "Vulkan HEVC decode submit execution reported submitted_access_units=0".to_string(),
            ));
        }
        let width = usize::try_from(coded_width).map_err(|_| {
            BackendError::InvalidInput("decoded HEVC width does not fit in usize".to_string())
        })?;
        let height = usize::try_from(coded_height).map_err(|_| {
            BackendError::InvalidInput("decoded HEVC height does not fit in usize".to_string())
        })?;
        let dpb_slot_count = usize::try_from(bootstrap.max_dpb_slots.max(1)).unwrap_or(1);
        let probe_covered_access_units =
            usize::try_from(submitted_access_units).unwrap_or(usize::MAX);
        ensure_hevc_non_metadata_probe_coverage(
            metadata_only,
            access_unit_headers.len(),
            probe_covered_access_units,
            readback_bytes,
            readback_planes,
            submitted_access_units,
        )?;
        let frame_count = access_unit_headers.len();
        let hevc_argb_frames = if metadata_only {
            None
        } else {
            let readback_sample_count =
                usize::try_from(readback_sample_count).unwrap_or(usize::MAX);
            if readback_sample_count < frame_count {
                return Err(BackendError::UnsupportedConfig(format!(
                    "Vulkan HEVC non-metadata output readback sample count is too small: got {readback_sample_count}, need at least {frame_count}; readback_handoff=bytes:{readback_bytes}, planes:{readback_planes}, sample_stride:{readback_sample_stride}, submitted_access_units:{submitted_access_units}"
                )));
            }
            Some(
                (0..frame_count)
                    .map(|sample_index| {
                        let sample = hevc_probe_readback_sample_at(
                            &readback_sample,
                            readback_sample_stride,
                            sample_index,
                        )?;
                        hevc_probe_readback_to_argb(output_format, sample, width, height)
                    })
                    .collect::<Result<Vec<_>, BackendError>>()?,
            )
        };
        let pts_step = decode_pts_step(self.config.fps);
        let mut next_slot = 0_usize;
        let frames = access_unit_headers
            .into_iter()
            .take(frame_count)
            .enumerate()
            .map(|(index, access_unit)| {
                let is_idr = access_unit.nal_unit_type == 19 || access_unit.nal_unit_type == 20;
                if is_idr {
                    next_slot = 0;
                }
                let slot = next_slot % dpb_slot_count;
                next_slot = next_slot.saturating_add(1);
                let pts_90k = Some(
                    self.next_pts_90k
                        .saturating_add(usize_to_i64(index) * pts_step),
                );
                let mut frame = metadata_only_frame(width, height, pts_90k);
                let probe_covered = index < probe_covered_access_units;
                frame.decode_info_flags = Some(build_hevc_metadata_decode_info_flags(
                    slot,
                    probe_covered,
                    readback_non_zero,
                ));
                if let Some(argb_frames) = hevc_argb_frames.as_ref() {
                    frame.argb = Some(argb_frames[index].clone());
                }
                frame.force_keyframe = is_idr;
                frame
            })
            .collect::<Vec<_>>();
        Ok(frames)
    }
}

impl VideoDecoder for VulkanDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(vulkan_capability_report(codec))
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        if self.pending_bitstream.is_empty()
            && let Some(pts_90k) = pts_90k
        {
            self.next_pts_90k = pts_90k;
        }
        self.pending_bitstream.extend_from_slice(chunk);
        Ok(Vec::new())
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        if self.pending_bitstream.is_empty() {
            return Ok(Vec::new());
        }
        let pending_bitstream = std::mem::take(&mut self.pending_bitstream);
        match self.decode_pending_bitstream(&pending_bitstream) {
            Ok(frames) => {
                let step = decode_pts_step(self.config.fps);
                self.next_pts_90k = self
                    .next_pts_90k
                    .saturating_add(usize_to_i64(frames.len()).saturating_mul(step));
                self.apply_decoded_summary(&frames);
                Ok(frames)
            }
            Err(err) => {
                self.pending_bitstream = pending_bitstream;
                Err(err)
            }
        }
    }

    fn decode_summary(&self) -> DecodeSummary {
        self.last_summary.clone()
    }
}

pub struct VulkanEncoderAdapter {
    codec: Codec,
    fps: i32,
    require_hardware: bool,
    options: VulkanEncoderOptions,
    pending_frames: Vec<Frame>,
    width: Option<usize>,
    height: Option<usize>,
}

impl VulkanEncoderAdapter {
    pub fn with_config(
        codec: Codec,
        fps: i32,
        require_hardware: bool,
        backend_options: BackendEncoderOptions,
    ) -> Self {
        let options = match backend_options {
            BackendEncoderOptions::Vulkan(options) => options,
            BackendEncoderOptions::Default
            | BackendEncoderOptions::Nvidia(_)
            | BackendEncoderOptions::Intel(_) => VulkanEncoderOptions::default(),
        };
        Self {
            codec,
            fps,
            require_hardware,
            options,
            pending_frames: Vec::new(),
            width: None,
            height: None,
        }
    }

    fn encode_pending_frames(
        &self,
        pending_frames: &[Frame],
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        let width = self.width.unwrap_or(0);
        let height = self.height.unwrap_or(0);
        if width == 0 || height == 0 {
            return Err(BackendError::InvalidInput(
                "encoder dimensions are not initialized".to_string(),
            ));
        }
        if matches!(self.codec, Codec::Hevc) {
            let coded_width = u32::try_from(width).map_err(|_| {
                BackendError::InvalidInput("frame width does not fit in u32".to_string())
            })?;
            let coded_height = u32::try_from(height).map_err(|_| {
                BackendError::InvalidInput("frame height does not fit in u32".to_string())
            })?;
            return Err(BackendError::UnsupportedConfig(
                hevc_encode_blocker_message_with_config(coded_width, coded_height, self.fps),
            ));
        }
        ensure_vk_codec_supported(self.codec, "encode")?;

        let mut encoder = create_vk_bytes_encoder(width, height, self.fps).map_err(|err| {
            if !self.require_hardware && self.options.allow_software_fallback.unwrap_or(true) {
                BackendError::UnsupportedConfig(format!(
                    "{err}; software fallback is not available in direct Vulkan backend"
                ))
            } else {
                err
            }
        })?;

        let mut packets = Vec::new();
        for frame in pending_frames {
            if frame.width != width || frame.height != height {
                return Err(BackendError::InvalidInput(format!(
                    "mixed frame dimensions are unsupported: expected {}x{}, got {}x{}",
                    width, height, frame.width, frame.height
                )));
            }
            let raw = frame_to_nv12_payload(frame, width, height)?;
            let encoded = encoder
                .encode(
                    &VkFrame {
                        data: raw,
                        pts: frame.pts_90k.and_then(i64_to_u64),
                    },
                    frame.force_keyframe,
                )
                .map_err(|err| {
                    BackendError::UnsupportedConfig(format!("Vulkan encode failed: {err}"))
                })?;
            if encoded.data.is_empty() {
                continue;
            }
            packets.push(EncodedPacket {
                codec: self.codec,
                data: encoded.data,
                pts_90k: encoded.pts.and_then(u64_to_i64),
                is_keyframe: encoded.is_keyframe,
            });
        }

        if packets.is_empty() {
            return Err(BackendError::Backend(
                "Vulkan encoder produced no output packets".to_string(),
            ));
        }
        Ok(packets)
    }
}

impl VideoEncoder for VulkanEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(vulkan_capability_report(codec))
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        if self.width.is_none() || self.height.is_none() {
            self.width = Some(frame.width);
            self.height = Some(frame.height);
        }
        self.pending_frames.push(frame);
        Ok(Vec::new())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        if self.pending_frames.is_empty() {
            return Ok(Vec::new());
        }
        let pending_frames = std::mem::take(&mut self.pending_frames);
        match self.encode_pending_frames(&pending_frames) {
            Ok(packets) => Ok(packets),
            Err(err) => {
                self.pending_frames = pending_frames;
                Err(err)
            }
        }
    }
}

fn ensure_vk_codec_supported(codec: Codec, operation: &str) -> Result<(), BackendError> {
    match codec {
        Codec::H264 => Ok(()),
        Codec::Hevc => {
            let message = if operation == "decode" {
                hevc_decode_blocker_message()
            } else {
                hevc_encode_blocker_message()
            };
            Err(BackendError::UnsupportedConfig(message))
        }
    }
}

fn hevc_decode_blocker_message() -> String {
    let base = "Vulkan HEVC decode initialization failed";
    match probe_hevc_decode_prerequisites() {
        HevcDecodePrerequisiteProbe::Ready => format!(
            "{base}; runtime prerequisites are present, but HEVC session bootstrap/decode submit failed"
        ),
        HevcDecodePrerequisiteProbe::MissingExtensions { missing } => {
            format!("{base}; missing Vulkan extensions: {}", missing.join(", "))
        }
        HevcDecodePrerequisiteProbe::MissingDecodeQueueFamily => {
            format!("{base}; no queue family advertises VIDEO_DECODE_KHR")
        }
        HevcDecodePrerequisiteProbe::DeviceInitializationFailed(details) => {
            format!("{base}; device bootstrap for HEVC decode failed: {details}")
        }
        HevcDecodePrerequisiteProbe::NoCompatibleAdapter => format!(
            "{base}; required extensions were observed but not on a single adapter that can run HEVC decode end-to-end"
        ),
        HevcDecodePrerequisiteProbe::ProbeUnavailable(details) => {
            format!("{base}; extension probe failed: {details}")
        }
    }
}

fn hevc_encode_blocker_message() -> String {
    hevc_encode_blocker_message_with_config(1920, 1080, 30)
}

fn hevc_encode_blocker_message_with_config(
    coded_width: u32,
    coded_height: u32,
    fps: i32,
) -> String {
    let base = "Vulkan HEVC encode initialization failed";
    match probe_hevc_encode_prerequisites() {
        HevcEncodePrerequisiteProbe::Ready => {
            let mut message = format!(
                "{base}; runtime prerequisites are present, but the direct ash-level HEVC encode submit path is not wired yet"
            );
            let target_fps = u32::try_from(fps.max(1)).unwrap_or(30);
            match probe_hevc_encode_session_bootstrap(coded_width, coded_height, target_fps) {
                Ok(bootstrap) => {
                    append_hevc_encode_bootstrap_status(&mut message, &bootstrap);
                }
                Err(err) => {
                    message.push_str(&format!("; encode session bootstrap probe failed: {err}"));
                }
            }
            message
        }
        HevcEncodePrerequisiteProbe::MissingExtensions { missing } => {
            format!("{base}; missing Vulkan extensions: {}", missing.join(", "))
        }
        HevcEncodePrerequisiteProbe::MissingEncodeQueueFamily => {
            format!("{base}; no queue family advertises VIDEO_ENCODE_KHR")
        }
        HevcEncodePrerequisiteProbe::DeviceInitializationFailed(details) => {
            format!("{base}; device bootstrap for HEVC encode failed: {details}")
        }
        HevcEncodePrerequisiteProbe::NoCompatibleAdapter => format!(
            "{base}; required extensions were observed but not on a single adapter that can run HEVC encode end-to-end"
        ),
        HevcEncodePrerequisiteProbe::ProbeUnavailable(details) => {
            format!("{base}; extension probe failed: {details}")
        }
    }
}

fn append_hevc_encode_bootstrap_status(
    message: &mut String,
    bootstrap: &HevcEncodeSessionBootstrap,
) {
    let input_formats = format_vk_formats(&bootstrap.encode_input_formats);
    let dpb_formats = format_vk_formats(&bootstrap.encode_dpb_formats);
    let session_create = match &bootstrap.video_session_create_probe {
        HevcEncodeVideoSessionCreateProbe::Created => "created".to_string(),
        HevcEncodeVideoSessionCreateProbe::Failed(err) => format!("failed ({err})"),
    };
    let session_parameters_create = match &bootstrap.video_session_parameters_create_probe {
        HevcEncodeVideoSessionParametersCreateProbe::Created => "created".to_string(),
        HevcEncodeVideoSessionParametersCreateProbe::Failed(err) => format!("failed ({err})"),
        HevcEncodeVideoSessionParametersCreateProbe::Skipped(reason) => {
            format!("skipped ({reason})")
        }
    };
    let submit_execution = match &bootstrap.encode_submit_execution_probe {
        HevcEncodeSubmitExecutionProbe::Ready { queue_family_index } => {
            format!("ready(queue_family_index={queue_family_index})")
        }
        HevcEncodeSubmitExecutionProbe::Failed(err) => format!("failed ({err})"),
        HevcEncodeSubmitExecutionProbe::Skipped(reason) => format!("skipped ({reason})"),
    };
    let rate_control_modes = format_hevc_encode_rate_control_modes(bootstrap.rate_control_modes);
    let encode_capability_flags =
        format_hevc_encode_capability_flags(bootstrap.encode_capability_flags);
    let encode_h265_capability_flags =
        format_hevc_encode_h265_capability_flags(bootstrap.encode_h265_capability_flags);
    let encode_feedback_flags =
        format_hevc_encode_feedback_flags(bootstrap.supported_encode_feedback_flags);
    message.push_str(&format!(
        "; encode session bootstrap probe: coded={}x{}, adapter='{}'(vendor=0x{:04x}, device=0x{:04x}, driver=0x{:08x}, api=0x{:08x}), supported={}x{}..{}x{}, picture_access_granularity={}x{}, encode_input_granularity={}x{}, coded_extent_aligned_to_input_granularity={}, max_dpb_slots={}, max_active_refs={}, rate_control_modes={}, max_rate_control_layers={}, max_bitrate={}, max_quality_levels={}, encode_capability_flags={}, encode_h265_capability_flags={}, encode_feedback_flags={}, min_dst_offset_align={}, min_dst_size_align={}, max_level_idc={}, input_formats=[{}], dpb_formats=[{}], video_session_create={}, video_session_parameters_create={}, encode_submit_execution={}",
        bootstrap.coded_width,
        bootstrap.coded_height,
        bootstrap.adapter_name,
        bootstrap.adapter_vendor_id,
        bootstrap.adapter_device_id,
        bootstrap.adapter_driver_version,
        bootstrap.adapter_api_version,
        bootstrap.min_coded_width,
        bootstrap.min_coded_height,
        bootstrap.max_coded_width,
        bootstrap.max_coded_height,
        bootstrap.picture_access_granularity_width,
        bootstrap.picture_access_granularity_height,
        bootstrap.encode_input_granularity_width,
        bootstrap.encode_input_granularity_height,
        bootstrap.coded_extent_input_granularity_aligned,
        bootstrap.max_dpb_slots,
        bootstrap.max_active_reference_pictures,
        rate_control_modes,
        bootstrap.max_rate_control_layers,
        bootstrap.max_bitrate,
        bootstrap.max_quality_levels,
        encode_capability_flags,
        encode_h265_capability_flags,
        encode_feedback_flags,
        bootstrap.min_bitstream_buffer_offset_alignment,
        bootstrap.min_bitstream_buffer_size_alignment,
        bootstrap.max_level_idc,
        input_formats,
        dpb_formats,
        session_create,
        session_parameters_create,
        submit_execution
    ));
}

fn format_hevc_encode_rate_control_modes(modes: vk::VideoEncodeRateControlModeFlagsKHR) -> String {
    let mut labels = Vec::new();
    if modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED) {
        labels.push("DISABLED");
    }
    if modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::CBR) {
        labels.push("CBR");
    }
    if modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::VBR) {
        labels.push("VBR");
    }
    if labels.is_empty() {
        "none".to_string()
    } else {
        labels.join("|")
    }
}

fn format_hevc_encode_capability_flags(flags: vk::VideoEncodeCapabilityFlagsKHR) -> String {
    let mut labels = Vec::new();
    if flags.contains(vk::VideoEncodeCapabilityFlagsKHR::PRECEDING_EXTERNALLY_ENCODED_BYTES) {
        labels.push("PRECEDING_EXTERNALLY_ENCODED_BYTES");
    }
    if flags.contains(vk::VideoEncodeCapabilityFlagsKHR::INSUFFICIENTSTREAM_BUFFER_RANGE_DETECTION)
    {
        labels.push("INSUFFICIENTSTREAM_BUFFER_RANGE_DETECTION");
    }
    if labels.is_empty() {
        "none".to_string()
    } else {
        labels.join("|")
    }
}

fn format_hevc_encode_h265_capability_flags(
    flags: vk::VideoEncodeH265CapabilityFlagsKHR,
) -> String {
    let raw = flags.as_raw();
    if raw == 0 {
        "none".to_string()
    } else {
        let mut labels = Vec::new();
        if (raw & 0x0001) != 0 {
            labels.push("HRD_COMPLIANCE".to_string());
        }
        if (raw & 0x0002) != 0 {
            labels.push("PREDICTION_WEIGHT_TABLE_GENERATED".to_string());
        }
        if (raw & 0x0004) != 0 {
            labels.push("ROW_UNALIGNED_SLICE_SEGMENT".to_string());
        }
        if (raw & 0x0008) != 0 {
            labels.push("DIFFERENT_SLICE_SEGMENT_TYPE".to_string());
        }
        if (raw & 0x0010) != 0 {
            labels.push("B_FRAME_IN_L0_LIST".to_string());
        }
        if (raw & 0x0020) != 0 {
            labels.push("B_FRAME_IN_L1_LIST".to_string());
        }
        if (raw & 0x0040) != 0 {
            labels.push("PER_PICTURE_TYPE_MIN_MAX_QP".to_string());
        }
        if (raw & 0x0080) != 0 {
            labels.push("PER_SLICE_SEGMENT_CONSTANT_QP".to_string());
        }
        if (raw & 0x0100) != 0 {
            labels.push("MULTIPLE_TILES_PER_SLICE_SEGMENT".to_string());
        }
        if (raw & 0x0200) != 0 {
            labels.push("MULTIPLE_SLICE_SEGMENTS_PER_TILE".to_string());
        }
        if (raw & 0x0400) != 0 {
            labels.push("CU_QP_DIFF_WRAPAROUND (quantization-map)".to_string());
        }
        if (raw & 0x0800) != 0 {
            labels.push("B_PICTURE_INTRA_REFRESH".to_string());
        }
        let known_mask = 0x0fff;
        let unknown_bits = raw & !known_mask;
        if unknown_bits != 0 {
            labels.push(format!("UNKNOWN_BITS(0x{unknown_bits:x})"));
        }
        format!("{} (raw=0x{:x})", labels.join("|"), raw)
    }
}

fn format_hevc_encode_feedback_flags(flags: vk::VideoEncodeFeedbackFlagsKHR) -> String {
    let mut labels = Vec::new();
    if flags.contains(vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BUFFER_OFFSET) {
        labels.push("BITSTREAM_BUFFER_OFFSET");
    }
    if flags.contains(vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BYTES_WRITTEN) {
        labels.push("BITSTREAM_BYTES_WRITTEN");
    }
    if flags.contains(vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_HAS_OVERRIDES) {
        labels.push("BITSTREAM_HAS_OVERRIDES");
    }
    if labels.is_empty() {
        "none".to_string()
    } else {
        labels.join("|")
    }
}

fn format_vk_formats(formats: &[vk::Format]) -> String {
    let formatted = formats
        .iter()
        .map(|format| format!("{format:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    if formatted.is_empty() {
        "none".to_string()
    } else {
        formatted
    }
}

fn hevc_decode_blocker_message_with_bitstream(bitstream: &[u8]) -> String {
    let mut message = hevc_decode_blocker_message();
    match extract_hevc_parameter_sets_annexb(bitstream) {
        Ok(parameter_sets) => {
            message.push_str(&format!(
                "; parsed HEVC parameter sets: vps={} bytes, sps={} bytes, pps={} bytes, coded={}x{}",
                parameter_sets.vps.len(),
                parameter_sets.sps.len(),
                parameter_sets.pps.len(),
                parameter_sets.coded_width,
                parameter_sets.coded_height
            ));
            match probe_hevc_decode_session_bootstrap(bitstream) {
                Ok(bootstrap) => {
                    let formats = bootstrap
                        .decode_output_formats
                        .iter()
                        .map(|format| format!("{format:?}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let formats = if formats.is_empty() {
                        "none".to_string()
                    } else {
                        formats
                    };
                    let session_create = match &bootstrap.video_session_create_probe {
                        HevcVideoSessionCreateProbe::Created => "created".to_string(),
                        HevcVideoSessionCreateProbe::Failed(err) => format!("failed ({err})"),
                    };
                    let session_parameters_create =
                        match &bootstrap.video_session_parameters_create_probe {
                            HevcVideoSessionParametersCreateProbe::Created => "created".to_string(),
                            HevcVideoSessionParametersCreateProbe::Failed(err) => {
                                format!("failed ({err})")
                            }
                            HevcVideoSessionParametersCreateProbe::Skipped(reason) => {
                                format!("skipped ({reason})")
                            }
                        };
                    let decode_submit_skeleton = match &bootstrap.decode_submit_skeleton_probe {
                        HevcDecodeSubmitSkeletonProbe::Ready(skeleton) => format!(
                            "ready(vps_id={}, sps_id={}, pps_id={}, vcl_nalus={}, first_slice_nal_type={:?}, first_slice_pps_id={:?}, first_slice_pic_order_cnt_lsb={:?}, dpb_slots={:?}, ref_slots={:?})",
                            skeleton.vps_id,
                            skeleton.sps_id,
                            skeleton.pps_id,
                            skeleton.vcl_nalu_count,
                            skeleton.first_slice_nal_type,
                            skeleton.first_slice_pps_id,
                            skeleton.first_slice_pic_order_cnt_lsb,
                            skeleton.planned_dpb_slots,
                            skeleton.planned_reference_slots
                        ),
                        HevcDecodeSubmitSkeletonProbe::Failed(err) => format!("failed ({err})"),
                        HevcDecodeSubmitSkeletonProbe::Skipped(reason) => {
                            format!("skipped ({reason})")
                        }
                    };
                    let decode_submit_execution = match &bootstrap.decode_submit_execution_probe {
                        HevcDecodeSubmitExecutionProbe::Ready {
                            queue_family_index,
                            output_format,
                            coded_width,
                            coded_height,
                            readback_non_zero,
                            readback_bytes,
                            readback_planes,
                            readback_sample_stride,
                            readback_sample_count,
                            readback_sample,
                            submitted_access_units,
                            experimental_dpb_enabled,
                            experimental_dpb_mode,
                            experimental_dpb_status,
                        } => format!(
                            "ready(queue_family_index={}, output_format={output_format:?}, coded={}x{}, readback_non_zero={}, readback_bytes={}, readback_planes={}, readback_sample_stride={}, readback_sample_count={}, readback_sample_len={}, submitted_access_units={}, experimental_dpb_enabled={}, experimental_dpb_mode={}, experimental_dpb_status={experimental_dpb_status:?})",
                            queue_family_index,
                            coded_width,
                            coded_height,
                            readback_non_zero,
                            readback_bytes,
                            readback_planes,
                            readback_sample_stride,
                            readback_sample_count,
                            readback_sample.len(),
                            submitted_access_units,
                            experimental_dpb_enabled,
                            experimental_dpb_mode
                        ),
                        HevcDecodeSubmitExecutionProbe::Failed(err) => format!("failed ({err})"),
                        HevcDecodeSubmitExecutionProbe::Skipped(reason) => {
                            format!("skipped ({reason})")
                        }
                    };
                    message.push_str(&format!(
                        "; session bootstrap probe: coded={}x{}, supported={}x{}..{}x{}, max_dpb_slots={}, max_active_refs={}, max_level_idc={}, output_formats=[{}], video_session_create={}, video_session_parameters_create={}, decode_submit_skeleton={}, decode_submit_execution={}",
                        bootstrap.coded_width,
                        bootstrap.coded_height,
                        bootstrap.min_coded_width,
                        bootstrap.min_coded_height,
                        bootstrap.max_coded_width,
                        bootstrap.max_coded_height,
                        bootstrap.max_dpb_slots,
                        bootstrap.max_active_reference_pictures,
                        bootstrap.max_level_idc,
                        formats,
                        session_create,
                        session_parameters_create,
                        decode_submit_skeleton,
                        decode_submit_execution
                    ));
                }
                Err(err) => {
                    message.push_str(&format!("; session bootstrap probe failed: {err}"));
                }
            }
        }
        Err(err) => {
            message.push_str(&format!("; parameter-set extraction failed: {err}"));
        }
    }
    message
}

fn vulkan_capability_report(codec: Codec) -> CapabilityReport {
    let supports_h264 = matches!(codec, Codec::H264);
    let (decode_h264_supported, encode_h264_supported) = probe_vulkan_support();
    let decode_supported = supports_h264 && decode_h264_supported;
    let encode_supported = supports_h264 && encode_h264_supported;
    CapabilityReport {
        codec,
        decode_supported,
        encode_supported,
        hardware_acceleration: decode_supported || encode_supported,
    }
}

fn build_backend_frames(
    decoded: Vec<VkFrame<RawFrameData>>,
    output_mode: DecodeOutputMode,
    start_pts_90k: i64,
    fps: i32,
) -> Result<Vec<Frame>, BackendError> {
    let metadata_mode = matches!(output_mode, DecodeOutputMode::Metadata);
    let pts_step = decode_pts_step(fps);
    decoded
        .into_iter()
        .enumerate()
        .map(|(index, decoded_frame)| {
            let width = usize::try_from(decoded_frame.data.width).map_err(|_| {
                BackendError::InvalidInput("decoded width does not fit in usize".to_string())
            })?;
            let height = usize::try_from(decoded_frame.data.height).map_err(|_| {
                BackendError::InvalidInput("decoded height does not fit in usize".to_string())
            })?;
            let pts_90k = Some(start_pts_90k.saturating_add(usize_to_i64(index) * pts_step));
            let mut frame = metadata_only_frame(width, height, pts_90k);
            if !metadata_mode {
                frame.argb = Some(raw_nv12_to_argb(&decoded_frame.data)?);
            }
            Ok(frame)
        })
        .collect::<Result<Vec<_>, BackendError>>()
}

fn metadata_only_frame(width: usize, height: usize, pts_90k: Option<i64>) -> Frame {
    Frame {
        width,
        height,
        pixel_format: None,
        pts_90k,
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb: None,
        #[cfg(feature = "unstable-raw-inputs")]
        nv12: None,
        force_keyframe: false,
    }
}

fn build_hevc_metadata_decode_info_flags(
    slot: usize,
    probe_covered: bool,
    readback_non_zero: bool,
) -> u32 {
    let slot_bits = u32::try_from(slot).unwrap_or(u32::MAX) << HEVC_DECODE_INFO_SLOT_SHIFT;
    let readback_bits = if probe_covered && readback_non_zero {
        HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG
    } else {
        0
    };
    let probe_bits = if probe_covered {
        HEVC_DECODE_INFO_PROBE_COVERED_FLAG
    } else {
        0
    };
    slot_bits | readback_bits | probe_bits
}

fn hevc_probe_readback_sample_at(
    readback_sample: &[u8],
    sample_stride: usize,
    sample_index: usize,
) -> Result<&[u8], BackendError> {
    if sample_stride == 0 {
        return Err(BackendError::UnsupportedConfig(
            "Vulkan HEVC readback sample stride must be greater than zero".to_string(),
        ));
    }
    let start = sample_index.checked_mul(sample_stride).ok_or_else(|| {
        BackendError::UnsupportedConfig("Vulkan HEVC readback sample offset overflow".to_string())
    })?;
    let end = start.checked_add(sample_stride).ok_or_else(|| {
        BackendError::UnsupportedConfig(
            "Vulkan HEVC readback sample end offset overflow".to_string(),
        )
    })?;
    let sample = readback_sample.get(start..end).ok_or_else(|| {
        BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC readback sample range out of bounds: index={sample_index}, stride={sample_stride}, total={}",
            readback_sample.len()
        ))
    })?;
    Ok(sample)
}

fn hevc_probe_readback_to_argb(
    output_format: vk::Format,
    readback_sample: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<u8>, BackendError> {
    if output_format != vk::Format::G8_B8R8_2PLANE_420_UNORM {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC non-metadata output currently requires G8_B8R8_2PLANE_420_UNORM readback (got {output_format:?})"
        )));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC non-metadata output currently requires even coded extent, got {}x{}",
            width, height
        )));
    }
    let expected_len = width
        .checked_mul(height)
        .and_then(|y| y.checked_add(y / 2))
        .ok_or_else(|| {
            BackendError::UnsupportedConfig(
                "Vulkan HEVC readback size overflow while preparing NV12 payload".to_string(),
            )
        })?;
    if readback_sample.len() < expected_len {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC readback sample too short for NV12 payload: got {}, need at least {expected_len}",
            readback_sample.len(),
        )));
    }
    let nv12 = Nv12Frame {
        width,
        height,
        pitch: width,
        pts_90k: None,
        data: readback_sample[..expected_len].to_vec(),
    };
    let rgb = nv12_to_rgb24(&nv12)?;
    let mut argb = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
    for px in rgb.data.chunks_exact(3) {
        argb.extend_from_slice(&[255, px[0], px[1], px[2]]);
    }
    Ok(argb)
}

fn ensure_hevc_non_metadata_probe_coverage(
    metadata_only: bool,
    total_access_units: usize,
    probe_covered_access_units: usize,
    readback_bytes: usize,
    readback_planes: u32,
    submitted_access_units: u32,
) -> Result<(), BackendError> {
    if metadata_only {
        return Ok(());
    }
    if probe_covered_access_units == 0 || total_access_units == 0 {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC non-metadata output requires at least one probe-covered access unit; readback_handoff=bytes:{readback_bytes}, planes:{readback_planes}, submitted_access_units:{submitted_access_units}"
        )));
    }
    if total_access_units > probe_covered_access_units {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC non-metadata output currently supports up to {probe_covered_access_units} probe-covered access units, but stream has {total_access_units}; readback_handoff=bytes:{readback_bytes}, planes:{readback_planes}, submitted_access_units:{submitted_access_units}"
        )));
    }
    Ok(())
}

fn raw_nv12_to_argb(raw: &RawFrameData) -> Result<Vec<u8>, BackendError> {
    let width = usize::try_from(raw.width).map_err(|_| {
        BackendError::InvalidInput("decoded width does not fit in usize".to_string())
    })?;
    let height = usize::try_from(raw.height).map_err(|_| {
        BackendError::InvalidInput("decoded height does not fit in usize".to_string())
    })?;
    let rgb = nv12_to_rgb24(&Nv12Frame {
        width,
        height,
        pitch: width,
        pts_90k: None,
        data: raw.frame.clone(),
    })?;
    let mut argb = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
    for px in rgb.data.chunks_exact(3) {
        argb.extend_from_slice(&[255, px[0], px[1], px[2]]);
    }
    Ok(argb)
}

fn frame_to_nv12_payload(
    frame: &Frame,
    width: usize,
    height: usize,
) -> Result<RawFrameData, BackendError> {
    #[cfg(feature = "unstable-raw-inputs")]
    if let Some(nv12) = frame.nv12.as_ref() {
        if nv12.pitch < width {
            return Err(BackendError::InvalidInput(
                "NV12 pitch must be >= frame width".to_string(),
            ));
        }
        let width_u32 = u32::try_from(width).map_err(|_| {
            BackendError::InvalidInput("frame width does not fit in u32".to_string())
        })?;
        let height_u32 = u32::try_from(height).map_err(|_| {
            BackendError::InvalidInput("frame height does not fit in u32".to_string())
        })?;
        return Ok(RawFrameData {
            frame: nv12.data.clone(),
            width: width_u32,
            height: height_u32,
        });
    }

    let argb = frame.argb.as_ref().ok_or_else(|| {
        BackendError::InvalidInput("Vulkan encode requires ARGB input payload".to_string())
    })?;
    let (pitch, nv12) = argb_to_nv12(argb, width, height)?;
    if pitch != width {
        return Err(BackendError::InvalidInput(format!(
            "NV12 pitch mismatch: expected {}, got {}",
            width, pitch
        )));
    }
    let width_u32 = u32::try_from(width)
        .map_err(|_| BackendError::InvalidInput("frame width does not fit in u32".to_string()))?;
    let height_u32 = u32::try_from(height)
        .map_err(|_| BackendError::InvalidInput("frame height does not fit in u32".to_string()))?;
    Ok(RawFrameData {
        frame: nv12,
        width: width_u32,
        height: height_u32,
    })
}

fn probe_vulkan_support() -> (bool, bool) {
    let Ok(instance) = VulkanInstance::new() else {
        return (false, false);
    };
    let Ok(adapters) = instance.iter_adapters(None) else {
        return (false, false);
    };
    let mut decode_supported = false;
    let mut encode_supported = false;
    for adapter in adapters {
        decode_supported |= adapter.supports_decoding();
        encode_supported |= adapter.supports_encoding();
    }
    (decode_supported, encode_supported)
}

fn create_vk_bytes_decoder() -> Result<BytesDecoder, BackendError> {
    let device = create_vk_device(true, false)?;
    device
        .create_bytes_decoder(DecoderParameters::default())
        .map_err(|err| {
            BackendError::UnsupportedConfig(format!("failed to create Vulkan decoder: {err}"))
        })
}

fn create_vk_bytes_encoder(
    width: usize,
    height: usize,
    fps: i32,
) -> Result<BytesEncoder, BackendError> {
    let device = create_vk_device(false, true)?;
    let width_u32 = u32::try_from(width)
        .map_err(|_| BackendError::InvalidInput("frame width does not fit in u32".to_string()))?;
    let height_u32 = u32::try_from(height)
        .map_err(|_| BackendError::InvalidInput("frame height does not fit in u32".to_string()))?;
    let width = NonZeroU32::new(width_u32)
        .ok_or_else(|| BackendError::InvalidInput("frame width must be > 0".to_string()))?;
    let height = NonZeroU32::new(height_u32)
        .ok_or_else(|| BackendError::InvalidInput("frame height must be > 0".to_string()))?;
    let target_fps = u32::try_from(fps.max(1)).unwrap_or(30);
    let video_parameters = VideoParameters {
        width,
        height,
        target_framerate: target_fps.into(),
    };
    let parameters = device
        .encoder_parameters_high_quality(
            video_parameters,
            RateControl::VariableBitrate {
                average_bitrate: 4_000_000,
                max_bitrate: 8_000_000,
                virtual_buffer_size: Duration::from_secs(2),
            },
        )
        .map_err(|err| {
            BackendError::UnsupportedConfig(format!(
                "failed to build Vulkan encoder parameters: {err}"
            ))
        })?;
    device.create_bytes_encoder(parameters).map_err(|err| {
        BackendError::UnsupportedConfig(format!("failed to create Vulkan encoder: {err}"))
    })
}

fn create_vk_device(
    require_decode: bool,
    require_encode: bool,
) -> Result<Arc<VulkanDevice>, BackendError> {
    let instance = VulkanInstance::new().map_err(|err| {
        BackendError::UnsupportedConfig(format!("failed to initialize Vulkan: {err}"))
    })?;
    let adapter = instance
        .iter_adapters(None)
        .map_err(|err| {
            BackendError::UnsupportedConfig(format!("failed to enumerate Vulkan adapters: {err}"))
        })?
        .find(|adapter| {
            (!require_decode || adapter.supports_decoding())
                && (!require_encode || adapter.supports_encoding())
        })
        .ok_or_else(|| {
            let message = match (require_decode, require_encode) {
                (true, true) => {
                    "no Vulkan adapter supports both decode and encode for direct backend"
                }
                (true, false) => "no Vulkan adapter supports decode for direct backend",
                (false, true) => "no Vulkan adapter supports encode for direct backend",
                (false, false) => "no Vulkan adapter is available for direct backend",
            };
            BackendError::UnsupportedConfig(message.to_string())
        })?;
    adapter
        .create_device(Default::default(), Default::default(), Default::default())
        .map_err(|err| {
            BackendError::UnsupportedConfig(format!("failed to create Vulkan device: {err}"))
        })
}

fn decode_pts_step(fps: i32) -> i64 {
    if fps <= 0 {
        3_000
    } else {
        i64::from(90_000 / fps.max(1))
    }
}

fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn i64_to_u64(value: i64) -> Option<u64> {
    u64::try_from(value).ok()
}

fn u64_to_i64(value: u64) -> Option<i64> {
    i64::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_vk_codec_supported_accepts_h264() {
        ensure_vk_codec_supported(Codec::H264, "decode")
            .expect("H264 should be accepted by Vulkan backend");
        ensure_vk_codec_supported(Codec::H264, "encode")
            .expect("H264 should be accepted by Vulkan backend");
    }

    #[test]
    fn ensure_vk_codec_supported_rejects_hevc_decode_with_actionable_message() {
        let err = ensure_vk_codec_supported(Codec::Hevc, "decode")
            .expect_err("HEVC must be rejected until ash-level path is implemented");
        match err {
            BackendError::UnsupportedConfig(message) => {
                assert!(
                    message.contains("Vulkan HEVC decode initialization failed"),
                    "expected HEVC decode initialization message: {message}"
                );
            }
            other => panic!("unexpected HEVC check error: {other:?}"),
        }
    }

    #[test]
    fn ensure_vk_codec_supported_rejects_hevc_encode_with_actionable_message() {
        let err = ensure_vk_codec_supported(Codec::Hevc, "encode")
            .expect_err("HEVC encode must be rejected until ash-level path is implemented");
        match err {
            BackendError::UnsupportedConfig(message) => {
                assert!(
                    message.contains("Vulkan HEVC encode initialization failed"),
                    "expected HEVC encode initialization message: {message}"
                );
            }
            other => panic!("unexpected HEVC check error: {other:?}"),
        }
    }

    #[test]
    fn hevc_encode_blocker_message_with_config_surfaces_base_message() {
        let message = hevc_encode_blocker_message_with_config(1920, 1080, 30);
        assert!(message.contains("Vulkan HEVC encode initialization failed"));
    }

    #[test]
    fn hevc_capability_report_never_claims_hardware_acceleration() {
        let report = vulkan_capability_report(Codec::Hevc);
        assert!(!report.decode_supported);
        assert!(!report.encode_supported);
        assert!(!report.hardware_acceleration);
    }

    #[test]
    fn hevc_decode_blocker_message_with_bitstream_appends_parameter_set_status() {
        let message = hevc_decode_blocker_message_with_bitstream(&[0_u8, 1, 2, 3]);
        assert!(message.contains("Vulkan HEVC decode initialization failed"));
        assert!(message.contains("parameter-set extraction failed"));
    }

    #[test]
    fn hevc_metadata_decode_info_flags_set_probe_and_readback_when_covered() {
        let flags = build_hevc_metadata_decode_info_flags(3, true, true);
        assert_ne!(flags & HEVC_DECODE_INFO_PROBE_COVERED_FLAG, 0);
        assert_eq!(flags & HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG, 1);
        let slot_mask =
            !(HEVC_DECODE_INFO_PROBE_COVERED_FLAG | HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG);
        assert_eq!(flags & slot_mask, 3 << HEVC_DECODE_INFO_SLOT_SHIFT);
    }

    #[test]
    fn hevc_metadata_decode_info_flags_clear_probe_and_readback_when_uncovered() {
        let flags = build_hevc_metadata_decode_info_flags(2, false, true);
        assert_eq!(flags & HEVC_DECODE_INFO_PROBE_COVERED_FLAG, 0);
        assert_eq!(flags & HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG, 0);
        let slot_mask =
            !(HEVC_DECODE_INFO_PROBE_COVERED_FLAG | HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG);
        assert_eq!(flags & slot_mask, 2 << HEVC_DECODE_INFO_SLOT_SHIFT);
    }

    #[test]
    fn hevc_probe_readback_to_argb_supports_nv12_sample() {
        let readback_sample = vec![16_u8, 16, 16, 16, 128, 128];
        let argb = hevc_probe_readback_to_argb(
            vk::Format::G8_B8R8_2PLANE_420_UNORM,
            &readback_sample,
            2,
            2,
        )
        .expect("NV12 readback sample should convert to ARGB");
        assert_eq!(argb.len(), 16);
    }

    #[test]
    fn hevc_probe_readback_to_argb_rejects_unsupported_format() {
        let err = hevc_probe_readback_to_argb(vk::Format::D32_SFLOAT, &[0_u8; 6], 2, 2)
            .expect_err("unsupported format should fail");
        match err {
            BackendError::UnsupportedConfig(message) => {
                assert!(message.contains("requires G8_B8R8_2PLANE_420_UNORM"));
            }
            other => panic!("unexpected error for unsupported readback format: {other:?}"),
        }
    }

    #[test]
    fn hevc_non_metadata_probe_coverage_requires_full_stream_coverage() {
        let err = ensure_hevc_non_metadata_probe_coverage(false, 303, 16, 6144, 2, 16)
            .expect_err("partial probe coverage should fail for non-metadata mode");
        match err {
            BackendError::UnsupportedConfig(message) => {
                assert!(message.contains("supports up to 16 probe-covered access units"));
                assert!(message.contains("stream has 303"));
            }
            other => panic!("unexpected coverage error: {other:?}"),
        }
    }

    #[test]
    fn hevc_non_metadata_probe_coverage_allows_fully_covered_stream() {
        ensure_hevc_non_metadata_probe_coverage(false, 16, 16, 6144, 2, 16)
            .expect("fully covered stream should pass in non-metadata mode");
    }

    #[test]
    fn hevc_metadata_mode_ignores_probe_coverage_limit() {
        ensure_hevc_non_metadata_probe_coverage(true, 303, 0, 0, 0, 0)
            .expect("metadata mode should not require probe coverage");
    }
}
