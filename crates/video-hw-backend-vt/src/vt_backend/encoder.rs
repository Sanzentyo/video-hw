//! VideoToolbox encoder session, batching, metrics, and session-switch adapter.

use super::*;

pub struct VtEncoderAdapter {
    codec: Codec,
    fps: i32,
    require_hardware: bool,
    input_format: EncodeInputFormat,
    pub(super) report_metrics: bool,
    pub(super) pending_frames: Vec<Frame>,
    width: Option<usize>,
    height: Option<usize>,
    pub(super) pending_switch: Option<VtPendingSessionSwitch>,
    config_generation: u64,
    next_generation: u64,
    pub(super) force_next_keyframe: bool,
    pub(super) session_reconfigure_pending: bool,
    encode_session: Option<VtEncodeSession>,
}

struct VtEncodeSession {
    session: VTCompressionSession,
    width: usize,
    height: usize,
}

impl Drop for VtEncodeSession {
    fn drop(&mut self) {
        self.session.invalidate();
    }
}

#[derive(Clone)]
struct VtPendingPacket {
    frame_index: usize,
    packet: EncodedPacket,
}

pub(super) struct VtPendingSessionSwitch {
    config: VtSessionConfig,
    mode: SessionSwitchMode,
    target_generation: u64,
}

#[derive(Debug, Default, Clone)]
pub(super) struct SampleStats {
    samples: Vec<f64>,
}

impl SampleStats {
    pub(super) fn push_value(&mut self, value: f64) {
        self.samples.push(value);
    }

    pub(super) fn mean(&self) -> f64 {
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

    pub(super) fn p95(&self) -> f64 {
        self.percentile(95.0)
    }

    pub(super) fn p99(&self) -> f64 {
        self.percentile(99.0)
    }
}

struct VtEncodeBatchConfig {
    codec: Codec,
    fps: i32,
    width: usize,
    height: usize,
    report_metrics: bool,
    ensure_elapsed: Duration,
    flush_start: Instant,
}

fn encode_pending_batch(
    session: &VTCompressionSession,
    pending_frames: &[Frame],
    config: VtEncodeBatchConfig,
) -> Result<Vec<EncodedPacket>, BackendError> {
    let VtEncodeBatchConfig {
        codec,
        fps,
        width,
        height,
        report_metrics,
        ensure_elapsed,
        flush_start,
    } = config;
    let output_packets = Arc::new(Mutex::new(Vec::<VtPendingPacket>::new()));
    let output_error = Arc::new(Mutex::new(None::<String>));
    let mut frame_prep_elapsed = Duration::default();
    let mut submit_elapsed = Duration::default();
    let mut input_copy_bytes = 0_u64;
    let mut input_copy_frames = 0_u64;
    let queue_depth = Arc::new(AtomicUsize::new(0));
    let queue_depth_peak = Arc::new(AtomicUsize::new(0));
    let queue_depth_samples = Arc::new(Mutex::new(Vec::<f64>::new()));
    for (frame_index, frame) in pending_frames.iter().enumerate() {
        let frame_prep_start = Instant::now();
        let argb = frame.argb.as_deref().ok_or_else(|| {
            BackendError::InvalidInput(
                "VideoToolbox encode requires ARGB input payload".to_string(),
            )
        })?;
        let pixel_buffer = make_bgra_frame(width, height, argb)?;
        frame_prep_elapsed += frame_prep_start.elapsed();
        input_copy_bytes =
            input_copy_bytes.saturating_add(width.saturating_mul(height).saturating_mul(4) as u64);
        input_copy_frames = input_copy_frames.saturating_add(1);
        // SAFETY: `pixel_buffer` owns a valid CoreVideo image reference and remains alive
        // through submission; the wrapper retains it for the encode call.
        let image_buffer =
            unsafe { CVImageBuffer::wrap_under_get_rule(pixel_buffer.as_concrete_TypeRef()) };

        let packets_ref = Arc::clone(&output_packets);
        let error_ref = Arc::clone(&output_error);
        let queue_depth_ref = Arc::clone(&queue_depth);
        let queue_depth_peak_ref = Arc::clone(&queue_depth_peak);
        let queue_depth_samples_ref = Arc::clone(&queue_depth_samples);
        let packet_codec = codec;
        let packet_pts_90k = frame.pts_90k;
        let packet_is_keyframe_hint = frame_index == 0 || frame.force_keyframe;
        let presentation_time_stamp = frame
            .pts_90k
            .map(cm_time_from_90k)
            .unwrap_or_else(|| CMTime::make(frame_index as i64, fps));
        let frame_duration = CMTime::make(1, fps);
        let submit_start = Instant::now();
        let depth_after_submit = queue_depth_ref.fetch_add(1, Ordering::Relaxed) + 1;
        update_peak(&queue_depth_peak_ref, depth_after_submit);
        if let Ok(mut samples) = queue_depth_samples_ref.lock() {
            samples.push(depth_after_submit as f64);
        }
        session
            .encode_frame_with_closure(
                image_buffer,
                presentation_time_stamp,
                frame_duration,
                frame_encode_properties(frame.force_keyframe),
                move |status, _info_flags, sample_buffer_ref| {
                    let depth_after_callback = queue_depth_ref
                        .fetch_sub(1, Ordering::Relaxed)
                        .saturating_sub(1);
                    if let Ok(mut samples) = queue_depth_samples_ref.lock() {
                        samples.push(depth_after_callback as f64);
                    }
                    if status != 0 {
                        record_vt_encode_callback_error(
                            &error_ref,
                            format!("VideoToolbox encode callback failed with status {status}"),
                        );
                        return;
                    }
                    if sample_buffer_ref.is_null() {
                        record_vt_encode_callback_error(
                            &error_ref,
                            "VideoToolbox encode callback returned a null sample buffer"
                                .to_string(),
                        );
                        return;
                    }
                    // SAFETY: a successful callback owns a valid sample buffer
                    // reference for the callback duration; the wrapper retains it.
                    let sample_buffer =
                        unsafe { CMSampleBuffer::wrap_under_get_rule(sample_buffer_ref) };
                    let Some(data_buffer) = sample_buffer.get_data_buffer() else {
                        record_vt_encode_callback_error(
                            &error_ref,
                            "VideoToolbox sample has no encoded data buffer".to_string(),
                        );
                        return;
                    };
                    let len = data_buffer.get_data_length();
                    let mut bytes = vec![0u8; len];
                    if let Err(status) = data_buffer.copy_data_bytes(0, &mut bytes) {
                        record_vt_encode_callback_error(
                            &error_ref,
                            format!("failed to copy VideoToolbox encoded data: {status}"),
                        );
                        return;
                    }
                    let is_keyframe = detect_keyframe_from_avcc_hvcc_payload(packet_codec, &bytes)
                        .unwrap_or(packet_is_keyframe_hint);
                    if is_keyframe
                        && let Some(parameter_sets) =
                            parameter_sets_from_sample_buffer_ref(packet_codec, sample_buffer_ref)
                        && let Err(error) =
                            prepend_length_prefixed_parameter_sets(&mut bytes, &parameter_sets)
                    {
                        record_vt_encode_callback_error(&error_ref, error.to_string());
                        return;
                    }
                    match packets_ref.lock() {
                        Ok(mut packets) => packets.push(VtPendingPacket {
                            frame_index,
                            packet: EncodedPacket {
                                codec: packet_codec,
                                layout: vt_encoded_layout(packet_codec),
                                kind: EncodedContentKind::Frame,
                                data: bytes,
                                pts_90k: packet_pts_90k,
                                is_keyframe,
                            },
                        }),
                        Err(_) => record_vt_encode_callback_error(
                            &error_ref,
                            "VideoToolbox output packet lock poisoned".to_string(),
                        ),
                    }
                },
            )
            .map_err(|status| {
                vt_error("VTCompressionSession::encode_frame_with_closure", status)
            })?;
        submit_elapsed += submit_start.elapsed();
    }

    let complete_start = Instant::now();
    // SAFETY: this reads CoreMedia's immutable invalid-time sentinel to request all frames.
    let complete_all_frames = unsafe { kCMTimeInvalid };
    session
        .complete_frames(complete_all_frames)
        .map_err(|status| vt_error("VTCompressionSession::complete_frames", status))?;
    let complete_elapsed = complete_start.elapsed();

    let callback_error = output_error
        .lock()
        .map_err(|_| BackendError::Backend("encode callback error lock poisoned".to_string()))?
        .clone();
    if let Some(error) = callback_error {
        return Err(BackendError::Backend(error));
    }

    let mut pending_packets = output_packets
        .lock()
        .map(|v| v.clone())
        .map_err(|_| BackendError::Backend("encode output lock".to_string()))?;
    pending_packets.sort_by_key(|p| p.frame_index);
    let packets: Vec<EncodedPacket> = pending_packets.into_iter().map(|p| p.packet).collect();

    if report_metrics {
        let output_bytes: usize = packets.iter().map(|p| p.data.len()).sum();
        let mut queue_stats = SampleStats::default();
        if let Ok(values) = queue_depth_samples.lock() {
            for v in values.iter().copied() {
                queue_stats.push_value(v);
            }
        }
        let mut jitter_stats = SampleStats::default();
        let expected_frame_ms = if fps > 0 {
            1_000.0 / fps as f64
        } else {
            33.333
        };
        let mut last_pts_90k = None;
        for packet in &packets {
            update_jitter_samples(
                &mut jitter_stats,
                &mut last_pts_90k,
                packet.pts_90k,
                expected_frame_ms,
            );
        }
        eprintln!(
            "[vt.encode] frames={}, packets={}, output_bytes={}, width={}, height={}, ensure_ms={:.3}, frame_prep_ms={:.3}, submit_ms={:.3}, complete_ms={:.3}, total_ms={:.3}, queue_peak={}, queue_p95={:.3}, queue_p99={:.3}, jitter_ms_mean={:.3}, jitter_ms_p95={:.3}, jitter_ms_p99={:.3}, input_copy_bytes={}, input_copy_frames={}, output_copy_bytes={}, output_copy_packets={}",
            pending_frames.len(),
            packets.len(),
            output_bytes,
            width,
            height,
            ensure_elapsed.as_secs_f64() * 1_000.0,
            frame_prep_elapsed.as_secs_f64() * 1_000.0,
            submit_elapsed.as_secs_f64() * 1_000.0,
            complete_elapsed.as_secs_f64() * 1_000.0,
            flush_start.elapsed().as_secs_f64() * 1_000.0,
            queue_depth_peak.load(Ordering::Relaxed),
            queue_stats.p95(),
            queue_stats.p99(),
            jitter_stats.mean(),
            jitter_stats.p95(),
            jitter_stats.p99(),
            input_copy_bytes,
            input_copy_frames,
            output_bytes as u64,
            packets.len() as u64,
        );
    }

    Ok(packets)
}

impl VtEncoderAdapter {
    pub fn new(config: EncoderConfig) -> Self {
        let EncoderConfig {
            codec,
            fps,
            require_hardware,
            input_format,
            backend_options,
        } = config;
        let options = vt_encoder_options(backend_options);
        let report_metrics = option_or_env_bool(options.report_metrics, "VIDEO_HW_VT_METRICS");
        Self {
            codec,
            fps,
            require_hardware,
            input_format,
            report_metrics,
            pending_frames: Vec::new(),
            width: None,
            height: None,
            pending_switch: None,
            config_generation: 1,
            next_generation: 2,
            force_next_keyframe: false,
            session_reconfigure_pending: false,
            encode_session: None,
        }
    }

    pub fn configured_generation(&self) -> u64 {
        self.config_generation
    }

    pub fn pending_switch_generation(&self) -> Option<u64> {
        self.pending_switch.as_ref().map(|p| p.target_generation)
    }

    fn create_encode_session(
        &self,
        width: usize,
        height: usize,
    ) -> Result<VTCompressionSession, BackendError> {
        let width = i32::try_from(width).map_err(|_| {
            BackendError::InvalidInput(format!(
                "frame width does not fit VideoToolbox i32: {width}"
            ))
        })?;
        let height = i32::try_from(height).map_err(|_| {
            BackendError::InvalidInput(format!(
                "frame height does not fit VideoToolbox i32: {height}"
            ))
        })?;
        let mut encoder_specification = CFMutableDictionary::<CFString, CFType>::new();
        if self.require_hardware {
            encoder_specification.add(
                &VideoEncoderSpecification::RequireHardwareAcceleratedVideoEncoder.into(),
                &CFBoolean::true_value().as_CFType(),
            );
        }

        let source_image_buffer_attributes = CFMutableDictionary::<CFString, CFType>::new();
        // SAFETY: `kCFAllocatorSystemDefault` is a process-lifetime CoreFoundation singleton.
        let allocator = unsafe { CFAllocator::wrap_under_get_rule(kCFAllocatorSystemDefault) };

        let session = VTCompressionSession::new(
            width,
            height,
            to_cm_codec_type(self.codec),
            encoder_specification.to_immutable(),
            source_image_buffer_attributes.to_immutable(),
            allocator,
        )
        .map_err(|status| vt_error("VTCompressionSession::new", status))?;

        let session_ref = session.as_session();
        session_ref
            .set_property(
                CompressionPropertyKey::RealTime.into(),
                CFBoolean::false_value().as_CFType(),
            )
            .map_err(|status| vt_error("VTSessionSetProperty(RealTime)", status))?;
        session_ref
            .set_property(
                CompressionPropertyKey::ExpectedFrameRate.into(),
                CFNumber::from(self.fps).as_CFType(),
            )
            .map_err(|status| vt_error("VTSessionSetProperty(ExpectedFrameRate)", status))?;
        session_ref
            .set_property(
                CompressionPropertyKey::AllowFrameReordering.into(),
                CFBoolean::false_value().as_CFType(),
            )
            .map_err(|status| vt_error("VTSessionSetProperty(AllowFrameReordering)", status))?;
        session_ref
            .set_property(
                CompressionPropertyKey::MaxFrameDelayCount.into(),
                CFNumber::from(0).as_CFType(),
            )
            .ok();
        session_ref
            .set_property(
                CompressionPropertyKey::MaxKeyFrameInterval.into(),
                CFNumber::from(self.fps.saturating_mul(2)).as_CFType(),
            )
            .map_err(|status| vt_error("VTSessionSetProperty(MaxKeyFrameInterval)", status))?;

        session
            .prepare_to_encode_frames()
            .map_err(|status| vt_error("VTCompressionSession::prepare_to_encode_frames", status))?;

        Ok(session)
    }

    fn ensure_encode_session(
        &mut self,
        width: usize,
        height: usize,
    ) -> Result<&VTCompressionSession, BackendError> {
        let needs_recreate = match self.encode_session.as_ref() {
            Some(existing) => {
                existing.width != width
                    || existing.height != height
                    || self.session_reconfigure_pending
            }
            None => true,
        };
        if needs_recreate {
            let session = self.create_encode_session(width, height)?;
            self.encode_session = Some(VtEncodeSession {
                session,
                width,
                height,
            });
            self.session_reconfigure_pending = false;
        }
        self.encode_session
            .as_ref()
            .map(|s| &s.session)
            .ok_or_else(|| BackendError::Backend("active VT encode session is missing".to_string()))
    }

    pub(super) fn apply_vt_session_switch(
        &mut self,
        config: VtSessionConfig,
        mode: SessionSwitchMode,
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        // VideoToolbox batches frames until flush, so switching configuration
        // first requires draining and returning the old session's packets.
        let drained = if self.pending_frames.is_empty() {
            Vec::new()
        } else {
            self.flush()?
        };
        let target_generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        self.pending_switch = Some(VtPendingSessionSwitch {
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
        self.config_generation = pending.target_generation;
        self.session_reconfigure_pending = true;
        if pending.config.force_keyframe_on_activate
            || matches!(pending.mode, SessionSwitchMode::OnNextKeyframe)
        {
            self.force_next_keyframe = true;
        }

        if matches!(pending.mode, SessionSwitchMode::DrainThenSwap)
            || matches!(pending.mode, SessionSwitchMode::Immediate)
        {
            let _ = self.encode_session.take();
        }
        Ok(())
    }
}

impl VideoEncoder for VtEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(vt_capability_report(codec))
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        ensure_vt_codec_implemented(self.codec, "encode")?;
        if self.input_format != EncodeInputFormat::Argb8888 {
            return Err(BackendError::UnsupportedConfig(format!(
                "VideoToolbox encode does not support {} input",
                self.input_format
            )));
        }
        let mut frame = frame;
        if frame.width == 0 || frame.height == 0 {
            return Err(BackendError::InvalidInput(
                "frame dimensions must be positive".to_string(),
            ));
        }
        if i32::try_from(frame.width).is_err() || i32::try_from(frame.height).is_err() {
            return Err(BackendError::InvalidInput(format!(
                "frame dimensions exceed VideoToolbox limits: {}x{}",
                frame.width, frame.height
            )));
        }
        let argb = frame.argb.as_ref().ok_or_else(|| {
            BackendError::InvalidInput(
                "VideoToolbox encode requires ARGB input payload".to_string(),
            )
        })?;
        let expected = frame
            .width
            .checked_mul(frame.height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| {
                BackendError::InvalidInput("ARGB frame size overflows usize".to_string())
            })?;
        if argb.len() != expected {
            return Err(BackendError::InvalidInput(format!(
                "argb payload size mismatch: expected {expected}, got {}",
                argb.len()
            )));
        }

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

        // A malformed frame must not consume a pending switch/keyframe request.
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

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        ensure_vt_codec_implemented(self.codec, "encode")?;
        let flush_start = Instant::now();
        if self.pending_frames.is_empty() {
            return Ok(Vec::new());
        }
        self.apply_pending_switch_if_needed()?;
        let width = self.width.ok_or_else(|| {
            BackendError::Backend("pending VideoToolbox frames have no recorded width".to_string())
        })?;
        let height = self.height.ok_or_else(|| {
            BackendError::Backend("pending VideoToolbox frames have no recorded height".to_string())
        })?;
        let codec = self.codec;
        let fps = self.fps.max(1);
        let report_metrics = self.report_metrics;
        let pending_frames = std::mem::take(&mut self.pending_frames);
        let ensure_start = Instant::now();
        let batch_result = match self.ensure_encode_session(width, height) {
            Ok(session) => encode_pending_batch(
                session,
                &pending_frames,
                VtEncodeBatchConfig {
                    codec,
                    fps,
                    width,
                    height,
                    report_metrics,
                    ensure_elapsed: ensure_start.elapsed(),
                    flush_start,
                },
            ),
            Err(error) => Err(error),
        };

        match batch_result {
            Ok(packets) => {
                self.width = None;
                self.height = None;
                Ok(packets)
            }
            Err(error) => {
                // Submission is an all-or-nothing adapter transaction. A failed native batch may
                // have accepted a prefix, so discard that session before making the complete
                // original batch visible for retry.
                self.encode_session = None;
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
            SessionSwitchRequest::VideoToolbox { config, mode } => {
                self.apply_vt_session_switch(config, mode)
            }
            SessionSwitchRequest::Nvidia { .. } => Err(BackendError::UnsupportedConfig(
                "NVIDIA session switch request is not supported by VideoToolbox backend"
                    .to_string(),
            )),
        }
    }
}
