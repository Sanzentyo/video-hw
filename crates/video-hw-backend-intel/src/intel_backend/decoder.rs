//! Intel oneVPL decoder adapter and decode-session orchestration.

use super::*;

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
            | BackendDecoderOptions::Vulkan(_)
            | BackendDecoderOptions::Android(_) => IntelDecoderOptions::default(),
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
        Ok(intel_capability_report(codec))
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

    let mut loader = configured_onevpl_loader(codec, OneVplOperation::Decode, use_hardware)?;

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

pub(super) fn bump_decode_pts(next_pts_90k: &mut i64, fps: i32) -> i64 {
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
