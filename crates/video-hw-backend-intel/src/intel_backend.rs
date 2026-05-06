use std::collections::VecDeque;
use std::io::{Cursor, Read, Write};
use std::sync::OnceLock;

use onevpl::MfxStatus;
use onevpl::bitstream::Bitstream;
use onevpl::constants::{
    ChromaFormat, Codec as OneVplCodec, CodingOptionValue, FourCC, FrameType, IoPattern,
    MemoryFlag, PicStruct, RateControlMethod, TargetUsage,
};
use onevpl::encode::EncodeCtrl;
use onevpl::vpp::VppVideoParams;
use rayon::prelude::*;
use tokio::runtime::Builder as RuntimeBuilder;

#[cfg(feature = "unstable-raw-inputs")]
use crate::Nv12FramePayload;
use crate::{
    BackendDecoderOptions, BackendEncoderOptions, BackendError, CapabilityReport, Codec,
    DecodeOutputMode, DecodeSummary, DecoderConfig, EncodedPacket, Frame, IntelDecoderOptions,
    IntelEncoderOptions, SessionSwitchRequest, VideoDecoder, VideoEncoder,
};

pub struct IntelDecoderAdapter {
    config: DecoderConfig,
    options: IntelDecoderOptions,
    pending_bitstream: Vec<u8>,
    next_pts_90k: i64,
    last_summary: DecodeSummary,
}

impl IntelDecoderAdapter {
    pub fn new(config: DecoderConfig) -> Self {
        let options = match &config.backend_options {
            BackendDecoderOptions::Intel(options) => options.clone(),
            BackendDecoderOptions::Default
            | BackendDecoderOptions::VideoToolbox(_)
            | BackendDecoderOptions::Nvidia(_)
            | BackendDecoderOptions::Vulkan(_) => IntelDecoderOptions::default(),
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
        let runtime = RuntimeBuilder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| {
                BackendError::UnsupportedConfig(format!("failed to create Tokio runtime: {err}"))
            })?;
        let codec = self.config.codec;
        let output_mode = self.config.output_mode;
        let fps = self.config.fps;

        if self.config.require_hardware && self.options.force_software {
            return Err(BackendError::UnsupportedConfig(
                "Intel decode cannot use require_hardware=true together with IntelDecoderOptions.force_software=true".to_string(),
            ));
        }
        if self.options.force_software {
            return runtime.block_on(decode_with_onevpl(
                bitstream,
                codec,
                output_mode,
                fps,
                self.next_pts_90k,
                false,
            ));
        }
        let hardware_attempt = runtime.block_on(decode_with_onevpl(
            bitstream,
            codec,
            output_mode,
            fps,
            self.next_pts_90k,
            true,
        ));
        if self.config.require_hardware {
            return hardware_attempt;
        }
        match hardware_attempt {
            Ok(frames) => Ok(frames),
            Err(_) => runtime.block_on(decode_with_onevpl(
                bitstream,
                codec,
                output_mode,
                fps,
                self.next_pts_90k,
                false,
            )),
        }
    }
}

impl VideoDecoder for IntelDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        let decode_supported = matches!(codec, Codec::H264 | Codec::Hevc | Codec::Av1);
        Ok(CapabilityReport {
            codec,
            decode_supported,
            encode_supported: matches!(codec, Codec::H264 | Codec::Hevc | Codec::Av1),
            hardware_acceleration: true,
            decode_output_modes: if decode_supported {
                vec![
                    DecodeOutputMode::Metadata,
                    DecodeOutputMode::Nv12,
                    DecodeOutputMode::Rgb24,
                ]
            } else {
                Vec::new()
            },
        })
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
                if let Some(last) = frames.last().and_then(|frame| frame.pts_90k) {
                    let step = decode_pts_step(self.config.fps);
                    self.next_pts_90k = last.saturating_add(step);
                }
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

pub struct IntelEncoderAdapter {
    codec: Codec,
    fps: i32,
    require_hardware: bool,
    options: IntelEncoderOptions,
    pending_frames: Vec<Frame>,
    width: Option<usize>,
    height: Option<usize>,
}

impl IntelEncoderAdapter {
    pub fn with_config(
        codec: Codec,
        fps: i32,
        require_hardware: bool,
        backend_options: BackendEncoderOptions,
    ) -> Self {
        let options = match backend_options {
            BackendEncoderOptions::Intel(options) => options,
            BackendEncoderOptions::Default
            | BackendEncoderOptions::VideoToolbox(_)
            | BackendEncoderOptions::Nvidia(_)
            | BackendEncoderOptions::Vulkan(_) => IntelEncoderOptions::default(),
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
        let runtime = RuntimeBuilder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|err| {
                BackendError::UnsupportedConfig(format!("failed to create Tokio runtime: {err}"))
            })?;

        let width = self.width.unwrap_or(0);
        let height = self.height.unwrap_or(0);
        let codec = self.codec;
        let fps = self.fps;
        let options = self.options.clone();
        let require_hardware = self.require_hardware;

        runtime.block_on(async move {
            if require_hardware && options.force_software {
                return Err(BackendError::UnsupportedConfig(
                    "Intel encode cannot use require_hardware=true together with IntelEncoderOptions.force_software=true".to_string(),
                ));
            }
            if options.force_software {
                return encode_with_onevpl(
                    pending_frames,
                    width,
                    height,
                    codec,
                    fps,
                    &options,
                    false,
                )
                .await;
            }
            let hardware_attempt =
                encode_with_onevpl(pending_frames, width, height, codec, fps, &options, true).await;
            if require_hardware {
                return hardware_attempt;
            }
            match hardware_attempt {
                Ok(packets) => Ok(packets),
                Err(_) => {
                    encode_with_onevpl(pending_frames, width, height, codec, fps, &options, false)
                        .await
                }
            }
        })
    }
}

impl VideoEncoder for IntelEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        let decode_supported = matches!(codec, Codec::H264 | Codec::Hevc | Codec::Av1);
        Ok(CapabilityReport {
            codec,
            decode_supported,
            encode_supported: matches!(codec, Codec::H264 | Codec::Hevc | Codec::Av1),
            hardware_acceleration: true,
            decode_output_modes: if decode_supported {
                vec![
                    DecodeOutputMode::Metadata,
                    DecodeOutputMode::Nv12,
                    DecodeOutputMode::Rgb24,
                ]
            } else {
                Vec::new()
            },
        })
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        if frame.width == 0 || frame.height == 0 {
            return Err(BackendError::InvalidInput(
                "frame dimensions must be positive".to_string(),
            ));
        }
        #[cfg(feature = "unstable-raw-inputs")]
        let has_supported_payload = frame.argb.is_some() || frame.nv12.is_some();
        #[cfg(not(feature = "unstable-raw-inputs"))]
        let has_supported_payload = frame.argb.is_some();
        if !has_supported_payload {
            return Err(BackendError::InvalidInput(
                "Intel backend requires ARGB or NV12 frame payload".to_string(),
            ));
        }
        if let Some(width) = self.width {
            if width != frame.width {
                return Err(BackendError::InvalidInput(
                    "all frames in one flush cycle must have the same width".to_string(),
                ));
            }
        } else {
            self.width = Some(frame.width);
        }
        if let Some(height) = self.height {
            if height != frame.height {
                return Err(BackendError::InvalidInput(
                    "all frames in one flush cycle must have the same height".to_string(),
                ));
            }
        } else {
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
            Ok(packets) => {
                self.width = None;
                self.height = None;
                Ok(packets)
            }
            Err(err) => {
                self.pending_frames = pending_frames;
                Err(err)
            }
        }
    }

    fn try_reap(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        Ok(Vec::new())
    }

    fn request_session_switch(
        &mut self,
        _request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        Err(BackendError::UnsupportedConfig(
            "session switching is not supported by Intel backend".to_string(),
        ))
    }
}

async fn decode_with_onevpl(
    bitstream_data: &[u8],
    codec: Codec,
    output_mode: DecodeOutputMode,
    fps: i32,
    initial_pts_90k: i64,
    use_hardware: bool,
) -> Result<Vec<Frame>, BackendError> {
    if bitstream_data.is_empty() {
        return Ok(Vec::new());
    }

    let mut loader =
        onevpl::Loader::new().map_err(|status| map_onevpl_status(status, "Loader::new"))?;
    loader.use_hardware(use_hardware);
    loader.require_decoder(to_onevpl_codec(codec));
    loader.use_api_version(2, 2);

    let session = loader
        .new_session(0)
        .map_err(|status| map_onevpl_status(status, "Loader::new_session"))?;
    let mut backing_buffer = vec![0_u8; bitstream_data.len().max(2 * 1024 * 1024)];
    let mut bitstream = Bitstream::with_codec(&mut backing_buffer, to_onevpl_codec(codec));
    bitstream.write_all(bitstream_data).map_err(|err| {
        BackendError::Backend(format!(
            "failed to write bitstream payload for decode: {err}"
        ))
    })?;

    let io_pattern = if use_hardware && matches!(output_mode, DecodeOutputMode::Metadata) {
        IoPattern::OUT_VIDEO_MEMORY
    } else {
        IoPattern::OUT_SYSTEM_MEMORY
    };
    let mut decode_params = session
        .decode_header(&mut bitstream, io_pattern)
        .map_err(|status| map_onevpl_status(status, "Session::decode_header"))?;
    let decode_async_depth = std::env::var("VIDEO_HW_INTEL_DECODE_ASYNC_DEPTH")
        .ok()
        .and_then(|raw| raw.parse::<u16>().ok())
        .filter(|depth| (1..=16).contains(depth))
        .unwrap_or(16);
    decode_params.set_async_depth(decode_async_depth);
    let decoder = session
        .decoder(decode_params)
        .map_err(|status| map_onevpl_status(status, "Session::decoder"))?;

    let mut frames = Vec::new();
    let mut next_pts_90k = initial_pts_90k;
    let mut metadata_cache = None;

    let queue_limit = usize::from(decode_async_depth.max(1));
    let mut queued_surfaces = VecDeque::new();
    let mut emit_surface = |mut surface: onevpl::FrameSurface<'_>| -> Result<(), BackendError> {
        if matches!(output_mode, DecodeOutputMode::Metadata) {
            frames.push(metadata_frame_from_surface(
                &mut surface,
                fps,
                &mut next_pts_90k,
                &mut metadata_cache,
            ));
        } else {
            frames.push(surface_to_backend_frame(
                &mut surface,
                output_mode,
                fps,
                &mut next_pts_90k,
            )?);
        }
        Ok(())
    };
    loop {
        match decoder.decode_async(Some(&mut bitstream), None) {
            Ok(surface) => {
                queued_surfaces.push_back(surface);
                if queued_surfaces.len() >= queue_limit
                    && let Some(surface) = queued_surfaces.pop_front()
                {
                    let mut surface = surface;
                    match surface.synchronize(Some(0)) {
                        Ok(()) => emit_surface(surface)?,
                        Err(MfxStatus::InExecution | MfxStatus::DeviceBusy) => {
                            queued_surfaces.push_back(surface);
                        }
                        Err(status) => {
                            return Err(map_onevpl_status(status, "FrameSurface::synchronize"));
                        }
                    }
                }
            }
            Err(MfxStatus::DeviceBusy) => {
                if let Some(surface) = queued_surfaces.pop_front() {
                    let mut surface = surface;
                    match surface.synchronize(Some(0)) {
                        Ok(()) => emit_surface(surface)?,
                        Err(MfxStatus::InExecution | MfxStatus::DeviceBusy) => {
                            queued_surfaces.push_back(surface);
                        }
                        Err(status) => {
                            return Err(map_onevpl_status(status, "FrameSurface::synchronize"));
                        }
                    }
                    std::thread::yield_now();
                } else {
                    std::thread::yield_now();
                }
            }
            Err(MfxStatus::MoreData) => break,
            Err(MfxStatus::VideoParamChanged) => {}
            Err(status) => return Err(map_onevpl_status(status, "Decoder::decode_async")),
        }
    }

    loop {
        match decoder.decode_async(None, None) {
            Ok(surface) => {
                queued_surfaces.push_back(surface);
                if queued_surfaces.len() >= queue_limit
                    && let Some(surface) = queued_surfaces.pop_front()
                {
                    let mut surface = surface;
                    match surface.synchronize(Some(0)) {
                        Ok(()) => emit_surface(surface)?,
                        Err(MfxStatus::InExecution | MfxStatus::DeviceBusy) => {
                            queued_surfaces.push_back(surface);
                        }
                        Err(status) => {
                            return Err(map_onevpl_status(status, "FrameSurface::synchronize"));
                        }
                    }
                }
            }
            Err(MfxStatus::DeviceBusy) => {
                if let Some(surface) = queued_surfaces.pop_front() {
                    let mut surface = surface;
                    match surface.synchronize(Some(0)) {
                        Ok(()) => emit_surface(surface)?,
                        Err(MfxStatus::InExecution | MfxStatus::DeviceBusy) => {
                            queued_surfaces.push_back(surface);
                        }
                        Err(status) => {
                            return Err(map_onevpl_status(status, "FrameSurface::synchronize"));
                        }
                    }
                    std::thread::yield_now();
                } else {
                    std::thread::yield_now();
                }
            }
            Err(MfxStatus::MoreData) => break,
            Err(MfxStatus::VideoParamChanged) => {}
            Err(status) => return Err(map_onevpl_status(status, "Decoder::decode_async drain")),
        }
    }

    while let Some(surface) = queued_surfaces.pop_front() {
        let mut surface = surface;
        surface
            .synchronize(None)
            .map_err(|status| map_onevpl_status(status, "FrameSurface::synchronize"))?;
        emit_surface(surface)?;
    }

    Ok(frames)
}

fn metadata_frame_from_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    fps: i32,
    next_pts_90k: &mut i64,
    cache: &mut Option<(usize, usize, Option<u32>)>,
) -> Frame {
    let (width, height, pixel_format) = if let Some(cached) = *cache {
        cached
    } else {
        let bounds = surface.bounds();
        let width = usize::from(if bounds.crop_width > 0 {
            bounds.crop_width
        } else {
            bounds.width
        });
        let height = usize::from(if bounds.crop_height > 0 {
            bounds.crop_height
        } else {
            bounds.height
        });
        let fourcc = surface.fourcc();
        let pixel_format = Some(fourcc.repr() as u32);
        *cache = Some((width, height, pixel_format));
        (width, height, pixel_format)
    };
    let pts_90k = Some(bump_decode_pts(next_pts_90k, fps));
    Frame {
        width,
        height,
        pixel_format,
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

fn surface_to_backend_frame(
    surface: &mut onevpl::FrameSurface<'_>,
    output_mode: DecodeOutputMode,
    fps: i32,
    next_pts_90k: &mut i64,
) -> Result<Frame, BackendError> {
    let bounds = surface.bounds();
    let width = usize::from(if bounds.crop_width > 0 {
        bounds.crop_width
    } else {
        bounds.width
    });
    let height = usize::from(if bounds.crop_height > 0 {
        bounds.crop_height
    } else {
        bounds.height
    });
    let fourcc = surface.fourcc();
    let pixel_format = Some(fourcc.repr() as u32);
    let pts_90k = Some(bump_decode_pts(next_pts_90k, fps));

    let argb = if matches!(output_mode, DecodeOutputMode::Metadata) {
        None
    } else {
        let mut data = Vec::new();
        surface.read_to_end(&mut data).map_err(|err| {
            BackendError::Backend(format!("failed to read decoded frame payload: {err}"))
        })?;
        Some(surface_payload_to_argb(&data, width, height, fourcc)?)
    };

    Ok(Frame {
        width,
        height,
        pixel_format,
        pts_90k,
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb,
        nv12: None,
        force_keyframe: false,
    })
}

fn surface_payload_to_argb(
    data: &[u8],
    width: usize,
    height: usize,
    fourcc: FourCC,
) -> Result<Vec<u8>, BackendError> {
    match fourcc {
        FourCC::NV12 => nv12_to_argb(data, width, height),
        FourCC::IyuvOrI420 | FourCC::YV12 => i420_to_argb(data, width, height),
        FourCC::Rgb4OrBgra => bgra_to_argb(data, width, height),
        _ => Err(BackendError::UnsupportedConfig(format!(
            "unsupported decoded pixel format from Intel backend: {fourcc:?}"
        ))),
    }
}

fn bgra_to_argb(data: &[u8], width: usize, height: usize) -> Result<Vec<u8>, BackendError> {
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("BGRA size overflow".to_string()))?;
    if data.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "BGRA payload size mismatch: expected {}, got {}",
            expected,
            data.len()
        )));
    }
    let mut out = vec![0_u8; expected];
    for (src, dst) in data.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        dst[0] = src[3];
        dst[1] = src[2];
        dst[2] = src[1];
        dst[3] = src[0];
    }
    Ok(out)
}

fn i420_to_argb(data: &[u8], width: usize, height: usize) -> Result<Vec<u8>, BackendError> {
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "decoded frame dimensions must be positive".to_string(),
        ));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "I420 decode currently requires even frame dimensions".to_string(),
        ));
    }
    let y_size = width
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("i420 luma size overflow".to_string()))?;
    let uv_size = y_size / 4;
    let expected = y_size
        .checked_add(uv_size.saturating_mul(2))
        .ok_or_else(|| BackendError::InvalidInput("i420 total size overflow".to_string()))?;
    if data.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "I420 payload size mismatch: expected {}, got {}",
            expected,
            data.len()
        )));
    }

    let (y_plane, uv_planes) = data.split_at(y_size);
    let (u_plane, v_plane) = uv_planes.split_at(uv_size);
    let mut out = vec![0_u8; y_size.saturating_mul(4)];
    for y in 0..height {
        for x in 0..width {
            let y_sample = y_plane[y * width + x] as f32;
            let uv_index = (y / 2) * (width / 2) + (x / 2);
            let u_sample = u_plane[uv_index] as f32;
            let v_sample = v_plane[uv_index] as f32;
            let (r, g, b) = yuv_to_rgb(y_sample, u_sample, v_sample);
            let dst = (y * width + x) * 4;
            out[dst] = 255;
            out[dst + 1] = r;
            out[dst + 2] = g;
            out[dst + 3] = b;
        }
    }
    Ok(out)
}

fn nv12_to_argb(data: &[u8], width: usize, height: usize) -> Result<Vec<u8>, BackendError> {
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "decoded frame dimensions must be positive".to_string(),
        ));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "NV12 decode currently requires even frame dimensions".to_string(),
        ));
    }

    let y_size = width
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    let uv_size = y_size / 2;
    let expected = y_size
        .checked_add(uv_size)
        .ok_or_else(|| BackendError::InvalidInput("nv12 total size overflow".to_string()))?;
    if data.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "NV12 payload size mismatch: expected {}, got {}",
            expected,
            data.len()
        )));
    }

    let (y_plane, uv_plane) = data.split_at(y_size);
    let mut out = vec![0_u8; y_size.saturating_mul(4)];
    for y in 0..height {
        for x in 0..width {
            let y_sample = y_plane[y * width + x] as f32;
            let uv_base = (y / 2) * width + (x & !1);
            let u_sample = uv_plane[uv_base] as f32;
            let v_sample = uv_plane[uv_base + 1] as f32;
            let (r, g, b) = yuv_to_rgb(y_sample, u_sample, v_sample);
            let dst = (y * width + x) * 4;
            out[dst] = 255;
            out[dst + 1] = r;
            out[dst + 2] = g;
            out[dst + 3] = b;
        }
    }
    Ok(out)
}

fn yuv_to_rgb(y: f32, u: f32, v: f32) -> (u8, u8, u8) {
    let c = y - 16.0;
    let d = u - 128.0;
    let e = v - 128.0;
    let r = (1.164 * c + 1.596 * e).round().clamp(0.0, 255.0) as u8;
    let g = (1.164 * c - 0.392 * d - 0.813 * e)
        .round()
        .clamp(0.0, 255.0) as u8;
    let b = (1.164 * c + 2.017 * d).round().clamp(0.0, 255.0) as u8;
    (r, g, b)
}

fn bump_decode_pts(next_pts_90k: &mut i64, fps: i32) -> i64 {
    let current = *next_pts_90k;
    let step = decode_pts_step(fps);
    *next_pts_90k = next_pts_90k.saturating_add(step);
    current
}

fn decode_pts_step(fps: i32) -> i64 {
    if fps > 0 {
        (90_000 / i64::from(fps)).max(1)
    } else {
        3_000
    }
}

async fn encode_with_onevpl(
    pending_frames: &[Frame],
    width: usize,
    height: usize,
    codec: Codec,
    fps: i32,
    options: &IntelEncoderOptions,
    use_hardware: bool,
) -> Result<Vec<EncodedPacket>, BackendError> {
    let width_u16 = u16::try_from(width).map_err(|_| {
        BackendError::InvalidInput(format!("frame width {} exceeds oneVPL limits", width))
    })?;
    let height_u16 = u16::try_from(height).map_err(|_| {
        BackendError::InvalidInput(format!("frame height {} exceeds oneVPL limits", height))
    })?;
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "frame dimensions must be positive".to_string(),
        ));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "Intel backend currently requires even frame dimensions".to_string(),
        ));
    }
    #[cfg(feature = "unstable-raw-inputs")]
    let use_nv12_input = {
        let has_argb = pending_frames.iter().any(|frame| frame.argb.is_some());
        let has_nv12 = pending_frames.iter().any(|frame| frame.nv12.is_some());
        if has_argb && has_nv12 {
            return Err(BackendError::InvalidInput(
                "Intel backend does not support mixing ARGB and NV12 frames in one flush cycle"
                    .to_string(),
            ));
        }
        if !has_argb && !has_nv12 {
            return Err(BackendError::InvalidInput(
                "Intel backend requires ARGB or NV12 frame payload".to_string(),
            ));
        }
        has_nv12
    };
    #[cfg(not(feature = "unstable-raw-inputs"))]
    let use_nv12_input = false;

    let mut loader =
        onevpl::Loader::new().map_err(|status| map_onevpl_status(status, "Loader::new"))?;
    loader.use_hardware(use_hardware);
    loader.require_encoder(to_onevpl_codec(codec));
    loader.use_api_version(2, 2);

    let session = loader
        .new_session(0)
        .map_err(|status| map_onevpl_status(status, "Loader::new_session"))?;

    let fps_numerator = u32::try_from(fps.max(1)).unwrap_or(30);
    let target_kbps = options
        .target_kbps
        .unwrap_or_else(|| default_target_kbps(width, height, fps_numerator));
    let input_pic_struct = PicStruct::Progressive;
    let default_rate_control_method = default_rate_control_method(codec);
    let rate_control_method = std::env::var("VIDEO_HW_INTEL_RATE_CONTROL")
        .ok()
        .and_then(|raw| parse_rate_control_method(&raw))
        .unwrap_or(default_rate_control_method);
    let hevc_low_power =
        codec == Codec::Hevc && env_bool("VIDEO_HW_INTEL_HEVC_LOW_POWER").unwrap_or(true);
    let async_depth = std::env::var("VIDEO_HW_INTEL_ASYNC_DEPTH")
        .ok()
        .and_then(|raw| raw.parse::<u16>().ok())
        .filter(|depth| (1..=16).contains(depth))
        .unwrap_or(10);
    let hw_width = onevpl::utils::hw_align_width(width_u16);
    let hw_height = onevpl::utils::hw_align_height(height_u16, input_pic_struct);
    let mut encode_params = onevpl::MfxVideoParams::default();
    encode_params.set_codec(to_onevpl_codec(codec));
    encode_params.set_target_usage(TargetUsage::Level7);
    encode_params.set_rate_control_method(rate_control_method);
    encode_params.set_target_kbps(target_kbps);
    encode_params.set_framerate(fps_numerator, 1);
    encode_params.set_async_depth(async_depth);
    encode_params.set_gop_ref_dist(1);
    encode_params.set_num_ref_frame(1);
    encode_params.set_chroma_format(ChromaFormat::YUV420);
    if matches!(rate_control_method, RateControlMethod::CQP) {
        let qp = std::env::var("VIDEO_HW_INTEL_CQP")
            .ok()
            .and_then(|raw| raw.parse::<u16>().ok())
            .map(|value| value.clamp(1, 51))
            .unwrap_or(24);
        encode_params.set_qpi(qp);
        encode_params.set_qpp(qp);
    }
    {
        // Some runtimes reject AVC init unless FrameInfo.PicStruct is explicitly set.
        let raw = &mut **encode_params;
        raw.__bindgen_anon_1.mfx.FrameInfo.PicStruct = input_pic_struct.repr() as u16;
        if codec == Codec::Hevc {
            raw.__bindgen_anon_1.mfx.LowPower = if hevc_low_power {
                CodingOptionValue::On.repr() as u16
            } else {
                CodingOptionValue::Off.repr() as u16
            };
        }
    }
    if let Some(gop_length) = options.gop_length {
        encode_params.set_gop_pic_size(gop_length);
    }

    let mut vpp = None;
    let mut vpp_input_fourcc = None;
    let mut hw_direct_rgb4 = false;
    let mut hevc_cpu_nv12 = false;
    if use_hardware {
        let hevc_direct_rgb4 =
            codec == Codec::Hevc && env_bool("VIDEO_HW_INTEL_HEVC_DIRECT_RGB4").unwrap_or(false);
        let hevc_force_vpp = codec == Codec::Hevc
            && options
                .hevc_use_vpp
                .unwrap_or_else(|| env_bool("VIDEO_HW_INTEL_HEVC_USE_VPP").unwrap_or(false));
        if use_nv12_input || (codec == Codec::Hevc && !hevc_direct_rgb4 && !hevc_force_vpp) {
            encode_params.set_fourcc(FourCC::NV12);
            encode_params.set_io_pattern(IoPattern::IN_SYSTEM_MEMORY);
            encode_params.set_height(hw_height);
            encode_params.set_width(hw_width);
            encode_params.set_crop(width_u16, height_u16);
            hevc_cpu_nv12 = codec == Codec::Hevc && !use_nv12_input;
        } else if codec == Codec::H264 || hevc_direct_rgb4 {
            encode_params.set_fourcc(FourCC::Rgb4OrBgra);
            encode_params.set_io_pattern(IoPattern::IN_SYSTEM_MEMORY);
            encode_params.set_height(hw_height);
            encode_params.set_width(hw_width);
            encode_params.set_crop(width_u16, height_u16);
            hw_direct_rgb4 = true;
        } else {
            encode_params.set_fourcc(FourCC::NV12);
            encode_params.set_io_pattern(IoPattern::IN_VIDEO_MEMORY);
            encode_params.set_height(hw_height);
            encode_params.set_width(hw_width);
            encode_params.set_crop(width_u16, height_u16);

            let build_vpp_params = |in_fourcc: FourCC| {
                let mut vpp_params = VppVideoParams::default();
                vpp_params
                    .set_io_pattern(IoPattern::IN_SYSTEM_MEMORY | IoPattern::OUT_VIDEO_MEMORY);
                vpp_params.set_in_fourcc(in_fourcc);
                vpp_params.set_in_picstruct(input_pic_struct);
                vpp_params.set_in_height(hw_height);
                vpp_params.set_in_width(hw_width);
                vpp_params.set_in_crop(0, 0, width_u16, height_u16);
                vpp_params.set_in_framerate(fps_numerator, 1);
                vpp_params.set_out_fourcc(FourCC::NV12);
                vpp_params.set_out_picstruct(PicStruct::Progressive);
                vpp_params.set_out_height(hw_height);
                vpp_params.set_out_width(hw_width);
                vpp_params.set_out_crop(0, 0, width_u16, height_u16);
                vpp_params.set_out_framerate(fps_numerator, 1);
                vpp_params
            };

            let mut bgra_vpp_params = build_vpp_params(FourCC::Rgb4OrBgra);
            match session.video_processor(&mut bgra_vpp_params) {
                Ok(processor) => {
                    vpp = Some(processor);
                    vpp_input_fourcc = Some(FourCC::Rgb4OrBgra);
                }
                Err(
                    MfxStatus::Unsupported
                    | MfxStatus::NotImplemented
                    | MfxStatus::IncompatibleVideoParam,
                ) => {
                    let mut yv12_vpp_params = build_vpp_params(FourCC::YV12);
                    match session.video_processor(&mut yv12_vpp_params) {
                        Ok(processor) => {
                            vpp = Some(processor);
                            vpp_input_fourcc = Some(FourCC::YV12);
                        }
                        Err(
                            MfxStatus::Unsupported
                            | MfxStatus::NotImplemented
                            | MfxStatus::IncompatibleVideoParam,
                        ) => {
                            return Err(BackendError::UnsupportedConfig(
                                "Intel hardware encode requires oneVPL VPP (BGRA/YV12 -> NV12), but this runtime does not expose a compatible VPP path".to_string(),
                            ));
                        }
                        Err(status) => {
                            return Err(map_onevpl_status(
                                status,
                                "Session::video_processor (YV12 fallback)",
                            ));
                        }
                    }
                }
                Err(status) => {
                    return Err(map_onevpl_status(status, "Session::video_processor"));
                }
            }
        }
    } else {
        if use_nv12_input {
            encode_params.set_fourcc(FourCC::NV12);
        } else {
            encode_params.set_fourcc(FourCC::IyuvOrI420);
        }
        encode_params.set_io_pattern(IoPattern::IN_SYSTEM_MEMORY);
        encode_params.set_height(height_u16);
        encode_params.set_width(width_u16);
        encode_params.set_crop(width_u16, height_u16);
    }

    let mut encoder = match session.encoder(encode_params) {
        Ok(encoder) => encoder,
        Err(MfxStatus::InvalidVideoParam) if use_hardware => {
            return Err(BackendError::UnsupportedConfig(format!(
                "Intel hardware encoder rejected {} parameters (Session::encoder: InvalidVideoParam); this runtime may not expose hardware encode for the requested codec/profile",
                codec
            )));
        }
        Err(status) => return Err(map_onevpl_status(status, "Session::encoder")),
    };
    let encoder_params = encoder
        .params()
        .map_err(|status| map_onevpl_status(status, "Encoder::params"))?;

    let min_bitstream_capacity = if codec == Codec::Hevc {
        2 * 1024 * 1024
    } else {
        512 * 1024
    };
    let buffer_size = encoder_params
        .suggested_buffer_size()
        .max(min_bitstream_capacity);
    let mut backing_buffer = vec![0_u8; buffer_size];
    let mut bitstream = Bitstream::with_codec(&mut backing_buffer, to_onevpl_codec(codec));
    let mut packets = Vec::new();
    let mut pending_pts = VecDeque::new();
    let aligned_width = usize::from(hw_width);
    let aligned_height = usize::from(hw_height);

    for frame in pending_frames {
        let mut ctrl = EncodeCtrl::new();
        if frame.force_keyframe {
            ctrl.set_frame_type(FrameType::I | FrameType::IDR | FrameType::REF);
        }

        pending_pts.push_back(frame.pts_90k);
        if use_nv12_input {
            #[cfg(feature = "unstable-raw-inputs")]
            {
                let nv12 = frame.nv12.as_ref().ok_or_else(|| {
                    BackendError::InvalidInput(
                        "Intel backend requires NV12 frame payload".to_string(),
                    )
                })?;
                let mut input_surface = encoder
                    .get_surface()
                    .map_err(|status| map_onevpl_status(status, "Encoder::get_surface"))?;
                write_nv12_to_surface(&mut input_surface, nv12, width, height)?;
                match encoder
                    .encode(&mut ctrl, Some(input_surface), &mut bitstream, None)
                    .await
                {
                    Ok(_) => {
                        collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?
                    }
                    Err(MfxStatus::MoreData) => {}
                    Err(status) => {
                        return Err(map_onevpl_status(status, "Encoder::encode (nv12-input)"));
                    }
                }
                continue;
            }
            #[cfg(not(feature = "unstable-raw-inputs"))]
            {
                return Err(BackendError::UnsupportedConfig(
                    "NV12 input payloads require unstable-raw-inputs feature".to_string(),
                ));
            }
        }
        if hevc_cpu_nv12 {
            let argb = frame.argb.as_deref().ok_or_else(|| {
                BackendError::InvalidInput("Intel backend requires ARGB frame payload".to_string())
            })?;
            let mut input_surface = encoder
                .get_surface()
                .map_err(|status| map_onevpl_status(status, "Encoder::get_surface"))?;
            write_argb_to_nv12_surface(&mut input_surface, argb, width, height)?;
            match encoder
                .encode(&mut ctrl, Some(input_surface), &mut bitstream, None)
                .await
            {
                Ok(_) => collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?,
                Err(MfxStatus::MoreData) => {}
                Err(status) => {
                    return Err(map_onevpl_status(
                        status,
                        "Encoder::encode (hardware hevc-nv12)",
                    ));
                }
            }
            continue;
        }
        let argb = frame.argb.as_deref().ok_or_else(|| {
            BackendError::InvalidInput("Intel backend requires ARGB frame payload".to_string())
        })?;
        if hw_direct_rgb4 {
            let mut input_surface = encoder
                .get_surface()
                .map_err(|status| map_onevpl_status(status, "Encoder::get_surface"))?;
            write_argb_to_bgra_surface(&mut input_surface, argb, width, height)?;
            match encoder
                .encode(&mut ctrl, Some(input_surface), &mut bitstream, None)
                .await
            {
                Ok(_) => collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?,
                Err(MfxStatus::MoreData) => {}
                Err(status) => {
                    return Err(map_onevpl_status(
                        status,
                        "Encoder::encode (hardware direct-rgb4)",
                    ));
                }
            }
            continue;
        }
        match vpp_input_fourcc {
            Some(FourCC::Rgb4OrBgra) => {
                let mut input_surface = vpp
                    .as_mut()
                    .ok_or_else(|| BackendError::Backend("VPP should be initialized".to_string()))?
                    .get_surface_input()
                    .map_err(|status| map_onevpl_status(status, "VPP::get_surface_input"))?;
                write_argb_to_bgra_surface(&mut input_surface, argb, width, height)?;
                let vpp_frame = vpp
                    .as_ref()
                    .ok_or_else(|| BackendError::Backend("VPP should be initialized".to_string()))?
                    .process(Some(&mut input_surface), None)
                    .await
                    .map_err(|status| map_onevpl_status(status, "VPP::process"))?;
                match encoder
                    .encode(&mut ctrl, Some(vpp_frame), &mut bitstream, None)
                    .await
                {
                    Ok(_) => {
                        collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?
                    }
                    Err(MfxStatus::MoreData) => {}
                    Err(status) => {
                        return Err(map_onevpl_status(status, "Encoder::encode (hardware)"));
                    }
                }
            }
            Some(FourCC::YV12) => {
                let converted = argb_to_yv12(argb, width, height, aligned_width, aligned_height)?;
                let mut input_surface = vpp
                    .as_mut()
                    .ok_or_else(|| BackendError::Backend("VPP should be initialized".to_string()))?
                    .get_surface_input()
                    .map_err(|status| map_onevpl_status(status, "VPP::get_surface_input"))?;
                let mut input_cursor = Cursor::new(converted.as_slice());
                input_surface
                    .read_raw_frame(&mut input_cursor, FourCC::YV12)
                    .await
                    .map_err(|status| map_onevpl_status(status, "FrameSurface::read_raw_frame"))?;
                let vpp_frame = vpp
                    .as_ref()
                    .ok_or_else(|| BackendError::Backend("VPP should be initialized".to_string()))?
                    .process(Some(&mut input_surface), None)
                    .await
                    .map_err(|status| map_onevpl_status(status, "VPP::process"))?;
                match encoder
                    .encode(&mut ctrl, Some(vpp_frame), &mut bitstream, None)
                    .await
                {
                    Ok(_) => {
                        collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?
                    }
                    Err(MfxStatus::MoreData) => {}
                    Err(status) => {
                        return Err(map_onevpl_status(status, "Encoder::encode (hardware)"));
                    }
                }
            }
            Some(other) => {
                return Err(BackendError::UnsupportedConfig(format!(
                    "unsupported VPP input fourcc configured for Intel encoder: {other:?}"
                )));
            }
            None => {
                let converted = argb_to_i420(argb, width, height, width, height)?;
                let mut input_surface = encoder
                    .get_surface()
                    .map_err(|status| map_onevpl_status(status, "Encoder::get_surface"))?;
                let mut input_cursor = Cursor::new(converted.as_slice());
                input_surface
                    .read_raw_frame(&mut input_cursor, FourCC::IyuvOrI420)
                    .await
                    .map_err(|status| map_onevpl_status(status, "FrameSurface::read_raw_frame"))?;
                match encoder
                    .encode(&mut ctrl, Some(input_surface), &mut bitstream, None)
                    .await
                {
                    Ok(_) => {
                        collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?
                    }
                    Err(MfxStatus::MoreData) => {}
                    Err(status) => {
                        return Err(map_onevpl_status(status, "Encoder::encode (software)"));
                    }
                }
            }
        }
    }

    loop {
        let mut ctrl = EncodeCtrl::new();
        match encoder.encode(&mut ctrl, None, &mut bitstream, None).await {
            Ok(_) => collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?,
            Err(MfxStatus::MoreData) => break,
            Err(status) => return Err(map_onevpl_status(status, "Encoder::encode drain")),
        }
    }

    Ok(packets)
}

fn collect_packets(
    bitstream: &mut Bitstream<'_>,
    codec: Codec,
    pending_pts: &mut VecDeque<Option<i64>>,
    packets: &mut Vec<EncodedPacket>,
) -> Result<(), BackendError> {
    let data_len = bitstream.size() as usize;
    if data_len == 0 {
        return Ok(());
    }

    if bitstream.offset() > 0 {
        bitstream.write_all(&[]).map_err(|err| {
            BackendError::Backend(format!(
                "failed to normalize encoded bitstream offset before read: {err}"
            ))
        })?;
    }
    let mut data = vec![0_u8; data_len];
    bitstream.read_exact(&mut data).map_err(|err| {
        BackendError::Backend(format!("failed to read encoded bitstream payload: {err}"))
    })?;
    let frame_type = bitstream.frame_type();
    let key_flags = FrameType::I | FrameType::IDR | FrameType::XI | FrameType::XIDR;
    let fallback_pts = pending_pts.pop_front().flatten();
    let pts_90k = i64::try_from(bitstream.timestamp()).ok().or(fallback_pts);
    packets.push(EncodedPacket {
        codec,
        data,
        pts_90k,
        is_keyframe: frame_type.intersects(key_flags),
    });
    Ok(())
}

#[cfg(feature = "unstable-raw-inputs")]
fn write_nv12_to_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    nv12: &Nv12FramePayload,
    width: usize,
    height: usize,
) -> Result<(), BackendError> {
    if nv12.pitch < width {
        return Err(BackendError::InvalidInput(format!(
            "nv12 pitch is smaller than width: pitch={}, width={}",
            nv12.pitch, width
        )));
    }
    if !nv12.pitch.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(format!(
            "nv12 pitch must be even, got {}",
            nv12.pitch
        )));
    }
    let expected_y = nv12
        .pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 Y size overflow".to_string()))?;
    let expected_uv = nv12
        .pitch
        .checked_mul(height / 2)
        .ok_or_else(|| BackendError::InvalidInput("nv12 UV size overflow".to_string()))?;
    let expected_total = expected_y
        .checked_add(expected_uv)
        .ok_or_else(|| BackendError::InvalidInput("nv12 total size overflow".to_string()))?;
    if nv12.data.len() != expected_total {
        return Err(BackendError::InvalidInput(format!(
            "nv12 payload size mismatch: expected {}, got {}",
            expected_total,
            nv12.data.len()
        )));
    }
    copy_nv12_to_surface(surface, nv12.data.as_slice(), nv12.pitch, width, height)
}

/// Returns a mutable slice over the full interleaved NV12 UV plane
/// (`(height / 2) * pitch` bytes).
///
/// # Safety
///
/// For system-memory NV12 surfaces, `Data.U` is the start of a contiguous
/// interleaved UV plane of `(crop_height / 2) * pitch` bytes, where `V = U + 1`.
/// The onevpl `u()` accessor exposes only `(crop_height / 2) * (pitch / 2)` bytes
/// (half the plane) because the library models U and V as planar with half-pitch.
/// Extending the slice to `(height / 2) * pitch` is valid as long as `height <=
/// crop_height` (verified by the caller) and the surface is mapped for writing.
unsafe fn nv12_uv_plane_full_mut<'a>(
    surface: &'a mut onevpl::FrameSurface<'_>,
    height: usize,
    pitch: usize,
) -> &'a mut [u8] {
    let u_plane = surface.u();
    let full_len = (height / 2) * pitch;
    // SAFETY: see function-level doc. Caller guarantees height <= crop_height
    // and surface is mapped WRITE. The u_plane pointer is valid for full_len bytes.
    unsafe { std::slice::from_raw_parts_mut(u_plane.as_mut_ptr(), full_len) }
}

#[cfg(feature = "unstable-raw-inputs")]
fn copy_nv12_to_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    nv12: &[u8],
    source_pitch: usize,
    width: usize,
    height: usize,
) -> Result<(), BackendError> {
    surface
        .map(MemoryFlag::WRITE)
        .map_err(|status| map_onevpl_status(status, "FrameSurface::map"))?;
    let conversion_result = (|| {
        let bounds = surface.bounds();
        let pitch = usize::from(bounds.pitch);
        if !pitch.is_multiple_of(2) {
            return Err(BackendError::Backend(format!(
                "unexpected NV12 surface pitch (not 2-byte aligned): {}",
                bounds.pitch
            )));
        }
        if pitch < width {
            return Err(BackendError::Backend(format!(
                "unexpected NV12 surface pitch (smaller than width): pitch={}, width={width}",
                bounds.pitch
            )));
        }
        if usize::from(bounds.crop_width) < width || usize::from(bounds.crop_height) < height {
            return Err(BackendError::Backend(format!(
                "NV12 surface crop is smaller than input: crop={}x{}, input={}x{}",
                bounds.crop_width, bounds.crop_height, width, height
            )));
        }

        let y_plane = surface.y();
        y_plane.fill(16);
        for row in 0..height {
            let src_start = row * source_pitch;
            let dst_start = row * pitch;
            y_plane[dst_start..(dst_start + width)]
                .copy_from_slice(&nv12[src_start..(src_start + width)]);
        }

        let src_uv_base = source_pitch
            .checked_mul(height)
            .ok_or_else(|| BackendError::InvalidInput("nv12 Y size overflow".to_string()))?;
        let uv_width = width / 2;
        // SAFETY: height <= crop_height, surface mapped WRITE.
        let uv_plane = unsafe { nv12_uv_plane_full_mut(surface, height, pitch) };
        uv_plane.fill(0x80);
        for row in 0..(height / 2) {
            let src_row = &nv12
                [(src_uv_base + row * source_pitch)..(src_uv_base + row * source_pitch + width)];
            let dst_row = &mut uv_plane[(row * pitch)..(row * pitch + width)];
            for col in 0..uv_width {
                let base = col * 2;
                dst_row[base] = src_row[base]; // U
                dst_row[base + 1] = src_row[base + 1]; // V
            }
        }

        Ok(())
    })();

    let unmap_result = surface
        .unmap()
        .map_err(|status| map_onevpl_status(status, "FrameSurface::unmap"));

    conversion_result?;
    unmap_result
}

#[derive(Debug)]
struct ArgbToNv12Tables {
    y_r: [i32; 256],
    y_g: [i32; 256],
    y_b: [i32; 256],
    u_r: [i32; 256],
    u_g: [i32; 256],
    u_b: [i32; 256],
    v_r: [i32; 256],
    v_g: [i32; 256],
    v_b: [i32; 256],
}

static ARGB_TO_NV12_TABLES: OnceLock<ArgbToNv12Tables> = OnceLock::new();

fn argb_to_yuv_tables() -> &'static ArgbToNv12Tables {
    ARGB_TO_NV12_TABLES.get_or_init(|| {
        let mut tables = ArgbToNv12Tables {
            y_r: [0; 256],
            y_g: [0; 256],
            y_b: [0; 256],
            u_r: [0; 256],
            u_g: [0; 256],
            u_b: [0; 256],
            v_r: [0; 256],
            v_g: [0; 256],
            v_b: [0; 256],
        };
        for idx in 0..256 {
            let value = idx as i32;
            tables.y_r[idx] = 66 * value;
            tables.y_g[idx] = 129 * value;
            tables.y_b[idx] = 25 * value;
            tables.u_r[idx] = -38 * value;
            tables.u_g[idx] = -74 * value;
            tables.u_b[idx] = 112 * value;
            tables.v_r[idx] = 112 * value;
            tables.v_g[idx] = -94 * value;
            tables.v_b[idx] = -18 * value;
        }
        tables
    })
}

#[inline]
fn clip_to_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[inline]
fn argb_pixel_to_nv12_yuv(tables: &ArgbToNv12Tables, r: u8, g: u8, b: u8) -> (u8, i32, i32) {
    let r = usize::from(r);
    let g = usize::from(g);
    let b = usize::from(b);
    let y = ((tables.y_r[r] + tables.y_g[g] + tables.y_b[b] + 128) >> 8) + 16;
    let u = ((tables.u_r[r] + tables.u_g[g] + tables.u_b[b] + 128) >> 8) + 128;
    let v = ((tables.v_r[r] + tables.v_g[g] + tables.v_b[b] + 128) >> 8) + 128;
    (
        clip_to_u8(y),
        i32::from(clip_to_u8(u)),
        i32::from(clip_to_u8(v)),
    )
}

fn write_argb_to_nv12_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    argb: &[u8],
    width: usize,
    height: usize,
) -> Result<(), BackendError> {
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("argb size overflow".to_string()))?;
    if argb.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "argb payload size mismatch: expected {}, got {}",
            expected,
            argb.len()
        )));
    }
    surface
        .map(MemoryFlag::WRITE)
        .map_err(|status| map_onevpl_status(status, "FrameSurface::map"))?;
    let conversion_result = (|| {
        let bounds = surface.bounds();
        let pitch = usize::from(bounds.pitch);
        if !pitch.is_multiple_of(2) {
            return Err(BackendError::Backend(format!(
                "unexpected NV12 surface pitch (not 2-byte aligned): {}",
                bounds.pitch
            )));
        }
        if pitch < width {
            return Err(BackendError::Backend(format!(
                "unexpected NV12 surface pitch (smaller than width): pitch={}, width={width}",
                bounds.pitch
            )));
        }
        if usize::from(bounds.crop_width) < width || usize::from(bounds.crop_height) < height {
            return Err(BackendError::Backend(format!(
                "NV12 surface crop is smaller than input: crop={}x{}, input={}x{}",
                bounds.crop_width, bounds.crop_height, width, height
            )));
        }

        let tables = argb_to_yuv_tables();
        let y_plane = surface.y();
        y_plane.fill(16);
        // SAFETY: height <= crop_height (checked above), surface is mapped WRITE.
        let uv_plane = unsafe { nv12_uv_plane_full_mut(surface, height, pitch) };
        uv_plane.fill(0x80);

        if width.is_multiple_of(2) && height.is_multiple_of(2) {
            let y_rows_len = pitch * height;
            let uv_rows_len = pitch * (height / 2);
            y_plane[..y_rows_len]
                .par_chunks_mut(pitch * 2)
                .zip(uv_plane[..uv_rows_len].par_chunks_mut(pitch))
                .enumerate()
                .for_each(|(row_pair, (y_rows, uv_row))| {
                    let (y_row0, y_row1) = y_rows.split_at_mut(pitch);
                    let src_row0 = row_pair * width * 8;
                    let src_row1 = src_row0 + width * 4;
                    for x in (0..width).step_by(2) {
                        let src00 = src_row0 + x * 4;
                        let src01 = src00 + 4;
                        let src10 = src_row1 + x * 4;
                        let src11 = src10 + 4;

                        let (y00, u00, v00) = argb_pixel_to_nv12_yuv(
                            tables,
                            argb[src00 + 1],
                            argb[src00 + 2],
                            argb[src00 + 3],
                        );
                        let (y01, u01, v01) = argb_pixel_to_nv12_yuv(
                            tables,
                            argb[src01 + 1],
                            argb[src01 + 2],
                            argb[src01 + 3],
                        );
                        let (y10, u10, v10) = argb_pixel_to_nv12_yuv(
                            tables,
                            argb[src10 + 1],
                            argb[src10 + 2],
                            argb[src10 + 3],
                        );
                        let (y11, u11, v11) = argb_pixel_to_nv12_yuv(
                            tables,
                            argb[src11 + 1],
                            argb[src11 + 2],
                            argb[src11 + 3],
                        );

                        y_row0[x] = y00;
                        y_row0[x + 1] = y01;
                        y_row1[x] = y10;
                        y_row1[x + 1] = y11;

                        let uv_col = x / 2;
                        uv_row[2 * uv_col] = clip_to_u8((u00 + u01 + u10 + u11) / 4);
                        uv_row[2 * uv_col + 1] = clip_to_u8((v00 + v01 + v10 + v11) / 4);
                    }
                });
        } else {
            for y in (0..height).step_by(2) {
                let uv_row_base = (y / 2) * pitch;
                for x in (0..width).step_by(2) {
                    let mut u_acc = 0_i32;
                    let mut v_acc = 0_i32;
                    let mut sample_count = 0_i32;

                    for dy in 0..2 {
                        let py = y + dy;
                        if py >= height {
                            continue;
                        }
                        let y_row = py * pitch;
                        for dx in 0..2 {
                            let px = x + dx;
                            if px >= width {
                                continue;
                            }
                            let src = (py * width + px) * 4;
                            let (yy, uu, vv) = argb_pixel_to_nv12_yuv(
                                tables,
                                argb[src + 1],
                                argb[src + 2],
                                argb[src + 3],
                            );
                            y_plane[y_row + px] = yy;
                            u_acc += uu;
                            v_acc += vv;
                            sample_count += 1;
                        }
                    }

                    let uv_col = x / 2;
                    let denom = sample_count.max(1);
                    uv_plane[uv_row_base + 2 * uv_col] = clip_to_u8(u_acc / denom);
                    uv_plane[uv_row_base + 2 * uv_col + 1] = clip_to_u8(v_acc / denom);
                }
            }
        }

        Ok(())
    })();

    let unmap_result = surface
        .unmap()
        .map_err(|status| map_onevpl_status(status, "FrameSurface::unmap"));

    conversion_result?;
    unmap_result
}

fn argb_to_i420(
    argb: &[u8],
    width: usize,
    height: usize,
    aligned_width: usize,
    aligned_height: usize,
) -> Result<Vec<u8>, BackendError> {
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("argb size overflow".to_string()))?;
    if argb.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "argb payload size mismatch: expected {}, got {}",
            expected,
            argb.len()
        )));
    }
    if aligned_width < width || aligned_height < height {
        return Err(BackendError::InvalidInput(
            "aligned dimensions cannot be smaller than input dimensions".to_string(),
        ));
    }
    if !aligned_width.is_multiple_of(2) || !aligned_height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "aligned dimensions must be even for I420".to_string(),
        ));
    }

    let y_size = aligned_width
        .checked_mul(aligned_height)
        .ok_or_else(|| BackendError::InvalidInput("i420 luma size overflow".to_string()))?;
    let uv_width = aligned_width / 2;
    let uv_height = aligned_height / 2;
    let uv_size = uv_width
        .checked_mul(uv_height)
        .ok_or_else(|| BackendError::InvalidInput("i420 chroma size overflow".to_string()))?;

    let mut y_plane = vec![16_u8; y_size];
    let mut u_acc = vec![0_u32; uv_size];
    let mut v_acc = vec![0_u32; uv_size];
    let mut uv_count = vec![0_u32; uv_size];

    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 4;
            let r = argb[src + 1] as f32;
            let g = argb[src + 2] as f32;
            let b = argb[src + 3] as f32;

            let yy = (0.257 * r + 0.504 * g + 0.098 * b + 16.0).round() as i32;
            let uu = (-0.148 * r - 0.291 * g + 0.439 * b + 128.0).round() as i32;
            let vv = (0.439 * r - 0.368 * g - 0.071 * b + 128.0).round() as i32;

            y_plane[y * aligned_width + x] = yy.clamp(0, 255) as u8;
            let uv_index = (y / 2) * uv_width + (x / 2);
            u_acc[uv_index] = u_acc[uv_index].saturating_add(uu.clamp(0, 255) as u32);
            v_acc[uv_index] = v_acc[uv_index].saturating_add(vv.clamp(0, 255) as u32);
            uv_count[uv_index] = uv_count[uv_index].saturating_add(1);
        }
    }

    let mut u_plane = vec![128_u8; uv_size];
    let mut v_plane = vec![128_u8; uv_size];
    for idx in 0..uv_size {
        if uv_count[idx] > 0 {
            u_plane[idx] = (u_acc[idx] / uv_count[idx]) as u8;
            v_plane[idx] = (v_acc[idx] / uv_count[idx]) as u8;
        }
    }

    let mut out = Vec::with_capacity(y_size + uv_size * 2);
    out.extend_from_slice(&y_plane);
    out.extend_from_slice(&u_plane);
    out.extend_from_slice(&v_plane);
    Ok(out)
}

fn write_argb_to_bgra_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    argb: &[u8],
    width: usize,
    height: usize,
) -> Result<(), BackendError> {
    surface
        .map(MemoryFlag::WRITE)
        .map_err(|status| map_onevpl_status(status, "FrameSurface::map"))?;

    let conversion_result = (|| {
        let bounds = surface.bounds();
        let pitch_bytes = usize::from(bounds.pitch);
        if !pitch_bytes.is_multiple_of(4) {
            return Err(BackendError::Backend(format!(
                "unexpected BGRA surface pitch (not 4-byte aligned): {}",
                bounds.pitch
            )));
        }

        let aligned_width = pitch_bytes / 4;
        let aligned_height = usize::from(bounds.crop_height);
        let out = surface.b();
        argb_to_bgra_inplace(argb, width, height, aligned_width, aligned_height, out)
    })();

    let unmap_result = surface
        .unmap()
        .map_err(|status| map_onevpl_status(status, "FrameSurface::unmap"));

    conversion_result?;
    unmap_result
}

fn argb_to_bgra_inplace(
    argb: &[u8],
    width: usize,
    height: usize,
    aligned_width: usize,
    aligned_height: usize,
    out: &mut [u8],
) -> Result<(), BackendError> {
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("argb size overflow".to_string()))?;
    if argb.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "argb payload size mismatch: expected {}, got {}",
            expected,
            argb.len()
        )));
    }
    if aligned_width < width || aligned_height < height {
        return Err(BackendError::InvalidInput(
            "aligned dimensions cannot be smaller than input dimensions".to_string(),
        ));
    }

    let required_size = aligned_width
        .checked_mul(aligned_height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("BGRA buffer size overflow".to_string()))?;
    if out.len() != required_size {
        return Err(BackendError::InvalidInput(format!(
            "BGRA output buffer size mismatch: expected {}, got {}",
            required_size,
            out.len()
        )));
    }

    for y in 0..height {
        let src_row_start = y
            .checked_mul(width)
            .and_then(|offset| offset.checked_mul(4))
            .ok_or_else(|| BackendError::InvalidInput("ARGB row offset overflow".to_string()))?;
        let dst_row_start = y
            .checked_mul(aligned_width)
            .and_then(|offset| offset.checked_mul(4))
            .ok_or_else(|| BackendError::InvalidInput("BGRA row offset overflow".to_string()))?;
        let src_row_end = src_row_start
            .checked_add(width.saturating_mul(4))
            .ok_or_else(|| BackendError::InvalidInput("ARGB row end overflow".to_string()))?;
        let dst_row_end = dst_row_start
            .checked_add(width.saturating_mul(4))
            .ok_or_else(|| BackendError::InvalidInput("BGRA row end overflow".to_string()))?;
        let src_row = &argb[src_row_start..src_row_end];
        let dst_row = &mut out[dst_row_start..dst_row_end];
        for (src_px, dst_px) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
            let packed = u32::from_le_bytes([src_px[0], src_px[1], src_px[2], src_px[3]]);
            dst_px.copy_from_slice(&packed.swap_bytes().to_le_bytes());
        }

        let padded_row_end = dst_row_start
            .checked_add(aligned_width.saturating_mul(4))
            .ok_or_else(|| {
                BackendError::InvalidInput("BGRA padded row end overflow".to_string())
            })?;
        if dst_row_end < padded_row_end {
            out[dst_row_end..padded_row_end].fill(0);
        }
    }

    Ok(())
}

fn argb_to_yv12(
    argb: &[u8],
    width: usize,
    height: usize,
    aligned_width: usize,
    aligned_height: usize,
) -> Result<Vec<u8>, BackendError> {
    let i420 = argb_to_i420(argb, width, height, aligned_width, aligned_height)?;
    i420_to_yv12(&i420, aligned_width, aligned_height)
}

fn i420_to_yv12(
    i420: &[u8],
    aligned_width: usize,
    aligned_height: usize,
) -> Result<Vec<u8>, BackendError> {
    if !aligned_width.is_multiple_of(2) || !aligned_height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "aligned dimensions must be even for YV12".to_string(),
        ));
    }
    let y_size = aligned_width
        .checked_mul(aligned_height)
        .ok_or_else(|| BackendError::InvalidInput("YV12 luma size overflow".to_string()))?;
    let uv_size = y_size / 4;
    let expected = y_size
        .checked_add(uv_size.saturating_mul(2))
        .ok_or_else(|| BackendError::InvalidInput("I420 size overflow".to_string()))?;
    if i420.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "I420 payload size mismatch for YV12 conversion: expected {}, got {}",
            expected,
            i420.len()
        )));
    }

    let (y_plane, uv_planes) = i420.split_at(y_size);
    let (u_plane, v_plane) = uv_planes.split_at(uv_size);
    // YV12 stores planes as Y + V + U (I420 is Y + U + V).
    let mut out = Vec::with_capacity(y_size.saturating_add(uv_size.saturating_mul(2)));
    out.extend_from_slice(y_plane);
    out.extend_from_slice(v_plane);
    out.extend_from_slice(u_plane);
    Ok(out)
}

fn to_onevpl_codec(codec: Codec) -> OneVplCodec {
    match codec {
        Codec::H264 => OneVplCodec::AVC,
        Codec::Hevc => OneVplCodec::HEVC,
        Codec::Av1 => OneVplCodec::AV1,
    }
}

fn default_target_kbps(width: usize, height: usize, fps: u32) -> u16 {
    let pixels_per_second = width
        .saturating_mul(height)
        .saturating_mul(usize::try_from(fps).unwrap_or(30));
    let target = (pixels_per_second / 3_000).clamp(800, usize::from(u16::MAX));
    u16::try_from(target).unwrap_or(u16::MAX)
}

fn parse_rate_control_method(raw: &str) -> Option<RateControlMethod> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "cbr" => Some(RateControlMethod::CBR),
        "vbr" => Some(RateControlMethod::VBR),
        "cqp" => Some(RateControlMethod::CQP),
        "avbr" => Some(RateControlMethod::AVBR),
        "icq" => Some(RateControlMethod::ICQ),
        "qvbr" => Some(RateControlMethod::QVBR),
        _ => None,
    }
}

fn default_rate_control_method(codec: Codec) -> RateControlMethod {
    match codec {
        Codec::H264 => RateControlMethod::CBR,
        Codec::Hevc | Codec::Av1 => RateControlMethod::CQP,
    }
}

fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

fn map_onevpl_status(status: MfxStatus, context: &str) -> BackendError {
    match status {
        MfxStatus::MoreData
        | MfxStatus::MoreBitstream
        | MfxStatus::DeviceBusy
        | MfxStatus::InExecution
        | MfxStatus::TaskBusy
        | MfxStatus::TaskWorking
        | MfxStatus::AllocTimeoutExpired => {
            BackendError::TemporaryBackpressure(format!("{context}: {status:?}"))
        }
        MfxStatus::DeviceLost | MfxStatus::GpuHang => {
            BackendError::DeviceLost(format!("{context}: {status:?}"))
        }
        MfxStatus::Unsupported
        | MfxStatus::NotInitialized
        | MfxStatus::NotFound
        | MfxStatus::NotImplemented
        | MfxStatus::IncompatibleVideoParam
        | MfxStatus::PartialAcceleration => {
            BackendError::UnsupportedConfig(format!("{context}: {status:?}"))
        }
        MfxStatus::InvalidVideoParam if context.starts_with("Session::encoder") => {
            BackendError::UnsupportedConfig(format!("{context}: {status:?}"))
        }
        MfxStatus::InvalidVideoParam
        | MfxStatus::InvalidHandle
        | MfxStatus::NotEnoughBuffer
        | MfxStatus::OutOfRange
        | MfxStatus::NullPtr => BackendError::InvalidInput(format!("{context}: {status:?}")),
        _ => BackendError::Backend(format!("{context}: {status:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{argb_pixel_to_nv12_yuv, argb_to_yuv_tables, i420_to_argb, nv12_to_argb};

    #[test]
    fn nv12_to_argb_neutral_chroma_produces_grayscale() {
        let nv12 = vec![
            16, 82, //
            145, 235, //
            128, 128, // UV
        ];
        let argb = nv12_to_argb(&nv12, 2, 2).expect("nv12->argb should succeed");
        assert_eq!(argb.len(), 16);

        let pixels = argb.chunks_exact(4).collect::<Vec<_>>();
        assert_eq!(pixels[0], &[255, 0, 0, 0]);
        assert!(pixels[3][1] >= 250 && pixels[3][2] >= 250 && pixels[3][3] >= 250);

        for px in pixels {
            assert_eq!(px[1], px[2]);
            assert_eq!(px[2], px[3]);
        }
    }

    #[test]
    fn i420_to_argb_neutral_chroma_produces_grayscale() {
        let i420 = vec![
            16, 82, //
            145, 235, //
            128, // U
            128, // V
        ];
        let argb = i420_to_argb(&i420, 2, 2).expect("i420->argb should succeed");
        assert_eq!(argb.len(), 16);

        let pixels = argb.chunks_exact(4).collect::<Vec<_>>();
        assert_eq!(pixels[0], &[255, 0, 0, 0]);
        assert!(pixels[3][1] >= 250 && pixels[3][2] >= 250 && pixels[3][3] >= 250);

        for px in pixels {
            assert_eq!(px[1], px[2]);
            assert_eq!(px[2], px[3]);
        }
    }

    #[test]
    fn argb_pixel_to_nv12_yuv_matches_reference_formula() {
        let tables = argb_to_yuv_tables();
        for r in (0_u8..=255).step_by(17) {
            for g in (0_u8..=255).step_by(17) {
                for b in (0_u8..=255).step_by(17) {
                    let (y, u, v) = argb_pixel_to_nv12_yuv(tables, r, g, b);
                    let rr = i32::from(r);
                    let gg = i32::from(g);
                    let bb = i32::from(b);
                    let expected_y = ((66 * rr + 129 * gg + 25 * bb + 128) >> 8) + 16;
                    let expected_u = ((-38 * rr - 74 * gg + 112 * bb + 128) >> 8) + 128;
                    let expected_v = ((112 * rr - 94 * gg - 18 * bb + 128) >> 8) + 128;
                    assert_eq!(y, expected_y.clamp(0, 255) as u8);
                    assert_eq!(u, expected_u.clamp(0, 255));
                    assert_eq!(v, expected_v.clamp(0, 255));
                }
            }
        }
    }
}
