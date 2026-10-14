//! Public Vulkan Video adapters backed by private capability, device, pixel, and ordering modules.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use ash::vk;
use vk_video::parameters::{DecoderParameters, RateControl, VideoParameters};
use vk_video::{
    BytesDecoder, BytesEncoder, EncodedInputChunk, Frame as VkFrame, RawFrameData, VulkanDevice,
    VulkanInstance, WgpuTexturesDecoder,
};

use video_hw_core::{
    BackendDecoderOptions, BackendEncoderOptions, BackendError, CapabilityContract,
    CapabilityReport, Codec, ColorConvertOptions, ColorMatrix, ColorRange, DecodeCapability,
    DecodeOutputCapability, DecodeOutputMode, DecodeOutputOrigin, DecodeSummary, DecodedFrame,
    DecoderConfig, DimensionConstraints, Dimensions, EncodeCapability, EncodeInputFormat,
    EncodedContentKind, EncodedLayout, EncodedPacket, FallbackPolicy, Frame, Nv12FramePayload,
    PixelOutputLayout, RuntimeCapability, RuntimeStatus, StreamingMode, VideoDecoder, VideoEncoder,
    VulkanDecoderOptions, VulkanEncoderOptions,
};

use crate::{
    vulkan_av1_decode::{
        Av1DecodeCommandVisit, Av1DecodePictureViews, Av1DecodePrerequisiteProbe,
        av1_display_readback_indices, build_av1_aligned_key_frame_decode_command_skeleton,
        build_av1_decode_info_skeleton, build_av1_decode_info_skeletons,
        build_av1_decode_picture_info_skeleton, build_av1_decode_submit_skeleton,
        build_av1_key_frame_decode_command_skeleton, count_av1_display_frames,
        decode_av1_bitstream_to_nv12_frames, extract_av1_std_sequence_header,
        inspect_av1_low_overhead_obus, probe_av1_decode_prerequisites,
        probe_av1_decode_session_parameters_for_bitstream, submit_av1_bitstream_without_readback,
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
/// A Vulkan physical adapter and its reported video queue support.
pub struct VulkanAdapterReport {
    /// Stable index within the result of [`vulkan_adapter_reports`].
    pub index: usize,
    /// Driver-provided adapter name.
    pub name: String,
    /// PCI vendor identifier reported by Vulkan.
    pub vendor_id: u32,
    /// Device identifier reported by Vulkan.
    pub device_id: u32,
    /// Whether the adapter exposes a supported Vulkan Video decode path.
    pub supports_decoding: bool,
    /// Whether the adapter exposes a supported Vulkan Video encode path.
    pub supports_encoding: bool,
}

/// Enumerates Vulkan adapters and their video queue support.
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

/// Buffered Vulkan Video decoder implementing [`VideoDecoder`].
pub struct VulkanDecoderAdapter {
    config: DecoderConfig,
    options: VulkanDecoderOptions,
    pending_bitstream: Vec<u8>,
    next_pts_90k: i64,
    last_summary: DecodeSummary,
}

impl VulkanDecoderAdapter {
    /// Creates a decoder from the backend-neutral decoder configuration.
    pub fn new(config: DecoderConfig) -> Self {
        let options = match &config.backend_options {
            BackendDecoderOptions::Vulkan(options) => options.clone(),
            BackendDecoderOptions::Default
            | BackendDecoderOptions::VideoToolbox(_)
            | BackendDecoderOptions::Nvidia(_)
            | BackendDecoderOptions::Intel(_)
            | BackendDecoderOptions::Android(_) => VulkanDecoderOptions::default(),
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
        let hevc_nv12_frames = if metadata_only {
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
                        hevc_probe_readback_to_nv12_payload(output_format, sample, width, height)
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
            .map(|(index, access_unit)| -> Result<_, BackendError> {
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
                if let Some(nv12_frames) = hevc_nv12_frames.as_ref() {
                    let nv12 = &nv12_frames[index];
                    match self.config.output_mode {
                        DecodeOutputMode::Metadata => {}
                        DecodeOutputMode::Nv12 => {
                            frame.pixel_format = Some(u32::from_le_bytes(*b"NV12"));
                            frame.nv12 = Some(nv12.clone());
                        }
                        DecodeOutputMode::Rgb24 => {
                            frame.rgb24 = Some(nv12_bytes_to_pixels(
                                &nv12.data,
                                width,
                                height,
                                nv12.pitch,
                                PixelOutputLayout::Rgb24,
                            )?);
                        }
                    }
                }
                frame.force_keyframe = is_idr;
                Ok((poc, frame))
            })
            .collect::<Result<_, _>>()?;
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
        if matches!(self.config.output_mode, DecodeOutputMode::Metadata) {
            submit_av1_bitstream_without_readback(bitstream).map_err(|err| {
                BackendError::UnsupportedConfig(format!(
                    "Vulkan AV1 decode submit failed: {err}; {}",
                    av1_decode_blocker_message_with_bitstream(bitstream)
                ))
            })?;
            let display_frame_count = count_av1_display_frames(bitstream).map_err(|err| {
                BackendError::UnsupportedConfig(format!(
                    "Vulkan AV1 decode could not count display frames: {err}; {}",
                    av1_decode_blocker_message_with_bitstream(bitstream)
                ))
            })?;
            let Some(first_decode) = decodes.first() else {
                return Ok(Vec::new());
            };
            let width = usize::try_from(first_decode.coded_width).map_err(|_| {
                BackendError::InvalidInput("decoded AV1 width does not fit in usize".to_string())
            })?;
            let height = usize::try_from(first_decode.coded_height).map_err(|_| {
                BackendError::InvalidInput("decoded AV1 height does not fit in usize".to_string())
            })?;
            let pts_step = decode_pts_step(self.config.fps);
            return (0..display_frame_count)
                .map(|index| {
                    let pts = self
                        .next_pts_90k
                        .saturating_add(usize_to_i64(index).saturating_mul(pts_step));
                    let mut frame = metadata_only_frame(width, height, Some(pts));
                    frame.decode_info_flags = Some(0);
                    Ok(frame)
                })
                .collect();
        }
        let display_readback_indices = av1_display_readback_indices(bitstream).map_err(|err| {
            BackendError::UnsupportedConfig(format!(
                "Vulkan AV1 decode could not map display frames: {err}; {}",
                av1_decode_blocker_message_with_bitstream(bitstream)
            ))
        })?;
        let readbacks = decode_av1_bitstream_to_nv12_frames(bitstream).map_err(|err| {
            BackendError::UnsupportedConfig(format!(
                "Vulkan AV1 decode submit/readback failed: {err}; {}",
                av1_decode_blocker_message_with_bitstream(bitstream)
            ))
        })?;
        let required_readback_count = display_readback_indices
            .iter()
            .copied()
            .map(|index| {
                usize::try_from(index)
                    .unwrap_or(usize::MAX)
                    .saturating_add(1)
            })
            .max()
            .unwrap_or(0);
        if readbacks.len() < required_readback_count {
            return Err(BackendError::UnsupportedConfig(format!(
                "Vulkan AV1 decode readback frame count is too small: got {}, need {}; {}",
                readbacks.len(),
                required_readback_count,
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
        display_readback_indices
            .into_iter()
            .enumerate()
            .map(|(index, readback_index)| {
                let readback = readbacks
                    .get(usize::try_from(readback_index).map_err(|_| {
                        BackendError::InvalidInput(
                            "AV1 display readback index does not fit usize".to_string(),
                        )
                    })?)
                    .ok_or_else(|| {
                        BackendError::UnsupportedConfig(format!(
                            "Vulkan AV1 display readback index {readback_index} is out of range for {} samples",
                            readbacks.len()
                        ))
                    })?;
                let pts = self
                    .next_pts_90k
                    .saturating_add(usize_to_i64(index).saturating_mul(pts_step));
                let mut frame = metadata_only_frame(width, height, Some(pts));
                frame.decode_info_flags = Some(if readback.readback_non_zero { 1 } else { 0 });
                match self.config.output_mode {
                    DecodeOutputMode::Metadata => {}
                    DecodeOutputMode::Nv12 => {
                        frame.pixel_format = Some(u32::from_le_bytes(*b"NV12"));
                        frame.nv12 = Some(av1_readback_to_nv12_payload(
                            &readback.data,
                            width,
                            height,
                        )?);
                        frame.force_keyframe = true;
                    }
                    DecodeOutputMode::Rgb24 => {
                        let nv12 =
                            av1_readback_to_nv12_payload(&readback.data, width, height)?;
                        frame.rgb24 = Some(nv12_bytes_to_pixels(
                            &nv12.data,
                            width,
                            height,
                            nv12.pitch,
                            PixelOutputLayout::Rgb24,
                        )?);
                        frame.force_keyframe = true;
                    }
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

/// Buffered Vulkan Video encoder implementing [`VideoEncoder`].
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
    /// Creates an encoder using Vulkan-specific options from `backend_options` when present.
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
            | BackendEncoderOptions::Intel(_)
            | BackendEncoderOptions::Android(_) => VulkanEncoderOptions::default(),
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
            let input_pts = frame
                .pts_90k
                .map(|pts| {
                    u64::try_from(pts).map_err(|_| {
                        BackendError::InvalidInput(format!(
                            "Vulkan encoder timestamps must be non-negative, got {pts}"
                        ))
                    })
                })
                .transpose()?;
            let encoded = encoder
                .encode(
                    &VkFrame {
                        data: raw,
                        pts: input_pts,
                    },
                    frame.force_keyframe,
                )
                .map_err(|err| {
                    BackendError::UnsupportedConfig(format!("Vulkan encode failed: {err}"))
                })?;
            if encoded.data.is_empty() {
                continue;
            }
            let pts_90k = encoded
                .pts
                .map(|pts| {
                    i64::try_from(pts).map_err(|_| {
                        BackendError::Backend(format!(
                            "Vulkan encoder returned timestamp outside i64 range: {pts}"
                        ))
                    })
                })
                .transpose()?;
            packets.push(EncodedPacket {
                codec: self.codec,
                layout: EncodedLayout::AnnexB,
                kind: EncodedContentKind::Frame,
                data: encoded.data,
                pts_90k,
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
        if encoded_frames.len() != pts_values.len() {
            return Err(BackendError::Backend(format!(
                "Vulkan HEVC encoder output count mismatch: encoded={}, submitted={}",
                encoded_frames.len(),
                pts_values.len()
            )));
        }
        let packets = encoded_frames
            .into_iter()
            .zip(pts_values)
            .map(|(data, pts_90k)| EncodedPacket {
                codec: self.codec,
                layout: EncodedLayout::AnnexB,
                kind: EncodedContentKind::Frame,
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
        validate_vulkan_encode_frame(&frame, self.width.zip(self.height), self.fps)?;
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

mod capability;
mod device;
mod ordering;
mod pixel;

#[cfg(test)]
mod tests;

use capability::*;
use device::*;
use ordering::*;
use pixel::*;
