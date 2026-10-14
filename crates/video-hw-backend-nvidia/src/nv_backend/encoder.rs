//! NVIDIA encode adapter, buffered flush, and session-switch state transitions.

use super::*;

pub struct NvEncoderAdapter {
    codec: Codec,
    fps: i32,
    require_hardware: bool,
    max_in_flight_outputs: usize,
    pub(super) gop_length: Option<u32>,
    pub(super) frame_interval_p: Option<i32>,
    target_bitrate: Option<u32>,
    input_format: EncodeInputFormat,
    cuda_ctx: Option<Arc<CudaContext>>,
    pub(super) pending_switch: Option<PendingSessionSwitch>,
    config_generation: u64,
    next_generation: u64,
    pub(super) pending_frames: Vec<Frame>,
    pub(super) force_next_keyframe: bool,
    pub(super) width: Option<usize>,
    pub(super) height: Option<usize>,
    report_metrics: bool,
}

impl NvEncoderAdapter {
    pub fn new(config: EncoderConfig) -> Self {
        let EncoderConfig {
            codec,
            fps,
            require_hardware,
            input_format,
            backend_options,
        } = config;
        let options = match backend_options {
            BackendEncoderOptions::Nvidia(options) => options,
            BackendEncoderOptions::Default
            | BackendEncoderOptions::VideoToolbox(_)
            | BackendEncoderOptions::Intel(_)
            | BackendEncoderOptions::Vulkan(_)
            | BackendEncoderOptions::Android(_) => NvidiaEncoderOptions::default(),
        };
        let max_in_flight_outputs = options.max_in_flight_outputs.clamp(1, 64);
        let gop_length = options.gop_length;
        let frame_interval_p = options.frame_interval_p;
        let target_bitrate = options.target_bitrate;
        let report_metrics = options
            .report_metrics
            .or_else(|| env_bool("VIDEO_HW_NV_METRICS"))
            .unwrap_or(false);
        Self {
            codec,
            fps,
            require_hardware,
            max_in_flight_outputs,
            gop_length,
            frame_interval_p,
            target_bitrate,
            input_format,
            cuda_ctx: None,
            pending_switch: None,
            config_generation: 1,
            next_generation: 2,
            pending_frames: Vec::new(),
            force_next_keyframe: false,
            width: None,
            height: None,
            report_metrics,
        }
    }

    pub fn configured_generation(&self) -> u64 {
        self.config_generation
    }

    pub fn pending_switch_generation(&self) -> Option<u64> {
        self.pending_switch.as_ref().map(|p| p.target_generation)
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
    ) -> Result<NvEncodeSession, BackendError> {
        let _ = self.require_hardware;
        let width_u32 = u32::try_from(width).map_err(|_| {
            BackendError::InvalidInput(format!("frame width does not fit NVENC u32: {width}"))
        })?;
        let height_u32 = u32::try_from(height).map_err(|_| {
            BackendError::InvalidInput(format!("frame height does not fit NVENC u32: {height}"))
        })?;

        let cuda_ctx = self.ensure_cuda_ctx()?;

        let encoder = Encoder::initialize_with_cuda(cuda_ctx).map_err(map_encode_error)?;
        let encode_guid = to_encode_guid(self.codec);

        let encode_guids = encoder.get_encode_guids().map_err(map_encode_error)?;
        if !encode_guids.contains(&encode_guid) {
            return Err(BackendError::UnsupportedCodec(self.codec));
        }
        let (input_layout, buffer_format) = match self.input_format {
            EncodeInputFormat::Argb8888 => (
                NvInputLayout::Argb,
                nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ABGR,
            ),
            EncodeInputFormat::Nv12 => (
                NvInputLayout::Nv12,
                nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_NV12,
            ),
        };

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
        configure_rate_control(&mut preset_config.presetCfg, self.target_bitrate);
        configure_codec_preset(self.codec, &mut preset_config.presetCfg);
        let mut init_params = EncoderInitParams::new(encode_guid, width_u32, height_u32);
        init_params
            .preset_guid(preset_guid)
            .tuning_info(tuning_info)
            .display_aspect_ratio(16, 9)
            .framerate(self.fps.max(1) as u32, 1)
            .enable_picture_type_decision()
            .encode_config(&mut preset_config.presetCfg);

        let session = encoder
            .start_session(buffer_format, init_params)
            .map_err(map_encode_error)?;

        Ok(NvEncodeSession::new(session, input_layout))
    }
}

impl VideoEncoder for NvEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(nvidia_capability_report(codec))
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        let mut frame = frame;
        if frame.width == 0 || frame.height == 0 {
            return Err(BackendError::InvalidInput(
                "frame dimensions must be positive".to_string(),
            ));
        }
        validate_nv_encode_frame(&frame, self.input_format)?;

        if let Some(width) = self.width
            && frame.width != width
        {
            return Err(BackendError::InvalidInput(
                "all frames in one flush cycle must have the same width".to_string(),
            ));
        }

        if let Some(height) = self.height
            && frame.height != height
        {
            return Err(BackendError::InvalidInput(
                "all frames in one flush cycle must have the same height".to_string(),
            ));
        }

        // Only consume a pending keyframe/switch request after the frame has
        // passed every validation step. Invalid input must not mutate session state.
        if self.pending_switch.is_some() && frame.force_keyframe {
            self.apply_pending_switch_if_needed()?;
        }
        if self.force_next_keyframe {
            frame.force_keyframe = true;
            self.force_next_keyframe = false;
            self.apply_pending_switch_if_needed()?;
        }

        self.width.get_or_insert(frame.width);
        self.height.get_or_insert(frame.height);
        self.pending_frames.push(frame);
        Ok(Vec::new())
    }

    fn try_reap(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        Ok(Vec::new())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        if self.pending_frames.is_empty() {
            self.width = None;
            self.height = None;
            return Ok(Vec::new());
        }
        self.apply_pending_switch_if_needed()?;

        let width = self.width.ok_or_else(|| {
            BackendError::Backend("pending NVIDIA frames have no recorded width".to_string())
        })?;
        let height = self.height.ok_or_else(|| {
            BackendError::Backend("pending NVIDIA frames have no recorded height".to_string())
        })?;
        let max_in_flight = self.max_in_flight_outputs;
        let safe_flush_options = SafeFlushOptions {
            width,
            height,
            fps: self.fps,
            codec: self.codec,
            max_in_flight,
            report_metrics: self.report_metrics,
        };
        let mut session = self.build_session(width, height)?;
        let pending_frames = std::mem::take(&mut self.pending_frames);
        match Self::flush_safe_per_frame(&mut session, &pending_frames, safe_flush_options) {
            Ok(packets) => {
                self.width = None;
                self.height = None;
                Ok(packets)
            }
            Err(error) => {
                self.pending_frames = pending_frames;
                Err(error)
            }
        }
    }

    fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        match request {
            SessionSwitchRequest::Nvidia { config, mode } => {
                self.apply_nvidia_session_switch(config, mode)
            }
            SessionSwitchRequest::VideoToolbox { .. } => Err(BackendError::UnsupportedConfig(
                "VideoToolbox session switch request is not supported by NVIDIA backend"
                    .to_string(),
            )),
        }
    }
}

impl NvEncoderAdapter {
    fn flush_safe_per_frame(
        session: &mut NvEncodeSession,
        pending_frames: &[Frame],
        options: SafeFlushOptions,
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        let SafeFlushOptions {
            width,
            height,
            fps,
            codec,
            max_in_flight,
            report_metrics,
        } = options;
        let mut packets = Vec::with_capacity(pending_frames.len());
        let mut timing = StageTiming::default();
        let mut copy_stats = CopyStats::default();
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
                .create_input_buffer()
                .map_err(map_encode_error)?;
            let output = session
                .session
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
                copy_stats.output_copy_bytes = copy_stats
                    .output_copy_bytes
                    .saturating_add(packet.data.len() as u64);
                copy_stats.output_copy_packets = copy_stats.output_copy_packets.saturating_add(1);
                packets.push(packet);
                free_pairs.push_back(pair);
                queue_depth_samples.push_value(pending_outputs.len() as f64);
            }
            let mut pair = free_pairs.pop_front().ok_or_else(|| {
                BackendError::Backend("safe lifetime buffer pair is missing".to_string())
            })?;

            let upload_start = Instant::now();
            let uploaded_bytes =
                upload_frame_to_input(&mut pair.input, frame, session.input_layout, width, height)?;
            timing.upload += upload_start.elapsed();
            copy_stats.input_upload_bytes = copy_stats
                .input_upload_bytes
                .saturating_add(uploaded_bytes as u64);
            copy_stats.input_upload_frames = copy_stats.input_upload_frames.saturating_add(1);

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
                is_keyframe_hint: index == 0 || frame.force_keyframe,
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
            copy_stats.output_copy_bytes = copy_stats
                .output_copy_bytes
                .saturating_add(packet.data.len() as u64);
            copy_stats.output_copy_packets = copy_stats.output_copy_packets.saturating_add(1);
            packets.push(packet);
            free_pairs.push_back(pair);
            queue_depth_samples.push_value(pending_outputs.len() as f64);
        }

        if report_metrics {
            eprintln!(
                "[nv.encode.safe] frames={}, packets={}, input_prep_ms={:.3}, upload_ms={:.3}, submit_ms={:.3}, reap_ms={:.3}, lock_ms={:.3}, queue_p95={:.3}, queue_p99={:.3}, jitter_ms_mean={:.3}, jitter_ms_p95={:.3}, jitter_ms_p99={:.3}, input_copy_bytes={}, input_copy_frames={}, output_copy_bytes={}, output_copy_packets={}",
                pending_frames.len(),
                packets.len(),
                timing.input_prep.as_secs_f64() * 1_000.0,
                timing.upload.as_secs_f64() * 1_000.0,
                timing.sdk.as_secs_f64() * 1_000.0,
                timing.reap.as_secs_f64() * 1_000.0,
                timing.output_lock.as_secs_f64() * 1_000.0,
                queue_depth_samples.p95(),
                queue_depth_samples.p99(),
                output_jitter_samples.mean(),
                output_jitter_samples.p95(),
                output_jitter_samples.p99(),
                copy_stats.input_upload_bytes,
                copy_stats.input_upload_frames,
                copy_stats.output_copy_bytes,
                copy_stats.output_copy_packets
            );
        }

        Ok(packets)
    }

    pub(super) fn apply_nvidia_session_switch(
        &mut self,
        config: NvidiaSessionConfig,
        mode: SessionSwitchMode,
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        // A flush-only encoder cannot change configuration in the middle of its
        // buffered batch. Drain first and return every produced packet to the caller.
        let drained = if self.pending_frames.is_empty() {
            Vec::new()
        } else {
            self.flush()?
        };
        let target_generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        self.pending_switch = Some(PendingSessionSwitch {
            config,
            mode,
            target_generation,
        });
        self.apply_pending_switch_if_needed()?;
        Ok(drained)
    }

    fn apply_pending_switch_if_needed(&mut self) -> Result<(), BackendError> {
        let Some(pending) = self.pending_switch.take() else {
            return Ok(());
        };
        self.gop_length = pending.config.gop_length;
        self.frame_interval_p = pending.config.frame_interval_p;
        self.config_generation = pending.target_generation;
        if pending.config.force_idr_on_activate
            || matches!(pending.mode, SessionSwitchMode::OnNextKeyframe)
        {
            self.force_next_keyframe = true;
        }
        Ok(())
    }
}
