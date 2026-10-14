use std::collections::VecDeque;
#[cfg(target_os = "android")]
use std::num::NonZeroU32;

use video_hw_core::{
    BackendError, CapabilityReport, Codec, DecodeSummary, DecoderConfig, EncodedPacket,
    EncoderConfig, Frame, VideoDecoder, VideoEncoder,
};

#[cfg(target_os = "android")]
const MAX_BLOCKING_DEQUEUE_TIMEOUT_US: i64 = 50_000;
#[cfg(target_os = "android")]
const MAX_EOS_DRAIN_MISSES: usize = 100;

use crate::capability::android_capability_report;

#[cfg(target_os = "android")]
use crate::ffi::codec::{
    BUFFER_FLAG_CODEC_CONFIG, BUFFER_FLAG_END_OF_STREAM, BUFFER_FLAG_KEY_FRAME, MediaCodec,
    MediaFormat, OutputEvent,
};
#[cfg(target_os = "android")]
use video_hw_core::{
    AndroidDecoderOptions, AndroidEncoderOptions, BackendDecoderOptions, BackendEncoderOptions,
    DecodeOutputMode, Dimensions, EncodedContentKind, Nv12FramePayload, bitstream,
};

#[cfg(target_os = "android")]
pub(crate) fn mime_for_codec(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "video/avc",
        Codec::Hevc => "video/hevc",
        Codec::Av1 => "video/av01",
    }
}

#[derive(Debug)]
/// Android MediaCodec decoder exposed through the common decoder trait.
pub struct AndroidDecoderAdapter {
    #[cfg(target_os = "android")]
    config: DecoderConfig,
    #[cfg(target_os = "android")]
    options: AndroidDecoderOptions,
    ready: VecDeque<Frame>,
    summary: DecodeSummary,
    #[cfg(target_os = "android")]
    codec: Option<MediaCodec>,
    #[cfg(target_os = "android")]
    output_format: AndroidOutputFormat,
    #[cfg(target_os = "android")]
    eos_queued: bool,
    #[cfg(target_os = "android")]
    eos_seen: bool,
}

impl AndroidDecoderAdapter {
    /// Creates a lazily initialized decoder for `config`.
    ///
    /// MediaCodec resources are not acquired until the first input is submitted.
    #[must_use]
    pub fn new(config: DecoderConfig) -> Self {
        #[cfg(target_os = "android")]
        let options = match &config.backend_options {
            BackendDecoderOptions::Android(options) => options.clone(),
            _ => AndroidDecoderOptions::default(),
        };
        #[cfg(not(target_os = "android"))]
        let _ = config;
        Self {
            #[cfg(target_os = "android")]
            config,
            #[cfg(target_os = "android")]
            options,
            ready: VecDeque::new(),
            summary: DecodeSummary {
                decoded_frames: 0,
                width: None,
                height: None,
                pixel_format: None,
            },
            #[cfg(target_os = "android")]
            codec: None,
            #[cfg(target_os = "android")]
            output_format: AndroidOutputFormat::default(),
            #[cfg(target_os = "android")]
            eos_queued: false,
            #[cfg(target_os = "android")]
            eos_seen: false,
        }
    }
}

impl VideoDecoder for AndroidDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(android_capability_report(codec))
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        #[cfg(target_os = "android")]
        {
            if self.eos_queued {
                return Err(BackendError::InvalidInput(
                    "cannot push an Android decoder chunk after end of stream".to_string(),
                ));
            }
            self.ensure_decoder_started()?;
            let pts_us = pts_90k.map_or(0, pts_90k_to_us);
            self.queue_decoder_input(chunk, pts_us, 0)?;
            self.drain_decoder(false)
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = (chunk, pts_90k);
            Err(BackendError::UnsupportedConfig(
                "Android MediaCodec decoder is only available on Android".to_string(),
            ))
        }
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        #[cfg(target_os = "android")]
        {
            if self.eos_seen {
                return Ok(self.ready.drain(..).collect());
            }
            self.ensure_decoder_started()?;
            if !self.eos_queued {
                self.queue_decoder_input(&[], 0, BUFFER_FLAG_END_OF_STREAM)?;
                self.eos_queued = true;
            }
            let drained = self.drain_decoder(true)?;
            self.ready.extend(drained);
            Ok(self.ready.drain(..).collect())
        }
        #[cfg(not(target_os = "android"))]
        {
            Err(BackendError::UnsupportedConfig(
                "Android MediaCodec decoder is only available on Android".to_string(),
            ))
        }
    }

    fn try_reap(&mut self) -> Result<Vec<Frame>, BackendError> {
        #[cfg(target_os = "android")]
        {
            let drained = if self.codec.is_some() {
                self.drain_decoder(false)?
            } else {
                Vec::new()
            };
            self.ready.extend(drained);
        }
        Ok(self.ready.drain(..).collect())
    }

    fn decode_summary(&self) -> DecodeSummary {
        self.summary.clone()
    }
}

#[derive(Debug)]
/// Android MediaCodec encoder exposed through the common encoder trait.
pub struct AndroidEncoderAdapter {
    #[cfg(target_os = "android")]
    config: EncoderConfig,
    #[cfg(target_os = "android")]
    options: AndroidEncoderOptions,
    ready: VecDeque<EncodedPacket>,
    #[cfg(target_os = "android")]
    codec: Option<MediaCodec>,
    #[cfg(target_os = "android")]
    eos_queued: bool,
    #[cfg(target_os = "android")]
    eos_seen: bool,
    #[cfg(target_os = "android")]
    dimensions: Option<(usize, usize)>,
}

impl AndroidEncoderAdapter {
    /// Creates a lazily initialized encoder for `config`.
    ///
    /// MediaCodec resources are not acquired until the first frame is submitted.
    #[must_use]
    pub fn with_config(config: EncoderConfig) -> Self {
        #[cfg(target_os = "android")]
        let options = match &config.backend_options {
            BackendEncoderOptions::Android(options) => options.clone(),
            _ => AndroidEncoderOptions::default(),
        };
        #[cfg(not(target_os = "android"))]
        let _ = config;
        Self {
            #[cfg(target_os = "android")]
            config,
            #[cfg(target_os = "android")]
            options,
            ready: VecDeque::new(),
            #[cfg(target_os = "android")]
            codec: None,
            #[cfg(target_os = "android")]
            eos_queued: false,
            #[cfg(target_os = "android")]
            eos_seen: false,
            #[cfg(target_os = "android")]
            dimensions: None,
        }
    }
}

impl VideoEncoder for AndroidEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(android_capability_report(codec))
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        #[cfg(target_os = "android")]
        {
            if self.eos_queued {
                return Err(BackendError::InvalidInput(
                    "cannot push an Android encoder frame after end of stream".to_string(),
                ));
            }
            self.ensure_encoder_started(&frame)?;
            let pts_us = frame.pts_90k.map_or(0, pts_90k_to_us);
            let nv12 = frame_to_nv12(&frame)?;
            self.queue_encoder_input(&nv12, pts_us)?;
            self.drain_encoder(false)
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = frame;
            Err(BackendError::UnsupportedConfig(
                "Android MediaCodec encoder is only available on Android".to_string(),
            ))
        }
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        #[cfg(target_os = "android")]
        {
            if self.codec.is_none() {
                return Ok(self.ready.drain(..).collect());
            }
            if self.eos_seen {
                return Ok(self.ready.drain(..).collect());
            }
            if self.codec.is_some() && !self.eos_queued {
                self.queue_encoder_eos()?;
                self.eos_queued = true;
            }
            let drained = self.drain_encoder(true)?;
            self.ready.extend(drained);
            Ok(self.ready.drain(..).collect())
        }
        #[cfg(not(target_os = "android"))]
        {
            Err(BackendError::UnsupportedConfig(
                "Android MediaCodec encoder is only available on Android".to_string(),
            ))
        }
    }

    fn try_reap(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        #[cfg(target_os = "android")]
        {
            let drained = if self.codec.is_some() {
                self.drain_encoder(false)?
            } else {
                Vec::new()
            };
            self.ready.extend(drained);
        }
        Ok(self.ready.drain(..).collect())
    }
}

#[cfg(target_os = "android")]
#[derive(Debug, Clone, Default)]
struct AndroidOutputFormat {
    width: usize,
    height: usize,
    stride: usize,
    slice_height: usize,
    color_format: Option<u32>,
}

#[cfg(target_os = "android")]
impl AndroidDecoderAdapter {
    fn ensure_decoder_started(&mut self) -> Result<(), BackendError> {
        if self.codec.is_some() {
            return Ok(());
        }
        if self.config.require_hardware {
            return Err(BackendError::UnsupportedConfig(
                "Android MediaCodec selection is OS managed and does not guarantee hardware decode"
                    .to_string(),
            ));
        }
        if self.config.output_mode != DecodeOutputMode::Metadata {
            return Err(BackendError::UnsupportedConfig(
                "Android decoder currently supports DecodeOutputMode::Metadata only".to_string(),
            ));
        }
        let width = self.options.video_width.map(usize::from).ok_or_else(|| {
            BackendError::UnsupportedConfig(
                "Android decoder requires AndroidDecoderOptions.video_width".to_string(),
            )
        })?;
        let height = self.options.video_height.map(usize::from).ok_or_else(|| {
            BackendError::UnsupportedConfig(
                "Android decoder requires AndroidDecoderOptions.video_height".to_string(),
            )
        })?;
        validate_android_dimensions(width, height)?;
        validate_android_fps(self.config.fps)?;
        validate_android_timeout(self.options.timeout_us)?;
        let mime = mime_for_codec(self.config.codec);
        let format = MediaFormat::video(mime, width, height, self.config.fps, None, None, false)?;
        let mut codec = match &self.options.codec_name {
            Some(name) => MediaCodec::codec_by_name(name)?,
            None => MediaCodec::decoder_by_type(mime)?,
        };
        codec.configure(&format, false)?;
        codec.start()?;
        self.output_format = AndroidOutputFormat {
            width,
            height,
            stride: width,
            slice_height: height,
            color_format: None,
        };
        self.summary.width = Some(width);
        self.summary.height = Some(height);
        self.codec = Some(codec);
        drop(format);
        Ok(())
    }

    fn queue_decoder_input(
        &mut self,
        data: &[u8],
        pts_us: i64,
        flags: u32,
    ) -> Result<(), BackendError> {
        validate_android_decode_access_unit(self.config.codec, data)?;
        let codec = self
            .codec
            .as_mut()
            .ok_or_else(|| BackendError::Backend("Android decoder was not started".to_string()))?;
        let index = codec
            .dequeue_input(bounded_blocking_timeout_us(self.options.timeout_us))
            .ok_or_else(|| {
                BackendError::TemporaryBackpressure("decoder input buffer unavailable".to_string())
            })?;
        let buffer_len = match codec.input_buffer(index) {
            Ok(input) => {
                let buffer_len = input.len();
                if data.len() <= buffer_len {
                    input[..data.len()].copy_from_slice(data);
                }
                buffer_len
            }
            Err(error) => {
                let reclaim = codec.queue_input(index, 0, pts_us, 0);
                return Err(input_error_after_reclaim(error, reclaim));
            }
        };
        if data.len() > buffer_len {
            let error = BackendError::InvalidInput(format!(
                "decoder input buffer too small: buffer={buffer_len}, data={}",
                data.len()
            ));
            let reclaim = codec.queue_input(index, 0, pts_us, 0);
            return Err(input_error_after_reclaim(error, reclaim));
        }
        codec.queue_input(index, data.len(), pts_us, flags)
    }

    fn drain_decoder(&mut self, wait_for_eos: bool) -> Result<Vec<Frame>, BackendError> {
        let mut out = Vec::new();
        let mut misses = 0_usize;
        loop {
            let event = {
                let codec = self.codec.as_mut().ok_or_else(|| {
                    BackendError::Backend("Android decoder was not started".to_string())
                })?;
                codec.dequeue_output(if wait_for_eos {
                    bounded_blocking_timeout_us(self.options.timeout_us)
                } else {
                    0
                })?
            };
            match event {
                OutputEvent::Buffer { index, info } => {
                    misses = 0;
                    let frame = if info.size > 0 {
                        let output_result = self
                            .codec
                            .as_mut()
                            .ok_or_else(|| {
                                BackendError::Backend("Android decoder was not started".to_string())
                            })?
                            .output_buffer(index)
                            .and_then(|buffer| {
                                decoder_output_to_frame(
                                    buffer,
                                    &info,
                                    &self.output_format,
                                    self.config.output_mode,
                                )
                            });
                        let release_result = self
                            .codec
                            .as_mut()
                            .ok_or_else(|| {
                                BackendError::Backend("Android decoder was not started".to_string())
                            })?
                            .release_output(index);
                        Some(output_after_release(output_result, release_result)?)
                    } else {
                        self.codec
                            .as_mut()
                            .ok_or_else(|| {
                                BackendError::Backend("Android decoder was not started".to_string())
                            })?
                            .release_output(index)?;
                        None
                    };
                    if let Some(frame) = frame {
                        self.summary.decoded_frames += 1;
                        out.push(frame);
                    }
                    if (info.flags & BUFFER_FLAG_END_OF_STREAM) != 0 {
                        self.eos_seen = true;
                        break;
                    }
                }
                OutputEvent::FormatChanged => {
                    misses = 0;
                    self.refresh_decoder_format();
                }
                OutputEvent::TryAgainLater => {
                    if !wait_for_eos {
                        break;
                    }
                    misses = misses.saturating_add(1);
                    if misses >= MAX_EOS_DRAIN_MISSES {
                        return Err(BackendError::TemporaryBackpressure(
                            "timed out waiting for Android decoder end of stream".to_string(),
                        ));
                    }
                }
            }
        }
        Ok(out)
    }

    fn refresh_decoder_format(&mut self) {
        let Some(codec) = self.codec.as_mut() else {
            return;
        };
        let Some(mut format) = codec.output_format() else {
            return;
        };
        let width = format
            .get_i32("width")
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(self.output_format.width);
        let height = format
            .get_i32("height")
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(self.output_format.height);
        let stride = format
            .get_i32("stride")
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(width);
        let slice_height = format
            .get_i32("slice-height")
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(height);
        let color_format = format.get_i32("color-format").map(|v| v as u32);
        self.output_format = AndroidOutputFormat {
            width,
            height,
            stride,
            slice_height,
            color_format,
        };
        self.summary.width = Some(width);
        self.summary.height = Some(height);
        self.summary.pixel_format = color_format;
    }
}

#[cfg(target_os = "android")]
impl AndroidEncoderAdapter {
    fn ensure_encoder_started(&mut self, frame: &Frame) -> Result<(), BackendError> {
        validate_android_dimensions(frame.width, frame.height)?;
        validate_android_fps(self.config.fps)?;
        validate_android_timeout(self.options.timeout_us)?;
        if let Some((width, height)) = self.dimensions
            && (frame.width != width || frame.height != height)
        {
            return Err(BackendError::InvalidInput(format!(
                "mixed Android encoder dimensions are unsupported: expected {width}x{height}, got {}x{}",
                frame.width, frame.height
            )));
        }
        if self.codec.is_some() {
            return Ok(());
        }
        if self.config.require_hardware {
            return Err(BackendError::UnsupportedConfig(
                "Android MediaCodec selection is OS managed and does not guarantee hardware encode"
                    .to_string(),
            ));
        }
        let mime = mime_for_codec(self.config.codec);
        let bitrate = self.options.bitrate.or(Some(1_000_000));
        let mut format = MediaFormat::video(
            mime,
            frame.width,
            frame.height,
            self.config.fps,
            bitrate,
            self.options.i_frame_interval_sec,
            true,
        )?;
        format.set_i32("prepend-sps-pps-to-idr-frames", 1);
        let mut codec = match &self.options.codec_name {
            Some(name) => MediaCodec::codec_by_name(name)?,
            None => MediaCodec::encoder_by_type(mime)?,
        };
        codec.configure(&format, true)?;
        codec.start()?;
        self.dimensions = Some((frame.width, frame.height));
        self.codec = Some(codec);
        drop(format);
        Ok(())
    }

    fn queue_encoder_input(
        &mut self,
        nv12: &Nv12FramePayload,
        pts_us: i64,
    ) -> Result<(), BackendError> {
        let codec = self
            .codec
            .as_mut()
            .ok_or_else(|| BackendError::Backend("Android encoder was not started".to_string()))?;
        let index = codec
            .dequeue_input(bounded_blocking_timeout_us(self.options.timeout_us))
            .ok_or_else(|| {
                BackendError::TemporaryBackpressure("encoder input buffer unavailable".to_string())
            })?;
        let buffer_len = match codec.input_buffer(index) {
            Ok(input) => {
                let buffer_len = input.len();
                if nv12.data.len() <= buffer_len {
                    input[..nv12.data.len()].copy_from_slice(&nv12.data);
                }
                buffer_len
            }
            Err(error) => {
                let reclaim = codec.queue_input(index, 0, pts_us, 0);
                return Err(input_error_after_reclaim(error, reclaim));
            }
        };
        if nv12.data.len() > buffer_len {
            let error = BackendError::InvalidInput(format!(
                "encoder input buffer too small: buffer={buffer_len}, data={}",
                nv12.data.len()
            ));
            let reclaim = codec.queue_input(index, 0, pts_us, 0);
            return Err(input_error_after_reclaim(error, reclaim));
        }
        codec.queue_input(index, nv12.data.len(), pts_us, 0)
    }

    fn queue_encoder_eos(&mut self) -> Result<(), BackendError> {
        let codec = self
            .codec
            .as_mut()
            .ok_or_else(|| BackendError::Backend("Android encoder was not started".to_string()))?;
        let index = codec
            .dequeue_input(bounded_blocking_timeout_us(self.options.timeout_us))
            .ok_or_else(|| {
                BackendError::TemporaryBackpressure("encoder input buffer unavailable".to_string())
            })?;
        codec.queue_input(index, 0, 0, BUFFER_FLAG_END_OF_STREAM)
    }

    fn drain_encoder(&mut self, wait_for_eos: bool) -> Result<Vec<EncodedPacket>, BackendError> {
        let mut out = Vec::new();
        let mut misses = 0_usize;
        loop {
            let event = {
                let codec = self.codec.as_mut().ok_or_else(|| {
                    BackendError::Backend("Android encoder was not started".to_string())
                })?;
                codec.dequeue_output(if wait_for_eos {
                    bounded_blocking_timeout_us(self.options.timeout_us)
                } else {
                    0
                })?
            };
            match event {
                OutputEvent::Buffer { index, info } => {
                    misses = 0;
                    let packet = if info.size > 0 {
                        let output_result = self
                            .codec
                            .as_mut()
                            .ok_or_else(|| {
                                BackendError::Backend("Android encoder was not started".to_string())
                            })?
                            .output_buffer(index)
                            .and_then(|buffer| {
                                encoder_output_to_packet(self.config.codec, buffer, &info)
                            });
                        let release_result = self
                            .codec
                            .as_mut()
                            .ok_or_else(|| {
                                BackendError::Backend("Android encoder was not started".to_string())
                            })?
                            .release_output(index);
                        output_after_release(output_result, release_result)?
                    } else {
                        self.codec
                            .as_mut()
                            .ok_or_else(|| {
                                BackendError::Backend("Android encoder was not started".to_string())
                            })?
                            .release_output(index)?;
                        None
                    };
                    if let Some(packet) = packet {
                        out.push(packet);
                    }
                    if (info.flags & BUFFER_FLAG_END_OF_STREAM) != 0 {
                        self.eos_seen = true;
                        break;
                    }
                }
                OutputEvent::FormatChanged => {
                    misses = 0;
                    let Some(mut format) = self.codec.as_mut().and_then(MediaCodec::output_format)
                    else {
                        continue;
                    };
                    out.extend(encoder_format_config_packets(
                        self.config.codec,
                        &mut format,
                    ));
                }
                OutputEvent::TryAgainLater => {
                    if !wait_for_eos {
                        break;
                    }
                    misses = misses.saturating_add(1);
                    if misses >= MAX_EOS_DRAIN_MISSES {
                        return Err(BackendError::TemporaryBackpressure(
                            "timed out waiting for Android encoder end of stream".to_string(),
                        ));
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(target_os = "android")]
fn frame_to_nv12(frame: &Frame) -> Result<Nv12FramePayload, BackendError> {
    validate_android_dimensions(frame.width, frame.height)?;
    let dimensions = checked_dimensions(frame.width, frame.height, "frame")?;
    if let Some(nv12) = &frame.nv12 {
        nv12.validate(dimensions)?;
        if nv12.pitch != frame.width {
            return Err(BackendError::InvalidInput(format!(
                "Android encoder currently requires tightly packed NV12: pitch={}, width={}",
                nv12.pitch, frame.width
            )));
        }
        let expected = frame
            .width
            .checked_mul(frame.height)
            .and_then(|luma| luma.checked_add(luma / 2))
            .ok_or_else(|| BackendError::InvalidInput("nv12 size overflow".to_string()))?;
        if nv12.data.len() != expected {
            return Err(BackendError::InvalidInput(format!(
                "tight NV12 payload size mismatch: expected {}, got {}",
                expected,
                nv12.data.len()
            )));
        }
        return Ok(nv12.clone());
    }
    let Some(argb) = frame.argb.as_deref() else {
        return Err(BackendError::InvalidInput(
            "Android encoder requires NV12 or ARGB payload".to_string(),
        ));
    };
    Nv12FramePayload::from_argb8888(dimensions, argb)
}

#[cfg(target_os = "android")]
fn validate_android_decode_access_unit(codec: Codec, data: &[u8]) -> Result<(), BackendError> {
    if data.is_empty() || codec == Codec::Av1 {
        return Ok(());
    }
    let access_units =
        bitstream::split_access_units(codec, bitstream::AnnexBAccessUnitRef::new(data))?;
    if access_units.len() > 1 {
        return Err(BackendError::InvalidBitstream(format!(
            "Android decoder expects one complete access unit per submit, got {}",
            access_units.len()
        )));
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn decoder_output_to_frame(
    buffer: &[u8],
    info: &crate::ffi::codec::AMediaCodecBufferInfo,
    format: &AndroidOutputFormat,
    output_mode: video_hw_core::DecodeOutputMode,
) -> Result<Frame, BackendError> {
    let offset = usize::try_from(info.offset)
        .map_err(|_| BackendError::Backend(format!("negative output offset: {}", info.offset)))?;
    let size = usize::try_from(info.size)
        .map_err(|_| BackendError::Backend(format!("negative output size: {}", info.size)))?;
    let end = offset
        .checked_add(size)
        .ok_or_else(|| BackendError::Backend("decoder output range overflow".to_string()))?;
    if end > buffer.len() {
        return Err(BackendError::Backend(format!(
            "decoder output range exceeds buffer: end={end}, len={}",
            buffer.len()
        )));
    }
    let nv12 = if matches!(
        output_mode,
        video_hw_core::DecodeOutputMode::Nv12 | video_hw_core::DecodeOutputMode::Rgb24
    ) {
        Some(Nv12FramePayload {
            pitch: format.stride.max(format.width),
            data: crate::color::copy_semiplanar_yuv420_to_nv12(
                &buffer[offset..end],
                format.width,
                format.height,
                format.stride.max(format.width),
                format.slice_height.max(format.height),
            )?,
        })
    } else {
        None
    };
    Ok(Frame {
        width: format.width,
        height: format.height,
        pixel_format: format.color_format,
        pts_90k: Some(us_to_pts_90k(info.presentation_time_us)),
        decode_info_flags: Some(info.flags),
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb: None,
        rgb24: None,
        nv12,
        force_keyframe: false,
    })
}

#[cfg(target_os = "android")]
fn encoder_output_to_packet(
    codec: Codec,
    buffer: &[u8],
    info: &crate::ffi::codec::AMediaCodecBufferInfo,
) -> Result<Option<EncodedPacket>, BackendError> {
    if info.size <= 0 {
        return Ok(None);
    }
    let offset = usize::try_from(info.offset)
        .map_err(|_| BackendError::Backend(format!("negative output offset: {}", info.offset)))?;
    let size = usize::try_from(info.size)
        .map_err(|_| BackendError::Backend(format!("negative output size: {}", info.size)))?;
    let end = offset
        .checked_add(size)
        .ok_or_else(|| BackendError::Backend("encoder output range overflow".to_string()))?;
    if end > buffer.len() {
        return Err(BackendError::Backend(format!(
            "encoder output range exceeds buffer: end={end}, len={}",
            buffer.len()
        )));
    }
    let is_codec_config = (info.flags & BUFFER_FLAG_CODEC_CONFIG) != 0;
    let data = buffer[offset..end].to_vec();
    Ok(Some(EncodedPacket {
        codec,
        layout: encoded_layout(codec),
        kind: if is_codec_config {
            EncodedContentKind::CodecConfig
        } else {
            EncodedContentKind::Frame
        },
        data: if is_codec_config {
            encoder_config_buffer_payload(codec, data)
        } else {
            data
        },
        pts_90k: (!is_codec_config).then(|| us_to_pts_90k(info.presentation_time_us)),
        is_keyframe: !is_codec_config && (info.flags & BUFFER_FLAG_KEY_FRAME) != 0,
    }))
}

#[cfg(target_os = "android")]
fn encoder_format_config_packets(codec: Codec, format: &mut MediaFormat) -> Vec<EncodedPacket> {
    let keys: &[&str] = match codec {
        Codec::H264 => &["csd-0", "csd-1"],
        Codec::Hevc | Codec::Av1 => &["csd-0", "csd-1", "csd-2"],
    };
    let data = keys
        .iter()
        .filter_map(|key| format.get_buffer(key))
        .flat_map(|buffer| encoder_config_buffer_payload(codec, buffer).into_iter())
        .collect::<Vec<_>>();
    if data.is_empty() {
        Vec::new()
    } else {
        vec![EncodedPacket {
            codec,
            layout: encoded_layout(codec),
            kind: EncodedContentKind::CodecConfig,
            data,
            pts_90k: None,
            is_keyframe: false,
        }]
    }
}

#[cfg(target_os = "android")]
fn encoder_config_buffer_payload(codec: Codec, buffer: Vec<u8>) -> Vec<u8> {
    match codec {
        Codec::H264 | Codec::Hevc => annexb_prefixed(buffer),
        Codec::Av1 => buffer,
    }
}

#[cfg(target_os = "android")]
fn encoded_layout(codec: Codec) -> video_hw_core::EncodedLayout {
    match codec {
        Codec::H264 | Codec::Hevc => video_hw_core::EncodedLayout::AnnexB,
        Codec::Av1 => video_hw_core::EncodedLayout::Av1,
    }
}

#[cfg(target_os = "android")]
fn annexb_prefixed(buffer: Vec<u8>) -> Vec<u8> {
    if buffer.starts_with(&[0, 0, 1]) || buffer.starts_with(&[0, 0, 0, 1]) {
        buffer
    } else {
        let mut out = Vec::with_capacity(buffer.len() + 4);
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&buffer);
        out
    }
}

#[cfg(target_os = "android")]
fn pts_90k_to_us(value: i64) -> i64 {
    value.saturating_mul(1_000_000) / 90_000
}

#[cfg(target_os = "android")]
fn us_to_pts_90k(value: i64) -> i64 {
    value.saturating_mul(90_000) / 1_000_000
}

#[cfg(target_os = "android")]
fn validate_android_dimensions(width: usize, height: usize) -> Result<(), BackendError> {
    if width < 16 || height < 16 {
        return Err(BackendError::InvalidInput(format!(
            "Android codec dimensions must be at least 16x16, got {width}x{height}"
        )));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(format!(
            "Android codec dimensions must be even, got {width}x{height}"
        )));
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn checked_dimensions(width: usize, height: usize, role: &str) -> Result<Dimensions, BackendError> {
    let width = u32::try_from(width)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| {
            BackendError::InvalidInput(format!(
                "Android {role} width must fit u32 and be greater than zero"
            ))
        })?;
    let height = u32::try_from(height)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| {
            BackendError::InvalidInput(format!(
                "Android {role} height must fit u32 and be greater than zero"
            ))
        })?;
    Ok(Dimensions { width, height })
}

#[cfg(target_os = "android")]
fn validate_android_fps(fps: i32) -> Result<(), BackendError> {
    if fps <= 0 {
        return Err(BackendError::InvalidInput(format!(
            "Android codec fps must be positive, got {fps}"
        )));
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn validate_android_timeout(timeout_us: i64) -> Result<(), BackendError> {
    if timeout_us < 0 {
        return Err(BackendError::InvalidInput(format!(
            "Android codec timeout must be non-negative, got {timeout_us}"
        )));
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn bounded_blocking_timeout_us(timeout_us: i64) -> i64 {
    timeout_us.clamp(0, MAX_BLOCKING_DEQUEUE_TIMEOUT_US)
}

#[cfg(target_os = "android")]
fn input_error_after_reclaim(
    error: BackendError,
    reclaim: Result<(), BackendError>,
) -> BackendError {
    match reclaim {
        Ok(()) => error,
        Err(reclaim_error) => BackendError::Backend(format!(
            "{error}; additionally failed to return dequeued input buffer: {reclaim_error}"
        )),
    }
}

#[cfg(target_os = "android")]
fn output_after_release<T>(
    output: Result<T, BackendError>,
    release: Result<(), BackendError>,
) -> Result<T, BackendError> {
    match (output, release) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(release_error)) => Err(BackendError::Backend(format!(
            "{error}; additionally failed to release output buffer: {release_error}"
        ))),
    }
}
