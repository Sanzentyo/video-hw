use std::collections::VecDeque;
use std::mem;
use std::pin::Pin;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use cudarc::driver::CudaContext;
use nvidia_video_codec_sdk::{
    DecodeCodec, Encoder, EncoderInitParams, ErrorKind, ReconfigureParams,
};

use crate::bitstream::{AccessUnit, StatefulBitstreamAssembler};
use crate::nv_meta_decoder::NvMetaDecoder;
use crate::pipeline_scheduler::PipelineScheduler;
use crate::{
    BackendEncoderOptions, BackendError, CapabilityReport, Codec, DecodeSummary, DecoderConfig,
    EncodedPacket, Frame, NvidiaSessionConfig, SessionSwitchMode, SessionSwitchRequest,
    VideoDecoder, VideoEncoder,
};

#[derive(Debug, Default)]
pub struct AnnexBPacker {
    data: Vec<u8>,
}

impl AnnexBPacker {
    fn pack<'a>(&'a mut self, access_unit: &AccessUnit) -> &'a [u8] {
        self.data.clear();
        let total_size: usize = access_unit
            .nalus
            .iter()
            .map(|nal| nal.len().saturating_add(4))
            .sum();
        self.data
            .reserve(total_size.saturating_sub(self.data.capacity()));

        for nal in &access_unit.nalus {
            self.data.extend_from_slice(&[0, 0, 0, 1]);
            self.data.extend_from_slice(nal);
        }

        &self.data
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct StageTiming {
    pack: Duration,
    sdk: Duration,
    upload: Duration,
    synth: Duration,
    output_lock: Duration,
    reap: Duration,
}

#[derive(Debug)]
struct DecodeReapSummary {
    frames: Vec<Frame>,
    map_samples: SampleStats,
    queue_depth_samples: SampleStats,
    jitter_samples: SampleStats,
}

#[derive(Debug, Default, Clone)]
struct SampleStats {
    samples: Vec<f64>,
}

impl SampleStats {
    fn push_duration_ms(&mut self, value: Duration) {
        self.samples.push(value.as_secs_f64() * 1_000.0);
    }

    fn push_value(&mut self, value: f64) {
        self.samples.push(value);
    }

    fn mean(&self) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        self.samples.iter().sum::<f64>() / self.samples.len() as f64
    }

    fn percentile(&self, percentile: f64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        let rank = ((percentile / 100.0) * n as f64)
            .ceil()
            .clamp(1.0, n as f64) as usize;
        sorted[rank - 1]
    }

    fn p95(&self) -> f64 {
        self.percentile(95.0)
    }

    fn p99(&self) -> f64 {
        self.percentile(99.0)
    }

    fn peak(&self) -> f64 {
        self.samples
            .iter()
            .copied()
            .max_by(f64::total_cmp)
            .unwrap_or(0.0)
    }
}

fn should_report_metrics() -> bool {
    std::env::var("VIDEO_HW_NV_METRICS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn should_use_safe_lifetime_mode() -> bool {
    std::env::var("VIDEO_HW_NV_SAFE_LIFETIME")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NvBufferLifetimeMode {
    ReusablePoolUnsafe,
    PerFrameSafe,
}

pub struct NvDecoderAdapter {
    config: DecoderConfig,
    assembler: StatefulBitstreamAssembler,
    packer: AnnexBPacker,
    decoder: Option<NvMetaDecoder>,
    next_pts_90k: i64,
    last_summary: DecodeSummary,
}

impl NvDecoderAdapter {
    pub fn new(config: DecoderConfig) -> Self {
        Self {
            assembler: StatefulBitstreamAssembler::with_codec(config.codec),
            packer: AnnexBPacker::default(),
            config,
            decoder: None,
            next_pts_90k: 0,
            last_summary: DecodeSummary {
                decoded_frames: 0,
                width: None,
                height: None,
                pixel_format: None,
            },
        }
    }

    fn ensure_decoder(&mut self) -> Result<(), BackendError> {
        if self.decoder.is_some() {
            return Ok(());
        }

        let cuda_ctx = CudaContext::new(0).map_err(|err| {
            BackendError::UnsupportedConfig(format!("failed to initialize CUDA context: {err}"))
        })?;
        let decoder = NvMetaDecoder::new(cuda_ctx, to_decode_codec(self.config.codec))?;

        self.decoder = Some(decoder);
        Ok(())
    }

    fn decode_access_units(
        &mut self,
        access_units: &[AccessUnit],
        fallback_pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        if access_units.is_empty() {
            return Ok(Vec::new());
        }

        self.ensure_decoder()?;
        let mut timing = StageTiming::default();
        let mut pack_samples = SampleStats::default();
        let mut sdk_samples = SampleStats::default();
        let expected_frame_ms = if self.config.fps > 0 {
            1_000.0 / self.config.fps as f64
        } else {
            33.333
        };
        let mut frames = Vec::new();
        let mut map_samples = SampleStats::default();
        let mut queue_depth_samples = SampleStats::default();
        let mut jitter_samples = SampleStats::default();
        let mut last_pts_90k = None;

        for au in access_units {
            let pts_90k = if let Some(pts) = au.pts_90k.or(fallback_pts_90k) {
                pts
            } else {
                self.bump_pts_90k()
            };
            let pack_start = Instant::now();
            let packed = self.packer.pack(au);
            let pack_elapsed = pack_start.elapsed();
            timing.pack += pack_elapsed;
            pack_samples.push_duration_ms(pack_elapsed);

            let decode_start = Instant::now();
            let decoded = {
                let decoder = self.decoder.as_mut().ok_or_else(|| {
                    BackendError::Backend("decoder should be initialized".to_string())
                })?;
                decoder.push_access_unit(packed, pts_90k)?
            };
            let sdk_elapsed = decode_start.elapsed();
            timing.sdk += sdk_elapsed;
            sdk_samples.push_duration_ms(sdk_elapsed);
            self.apply_decoded_summary(&decoded);

            queue_depth_samples.push_value(decoded.len() as f64);
            let map_start = Instant::now();
            for frame in decoded {
                update_jitter_samples(
                    &mut jitter_samples,
                    &mut last_pts_90k,
                    frame.pts_90k,
                    expected_frame_ms,
                );
                frames.push(frame);
            }
            map_samples.push_duration_ms(map_start.elapsed());
        }
        let reap_summary = DecodeReapSummary {
            frames,
            map_samples,
            queue_depth_samples,
            jitter_samples,
        };

        if should_report_metrics() {
            eprintln!(
                "[nv.decode] access_units={}, frames={}, pack_ms={:.3}, sdk_ms={:.3}, map_ms={:.3}, pack_p95_ms={:.3}, pack_p99_ms={:.3}, sdk_p95_ms={:.3}, sdk_p99_ms={:.3}, map_p95_ms={:.3}, map_p99_ms={:.3}, queue_depth_peak={:.0}, queue_depth_p95={:.3}, queue_depth_p99={:.3}, jitter_ms_mean={:.3}, jitter_ms_p95={:.3}, jitter_ms_p99={:.3}",
                access_units.len(),
                reap_summary.frames.len(),
                timing.pack.as_secs_f64() * 1_000.0,
                timing.sdk.as_secs_f64() * 1_000.0,
                reap_summary.map_samples.samples.iter().sum::<f64>(),
                pack_samples.p95(),
                pack_samples.p99(),
                sdk_samples.p95(),
                sdk_samples.p99(),
                reap_summary.map_samples.p95(),
                reap_summary.map_samples.p99(),
                reap_summary.queue_depth_samples.peak(),
                reap_summary.queue_depth_samples.p95(),
                reap_summary.queue_depth_samples.p99(),
                reap_summary.jitter_samples.mean(),
                reap_summary.jitter_samples.p95(),
                reap_summary.jitter_samples.p99()
            );
        }

        Ok(reap_summary.frames)
    }

    fn bump_pts_90k(&mut self) -> i64 {
        let current = self.next_pts_90k;
        let step = if self.config.fps > 0 {
            (90_000 / i64::from(self.config.fps)).max(1)
        } else {
            3_000
        };
        self.next_pts_90k = self.next_pts_90k.saturating_add(step);
        current
    }

    fn apply_decoded_summary(&mut self, decoded: &[Frame]) {
        self.last_summary.decoded_frames = self
            .last_summary
            .decoded_frames
            .saturating_add(decoded.len());

        if let Some(last) = decoded.last() {
            self.last_summary.width = Some(last.width);
            self.last_summary.height = Some(last.height);
        }
    }
}

impl VideoDecoder for NvDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec,
            decode_supported: matches!(codec, Codec::H264 | Codec::Hevc),
            encode_supported: matches!(codec, Codec::H264 | Codec::Hevc),
            hardware_acceleration: true,
        })
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        let (access_units, _cache) =
            self.assembler
                .push_chunk(chunk, self.config.codec, pts_90k)?;
        self.decode_access_units(&access_units, pts_90k)
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        let (access_units, _cache) = self.assembler.flush()?;
        let mut frames = self.decode_access_units(&access_units, None)?;

        if let Some(decoder) = self.decoder.as_mut() {
            let drained = decoder.flush()?;
            self.apply_decoded_summary(&drained);
            frames.extend(drained);
        }

        Ok(frames)
    }

    fn decode_summary(&self) -> DecodeSummary {
        self.last_summary.clone()
    }
}

pub struct NvEncoderAdapter {
    codec: Codec,
    fps: i32,
    require_hardware: bool,
    max_in_flight_outputs: usize,
    gop_length: Option<u32>,
    frame_interval_p: Option<i32>,
    cuda_ctx: Option<Arc<CudaContext>>,
    active_session: Option<NvEncodeSession>,
    session_reconfigure_pending: bool,
    pending_switch: Option<PendingSessionSwitch>,
    active_generation: u64,
    config_generation: u64,
    next_generation: u64,
    pending_frames: Vec<Frame>,
    force_next_keyframe: bool,
    width: Option<usize>,
    height: Option<usize>,
    buffer_lifetime_mode: NvBufferLifetimeMode,
}

impl NvEncoderAdapter {
    pub fn with_config(
        codec: Codec,
        fps: i32,
        require_hardware: bool,
        backend_options: BackendEncoderOptions,
    ) -> Self {
        let (max_in_flight_outputs, gop_length, frame_interval_p) = match backend_options {
            BackendEncoderOptions::Nvidia(options) => (
                options.max_in_flight_outputs.clamp(1, 64),
                options.gop_length,
                options.frame_interval_p,
            ),
            BackendEncoderOptions::Default => (6, None, None),
        };
        Self {
            codec,
            fps,
            require_hardware,
            max_in_flight_outputs,
            gop_length,
            frame_interval_p,
            cuda_ctx: None,
            active_session: None,
            session_reconfigure_pending: false,
            pending_switch: None,
            active_generation: 0,
            config_generation: 1,
            next_generation: 2,
            pending_frames: Vec::new(),
            force_next_keyframe: false,
            width: None,
            height: None,
            buffer_lifetime_mode: if should_use_safe_lifetime_mode() {
                NvBufferLifetimeMode::PerFrameSafe
            } else {
                NvBufferLifetimeMode::ReusablePoolUnsafe
            },
        }
    }

    #[allow(dead_code)]
    pub fn active_generation(&self) -> u64 {
        self.active_generation
    }

    #[allow(dead_code)]
    pub fn configured_generation(&self) -> u64 {
        self.config_generation
    }

    #[allow(dead_code)]
    pub fn pending_switch_generation(&self) -> Option<u64> {
        self.pending_switch.as_ref().map(|p| p.target_generation)
    }

    #[allow(dead_code)]
    pub fn sync_pipeline_generation(&self, scheduler: &PipelineScheduler) {
        let generation = self
            .pending_switch_generation()
            .unwrap_or_else(|| self.configured_generation());
        scheduler.set_generation(generation.max(1));
    }

    fn ensure_cuda_ctx(&mut self) -> Result<Arc<CudaContext>, BackendError> {
        if let Some(ctx) = &self.cuda_ctx {
            return Ok(Arc::clone(ctx));
        }
        let ctx = CudaContext::new(0).map_err(|err| {
            BackendError::UnsupportedConfig(format!("failed to initialize CUDA context: {err}"))
        })?;
        self.cuda_ctx = Some(Arc::clone(&ctx));
        Ok(ctx)
    }

    fn build_session(
        &mut self,
        width: usize,
        height: usize,
        generation: u64,
    ) -> Result<NvEncodeSession, BackendError> {
        let _ = self.require_hardware;

        let cuda_ctx = self.ensure_cuda_ctx()?;

        let encoder = Encoder::initialize_with_cuda(cuda_ctx).map_err(map_encode_error)?;
        let encode_guid = to_encode_guid(self.codec);

        let encode_guids = encoder.get_encode_guids().map_err(map_encode_error)?;
        if !encode_guids.contains(&encode_guid) {
            return Err(BackendError::UnsupportedCodec(self.codec));
        }
        let input_layout = NvInputLayout::Argb;

        let preset_guid = nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_PRESET_P1_GUID;
        let tuning_info =
            nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;

        let mut preset_config = encoder
            .get_preset_config(encode_guid, preset_guid, tuning_info)
            .map_err(map_encode_error)?;
        if let Some(gop_length) = self.gop_length {
            preset_config.presetCfg.gopLength = gop_length;
        }
        if let Some(frame_interval_p) = self.frame_interval_p {
            preset_config.presetCfg.frameIntervalP = frame_interval_p;
        }
        let frame_interval_p = usize::try_from(preset_config.presetCfg.frameIntervalP).unwrap_or(1);
        let lookahead_depth = usize::from(preset_config.presetCfg.rcParams.lookaheadDepth);
        let pool_size = frame_interval_p
            .saturating_add(lookahead_depth)
            .saturating_add(1)
            .max(3);

        let mut init_params = EncoderInitParams::new(encode_guid, width as u32, height as u32);
        init_params
            .preset_guid(preset_guid)
            .tuning_info(tuning_info)
            .display_aspect_ratio(16, 9)
            .framerate(self.fps.max(1) as u32, 1)
            .enable_picture_type_decision()
            .encode_config(&mut preset_config.presetCfg);

        let session = encoder
            .start_session(
                nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB,
                init_params,
            )
            .map_err(map_encode_error)?;

        NvEncodeSession::new(
            session,
            width,
            height,
            generation,
            self.buffer_lifetime_mode,
            input_layout,
            pool_size.max(self.max_in_flight_outputs),
        )
    }

    fn try_reconfigure_active_session(
        &mut self,
        force_idr: bool,
        target_generation: u64,
    ) -> Result<bool, BackendError> {
        let Some(session) = self.active_session.as_mut() else {
            return Ok(false);
        };
        session.reconfigure(
            self.codec,
            self.fps,
            self.gop_length,
            self.frame_interval_p,
            force_idr,
        )?;
        session.generation = target_generation;
        self.active_generation = target_generation;
        self.config_generation = target_generation;
        self.next_generation = self
            .next_generation
            .max(target_generation.saturating_add(1));
        self.session_reconfigure_pending = false;
        Ok(true)
    }

    fn ensure_session(
        &mut self,
        width: usize,
        height: usize,
    ) -> Result<&mut NvEncodeSession, BackendError> {
        let needs_recreate = match &self.active_session {
            Some(session) => {
                session.width != width
                    || session.height != height
                    || self.session_reconfigure_pending
            }
            None => true,
        };

        if needs_recreate {
            let generation = self.config_generation.max(1);
            self.active_session = Some(self.build_session(width, height, generation)?);
            self.active_generation = generation;
            self.next_generation = self.next_generation.max(generation.saturating_add(1));
            self.session_reconfigure_pending = false;
        }
        self.active_session
            .as_mut()
            .ok_or_else(|| BackendError::Backend("active NVENC session is missing".to_string()))
    }
}

impl VideoEncoder for NvEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec,
            decode_supported: matches!(codec, Codec::H264 | Codec::Hevc),
            encode_supported: matches!(codec, Codec::H264 | Codec::Hevc),
            hardware_acceleration: true,
        })
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        let mut frame = frame;
        if self.pending_switch.is_some() && frame.force_keyframe {
            self.apply_pending_switch_if_needed()?;
        }
        if self.force_next_keyframe {
            frame.force_keyframe = true;
            self.force_next_keyframe = false;
            self.apply_pending_switch_if_needed()?;
        }
        if frame.width == 0 || frame.height == 0 {
            return Err(BackendError::InvalidInput(
                "frame dimensions must be positive".to_string(),
            ));
        }

        if let Some(width) = self.width {
            if frame.width != width {
                return Err(BackendError::InvalidInput(
                    "all frames in one flush cycle must have the same width".to_string(),
                ));
            }
        } else {
            self.width = Some(frame.width);
        }

        if let Some(height) = self.height {
            if frame.height != height {
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
        self.apply_pending_switch_if_needed()?;

        let pending_frames = std::mem::take(&mut self.pending_frames);
        let width = self.width.take().unwrap_or(640);
        let height = self.height.take().unwrap_or(360);
        let max_in_flight = self.max_in_flight_outputs;
        let fps = self.fps;
        let codec = self.codec;
        let session = self.ensure_session(width, height)?;
        if session.buffer_lifetime_mode == NvBufferLifetimeMode::PerFrameSafe {
            return Self::flush_safe_per_frame(
                session,
                &pending_frames,
                width,
                height,
                fps,
                codec,
                max_in_flight,
            );
        }
        let input_layout = session.input_layout;
        let mut pending_outputs = VecDeque::<PendingOutput>::new();
        let mut packets = Vec::new();
        let mut timing = StageTiming::default();
        let mut output_depth_peak = 0usize;
        let mut queue_depth_samples = SampleStats::default();
        let mut output_jitter_samples = SampleStats::default();
        let expected_frame_ms = if fps > 0 {
            1_000.0 / fps as f64
        } else {
            33.333
        };
        let mut last_output_pts_90k = None;
        let (ready_tx, ready_rx) = mpsc::channel::<PendingOutput>();
        let (reaped_tx, reaped_rx) = mpsc::channel::<Result<ReapedOutput, BackendError>>();
        let mut dispatched_outputs = 0usize;

        std::thread::scope(|scope| -> Result<(), BackendError> {
            let reaper = scope.spawn(move || {
                while let Ok(pending) = ready_rx.recv() {
                    let lock_start = Instant::now();
                    let result =
                        lock_output_packet(codec, pending).map(|(packet, pair)| ReapedOutput {
                            packet,
                            pair,
                            lock_elapsed: lock_start.elapsed(),
                        });
                    if reaped_tx.send(result).is_err() {
                        break;
                    }
                }
            });

            for (index, frame) in pending_frames.iter().enumerate() {
                while session.available_pairs() == 0 {
                    let pending = pending_outputs.pop_front().ok_or_else(|| {
                        BackendError::Backend(
                            "buffer pool exhausted without pending output to reap".to_string(),
                        )
                    })?;
                    ready_tx.send(pending).map_err(|_| {
                        BackendError::Backend("encode reap channel disconnected".to_string())
                    })?;
                    dispatched_outputs = dispatched_outputs.saturating_add(1);

                    let result = reaped_rx.recv().map_err(|_| {
                        BackendError::Backend("encode reap result channel disconnected".to_string())
                    })?;
                    let reaped = result?;
                    timing.output_lock += reaped.lock_elapsed;
                    timing.reap += reaped.lock_elapsed;
                    update_jitter_samples(
                        &mut output_jitter_samples,
                        &mut last_output_pts_90k,
                        reaped.packet.pts_90k,
                        expected_frame_ms,
                    );
                    packets.push(reaped.packet);
                    session.checkin_pair(reaped.pair);
                    queue_depth_samples.push_value(pending_outputs.len() as f64);
                }
                let mut pair = session.checkout_pair()?;
                let synth_start = Instant::now();
                let _ = input_layout;
                let argb = frame
                    .argb
                    .clone()
                    .unwrap_or_else(|| make_synthetic_argb(width, height, index));
                if argb.len() != width.saturating_mul(height).saturating_mul(4) {
                    return Err(BackendError::InvalidInput(format!(
                        "argb payload size mismatch: expected {}, got {}",
                        width.saturating_mul(height).saturating_mul(4),
                        argb.len()
                    )));
                }
                timing.synth += synth_start.elapsed();
                {
                    let upload_start = Instant::now();
                    let mut lock = pair.input.lock().map_err(map_encode_error)?;
                    unsafe {
                        lock.write(&argb);
                    }
                    timing.upload += upload_start.elapsed();
                }
                let input_timestamp = frame
                    .pts_90k
                    .unwrap_or_else(|| (index as i64).saturating_mul(3_000))
                    .max(0) as u64;

                let encode_start = Instant::now();
                let encode_pic_flags = if frame.force_keyframe {
                    nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_PIC_FLAGS::NV_ENC_PIC_FLAG_FORCEIDR as u32
                } else {
                    0
                };
                let produced_output = match session.session.encode_picture(
                    &mut pair.input,
                    &mut pair.output,
                    nvidia_video_codec_sdk::EncodePictureParams {
                        input_timestamp,
                        encode_pic_flags,
                        ..Default::default()
                    },
                ) {
                    Ok(()) => true,
                    Err(err) if err.kind() == ErrorKind::NeedMoreInput => false,
                    Err(err) => return Err(map_encode_error(err)),
                };
                timing.sdk += encode_start.elapsed();

                pending_outputs.push_back(PendingOutput {
                    pair,
                    pts_90k: frame.pts_90k,
                    is_keyframe: index == 0,
                });
                output_depth_peak = output_depth_peak.max(pending_outputs.len());
                queue_depth_samples.push_value(pending_outputs.len() as f64);

                if produced_output {
                    while pending_outputs.len() >= max_in_flight {
                        let pending = pending_outputs.pop_front().ok_or_else(|| {
                            BackendError::Backend(
                                "missing pending output after encode submission".to_string(),
                            )
                        })?;
                        ready_tx.send(pending).map_err(|_| {
                            BackendError::Backend("encode reap channel disconnected".to_string())
                        })?;
                        dispatched_outputs = dispatched_outputs.saturating_add(1);
                        while let Ok(result) = reaped_rx.try_recv() {
                            let reaped = result?;
                            timing.output_lock += reaped.lock_elapsed;
                            timing.reap += reaped.lock_elapsed;
                            update_jitter_samples(
                                &mut output_jitter_samples,
                                &mut last_output_pts_90k,
                                reaped.packet.pts_90k,
                                expected_frame_ms,
                            );
                            packets.push(reaped.packet);
                            session.checkin_pair(reaped.pair);
                            queue_depth_samples.push_value(pending_outputs.len() as f64);
                        }
                    }
                }
            }

            session.session.end_of_stream().map_err(map_encode_error)?;

            while let Some(pending) = pending_outputs.pop_front() {
                ready_tx.send(pending).map_err(|_| {
                    BackendError::Backend("encode reap channel disconnected".to_string())
                })?;
                dispatched_outputs = dispatched_outputs.saturating_add(1);
            }
            drop(ready_tx);

            while packets.len() < dispatched_outputs {
                let result = reaped_rx.recv().map_err(|_| {
                    BackendError::Backend("encode reap result channel disconnected".to_string())
                })?;
                let reaped = result?;
                timing.output_lock += reaped.lock_elapsed;
                timing.reap += reaped.lock_elapsed;
                update_jitter_samples(
                    &mut output_jitter_samples,
                    &mut last_output_pts_90k,
                    reaped.packet.pts_90k,
                    expected_frame_ms,
                );
                packets.push(reaped.packet);
                session.checkin_pair(reaped.pair);
                queue_depth_samples.push_value(pending_outputs.len() as f64);
            }

            reaper
                .join()
                .map_err(|_| BackendError::Backend("encode reap worker panicked".to_string()))?;
            Ok(())
        })?;

        if should_report_metrics() {
            eprintln!(
                "[nv.encode] frames={}, packets={}, queue_peak={}, max_in_flight={}, synth_ms={:.3}, upload_ms={:.3}, submit_ms={:.3}, reap_ms={:.3}, encode_ms={:.3}, lock_ms={:.3}, queue_p95={:.3}, queue_p99={:.3}, jitter_ms_mean={:.3}, jitter_ms_p95={:.3}, jitter_ms_p99={:.3}",
                pending_frames.len(),
                packets.len(),
                output_depth_peak,
                max_in_flight,
                timing.synth.as_secs_f64() * 1_000.0,
                timing.upload.as_secs_f64() * 1_000.0,
                timing.sdk.as_secs_f64() * 1_000.0,
                timing.reap.as_secs_f64() * 1_000.0,
                timing.sdk.as_secs_f64() * 1_000.0,
                timing.output_lock.as_secs_f64() * 1_000.0,
                queue_depth_samples.p95(),
                queue_depth_samples.p99(),
                output_jitter_samples.mean(),
                output_jitter_samples.p95(),
                output_jitter_samples.p99()
            );
        }

        Ok(packets)
    }

    fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        let SessionSwitchRequest::Nvidia { config, mode } = request;
        self.apply_nvidia_session_switch(config, mode)
    }

    fn pipeline_generation_hint(&self) -> Option<u64> {
        Some(
            self.pending_switch
                .as_ref()
                .map(|p| p.target_generation)
                .unwrap_or(self.config_generation)
                .max(1),
        )
    }
}

impl NvEncoderAdapter {
    fn flush_safe_per_frame(
        session: &mut NvEncodeSession,
        pending_frames: &[Frame],
        width: usize,
        height: usize,
        fps: i32,
        codec: Codec,
        max_in_flight: usize,
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        let mut packets = Vec::with_capacity(pending_frames.len());
        let mut timing = StageTiming::default();
        let mut queue_depth_samples = SampleStats::default();
        let mut output_jitter_samples = SampleStats::default();
        let expected_frame_ms = if fps > 0 {
            1_000.0 / fps as f64
        } else {
            33.333
        };
        let mut last_output_pts_90k = None;
        let pool_size = max_in_flight.clamp(1, 64).max(3);
        let mut free_pairs = VecDeque::with_capacity(pool_size);
        for _ in 0..pool_size {
            let input = session
                .session
                .as_ref()
                .get_ref()
                .create_input_buffer()
                .map_err(map_encode_error)?;
            let output = session
                .session
                .as_ref()
                .get_ref()
                .create_output_bitstream()
                .map_err(map_encode_error)?;
            free_pairs.push_back(SafeBufferPair { input, output });
        }
        let mut pending_outputs = VecDeque::new();

        for (index, frame) in pending_frames.iter().enumerate() {
            while free_pairs.is_empty() {
                let pending = pending_outputs.pop_front().ok_or_else(|| {
                    BackendError::Backend(
                        "safe lifetime buffer pool exhausted without pending output".to_string(),
                    )
                })?;
                let (packet, pair) = lock_safe_output_packet(codec, pending)?;
                update_jitter_samples(
                    &mut output_jitter_samples,
                    &mut last_output_pts_90k,
                    packet.pts_90k,
                    expected_frame_ms,
                );
                packets.push(packet);
                free_pairs.push_back(pair);
                queue_depth_samples.push_value(pending_outputs.len() as f64);
            }
            let mut pair = free_pairs.pop_front().ok_or_else(|| {
                BackendError::Backend("safe lifetime buffer pair is missing".to_string())
            })?;

            let synth_start = Instant::now();
            let argb = frame
                .argb
                .clone()
                .unwrap_or_else(|| make_synthetic_argb(width, height, index));
            if argb.len() != width.saturating_mul(height).saturating_mul(4) {
                return Err(BackendError::InvalidInput(format!(
                    "argb payload size mismatch: expected {}, got {}",
                    width.saturating_mul(height).saturating_mul(4),
                    argb.len()
                )));
            }
            timing.synth += synth_start.elapsed();

            let upload_start = Instant::now();
            {
                let mut lock = pair.input.lock().map_err(map_encode_error)?;
                unsafe {
                    lock.write(&argb);
                }
            }
            timing.upload += upload_start.elapsed();

            let input_timestamp = frame
                .pts_90k
                .unwrap_or_else(|| (index as i64).saturating_mul(3_000))
                .max(0) as u64;
            let encode_pic_flags = if frame.force_keyframe {
                nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_PIC_FLAGS::NV_ENC_PIC_FLAG_FORCEIDR
                    as u32
            } else {
                0
            };

            let encode_start = Instant::now();
            match session.session.encode_picture(
                &mut pair.input,
                &mut pair.output,
                nvidia_video_codec_sdk::EncodePictureParams {
                    input_timestamp,
                    encode_pic_flags,
                    ..Default::default()
                },
            ) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::NeedMoreInput => {}
                Err(err) => return Err(map_encode_error(err)),
            }
            timing.sdk += encode_start.elapsed();
            pending_outputs.push_back(SafePendingOutput {
                pair,
                pts_90k: frame.pts_90k,
                is_keyframe: index == 0 || frame.force_keyframe,
            });
            queue_depth_samples.push_value(pending_outputs.len() as f64);
        }

        session.session.end_of_stream().map_err(map_encode_error)?;
        while let Some(pending) = pending_outputs.pop_front() {
            let lock_start = Instant::now();
            let (packet, pair) = lock_safe_output_packet(codec, pending)?;
            timing.output_lock += lock_start.elapsed();
            timing.reap += lock_start.elapsed();
            update_jitter_samples(
                &mut output_jitter_samples,
                &mut last_output_pts_90k,
                packet.pts_90k,
                expected_frame_ms,
            );
            packets.push(packet);
            free_pairs.push_back(pair);
            queue_depth_samples.push_value(pending_outputs.len() as f64);
        }

        if should_report_metrics() {
            eprintln!(
                "[nv.encode.safe] frames={}, packets={}, synth_ms={:.3}, upload_ms={:.3}, submit_ms={:.3}, reap_ms={:.3}, lock_ms={:.3}, queue_p95={:.3}, queue_p99={:.3}, jitter_ms_mean={:.3}, jitter_ms_p95={:.3}, jitter_ms_p99={:.3}",
                pending_frames.len(),
                packets.len(),
                timing.synth.as_secs_f64() * 1_000.0,
                timing.upload.as_secs_f64() * 1_000.0,
                timing.sdk.as_secs_f64() * 1_000.0,
                timing.reap.as_secs_f64() * 1_000.0,
                timing.output_lock.as_secs_f64() * 1_000.0,
                queue_depth_samples.p95(),
                queue_depth_samples.p99(),
                output_jitter_samples.mean(),
                output_jitter_samples.p95(),
                output_jitter_samples.p99()
            );
        }

        Ok(packets)
    }

    fn apply_nvidia_session_switch(
        &mut self,
        config: NvidiaSessionConfig,
        mode: SessionSwitchMode,
    ) -> Result<(), BackendError> {
        match mode {
            SessionSwitchMode::DrainThenSwap => {
                if !self.pending_frames.is_empty() {
                    let _ = self.flush()?;
                }
                let target_generation = self.next_generation;
                self.next_generation = self.next_generation.saturating_add(1);
                self.pending_switch = Some(PendingSessionSwitch {
                    config,
                    mode,
                    target_generation,
                });
                self.apply_pending_switch_if_needed()
            }
            SessionSwitchMode::Immediate | SessionSwitchMode::OnNextKeyframe => {
                let target_generation = self.next_generation;
                self.next_generation = self.next_generation.saturating_add(1);
                self.pending_switch = Some(PendingSessionSwitch {
                    config,
                    mode,
                    target_generation,
                });
                if matches!(mode, SessionSwitchMode::OnNextKeyframe) {
                    self.force_next_keyframe = true;
                }
                if self.pending_frames.is_empty() {
                    self.apply_pending_switch_if_needed()?;
                }
                Ok(())
            }
        }
    }

    fn apply_pending_switch_if_needed(&mut self) -> Result<(), BackendError> {
        let Some(pending) = self.pending_switch.take() else {
            return Ok(());
        };
        self.gop_length = pending.config.gop_length;
        self.frame_interval_p = pending.config.frame_interval_p;
        self.config_generation = pending.target_generation;
        self.session_reconfigure_pending = true;
        if pending.config.force_idr_on_activate
            || matches!(pending.mode, SessionSwitchMode::OnNextKeyframe)
        {
            self.force_next_keyframe = true;
        }

        let force_idr = pending.config.force_idr_on_activate
            || matches!(pending.mode, SessionSwitchMode::OnNextKeyframe);
        if self
            .try_reconfigure_active_session(force_idr, pending.target_generation)
            .is_err()
        {
            self.session_reconfigure_pending = true;
            if matches!(pending.mode, SessionSwitchMode::DrainThenSwap)
                && let Some(existing) = self.active_session.take()
            {
                let width = existing.width;
                let height = existing.height;
                drop(existing);
                self.active_session = Some(self.build_session(width, height, pending.target_generation)?);
                self.active_generation = pending.target_generation;
                self.config_generation = pending.target_generation;
                self.session_reconfigure_pending = false;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum NvInputLayout {
    Argb,
}

struct NvEncodeSession {
    session: Pin<Box<nvidia_video_codec_sdk::Session>>,
    width: usize,
    height: usize,
    generation: u64,
    buffer_lifetime_mode: NvBufferLifetimeMode,
    input_layout: NvInputLayout,
    reusable_inputs: VecDeque<nvidia_video_codec_sdk::Buffer<'static>>,
    reusable_outputs: VecDeque<nvidia_video_codec_sdk::Bitstream<'static>>,
}

impl NvEncodeSession {
    fn new(
        session: nvidia_video_codec_sdk::Session,
        width: usize,
        height: usize,
        generation: u64,
        buffer_lifetime_mode: NvBufferLifetimeMode,
        input_layout: NvInputLayout,
        pool_size: usize,
    ) -> Result<Self, BackendError> {
        let session = Box::pin(session);
        let mut reusable_inputs = VecDeque::with_capacity(pool_size.max(3));
        let mut reusable_outputs = VecDeque::with_capacity(pool_size.max(3));
        if buffer_lifetime_mode == NvBufferLifetimeMode::ReusablePoolUnsafe {
            let session_ref: &nvidia_video_codec_sdk::Session = Pin::as_ref(&session).get_ref();
            for _ in 0..pool_size.max(3) {
                let input = session_ref
                    .create_input_buffer()
                    .map_err(map_encode_error)?;
                let output = session_ref
                    .create_output_bitstream()
                    .map_err(map_encode_error)?;
                // Safety: session is pinned and outlives these buffers. Drop explicitly clears
                // buffers before session is dropped.
                reusable_inputs.push_back(unsafe {
                    mem::transmute::<
                        nvidia_video_codec_sdk::Buffer<'_>,
                        nvidia_video_codec_sdk::Buffer<'static>,
                    >(input)
                });
                reusable_outputs.push_back(unsafe {
                    mem::transmute::<
                        nvidia_video_codec_sdk::Bitstream<'_>,
                        nvidia_video_codec_sdk::Bitstream<'static>,
                    >(output)
                });
            }
        }
        Ok(Self {
            session,
            width,
            height,
            generation,
            buffer_lifetime_mode,
            input_layout,
            reusable_inputs,
            reusable_outputs,
        })
    }

    fn checkout_pair(&mut self) -> Result<BufferPair, BackendError> {
        let input = self.reusable_inputs.pop_front().ok_or_else(|| {
            BackendError::TemporaryBackpressure("no reusable NVENC input buffer".to_string())
        })?;
        let output = self.reusable_outputs.pop_front().ok_or_else(|| {
            BackendError::TemporaryBackpressure("no reusable NVENC output bitstream".to_string())
        })?;
        Ok(BufferPair { input, output })
    }

    fn checkin_pair(&mut self, pair: BufferPair) {
        self.reusable_inputs.push_back(pair.input);
        self.reusable_outputs.push_back(pair.output);
    }

    fn available_pairs(&self) -> usize {
        self.reusable_inputs.len().min(self.reusable_outputs.len())
    }

    fn reconfigure(
        &mut self,
        codec: Codec,
        fps: i32,
        gop_length: Option<u32>,
        frame_interval_p: Option<i32>,
        force_idr: bool,
    ) -> Result<(), BackendError> {
        let encode_guid = to_encode_guid(codec);
        let preset_guid = nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_PRESET_P1_GUID;
        let tuning_info =
            nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;

        let encoder = self.session.as_ref().get_ref().get_encoder();
        let mut preset_config = encoder
            .get_preset_config(encode_guid, preset_guid, tuning_info)
            .map_err(map_encode_error)?;
        if let Some(gop_length) = gop_length {
            preset_config.presetCfg.gopLength = gop_length;
        }
        if let Some(frame_interval_p) = frame_interval_p {
            preset_config.presetCfg.frameIntervalP = frame_interval_p;
        }

        let mut init_params =
            EncoderInitParams::new(encode_guid, self.width as u32, self.height as u32);
        init_params
            .preset_guid(preset_guid)
            .tuning_info(tuning_info)
            .display_aspect_ratio(16, 9)
            .framerate(fps.max(1) as u32, 1)
            .enable_picture_type_decision()
            .encode_config(&mut preset_config.presetCfg);

        self.session
            .as_mut()
            .get_mut()
            .reconfigure(
                ReconfigureParams::new(init_params)
                    .reset_encoder(false)
                    .force_idr(force_idr),
            )
            .map_err(map_encode_error)?;
        Ok(())
    }
}

impl Drop for NvEncodeSession {
    fn drop(&mut self) {
        self.reusable_inputs.clear();
        self.reusable_outputs.clear();
    }
}

struct BufferPair {
    input: nvidia_video_codec_sdk::Buffer<'static>,
    output: nvidia_video_codec_sdk::Bitstream<'static>,
}

#[derive(Debug, Clone)]
struct PendingSessionSwitch {
    config: NvidiaSessionConfig,
    mode: SessionSwitchMode,
    target_generation: u64,
}

struct PendingOutput {
    pair: BufferPair,
    pts_90k: Option<i64>,
    is_keyframe: bool,
}

struct SafeBufferPair<'a> {
    input: nvidia_video_codec_sdk::Buffer<'a>,
    output: nvidia_video_codec_sdk::Bitstream<'a>,
}

struct SafePendingOutput<'a> {
    pair: SafeBufferPair<'a>,
    pts_90k: Option<i64>,
    is_keyframe: bool,
}

struct ReapedOutput {
    packet: EncodedPacket,
    pair: BufferPair,
    lock_elapsed: Duration,
}

fn lock_output_packet(
    codec: Codec,
    pending: PendingOutput,
) -> Result<(EncodedPacket, BufferPair), BackendError> {
    let PendingOutput {
        mut pair,
        pts_90k,
        is_keyframe,
    } = pending;
    let data = {
        let lock = pair.output.lock().map_err(map_encode_error)?;
        lock.data().to_vec()
    };
    Ok((
        EncodedPacket {
            codec,
            data,
            pts_90k,
            is_keyframe,
        },
        pair,
    ))
}

fn lock_safe_output_packet(
    codec: Codec,
    mut pending: SafePendingOutput<'_>,
) -> Result<(EncodedPacket, SafeBufferPair<'_>), BackendError> {
    let data = {
        let lock = pending.pair.output.lock().map_err(map_encode_error)?;
        lock.data().to_vec()
    };
    Ok((
        EncodedPacket {
            codec,
            data,
            pts_90k: pending.pts_90k,
            is_keyframe: pending.is_keyframe,
        },
        pending.pair,
    ))
}

fn to_decode_codec(codec: Codec) -> DecodeCodec {
    match codec {
        Codec::H264 => DecodeCodec::H264,
        Codec::Hevc => DecodeCodec::H265,
    }
}

fn to_encode_guid(codec: Codec) -> nvidia_video_codec_sdk::sys::nvEncodeAPI::GUID {
    match codec {
        Codec::H264 => nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_CODEC_H264_GUID,
        Codec::Hevc => nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_CODEC_HEVC_GUID,
    }
}

fn map_encode_error(error: nvidia_video_codec_sdk::EncodeError) -> BackendError {
    match error.kind() {
        ErrorKind::NeedMoreInput | ErrorKind::EncoderBusy | ErrorKind::LockBusy => {
            BackendError::TemporaryBackpressure(error.to_string())
        }
        ErrorKind::DeviceNotExist => BackendError::DeviceLost(error.to_string()),
        ErrorKind::UnsupportedDevice
        | ErrorKind::UnsupportedParam
        | ErrorKind::NoEncodeDevice
        | ErrorKind::InvalidEncoderDevice => BackendError::UnsupportedConfig(error.to_string()),
        ErrorKind::InvalidParam | ErrorKind::InvalidCall => {
            BackendError::InvalidInput(error.to_string())
        }
        _ => BackendError::Backend(error.to_string()),
    }
}

fn update_jitter_samples(
    jitter_samples: &mut SampleStats,
    last_pts_90k: &mut Option<i64>,
    current_pts_90k: Option<i64>,
    expected_frame_ms: f64,
) {
    let Some(current) = current_pts_90k else {
        return;
    };
    if let Some(previous) = *last_pts_90k {
        let delta_ms = (current.saturating_sub(previous) as f64) / 90.0;
        jitter_samples.push_value((delta_ms - expected_frame_ms).abs());
    }
    *last_pts_90k = Some(current);
}

fn make_synthetic_argb(width: usize, height: usize, frame_index: usize) -> Vec<u8> {
    let mut buffer = vec![0_u8; width.saturating_mul(height).saturating_mul(4)];
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;
            buffer[offset] = ((x + frame_index) % 256) as u8;
            buffer[offset + 1] = ((y + frame_index * 2) % 256) as u8;
            buffer[offset + 2] = ((frame_index * 5) % 256) as u8;
            buffer[offset + 3] = 255;
        }
    }
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend_transform_adapter::NvidiaTransformAdapter;
    use crate::pipeline_scheduler::PipelineScheduler;

    #[test]
    fn switch_on_next_keyframe_stays_pending_when_frames_are_buffered() {
        let mut adapter =
            NvEncoderAdapter::with_config(Codec::H264, 30, true, BackendEncoderOptions::Default);
        adapter.pending_frames.push(Frame {
            width: 640,
            height: 360,
            pixel_format: None,
            pts_90k: Some(0),
            argb: None,
            force_keyframe: false,
        });

        adapter
            .apply_nvidia_session_switch(
                NvidiaSessionConfig {
                    gop_length: Some(60),
                    frame_interval_p: Some(1),
                    force_idr_on_activate: false,
                },
                SessionSwitchMode::OnNextKeyframe,
            )
            .unwrap();

        assert!(adapter.pending_switch.is_some());
        assert!(adapter.force_next_keyframe);
    }

    #[test]
    fn switch_immediate_updates_config_even_without_active_session() {
        let mut adapter =
            NvEncoderAdapter::with_config(Codec::H264, 30, true, BackendEncoderOptions::Default);
        adapter
            .apply_nvidia_session_switch(
                NvidiaSessionConfig {
                    gop_length: Some(48),
                    frame_interval_p: Some(1),
                    force_idr_on_activate: true,
                },
                SessionSwitchMode::Immediate,
            )
            .unwrap();

        assert_eq!(adapter.gop_length, Some(48));
        assert_eq!(adapter.frame_interval_p, Some(1));
        assert!(adapter.force_next_keyframe);
    }

    #[test]
    fn pending_switch_generation_syncs_to_pipeline_scheduler() {
        let scheduler = PipelineScheduler::new(NvidiaTransformAdapter::new(1, 4), 4);
        let mut adapter =
            NvEncoderAdapter::with_config(Codec::H264, 30, true, BackendEncoderOptions::Default);
        adapter
            .apply_nvidia_session_switch(
                NvidiaSessionConfig {
                    gop_length: Some(48),
                    frame_interval_p: Some(1),
                    force_idr_on_activate: false,
                },
                SessionSwitchMode::OnNextKeyframe,
            )
            .unwrap();
        adapter.sync_pipeline_generation(&scheduler);
        assert_eq!(scheduler.generation(), adapter.configured_generation());
    }
}
