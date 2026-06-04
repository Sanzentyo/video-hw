use std::collections::VecDeque;

use video_hw_core::{
    BackendError, CapabilityReport, Codec, DecodeSummary, DecoderConfig, EncodedPacket,
    EncoderConfig, Frame, VideoDecoder, VideoEncoder,
};

use crate::capability::android_capability_report;

#[cfg(target_os = "android")]
use crate::ffi::codec::{
    BUFFER_FLAG_CODEC_CONFIG, BUFFER_FLAG_END_OF_STREAM, BUFFER_FLAG_KEY_FRAME, MediaCodec,
    MediaFormat, OutputEvent,
};
#[cfg(target_os = "android")]
use video_hw_core::{
    AndroidDecoderOptions, AndroidEncoderOptions, BackendDecoderOptions, BackendEncoderOptions,
    Nv12FramePayload,
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
}

impl AndroidDecoderAdapter {
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
        }
    }
}

impl VideoDecoder for AndroidDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(android_capability_report(codec, false))
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        #[cfg(target_os = "android")]
        {
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
}

impl AndroidEncoderAdapter {
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
        }
    }
}

impl VideoEncoder for AndroidEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(android_capability_report(codec, true))
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        #[cfg(target_os = "android")]
        {
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
#[cfg(target_os = "android")]
impl AndroidDecoderAdapter {
    fn ensure_decoder_started(&mut self) -> Result<(), BackendError> {
        if self.codec.is_some() {
            return Ok(());
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
        let mime = mime_for_codec(self.config.codec);
        let format = MediaFormat::video(mime, width, height, self.config.fps, None, None, false)?;
        let mut codec = match &self.options.codec_name {
            Some(_name) => MediaCodec::decoder_by_type(mime)?,
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
        let codec = self
            .codec
            .as_mut()
            .ok_or_else(|| BackendError::Backend("Android decoder was not started".to_string()))?;
        let index = codec
            .dequeue_input(self.options.timeout_us)
            .ok_or_else(|| {
                BackendError::TemporaryBackpressure("decoder input buffer unavailable".to_string())
            })?;
        let input = codec.input_buffer(index)?;
        if data.len() > input.len() {
            return Err(BackendError::InvalidInput(format!(
                "decoder input buffer too small: buffer={}, data={}",
                input.len(),
                data.len()
            )));
        }
        input[..data.len()].copy_from_slice(data);
        codec.queue_input(index, data.len(), pts_us, flags)
    }

    fn drain_decoder(&mut self, wait_for_eos: bool) -> Result<Vec<Frame>, BackendError> {
        let mut out = Vec::new();
        let mut spins = 0_usize;
        loop {
            spins += 1;
            let event = {
                let codec = self.codec.as_mut().ok_or_else(|| {
                    BackendError::Backend("Android decoder was not started".to_string())
                })?;
                codec.dequeue_output(if wait_for_eos {
                    self.options.timeout_us
                } else {
                    0
                })?
            };
            match event {
                OutputEvent::Buffer { index, info } => {
                    let frame = {
                        let codec = self.codec.as_mut().unwrap();
                        let buffer = codec.output_buffer(index)?;
                        let frame = decoder_output_to_frame(
                            buffer,
                            &info,
                            &self.output_format,
                            self.config.output_mode,
                        )?;
                        codec.release_output(index);
                        frame
                    };
                    if info.size > 0 {
                        self.summary.decoded_frames += 1;
                        out.push(frame);
                    }
                    if (info.flags & BUFFER_FLAG_END_OF_STREAM) != 0 {
                        break;
                    }
                }
                OutputEvent::FormatChanged => {
                    self.refresh_decoder_format();
                }
                OutputEvent::TryAgainLater => {
                    if !wait_for_eos || spins > 10_000 {
                        break;
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
        if self.codec.is_some() {
            return Ok(());
        }
        let mime = mime_for_codec(self.config.codec);
        let bitrate = self.options.bitrate.or(Some(1_000_000));
        let format = MediaFormat::video(
            mime,
            frame.width,
            frame.height,
            self.config.fps,
            bitrate,
            self.options.i_frame_interval_sec,
            true,
        )?;
        let mut codec = match &self.options.codec_name {
            Some(_name) => MediaCodec::encoder_by_type(mime)?,
            None => MediaCodec::encoder_by_type(mime)?,
        };
        codec.configure(&format, true)?;
        codec.start()?;
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
            .dequeue_input(self.options.timeout_us)
            .ok_or_else(|| {
                BackendError::TemporaryBackpressure("encoder input buffer unavailable".to_string())
            })?;
        let input = codec.input_buffer(index)?;
        if nv12.data.len() > input.len() {
            return Err(BackendError::InvalidInput(format!(
                "encoder input buffer too small: buffer={}, data={}",
                input.len(),
                nv12.data.len()
            )));
        }
        input[..nv12.data.len()].copy_from_slice(&nv12.data);
        codec.queue_input(index, nv12.data.len(), pts_us, 0)
    }

    fn queue_encoder_eos(&mut self) -> Result<(), BackendError> {
        let codec = self
            .codec
            .as_mut()
            .ok_or_else(|| BackendError::Backend("Android encoder was not started".to_string()))?;
        let index = codec
            .dequeue_input(self.options.timeout_us)
            .ok_or_else(|| {
                BackendError::TemporaryBackpressure("encoder input buffer unavailable".to_string())
            })?;
        codec.queue_input(index, 0, 0, BUFFER_FLAG_END_OF_STREAM)
    }

    fn drain_encoder(&mut self, wait_for_eos: bool) -> Result<Vec<EncodedPacket>, BackendError> {
        let mut out = Vec::new();
        let mut spins = 0_usize;
        loop {
            spins += 1;
            let event = {
                let codec = self.codec.as_mut().ok_or_else(|| {
                    BackendError::Backend("Android encoder was not started".to_string())
                })?;
                codec.dequeue_output(if wait_for_eos {
                    self.options.timeout_us
                } else {
                    0
                })?
            };
            match event {
                OutputEvent::Buffer { index, info } => {
                    let packet = {
                        let codec = self.codec.as_mut().unwrap();
                        let buffer = codec.output_buffer(index)?;
                        let packet = encoder_output_to_packet(self.config.codec, buffer, &info)?;
                        codec.release_output(index);
                        packet
                    };
                    if let Some(packet) = packet {
                        out.push(packet);
                    }
                    if (info.flags & BUFFER_FLAG_END_OF_STREAM) != 0 {
                        break;
                    }
                }
                OutputEvent::FormatChanged => {}
                OutputEvent::TryAgainLater => {
                    if !wait_for_eos || spins > 10_000 {
                        break;
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(target_os = "android")]
fn frame_to_nv12(frame: &Frame) -> Result<Nv12FramePayload, BackendError> {
    if let Some(nv12) = &frame.nv12 {
        return Ok(nv12.clone());
    }
    let Some(argb) = frame.argb.as_deref() else {
        return Err(BackendError::InvalidInput(
            "Android encoder requires NV12 or ARGB payload".to_string(),
        ));
    };
    let (pitch, data) = crate::color::argb_to_nv12(argb, frame.width, frame.height)?;
    Ok(Nv12FramePayload { pitch, data })
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
    Ok(Some(EncodedPacket {
        codec,
        data: buffer[offset..end].to_vec(),
        pts_90k: Some(us_to_pts_90k(info.presentation_time_us)),
        is_keyframe: (info.flags & BUFFER_FLAG_KEY_FRAME) != 0
            || (info.flags & BUFFER_FLAG_CODEC_CONFIG) != 0,
    }))
}

#[cfg(target_os = "android")]
fn pts_90k_to_us(value: i64) -> i64 {
    value.saturating_mul(1_000_000) / 90_000
}

#[cfg(target_os = "android")]
fn us_to_pts_90k(value: i64) -> i64 {
    value.saturating_mul(90_000) / 1_000_000
}
