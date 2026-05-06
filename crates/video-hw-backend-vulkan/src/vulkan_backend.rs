use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use ash::vk;
use vk_video::parameters::{DecoderParameters, RateControl, VideoParameters};
use vk_video::{
    BytesDecoder, BytesEncoder, EncodedInputChunk, Frame as VkFrame, RawFrameData, VulkanDevice,
    VulkanInstance, WgpuTexturesDecoder,
};

use crate::{
    BackendDecoderOptions, BackendEncoderOptions, BackendError, CapabilityReport, Codec,
    DecodeOutputMode, DecodeSummary, DecoderConfig, EncodedPacket, Frame, Nv12Frame,
    Nv12FramePayload, VideoDecoder, VideoEncoder, VulkanDecoderOptions, VulkanEncoderOptions,
    argb_to_nv12, nv12_to_rgb24,
    vulkan_av1_decode::{
        Av1DecodeCommandVisit, Av1DecodePictureViews, Av1DecodePrerequisiteProbe,
        build_av1_aligned_key_frame_decode_command_skeleton, build_av1_decode_info_skeleton,
        build_av1_decode_info_skeletons, build_av1_decode_picture_info_skeleton,
        build_av1_decode_submit_skeleton, build_av1_key_frame_decode_command_skeleton,
        decode_av1_bitstream_to_nv12_frames, extract_av1_std_sequence_header,
        inspect_av1_low_overhead_obus, probe_av1_decode_prerequisites,
        probe_av1_decode_session_parameters_for_bitstream,
    },
    vulkan_hevc_decode::{
        HevcDecodePrerequisiteProbe, HevcDecodeSubmitExecutionProbe, HevcDecodeSubmitSkeletonProbe,
        HevcVideoSessionCreateProbe, HevcVideoSessionParametersCreateProbe,
        extract_hevc_access_unit_headers, extract_hevc_parameter_sets_annexb,
        probe_hevc_decode_prerequisites,
        probe_hevc_decode_session_bootstrap_with_access_unit_limit_and_physical_device_index,
    },
    vulkan_hevc_encode::{
        HevcEncodePrerequisiteProbe, encode_hevc_idr_frames_annexb, probe_hevc_encode_prerequisites,
    },
};

const HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG: u32 = 1;
const HEVC_DECODE_INFO_SLOT_SHIFT: u32 = 1;
const HEVC_DECODE_INFO_PROBE_COVERED_FLAG: u32 = 1 << 31;

#[derive(Debug, Clone)]
pub struct VulkanAdapterReport {
    pub index: usize,
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub supports_decoding: bool,
    pub supports_encoding: bool,
}

pub fn vulkan_adapter_reports() -> Result<Vec<VulkanAdapterReport>, BackendError> {
    let instance = VulkanInstance::new().map_err(|err| {
        BackendError::UnsupportedConfig(format!("failed to initialize Vulkan: {err}"))
    })?;
    let adapters = instance.iter_adapters(None).map_err(|err| {
        BackendError::UnsupportedConfig(format!("failed to enumerate Vulkan adapters: {err}"))
    })?;
    Ok(adapters
        .enumerate()
        .map(|(index, adapter)| {
            let info = adapter.info();
            VulkanAdapterReport {
                index,
                name: info.name.clone(),
                vendor_id: info.device_properties.vendor_id,
                device_id: info.device_properties.device_id,
                supports_decoding: adapter.supports_decoding(),
                supports_encoding: adapter.supports_encoding(),
            }
        })
        .collect())
}

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
            | BackendDecoderOptions::VideoToolbox(_)
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
        if matches!(self.config.codec, Codec::Av1) {
            return self.decode_pending_av1_bitstream(bitstream);
        }
        if matches!(self.config.output_mode, DecodeOutputMode::Metadata) {
            return self.decode_pending_h264_metadata(bitstream);
        }
        ensure_vk_codec_supported(self.config.codec, "decode")?;
        let mut decoder = create_vk_bytes_decoder(self.options.adapter_index).map_err(|err| {
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

    fn decode_pending_h264_metadata(&self, bitstream: &[u8]) -> Result<Vec<Frame>, BackendError> {
        ensure_vk_codec_supported(self.config.codec, "decode")?;
        let mut decoder =
            create_vk_wgpu_textures_decoder(self.options.adapter_index).map_err(|err| {
                if !self.config.require_hardware
                    && self.options.allow_software_fallback.unwrap_or(true)
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
                BackendError::UnsupportedConfig(format!("Vulkan metadata decode failed: {err}"))
            })?;
        decoded.extend(decoder.flush().map_err(|err| {
            BackendError::UnsupportedConfig(format!("Vulkan metadata decode flush failed: {err}"))
        })?);
        let pts_step = decode_pts_step(self.config.fps);
        decoded
            .into_iter()
            .enumerate()
            .map(|(index, decoded_frame)| {
                Ok(metadata_only_frame(
                    decoded_frame.data.width() as usize,
                    decoded_frame.data.height() as usize,
                    Some(
                        self.next_pts_90k
                            .saturating_add(usize_to_i64(index).saturating_mul(pts_step)),
                    ),
                ))
            })
            .collect()
    }

    fn decode_pending_hevc_bitstream(&self, bitstream: &[u8]) -> Result<Vec<Frame>, BackendError> {
        let metadata_only = matches!(self.config.output_mode, DecodeOutputMode::Metadata);
        let access_unit_headers = extract_hevc_access_unit_headers(bitstream).map_err(|err| {
            BackendError::UnsupportedConfig(format!(
                "Vulkan HEVC decode could not extract access units: {err}"
            ))
        })?;
        let submit_probe_access_unit_limit = (!metadata_only).then_some(access_unit_headers.len());
        let physical_device_index = resolve_hevc_decode_physical_device_index();
        let bootstrap =
            probe_hevc_decode_session_bootstrap_with_access_unit_limit_and_physical_device_index(
                bitstream,
                submit_probe_access_unit_limit,
                physical_device_index,
            )
            .or_else(|_| {
                probe_hevc_decode_session_bootstrap_with_access_unit_limit_and_physical_device_index(
                    bitstream,
                    submit_probe_access_unit_limit,
                    physical_device_index,
                )
            })
            .map_err(|_| {
                BackendError::UnsupportedConfig(hevc_decode_blocker_message_with_bitstream(
                    bitstream,
                    physical_device_index,
                ))
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
                    hevc_decode_blocker_message_with_bitstream(bitstream, physical_device_index)
                )));
            }
            HevcDecodeSubmitExecutionProbe::Skipped(reason) => {
                return Err(BackendError::UnsupportedConfig(format!(
                    "Vulkan HEVC decode submit execution was skipped: {reason}; {}",
                    hevc_decode_blocker_message_with_bitstream(bitstream, physical_device_index)
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
        // Build frames paired with their full picture order count for display-order sort.
        let frames_with_poc: Vec<(i32, Frame)> = access_unit_headers
            .into_iter()
            .take(frame_count)
            .enumerate()
            .map(|(index, access_unit)| {
                let poc = access_unit.poc_full;
                let is_idr = access_unit.nal_unit_type == 19 || access_unit.nal_unit_type == 20;
                if is_idr {
                    next_slot = 0;
                }
                let slot = next_slot % dpb_slot_count;
                next_slot = next_slot.saturating_add(1);
                // PTS is assigned after POC sort so output uses display-order PTS.
                let mut frame = metadata_only_frame(width, height, None);
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
                (poc, frame)
            })
            .collect();
        // Sort by full picture order count for display-order output.
        Ok(sort_hevc_display_order_frames(
            frames_with_poc,
            self.next_pts_90k,
            pts_step,
        ))
    }

    fn decode_pending_av1_bitstream(&self, bitstream: &[u8]) -> Result<Vec<Frame>, BackendError> {
        let decodes = build_av1_decode_info_skeletons(bitstream).map_err(|err| {
            BackendError::UnsupportedConfig(format!(
                "Vulkan AV1 decode could not build decode-info skeletons: {err}; {}",
                av1_decode_blocker_message_with_bitstream(bitstream)
            ))
        })?;
        let readbacks = decode_av1_bitstream_to_nv12_frames(bitstream).map_err(|err| {
            BackendError::UnsupportedConfig(format!(
                "Vulkan AV1 decode submit/readback failed: {err}; {}",
                av1_decode_blocker_message_with_bitstream(bitstream)
            ))
        })?;
        if readbacks.len() < decodes.len() {
            return Err(BackendError::UnsupportedConfig(format!(
                "Vulkan AV1 decode readback frame count is too small: got {}, need {}; {}",
                readbacks.len(),
                decodes.len(),
                av1_decode_blocker_message_with_bitstream(bitstream)
            )));
        }
        let Some(first_readback) = readbacks.first() else {
            return Ok(Vec::new());
        };
        let width = usize::try_from(first_readback.coded_width).map_err(|_| {
            BackendError::InvalidInput("decoded AV1 width does not fit in usize".to_string())
        })?;
        let height = usize::try_from(first_readback.coded_height).map_err(|_| {
            BackendError::InvalidInput("decoded AV1 height does not fit in usize".to_string())
        })?;
        let pts_step = decode_pts_step(self.config.fps);
        readbacks
            .into_iter()
            .take(decodes.len())
            .enumerate()
            .map(|(index, readback)| {
                let pts = self
                    .next_pts_90k
                    .saturating_add(usize_to_i64(index).saturating_mul(pts_step));
                let mut frame = metadata_only_frame(width, height, Some(pts));
                frame.decode_info_flags = Some(if readback.readback_non_zero { 1 } else { 0 });
                if !matches!(self.config.output_mode, DecodeOutputMode::Metadata) {
                    frame.pixel_format = Some(u32::from_le_bytes(*b"NV12"));
                    frame.nv12 = Some(av1_readback_to_nv12_payload(&readback.data, width, height)?);
                    frame.force_keyframe = true;
                }
                Ok(frame)
            })
            .collect()
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
            | BackendEncoderOptions::VideoToolbox(_)
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
            return self.encode_pending_hevc_frames(pending_frames, width, height);
        }
        ensure_vk_codec_supported(self.codec, "encode")?;

        let mut encoder =
            create_vk_bytes_encoder(width, height, self.fps, self.options.adapter_index).map_err(
                |err| {
                    if !self.require_hardware
                        && self.options.allow_software_fallback.unwrap_or(true)
                    {
                        BackendError::UnsupportedConfig(format!(
                            "{err}; software fallback is not available in direct Vulkan backend"
                        ))
                    } else {
                        err
                    }
                },
            )?;

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

    fn encode_pending_hevc_frames(
        &self,
        pending_frames: &[Frame],
        width: usize,
        height: usize,
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        let coded_width = u32::try_from(width).map_err(|_| {
            BackendError::InvalidInput("frame width does not fit in u32".to_string())
        })?;
        let coded_height = u32::try_from(height).map_err(|_| {
            BackendError::InvalidInput("frame height does not fit in u32".to_string())
        })?;
        let fps = u32::try_from(self.fps.max(1)).unwrap_or(30);
        let mut source_frames = Vec::with_capacity(pending_frames.len());
        let mut pts_values = Vec::with_capacity(pending_frames.len());
        for frame in pending_frames {
            if frame.width != width || frame.height != height {
                return Err(BackendError::InvalidInput(format!(
                    "mixed frame dimensions are unsupported: expected {}x{}, got {}x{}",
                    width, height, frame.width, frame.height
                )));
            }
            let raw = frame_to_nv12_payload(frame, width, height)?;
            source_frames.push(raw.frame);
            pts_values.push(frame.pts_90k);
        }
        let encoded_frames = encode_hevc_idr_frames_annexb(
            coded_width,
            coded_height,
            fps,
            &source_frames,
        )
        .map_err(|err| {
            BackendError::UnsupportedConfig(format!(
                "{}; experimental HEVC IDR encode failed: {err}",
                hevc_encode_blocker_message_with_config(coded_width, coded_height, self.fps)
            ))
        })?;
        let packets = encoded_frames
            .into_iter()
            .zip(pts_values)
            .map(|(data, pts_90k)| EncodedPacket {
                codec: self.codec,
                data,
                pts_90k,
                is_keyframe: true,
            })
            .collect::<Vec<_>>();
        if packets.is_empty() {
            return Err(BackendError::Backend(
                "Vulkan HEVC encoder produced no output packets".to_string(),
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
        Codec::Av1 => {
            let message = if operation == "decode" {
                av1_decode_blocker_message()
            } else {
                av1_encode_blocker_message()
            };
            Err(BackendError::UnsupportedConfig(message))
        }
    }
}

fn av1_decode_blocker_message() -> String {
    let base = "Vulkan AV1 decode initialization failed";
    match probe_av1_decode_prerequisites() {
        Av1DecodePrerequisiteProbe::Ready => format!(
            "{base}; runtime prerequisites are present, but AV1 session bootstrap/decode submit is not implemented in video-hw yet"
        ),
        Av1DecodePrerequisiteProbe::MissingExtensions { missing } => {
            format!("{base}; missing Vulkan extensions: {}", missing.join(", "))
        }
        Av1DecodePrerequisiteProbe::MissingDecodeQueueFamily => {
            format!("{base}; no queue family advertises VIDEO_DECODE_KHR with AV1 decode operation")
        }
        Av1DecodePrerequisiteProbe::SessionBootstrapFailed(details) => {
            format!("{base}; AV1 decode session bootstrap failed: {details}")
        }
        Av1DecodePrerequisiteProbe::NoCompatibleAdapter => format!(
            "{base}; required extensions were observed but not on a single adapter that can run AV1 decode prerequisites"
        ),
        Av1DecodePrerequisiteProbe::ProbeUnavailable(details) => {
            format!("{base}; extension probe failed: {details}")
        }
    }
}

fn av1_decode_blocker_message_with_bitstream(bitstream: &[u8]) -> String {
    let mut message = av1_decode_blocker_message();
    match inspect_av1_low_overhead_obus(bitstream) {
        Ok(inspection) => {
            message.push_str(&format!(
                "; parsed AV1 OBUs: obu_count={}, temporal_units={}, sequence_header={}, frame_payload={}, sequence_header_obu_len={}, coded={}x{}, std_sequence_header={}, bitstream_session_parameters={}, submit_skeleton={}, picture_info_skeleton={}, decode_info_skeleton={}, decode_info_count={}, command_skeleton={}",
                inspection.obu_count,
                inspection.temporal_unit_count,
                inspection.has_sequence_header,
                inspection.has_frame_payload,
                inspection
                    .sequence_header_obu_len
                    .map_or_else(|| "none".to_string(), |len| len.to_string()),
                inspection
                    .coded_width
                    .map_or_else(|| "unknown".to_string(), |width| width.to_string()),
                inspection
                    .coded_height
                    .map_or_else(|| "unknown".to_string(), |height| height.to_string()),
                extract_av1_std_sequence_header(bitstream)
                    .map(|_| "ready".to_string())
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                probe_av1_decode_session_parameters_for_bitstream(bitstream)
                    .map(|probe| {
                        format!(
                            "ready(coded={}x{}, picture_format={:?}, offset_align={}, size_align={}, upload_bytes={}, decode_image_layers={}, decode_image_barrier_layers={}, readback_bytes={}, readback_regions={}, command_record_decodes={}, command_buffer_recorded={}, command_buffer_submitted={}, session_memory_count={}, session_memory_bound={}, session_memory_total={}, session_memory_max_align={})",
                            probe.coded_width,
                            probe.coded_height,
                            probe.picture_format,
                            probe.min_bitstream_buffer_offset_alignment,
                            probe.min_bitstream_buffer_size_alignment,
                            probe.bitstream_upload_bytes,
                            probe.decode_image_layers,
                            probe.decode_image_barrier_layers,
                            probe.readback_bytes,
                            probe.readback_region_count,
                            probe.command_record_decode_count,
                            probe.command_buffer_recorded,
                            probe.command_buffer_submitted,
                            probe.session_memory_requirement_count,
                            probe.session_memory_bound_count,
                            probe.session_memory_total_size,
                            probe.session_memory_max_alignment
                        )
                    })
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                build_av1_decode_submit_skeleton(bitstream)
                    .map(|skeleton| {
                        format!(
                            "ready(tu={}, frame_header_offset={}, tile_count={})",
                            skeleton.temporal_unit_index,
                            skeleton.frame_header_offset,
                            skeleton.tile_offsets.len()
                        )
                    })
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                build_av1_decode_submit_skeleton(bitstream)
                    .and_then(|skeleton| build_av1_decode_picture_info_skeleton(&skeleton))
                    .map(|picture| {
                        format!(
                            "ready(frame_type={:?}, frame_header_offset={}, vk_frame_header_offset={}, tile_count={}, tile_bytes={}, reference_slots={})",
                            picture.std_picture_info.frame_type,
                            picture.frame_header_offset,
                            picture.vk_picture_info().frame_header_offset,
                            picture.tile_offsets.len(),
                            picture.tile_sizes.iter().copied().sum::<u32>(),
                            picture.reference_name_slot_indices.len()
                        )
                    })
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                build_av1_decode_info_skeleton(bitstream)
                    .map(|decode| {
                        let mut av1_picture_info = decode.picture_info.vk_picture_info();
                        let dst_picture_resource =
                            decode.dst_picture_resource(vk::ImageView::null(), 0);
                        let mut setup_dpb_info = decode.vk_setup_dpb_slot_info();
                        let setup_reference_slot = decode.vk_setup_reference_slot(
                            0,
                            &dst_picture_resource,
                            &mut setup_dpb_info,
                        );
                        let vk_decode_info = decode.vk_decode_info_with_setup_reference_slot(
                            vk::Buffer::null(),
                            dst_picture_resource,
                            &setup_reference_slot,
                            &mut av1_picture_info,
                        );
                        format!(
                            "ready(src_offset={}, src_range={}, vk_src_offset={}, vk_src_range={}, coded={}x{}, dst_extent={}x{}, setup_slot={}, setup_slot_chained={}, frame_header_offset={}, tile_count={})",
                            decode.src_buffer_offset,
                            decode.src_buffer_range,
                            vk_decode_info.src_buffer_offset,
                            vk_decode_info.src_buffer_range,
                            decode.coded_width,
                            decode.coded_height,
                            vk_decode_info.dst_picture_resource.coded_extent.width,
                            vk_decode_info.dst_picture_resource.coded_extent.height,
                            setup_reference_slot.slot_index,
                            !vk_decode_info.p_setup_reference_slot.is_null(),
                            decode.picture_info.frame_header_offset,
                            decode.tile_count()
                        )
                    })
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                build_av1_decode_info_skeletons(bitstream)
                    .map(|decodes| decodes.len().to_string())
                    .unwrap_or_else(|err| format!("unavailable({err})")),
                build_av1_key_frame_decode_command_skeleton(bitstream, 4)
                    .map(|command| {
                        let begin_resources =
                            command.begin_picture_resources(vk::ImageView::null());
                        let frame_resources = command
                            .frame_picture_resources(vk::ImageView::null())
                            .unwrap_or_default();
                        let frame_bundles = command.frame_record_bundles().unwrap_or_default();
                        let image_plan = command
                            .decode_image_plan(vk::Format::G8_B8R8_2PLANE_420_UNORM)
                            .ok();
                        let image_layers = image_plan
                            .as_ref()
                            .map(|plan| plan.array_layers)
                            .unwrap_or_default();
                        let image_usage = image_plan
                            .as_ref()
                            .map(|plan| plan.usage)
                            .unwrap_or_default();
                        let image_create_layers = image_plan
                            .as_ref()
                            .map(|plan| command.vk_decode_image_create_info(plan).array_layers)
                            .unwrap_or_default();
                        let begin_reference_infos = command.begin_std_reference_infos();
                        let mut begin_dpb_infos = command
                            .begin_dpb_slot_infos(&begin_reference_infos)
                            .unwrap_or_default();
                        let begin_reference_slots = command
                            .begin_reference_slots(&begin_resources, &mut begin_dpb_infos)
                            .unwrap_or_default();
                        let begin_coding_info = command
                            .vk_begin_coding_info(
                                vk::VideoSessionKHR::null(),
                                vk::VideoSessionParametersKHR::null(),
                                &begin_reference_slots,
                            )
                            .unwrap_or_default();
                        let reset_control = command.vk_reset_coding_control_info();
                        let end_coding_info = command.vk_end_coding_info();
                        let record_steps = command.record_steps();
                        format!(
                            "ready(frames={}, coded={}x{}, begin_slots={}, begin_resources={}, frame_resources={}, frame_bundles={}, image_layers={}, image_create_layers={}, image_usage={:?}, begin_reference_slots={}, vk_begin_refs={}, reset={}, end_flags_empty={}, record_steps={}, slots={})",
                            command.frames.len(),
                            command.coded_width,
                            command.coded_height,
                            command.begin_slots.len(),
                            begin_resources.len(),
                            frame_resources.len(),
                            frame_bundles.len(),
                            image_layers,
                            image_create_layers,
                            image_usage,
                            begin_reference_slots.len(),
                            begin_coding_info.reference_slot_count,
                            reset_control.flags.contains(vk::VideoCodingControlFlagsKHR::RESET),
                            end_coding_info.flags.is_empty(),
                            record_steps.len(),
                            command
                                .frames
                                .iter()
                                .map(|frame| frame.setup_slot_index.to_string())
                                .collect::<Vec<_>>()
                                .join("/")
                        )
                    })
                    .and_then(|command_status| {
                        let (offset_alignment, size_alignment, alignment_source) =
                            probe_av1_decode_session_parameters_for_bitstream(bitstream)
                                .map(|probe| {
                                    (
                                        probe.min_bitstream_buffer_offset_alignment,
                                        probe.min_bitstream_buffer_size_alignment,
                                        "capability",
                                    )
                                })
                                .unwrap_or((4096, 4096, "fallback"));
                        build_av1_aligned_key_frame_decode_command_skeleton(
                            bitstream,
                            4,
                            offset_alignment,
                            size_alignment,
                        )
                            .map(|(upload_plan, aligned_command)| {
                                let submit_bundles = upload_plan
                                    .frame_submit_bundles(&aligned_command)
                                    .unwrap_or_default();
                                let first_decode_info_ready = upload_plan
                                    .with_frame_decode_info(
                                        &aligned_command,
                                        0,
                                        vk::Buffer::null(),
                                        Av1DecodePictureViews {
                                            dst: vk::ImageView::null(),
                                            reference: vk::ImageView::null(),
                                        },
                                        |decode_info, _bundle| {
                                            !decode_info.p_next.is_null()
                                                && !decode_info.p_setup_reference_slot.is_null()
                                        },
                                    )
                                    .unwrap_or(false);
                                let decode_info_loop_count = upload_plan
                                    .with_frame_decode_infos(
                                        &aligned_command,
                                        vk::Buffer::null(),
                                        Av1DecodePictureViews {
                                            dst: vk::ImageView::null(),
                                            reference: vk::ImageView::null(),
                                        },
                                        |_decode_info, bundle| bundle.frame_index,
                                    )
                                    .map(|indices| indices.len())
                                    .unwrap_or_default();
                                let mut sequence_visits = 0usize;
                                let mut sequence_decodes = 0usize;
                                let mut sequence_begin_refs = 0u32;
                                let mut sequence_reset = false;
                                let mut sequence_first_decode_offset = 0u64;
                                let mut sequence_end_empty = false;
                                let sequence_ready = upload_plan
                                    .visit_decode_command_sequence(
                                        &aligned_command,
                                        vk::VideoSessionKHR::null(),
                                        vk::VideoSessionParametersKHR::null(),
                                        vk::Buffer::null(),
                                        Av1DecodePictureViews {
                                            dst: vk::ImageView::null(),
                                            reference: vk::ImageView::null(),
                                        },
                                        |visit| {
                                            sequence_visits += 1;
                                            match visit {
                                                Av1DecodeCommandVisit::BeginCoding(info) => {
                                                    sequence_begin_refs =
                                                        info.reference_slot_count;
                                                }
                                                Av1DecodeCommandVisit::ResetCoding(info) => {
                                                    sequence_reset = info.flags.contains(
                                                        vk::VideoCodingControlFlagsKHR::RESET,
                                                    );
                                                }
                                                Av1DecodeCommandVisit::DecodeFrame {
                                                    decode_info,
                                                    bundle,
                                                } => {
                                                    sequence_decodes += 1;
                                                    if bundle.frame_index == 0 {
                                                        sequence_first_decode_offset =
                                                            decode_info.src_buffer_offset;
                                                    }
                                                }
                                                Av1DecodeCommandVisit::EndCoding(info) => {
                                                    sequence_end_empty = info.flags.is_empty();
                                                }
                                            }
                                        },
                                    )
                                    .is_ok();
                                let record_summary = upload_plan
                                    .record_decode_command_sequence(
                                        &aligned_command,
                                        vk::VideoSessionKHR::null(),
                                        vk::VideoSessionParametersKHR::null(),
                                        vk::Buffer::null(),
                                        Av1DecodePictureViews {
                                            dst: vk::ImageView::null(),
                                            reference: vk::ImageView::null(),
                                        },
                                        |_visit| Ok(()),
                                    )
                                    .ok();
                                let record_summary_valid = record_summary
                                    .as_ref()
                                    .is_some_and(|summary| {
                                        summary.validate_for_command(&aligned_command).is_ok()
                                    });
                                format!(
                                    "{command_status}; aligned_upload=ready(bytes={}, frames={}, submit_bundles={}, first_decode_info={}, decode_info_loop={}, command_sequence={}, record_summary={}, record_summary_valid={}, sequence_visits={}, sequence_decodes={}, sequence_begin_refs={}, sequence_reset={}, sequence_first_decode_offset={}, sequence_end_empty={}, offset_align={}, size_align={}, align_source={}, first_offset={}, first_range={})",
                                    upload_plan.bytes.len(),
                                    aligned_command.frames.len(),
                                    submit_bundles.len(),
                                    first_decode_info_ready,
                                    decode_info_loop_count,
                                    sequence_ready,
                                    record_summary
                                        .as_ref()
                                        .map(|summary| format!(
                                            "{}/{}/{}/{}",
                                            summary.begin_count,
                                            summary.reset_count,
                                            summary.decode_count,
                                            summary.end_count
                                        ))
                                        .unwrap_or_else(|| "unavailable".to_string()),
                                    record_summary_valid,
                                    sequence_visits,
                                    sequence_decodes,
                                    sequence_begin_refs,
                                    sequence_reset,
                                    sequence_first_decode_offset,
                                    sequence_end_empty,
                                    offset_alignment,
                                    size_alignment,
                                    alignment_source,
                                    aligned_command
                                        .frames
                                        .first()
                                        .map(|frame| frame.src_buffer_offset)
                                        .unwrap_or_default(),
                                    aligned_command
                                        .frames
                                        .first()
                                        .map(|frame| frame.src_buffer_range)
                                        .unwrap_or_default()
                                )
                            })
                    })
                    .unwrap_or_else(|err| format!("unavailable({err})"))
            ));
        }
        Err(err) => {
            message.push_str(&format!("; OBU inspection failed: {err}"));
        }
    }
    message
}

fn av1_encode_blocker_message() -> String {
    "Vulkan AV1 encode initialization failed; ash 0.38.0 exposes VK_KHR_video_decode_av1 bindings but not VK_KHR_video_encode_av1, so video-hw cannot implement Vulkan AV1 encode without updating Vulkan bindings"
        .to_string()
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
            format!(
                "{base}; no queue family advertises VIDEO_DECODE_KHR with H.265 decode operation"
            )
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
    _coded_width: u32,
    _coded_height: u32,
    _fps: i32,
) -> String {
    let base = "Vulkan HEVC encode initialization failed";
    match probe_hevc_encode_prerequisites() {
        HevcEncodePrerequisiteProbe::Ready => {
            // The full session bootstrap probe is intentionally skipped: on some drivers it
            // triggers a STATUS_STACK_BUFFER_OVERRUN that cannot be caught.
            format!(
                "{base}; runtime prerequisites are present, but the direct ash-level HEVC encode submit path is not wired yet"
            )
        }
        HevcEncodePrerequisiteProbe::MissingExtensions { missing } => {
            format!("{base}; missing Vulkan extensions: {}", missing.join(", "))
        }
        HevcEncodePrerequisiteProbe::MissingEncodeQueueFamily => {
            format!(
                "{base}; no queue family advertises VIDEO_ENCODE_KHR with H.265 encode operation"
            )
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

fn resolve_hevc_decode_physical_device_index() -> Option<usize> {
    std::env::var("VIDEO_HW_VULKAN_HEVC_DECODE_PHYSICAL_DEVICE_INDEX")
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

fn hevc_decode_blocker_message_with_bitstream(
    bitstream: &[u8],
    physical_device_index: Option<usize>,
) -> String {
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
            match probe_hevc_decode_session_bootstrap_with_access_unit_limit_and_physical_device_index(
                bitstream,
                None,
                physical_device_index,
            ) {
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
        decode_output_modes: if decode_supported {
            vec![
                DecodeOutputMode::Metadata,
                DecodeOutputMode::Nv12,
                DecodeOutputMode::Rgb24,
            ]
        } else {
            Vec::new()
        },
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

fn av1_readback_to_nv12_payload(
    readback_sample: &[u8],
    width: usize,
    height: usize,
) -> Result<Nv12FramePayload, BackendError> {
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan AV1 non-metadata output currently requires even coded extent, got {}x{}",
            width, height
        )));
    }
    let expected_len = width
        .checked_mul(height)
        .and_then(|y| y.checked_add(y / 2))
        .ok_or_else(|| {
            BackendError::UnsupportedConfig(
                "Vulkan AV1 readback size overflow while preparing NV12 payload".to_string(),
            )
        })?;
    if readback_sample.len() < expected_len {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan AV1 readback sample too short for NV12 payload: got {}, need at least {expected_len}",
            readback_sample.len(),
        )));
    }
    Ok(Nv12FramePayload {
        pitch: width,
        data: readback_sample[..expected_len].to_vec(),
    })
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

fn create_vk_bytes_decoder(adapter_index: Option<usize>) -> Result<BytesDecoder, BackendError> {
    let device = create_vk_device(true, false, adapter_index)?;
    device
        .create_bytes_decoder(DecoderParameters::default())
        .map_err(|err| {
            BackendError::UnsupportedConfig(format!("failed to create Vulkan decoder: {err}"))
        })
}

fn create_vk_wgpu_textures_decoder(
    adapter_index: Option<usize>,
) -> Result<WgpuTexturesDecoder, BackendError> {
    let device = create_vk_device(true, false, adapter_index)?;
    device
        .create_wgpu_textures_decoder(DecoderParameters::default())
        .map_err(|err| {
            BackendError::UnsupportedConfig(format!(
                "failed to create Vulkan texture decoder: {err}"
            ))
        })
}

fn create_vk_bytes_encoder(
    width: usize,
    height: usize,
    fps: i32,
    adapter_index: Option<usize>,
) -> Result<BytesEncoder, BackendError> {
    let device = create_vk_device(false, true, adapter_index)?;
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
    adapter_index: Option<usize>,
) -> Result<Arc<VulkanDevice>, BackendError> {
    let instance = VulkanInstance::new().map_err(|err| {
        BackendError::UnsupportedConfig(format!("failed to initialize Vulkan: {err}"))
    })?;
    let adapters = instance.iter_adapters(None).map_err(|err| {
        BackendError::UnsupportedConfig(format!("failed to enumerate Vulkan adapters: {err}"))
    })?;
    let adapter = if let Some(adapter_index) = adapter_index {
        adapters
            .into_iter()
            .enumerate()
            .find_map(|(index, adapter)| (index == adapter_index).then_some(adapter))
            .ok_or_else(|| {
                BackendError::UnsupportedConfig(format!(
                    "Vulkan adapter index {adapter_index} is not available"
                ))
            })
            .and_then(|adapter| {
                let info = adapter.info();
                if require_decode && !adapter.supports_decoding() {
                    return Err(BackendError::UnsupportedConfig(format!(
                        "Vulkan adapter index {adapter_index} ({}) does not support decode",
                        info.name
                    )));
                }
                if require_encode && !adapter.supports_encoding() {
                    return Err(BackendError::UnsupportedConfig(format!(
                        "Vulkan adapter index {adapter_index} ({}) does not support encode",
                        info.name
                    )));
                }
                Ok(adapter)
            })?
    } else {
        adapters
            .into_iter()
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
            })?
    };
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

fn sort_hevc_display_order_frames(
    mut frames_with_poc: Vec<(i32, Frame)>,
    start_pts_90k: i64,
    pts_step: i64,
) -> Vec<Frame> {
    frames_with_poc.sort_by_key(|(poc, _)| *poc);
    frames_with_poc
        .into_iter()
        .enumerate()
        .map(|(display_idx, (_, mut frame))| {
            frame.pts_90k = Some(
                start_pts_90k.saturating_add(usize_to_i64(display_idx).saturating_mul(pts_step)),
            );
            frame
        })
        .collect()
}

fn i64_to_u64(value: i64) -> Option<u64> {
    u64::try_from(value).ok()
}

fn u64_to_i64(value: u64) -> Option<i64> {
    i64::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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
    fn ensure_vk_codec_supported_rejects_av1_with_actionable_message() {
        for operation in ["decode", "encode"] {
            let err = ensure_vk_codec_supported(Codec::Av1, operation)
                .expect_err("AV1 must be rejected until Vulkan AV1 path is implemented");
            match err {
                BackendError::UnsupportedConfig(message) => {
                    assert!(
                        message.contains(&format!("Vulkan AV1 {operation} initialization failed")),
                        "expected Vulkan AV1 implementation message: {message}"
                    );
                }
                other => panic!("unexpected AV1 check error: {other:?}"),
            }
        }
    }

    #[test]
    fn av1_decode_blocker_message_with_bitstream_appends_obu_status() {
        let sequence_header = av1_test_obu(1, &av1_test_sequence_header_payload(320, 180));
        let mut bitstream = av1_test_obu(2, &[]);
        bitstream.extend_from_slice(&sequence_header);
        bitstream.extend_from_slice(&av1_test_obu(6, &[0x11, 0x12]));
        let message = av1_decode_blocker_message_with_bitstream(&bitstream);
        assert!(message.contains("Vulkan AV1 decode initialization failed"));
        assert!(message.contains("parsed AV1 OBUs"));
        assert!(message.contains("sequence_header=true"));
        assert!(message.contains("coded=320x180"));
        assert!(message.contains("aligned_upload=ready"));
        assert!(message.contains("first_decode_info=true"));
        assert!(message.contains("decode_info_loop=1"));
        assert!(message.contains("command_sequence=true"));
        assert!(message.contains("record_summary=1/1/1/1"));
        assert!(message.contains("record_summary_valid=true"));
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
    fn av1_capability_report_never_claims_hardware_acceleration() {
        let report = vulkan_capability_report(Codec::Av1);
        assert!(!report.decode_supported);
        assert!(!report.encode_supported);
        assert!(!report.hardware_acceleration);
    }

    #[test]
    fn hevc_decode_blocker_message_with_bitstream_appends_parameter_set_status() {
        let message = hevc_decode_blocker_message_with_bitstream(&[0_u8, 1, 2, 3], None);
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
    fn av1_readback_to_nv12_payload_uses_expected_even_extent_size() {
        let payload = av1_readback_to_nv12_payload(&[1, 2, 3, 4, 5, 6], 2, 2)
            .expect("2x2 NV12 readback should map to one payload");
        assert_eq!(payload.pitch, 2);
        assert_eq!(payload.data, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn av1_readback_to_nv12_payload_rejects_odd_extent() {
        let err = av1_readback_to_nv12_payload(&[0; 16], 3, 2)
            .expect_err("odd AV1 readback extent should be rejected for facade NV12");
        assert!(matches!(err, BackendError::UnsupportedConfig(_)));
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

    #[test]
    fn hevc_display_order_sorting_assigns_pts_in_display_order() {
        let mut poc30 = metadata_only_frame(2, 2, None);
        poc30.decode_info_flags = Some(30);
        let mut poc10 = metadata_only_frame(2, 2, None);
        poc10.decode_info_flags = Some(10);
        let mut poc20 = metadata_only_frame(2, 2, None);
        poc20.decode_info_flags = Some(20);

        let frames = sort_hevc_display_order_frames(
            vec![(30, poc30), (10, poc10), (20, poc20)],
            9_000,
            3_000,
        );

        let pts_and_markers: Vec<(i64, u32)> = frames
            .into_iter()
            .map(|frame| {
                (
                    frame
                        .pts_90k
                        .expect("display-order frames should receive pts"),
                    frame.decode_info_flags.expect("test marker should be kept"),
                )
            })
            .collect();

        assert_eq!(
            pts_and_markers,
            vec![(9_000, 10), (12_000, 20), (15_000, 30)]
        );
    }

    #[test]
    fn hevc_display_order_sorting_reorders_repository_decode_order_pocs() {
        let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("foreman_cif.h265");
        let bitstream = std::fs::read(sample_path).expect("foreman_cif.h265 should be readable");
        let headers = extract_hevc_access_unit_headers(&bitstream)
            .expect("HEVC access-unit headers should parse");
        let first_gop_pocs = headers
            .iter()
            .take(5)
            .map(|header| header.poc_full)
            .collect::<Vec<_>>();
        assert_eq!(
            first_gop_pocs,
            vec![0, 4, 2, 1, 3],
            "repository sample should exercise display-order sorting"
        );

        let frames_with_poc = first_gop_pocs
            .iter()
            .map(|poc| {
                let mut frame = metadata_only_frame(2, 2, None);
                frame.decode_info_flags = Some(*poc as u32);
                (*poc, frame)
            })
            .collect::<Vec<_>>();
        let frames = sort_hevc_display_order_frames(frames_with_poc, 0, decode_pts_step(30));

        let pocs_and_pts = frames
            .into_iter()
            .map(|frame| {
                (
                    frame.decode_info_flags.expect("poc marker should remain"),
                    frame.pts_90k.expect("display-order PTS should be assigned"),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            pocs_and_pts,
            vec![(0, 0), (1, 3_000), (2, 6_000), (3, 9_000), (4, 12_000)]
        );
    }

    fn av1_test_obu(obu_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![(obu_type << 3) | 0x02];
        out.extend(av1_test_leb128(payload.len()));
        out.extend_from_slice(payload);
        out
    }

    fn av1_test_leb128(mut value: usize) -> Vec<u8> {
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

    fn av1_test_sequence_header_payload(width: u32, height: u32) -> Vec<u8> {
        let width_minus_1 = width.checked_sub(1).expect("width must be positive");
        let height_minus_1 = height.checked_sub(1).expect("height must be positive");
        let width_bits = 32 - width_minus_1.leading_zeros();
        let height_bits = 32 - height_minus_1.leading_zeros();
        let mut writer = Av1TestBitWriter::default();
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

    #[derive(Default)]
    struct Av1TestBitWriter {
        data: Vec<u8>,
        bit_offset: usize,
    }

    impl Av1TestBitWriter {
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
