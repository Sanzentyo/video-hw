use std::{
    ffi::c_void,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crate::backend_transform_adapter::{DecodedUnit, VtTransformAdapter};
use crate::bitstream::{AccessUnit, ParameterSetCache, StatefulBitstreamAssembler};
use crate::pipeline_scheduler::PipelineScheduler;
use crate::{
    BackendError, CapabilityReport, Codec, ColorRequest, DecodeSummary, DecoderConfig,
    EncodedPacket, Frame, SessionSwitchMode, SessionSwitchRequest, VideoDecoder, VideoEncoder,
    VtSessionConfig,
};
use core_foundation::{
    base::{CFAllocator, CFType, TCFType, kCFAllocatorSystemDefault},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFMutableDictionary},
    number::CFNumber,
    string::CFString,
};
use core_media::{
    block_buffer::CMBlockBuffer,
    format_description::{
        CMFormatDescription, CMVideoCodecType, CMVideoFormatDescription, kCMVideoCodecType_H264,
        kCMVideoCodecType_HEVC,
    },
    sample_buffer::{CMSampleBuffer, CMSampleTimingInfo},
    time::{CMTime, kCMTimeInvalid},
};
use core_video::{
    image_buffer::CVImageBuffer,
    pixel_buffer::{CVPixelBuffer, kCVPixelFormatType_32BGRA},
};
use video_toolbox::{
    compression_properties::{
        CompressionPropertyKey, EncodeFrameOptionKey, VideoEncoderSpecification,
    },
    compression_session::VTCompressionSession,
    decompression_properties::VideoDecoderSpecification,
    decompression_session::{VTDecompressionOutputCallbackRecord, VTDecompressionSession},
    errors::VTDecodeFrameFlags,
    session::TVTSession,
};

pub struct PackedSample {
    pub data: Vec<u8>,
}

pub trait SamplePacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample, BackendError>;
}

#[derive(Debug, Default)]
pub struct AvccHvccPacker;

impl SamplePacker for AvccHvccPacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample, BackendError> {
        let total_size: usize = access_unit
            .nalus
            .iter()
            .map(|nal| nal.len().saturating_add(4))
            .sum();
        let mut data = Vec::with_capacity(total_size);

        for nal in &access_unit.nalus {
            let len = (nal.len() as u32).to_be_bytes();
            data.extend_from_slice(&len);
            data.extend_from_slice(nal);
        }

        Ok(PackedSample { data })
    }
}

#[derive(Debug, Clone, Default)]
struct DecodeOutputState {
    decoded_frames: usize,
    width: Option<usize>,
    height: Option<usize>,
    pixel_format: Option<u32>,
}

struct VtDecoderSession {
    session: VTDecompressionSession,
    format_description: CMVideoFormatDescription,
    decode_state: Box<Mutex<DecodeOutputState>>,
    next_pts: Mutex<i64>,
}

impl VtDecoderSession {
    fn new(config: &DecoderConfig, parameter_sets: &[Vec<u8>]) -> Result<Self, BackendError> {
        let codec_type = to_cm_codec_type(config.codec);
        if config.require_hardware
            && !VTDecompressionSession::is_hardware_decode_supported(codec_type)
        {
            return Err(BackendError::UnsupportedConfig(format!(
                "{} hardware decode is not supported on this machine",
                codec_label(config.codec)
            )));
        }

        let format_description = create_format_description(config.codec, parameter_sets)?;

        let decoder_specification = if config.require_hardware {
            let mut spec = CFMutableDictionary::<CFString, CFType>::new();
            spec.add(
                &VideoDecoderSpecification::RequireHardwareAcceleratedVideoDecoder.into(),
                &CFBoolean::true_value().as_CFType(),
            );
            Some(spec.to_immutable())
        } else {
            None
        };

        let mut decode_state = Box::new(Mutex::new(DecodeOutputState::default()));
        let decode_state_ptr =
            (&mut *decode_state as *mut Mutex<DecodeOutputState>).cast::<c_void>();
        let callback = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(vt_decode_output_callback),
            decompressionOutputRefCon: decode_state_ptr,
        };

        let session = unsafe {
            VTDecompressionSession::new_with_callback(
                format_description.clone(),
                decoder_specification,
                None,
                Some(&callback as *const VTDecompressionOutputCallbackRecord),
            )
        }
        .map_err(|status| vt_error("VTDecompressionSession::new_with_callback", status))?;

        Ok(Self {
            session,
            format_description,
            decode_state,
            next_pts: Mutex::new(0),
        })
    }

    fn decode_access_units(
        &self,
        access_units: &[AccessUnit],
        fps: i32,
    ) -> Result<(), BackendError> {
        let mut packer = AvccHvccPacker;
        for access_unit in access_units {
            let packed = packer.pack(access_unit)?;

            let block_buffer = unsafe {
                let block_buffer = CMBlockBuffer::new_with_memory_block(
                    None,
                    packed.data.len(),
                    None,
                    0,
                    packed.data.len(),
                    0,
                )
                .map_err(|status| cm_error("CMBlockBuffer::new_with_memory_block", status))?;
                block_buffer
                    .replace_data_bytes(&packed.data, 0)
                    .map_err(|status| cm_error("CMBlockBuffer::replace_data_bytes", status))?;
                Ok::<CMBlockBuffer, BackendError>(block_buffer)
            }?;

            let sample_size = [packed.data.len()];
            let format_description: CMFormatDescription = unsafe {
                CMFormatDescription::wrap_under_get_rule(
                    self.format_description.as_concrete_TypeRef(),
                )
            };
            let timing = CMSampleTimingInfo {
                duration: CMTime::make(1, fps),
                presentationTimeStamp: CMTime::make(self.next_pts(), fps),
                decodeTimeStamp: unsafe { kCMTimeInvalid },
            };
            let sample_buffer = CMSampleBuffer::new_ready(
                &block_buffer,
                Some(&format_description),
                1,
                Some(&[timing]),
                Some(&sample_size),
            )
            .map_err(|status| cm_error("CMSampleBuffer::new_ready", status))?;

            unsafe {
                self.session
                    .decode_frame(
                        sample_buffer,
                        VTDecodeFrameFlags::Frame_EnableAsynchronousDecompression,
                        std::ptr::null_mut(),
                    )
                    .map_err(|status| vt_error("VTDecompressionSession::decode_frame", status))?;
            }
        }

        Ok(())
    }

    fn wait_for_completion(&self) -> Result<(), BackendError> {
        self.session
            .finish_delayed_frames()
            .map_err(|status| vt_error("VTDecompressionSession::finish_delayed_frames", status))?;
        self.session
            .wait_for_asynchronous_frames()
            .map_err(|status| {
                vt_error(
                    "VTDecompressionSession::wait_for_asynchronous_frames",
                    status,
                )
            })?;
        Ok(())
    }

    fn snapshot_summary(&self) -> DecodeSummary {
        let state = self
            .decode_state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        let dims = self.format_description.get_dimensions();
        let fallback_width = usize::try_from(dims.width).ok().filter(|v| *v > 0);
        let fallback_height = usize::try_from(dims.height).ok().filter(|v| *v > 0);

        DecodeSummary {
            decoded_frames: state.decoded_frames,
            width: state.width.or(fallback_width),
            height: state.height.or(fallback_height),
            pixel_format: state.pixel_format,
        }
    }

    fn next_pts(&self) -> i64 {
        match self.next_pts.lock() {
            Ok(mut v) => {
                let current = *v;
                *v = v.saturating_add(1);
                current
            }
            Err(_) => 0,
        }
    }
}

pub struct VtDecoderAdapter {
    config: DecoderConfig,
    assembler: StatefulBitstreamAssembler,
    decoder: Option<VtDecoderSession>,
    last_summary: DecodeSummary,
    reported_decoded_frames: usize,
    next_output_pts_90k: i64,
    last_output_pts_90k: Option<i64>,
    pipeline_scheduler: Option<PipelineScheduler>,
}

impl VtDecoderAdapter {
    pub fn new(config: DecoderConfig) -> Self {
        Self {
            assembler: StatefulBitstreamAssembler::with_codec(config.codec),
            config,
            decoder: None,
            last_summary: DecodeSummary {
                decoded_frames: 0,
                width: None,
                height: None,
                pixel_format: None,
            },
            reported_decoded_frames: 0,
            next_output_pts_90k: 0,
            last_output_pts_90k: None,
            pipeline_scheduler: if should_enable_pipeline_scheduler() {
                let capacity = pipeline_queue_capacity();
                Some(PipelineScheduler::new(
                    VtTransformAdapter::with_config(1, capacity),
                    capacity,
                ))
            } else {
                None
            },
        }
    }

    fn ensure_decoder(&mut self, cache: &ParameterSetCache) -> Result<(), BackendError> {
        if self.decoder.is_some() {
            return Ok(());
        }
        if let Some(parameter_sets) = cache.required_for_codec(self.config.codec) {
            self.decoder = Some(VtDecoderSession::new(&self.config, &parameter_sets)?);
        }
        Ok(())
    }

    fn take_delta(&mut self, wait: bool) -> Result<Vec<Frame>, BackendError> {
        let start = Instant::now();
        if let Some(decoder) = self.decoder.as_ref() {
            if wait {
                decoder.wait_for_completion()?;
            }
            let summary = decoder.snapshot_summary();
            let delta = summary
                .decoded_frames
                .saturating_sub(self.reported_decoded_frames);
            self.reported_decoded_frames = summary.decoded_frames;
            self.last_summary = summary.clone();
            let frame_step_90k = (90_000_i64 / i64::from(self.config.fps.max(1))).max(1);
            let frames =
                summary_to_frames(delta, &summary, self.next_output_pts_90k, frame_step_90k);
            self.next_output_pts_90k = self
                .next_output_pts_90k
                .saturating_add(frame_step_90k.saturating_mul(delta as i64));
            let processed = self.preprocess_frames_via_pipeline(frames)?;
            if should_report_metrics() {
                let mut jitter_stats = SampleStats::default();
                let expected_frame_ms = if self.config.fps > 0 {
                    1_000.0 / self.config.fps as f64
                } else {
                    33.333
                };
                for frame in &processed {
                    update_jitter_samples(
                        &mut jitter_stats,
                        &mut self.last_output_pts_90k,
                        frame.pts_90k,
                        expected_frame_ms,
                    );
                }
                eprintln!(
                    "[vt.decode] wait={}, delta_frames={}, total_frames={}, width={:?}, height={:?}, elapsed_ms={:.3}, jitter_ms_mean={:.3}, jitter_ms_p95={:.3}, jitter_ms_p99={:.3}, output_copy_frames={}",
                    wait,
                    delta,
                    summary.decoded_frames,
                    summary.width,
                    summary.height,
                    start.elapsed().as_secs_f64() * 1_000.0,
                    jitter_stats.mean(),
                    jitter_stats.p95(),
                    jitter_stats.p99(),
                    processed.len(),
                );
            }
            return Ok(processed);
        }

        Ok(Vec::new())
    }

    fn sync_pipeline_generation(&self, scheduler: &PipelineScheduler) {
        scheduler.set_generation(1);
    }

    fn preprocess_frames_via_pipeline(
        &mut self,
        frames: Vec<Frame>,
    ) -> Result<Vec<Frame>, BackendError> {
        let Some(scheduler) = &self.pipeline_scheduler else {
            return Ok(frames);
        };
        self.sync_pipeline_generation(scheduler);

        let mut output = Vec::with_capacity(frames.len());
        for frame in frames {
            scheduler.submit_with_generation(
                1,
                DecodedUnit::MetadataOnly(frame),
                ColorRequest::KeepNative,
                None,
            )?;
            let piped = scheduler
                .recv_timeout(Duration::from_millis(100))?
                .ok_or_else(|| {
                    BackendError::TemporaryBackpressure(
                        "pipeline scheduler timed out while preprocessing decode output"
                            .to_string(),
                    )
                })??;
            match piped {
                DecodedUnit::MetadataOnly(frame) => output.push(frame),
                other => {
                    return Err(BackendError::Backend(format!(
                        "unexpected pipeline output for decoder preprocess: {other:?}"
                    )));
                }
            }
        }
        Ok(output)
    }
}

impl VideoDecoder for VtDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        let cm_codec = to_cm_codec_type(codec);
        Ok(CapabilityReport {
            codec,
            decode_supported: true,
            encode_supported: true,
            hardware_acceleration: VTDecompressionSession::is_hardware_decode_supported(cm_codec),
        })
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        let submit_start = Instant::now();
        let (access_units, cache) = self
            .assembler
            .push_chunk(chunk, self.config.codec, pts_90k)?;
        let input_copy_bytes = packed_access_units_bytes(&access_units);
        let access_unit_count = access_units.len();
        self.ensure_decoder(&cache)?;

        if let Some(decoder) = self.decoder.as_ref() {
            if !access_units.is_empty() {
                decoder.decode_access_units(&access_units, self.config.fps)?;
            }
        }
        if should_report_metrics() {
            eprintln!(
                "[vt.decode.submit] flush=false, access_units={}, input_copy_bytes={}, submit_ms={:.3}",
                access_unit_count,
                input_copy_bytes,
                submit_start.elapsed().as_secs_f64() * 1_000.0
            );
        }

        self.take_delta(false)
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        let submit_start = Instant::now();
        let (access_units, cache) = self.assembler.flush()?;
        let input_copy_bytes = packed_access_units_bytes(&access_units);
        let access_unit_count = access_units.len();
        self.ensure_decoder(&cache)?;

        if let Some(decoder) = self.decoder.as_ref() {
            if !access_units.is_empty() {
                decoder.decode_access_units(&access_units, self.config.fps)?;
            }
        }
        if should_report_metrics() {
            eprintln!(
                "[vt.decode.submit] flush=true, access_units={}, input_copy_bytes={}, submit_ms={:.3}",
                access_unit_count,
                input_copy_bytes,
                submit_start.elapsed().as_secs_f64() * 1_000.0
            );
        }

        self.take_delta(true)
    }

    fn decode_summary(&self) -> DecodeSummary {
        self.last_summary.clone()
    }
}

pub struct VtEncoderAdapter {
    codec: Codec,
    fps: i32,
    require_hardware: bool,
    pending_frames: Vec<Frame>,
    width: Option<usize>,
    height: Option<usize>,
    pending_switch: Option<VtPendingSessionSwitch>,
    config_generation: u64,
    next_generation: u64,
    force_next_keyframe: bool,
    session_reconfigure_pending: bool,
    pipeline_scheduler: Option<PipelineScheduler>,
    encode_session: Option<VtEncodeSession>,
}

struct VtEncodeSession {
    session: VTCompressionSession,
    width: usize,
    height: usize,
}

#[derive(Clone)]
struct VtPendingPacket {
    frame_index: usize,
    packet: EncodedPacket,
}

struct VtPendingSessionSwitch {
    config: VtSessionConfig,
    mode: SessionSwitchMode,
    target_generation: u64,
}

#[derive(Debug, Default, Clone)]
struct SampleStats {
    samples: Vec<f64>,
}

impl SampleStats {
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
}

impl VtEncoderAdapter {
    pub fn with_config(codec: Codec, fps: i32, require_hardware: bool) -> Self {
        Self {
            codec,
            fps,
            require_hardware,
            pending_frames: Vec::new(),
            width: None,
            height: None,
            pending_switch: None,
            config_generation: 1,
            next_generation: 2,
            force_next_keyframe: false,
            session_reconfigure_pending: false,
            pipeline_scheduler: if should_enable_pipeline_scheduler() {
                let capacity = pipeline_queue_capacity();
                Some(PipelineScheduler::new(
                    VtTransformAdapter::with_config(1, capacity),
                    capacity,
                ))
            } else {
                None
            },
            encode_session: None,
        }
    }

    fn sync_pipeline_generation(&self, scheduler: &PipelineScheduler) {
        scheduler.set_generation(self.pipeline_generation_hint().unwrap_or(1).max(1));
    }

    fn preprocess_frame_via_pipeline(&mut self, frame: Frame) -> Result<Frame, BackendError> {
        let Some(scheduler) = &self.pipeline_scheduler else {
            return Ok(frame);
        };
        self.sync_pipeline_generation(scheduler);
        let generation = self.pipeline_generation_hint().unwrap_or(1);
        scheduler.submit_with_generation(
            generation,
            DecodedUnit::MetadataOnly(frame),
            ColorRequest::KeepNative,
            None,
        )?;
        let output = scheduler
            .recv_timeout(Duration::from_millis(100))?
            .ok_or_else(|| {
                BackendError::TemporaryBackpressure(
                    "pipeline scheduler timed out while preprocessing frame".to_string(),
                )
            })??;
        match output {
            DecodedUnit::MetadataOnly(frame) => Ok(frame),
            other => Err(BackendError::Backend(format!(
                "unexpected pipeline output for encoder preprocess: {other:?}"
            ))),
        }
    }

    fn create_encode_session(
        &self,
        width: usize,
        height: usize,
    ) -> Result<VTCompressionSession, BackendError> {
        let mut encoder_specification = CFMutableDictionary::<CFString, CFType>::new();
        if self.require_hardware {
            encoder_specification.add(
                &VideoEncoderSpecification::RequireHardwareAcceleratedVideoEncoder.into(),
                &CFBoolean::true_value().as_CFType(),
            );
        }

        let source_image_buffer_attributes = CFMutableDictionary::<CFString, CFType>::new();
        let allocator = unsafe { CFAllocator::wrap_under_get_rule(kCFAllocatorSystemDefault) };

        let session = VTCompressionSession::new(
            width as i32,
            height as i32,
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

    fn apply_vt_session_switch(
        &mut self,
        config: VtSessionConfig,
        mode: SessionSwitchMode,
    ) -> Result<(), BackendError> {
        match mode {
            SessionSwitchMode::DrainThenSwap => {
                if !self.pending_frames.is_empty() {
                    let _ = self.flush()?;
                }
                let target_generation = self.next_generation;
                self.next_generation = self.next_generation.saturating_add(1);
                self.pending_switch = Some(VtPendingSessionSwitch {
                    config,
                    mode,
                    target_generation,
                });
                self.apply_pending_switch_if_needed()
            }
            SessionSwitchMode::Immediate | SessionSwitchMode::OnNextKeyframe => {
                let target_generation = self.next_generation;
                self.next_generation = self.next_generation.saturating_add(1);
                self.pending_switch = Some(VtPendingSessionSwitch {
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
        Ok(CapabilityReport {
            codec,
            decode_supported: true,
            encode_supported: true,
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

        if let Some(argb) = frame.argb.as_ref() {
            let expected = frame.width.saturating_mul(frame.height).saturating_mul(4);
            if argb.len() != expected {
                return Err(BackendError::InvalidInput(format!(
                    "argb payload size mismatch: expected {expected}, got {}",
                    argb.len()
                )));
            }
        }

        frame = self.preprocess_frame_via_pipeline(frame)?;
        self.pending_frames.push(frame);
        Ok(Vec::new())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        let flush_start = Instant::now();
        if self.pending_frames.is_empty() {
            return Ok(Vec::new());
        }
        self.apply_pending_switch_if_needed()?;
        let pending_frames = std::mem::take(&mut self.pending_frames);
        let width = self.width.take().unwrap_or(640);
        let height = self.height.take().unwrap_or(360);
        let codec = self.codec;
        let fps = self.fps.max(1);
        let ensure_start = Instant::now();
        let session = self.ensure_encode_session(width, height)?;
        let ensure_elapsed = ensure_start.elapsed();

        let output_packets = Arc::new(Mutex::new(Vec::<VtPendingPacket>::new()));
        let mut frame_prep_elapsed = Duration::default();
        let mut submit_elapsed = Duration::default();
        let mut input_copy_bytes = 0_u64;
        let mut input_copy_frames = 0_u64;
        let queue_depth = Arc::new(AtomicUsize::new(0));
        let queue_depth_peak = Arc::new(AtomicUsize::new(0));
        let queue_depth_samples = Arc::new(Mutex::new(Vec::<f64>::new()));
        for (frame_index, frame) in pending_frames.iter().enumerate() {
            let frame_prep_start = Instant::now();
            let pixel_buffer = make_bgra_frame(width, height, frame_index, frame.argb.as_deref())?;
            frame_prep_elapsed += frame_prep_start.elapsed();
            input_copy_bytes = input_copy_bytes
                .saturating_add(width.saturating_mul(height).saturating_mul(4) as u64);
            input_copy_frames = input_copy_frames.saturating_add(1);
            let image_buffer =
                unsafe { CVImageBuffer::wrap_under_get_rule(pixel_buffer.as_concrete_TypeRef()) };

            let packets_ref = Arc::clone(&output_packets);
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
                        if status != 0 || sample_buffer_ref.is_null() {
                            return;
                        }
                        let sample_buffer =
                            unsafe { CMSampleBuffer::wrap_under_get_rule(sample_buffer_ref) };
                        if let Some(data_buffer) = sample_buffer.get_data_buffer() {
                            let len = data_buffer.get_data_length();
                            let mut bytes = vec![0u8; len];
                            if data_buffer.copy_data_bytes(0, &mut bytes).is_ok() {
                                let is_keyframe =
                                    detect_keyframe_from_avcc_hvcc_payload(packet_codec, &bytes)
                                        .unwrap_or(packet_is_keyframe_hint);
                                if let Ok(mut packets) = packets_ref.lock() {
                                    packets.push(VtPendingPacket {
                                        frame_index,
                                        packet: EncodedPacket {
                                            codec: packet_codec,
                                            data: bytes,
                                            pts_90k: packet_pts_90k,
                                            is_keyframe,
                                        },
                                    });
                                }
                            }
                        }
                    },
                )
                .map_err(|status| {
                    vt_error("VTCompressionSession::encode_frame_with_closure", status)
                })?;
            submit_elapsed += submit_start.elapsed();
        }

        let complete_start = Instant::now();
        session
            .complete_frames(unsafe { kCMTimeInvalid })
            .map_err(|status| vt_error("VTCompressionSession::complete_frames", status))?;
        let complete_elapsed = complete_start.elapsed();

        let mut pending_packets = output_packets
            .lock()
            .map(|v| v.clone())
            .map_err(|_| BackendError::Backend("encode output lock".to_string()))?;
        pending_packets.sort_by_key(|p| p.frame_index);
        let packets: Vec<EncodedPacket> = pending_packets.into_iter().map(|p| p.packet).collect();

        if should_report_metrics() {
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

    fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
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

fn summary_to_frames(
    count: usize,
    summary: &DecodeSummary,
    start_pts_90k: i64,
    frame_step_90k: i64,
) -> Vec<Frame> {
    let width = summary.width.unwrap_or_default();
    let height = summary.height.unwrap_or_default();
    let pixel_format = summary.pixel_format;

    (0..count)
        .map(|index| Frame {
            width,
            height,
            pixel_format,
            pts_90k: Some(
                start_pts_90k.saturating_add((index as i64).saturating_mul(frame_step_90k)),
            ),
            argb: None,
            force_keyframe: false,
        })
        .collect()
}

fn to_cm_codec_type(codec: Codec) -> CMVideoCodecType {
    match codec {
        Codec::H264 => kCMVideoCodecType_H264,
        Codec::Hevc => kCMVideoCodecType_HEVC,
    }
}

fn codec_label(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "h264",
        Codec::Hevc => "hevc",
    }
}

fn create_format_description(
    codec: Codec,
    parameter_sets: &[Vec<u8>],
) -> Result<CMVideoFormatDescription, BackendError> {
    let refs = parameter_sets
        .iter()
        .map(|v| v.as_slice())
        .collect::<Vec<_>>();
    match codec {
        Codec::H264 => {
            CMVideoFormatDescription::from_h264_parameter_sets(&refs, 4).map_err(|status| {
                cm_error("CMVideoFormatDescription::from_h264_parameter_sets", status)
            })
        }
        Codec::Hevc => {
            CMVideoFormatDescription::from_hevc_parameter_sets(&refs, 4, Some(&empty_dictionary()))
                .map_err(|status| {
                    cm_error("CMVideoFormatDescription::from_hevc_parameter_sets", status)
                })
        }
    }
}

fn empty_dictionary() -> CFDictionary<CFString, CFType> {
    CFMutableDictionary::<CFString, CFType>::new().to_immutable()
}

fn make_bgra_frame(
    width: usize,
    height: usize,
    frame_index: usize,
    argb: Option<&[u8]>,
) -> Result<CVPixelBuffer, BackendError> {
    let pixel_buffer = CVPixelBuffer::new(kCVPixelFormatType_32BGRA, width, height, None)
        .map_err(|status| cv_error("CVPixelBuffer::new", status))?;

    let lock_status = pixel_buffer.lock_base_address(0);
    if lock_status != 0 {
        return Err(cv_error("CVPixelBuffer::lock_base_address", lock_status));
    }

    let bytes_per_row = pixel_buffer.get_bytes_per_row();
    let total = bytes_per_row.saturating_mul(height);
    let base_ptr = unsafe { pixel_buffer.get_base_address() } as *mut u8;

    let write_result = if !base_ptr.is_null() && total > 0 {
        unsafe {
            let buffer = std::slice::from_raw_parts_mut(base_ptr, total);
            if let Some(argb) = argb {
                let expected = width.saturating_mul(height).saturating_mul(4);
                if argb.len() != expected {
                    return Err(BackendError::InvalidInput(format!(
                        "argb payload size mismatch: expected {expected}, got {}",
                        argb.len()
                    )));
                }
                for y in 0..height {
                    for x in 0..width {
                        let dst = y * bytes_per_row + x * 4;
                        let src = (y * width + x) * 4;
                        if dst + 3 >= buffer.len() || src + 3 >= argb.len() {
                            continue;
                        }
                        buffer[dst] = argb[src + 3];
                        buffer[dst + 1] = argb[src + 2];
                        buffer[dst + 2] = argb[src + 1];
                        buffer[dst + 3] = argb[src];
                    }
                }
            } else {
                for y in 0..height {
                    for x in 0..width {
                        let offset = y * bytes_per_row + x * 4;
                        if offset + 3 >= buffer.len() {
                            continue;
                        }
                        buffer[offset] = ((x + frame_index) % 256) as u8;
                        buffer[offset + 1] = ((y + frame_index * 2) % 256) as u8;
                        buffer[offset + 2] = ((frame_index * 5) % 256) as u8;
                        buffer[offset + 3] = 255;
                    }
                }
            }
        }
        Ok(())
    } else {
        Ok(())
    };

    let unlock_status = pixel_buffer.unlock_base_address(0);
    if let Err(err) = write_result {
        return Err(err);
    }
    if unlock_status != 0 {
        return Err(cv_error(
            "CVPixelBuffer::unlock_base_address",
            unlock_status,
        ));
    }

    Ok(pixel_buffer)
}

fn frame_encode_properties(force_keyframe: bool) -> CFDictionary<CFString, CFType> {
    if !force_keyframe {
        return empty_dictionary();
    }
    let mut props = CFMutableDictionary::<CFString, CFType>::new();
    let key = CFString::from(EncodeFrameOptionKey::ForceKeyFrame);
    props.add(&key, &CFBoolean::true_value().as_CFType());
    props.to_immutable()
}

fn cm_time_from_90k(pts_90k: i64) -> CMTime {
    CMTime::make(pts_90k.max(0), 90_000)
}

fn should_enable_pipeline_scheduler() -> bool {
    std::env::var("VIDEO_HW_VT_PIPELINE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn update_peak(peak: &AtomicUsize, value: usize) {
    let mut current = peak.load(Ordering::Relaxed);
    while value > current {
        match peak.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(latest) => current = latest,
        }
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

fn should_report_metrics() -> bool {
    std::env::var("VIDEO_HW_VT_METRICS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn pipeline_queue_capacity() -> usize {
    std::env::var("VIDEO_HW_VT_PIPELINE_QUEUE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.clamp(1, 1024))
        .unwrap_or(8)
}

fn packed_access_units_bytes(access_units: &[AccessUnit]) -> usize {
    access_units
        .iter()
        .map(|au| {
            au.nalus
                .iter()
                .map(|nal| nal.len().saturating_add(4))
                .sum::<usize>()
        })
        .sum()
}

fn detect_keyframe_from_avcc_hvcc_payload(codec: Codec, payload: &[u8]) -> Option<bool> {
    let mut offset = 0usize;
    let mut saw_slice = false;
    let mut saw_irap = false;

    while offset.saturating_add(4) <= payload.len() {
        let len = u32::from_be_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]) as usize;
        offset = offset.saturating_add(4);

        if len == 0 || offset.saturating_add(len) > payload.len() {
            break;
        }
        let nalu = &payload[offset..offset + len];
        offset = offset.saturating_add(len);
        if nalu.is_empty() {
            continue;
        }

        match codec {
            Codec::H264 => {
                let nalu_type = nalu[0] & 0x1f;
                if nalu_type == 5 {
                    saw_irap = true;
                    saw_slice = true;
                } else if (1..=5).contains(&nalu_type) {
                    saw_slice = true;
                }
            }
            Codec::Hevc => {
                // HEVC nal_unit_type in bits[6:1] of first byte.
                let nalu_type = (nalu[0] >> 1) & 0x3f;
                if (16..=21).contains(&nalu_type) {
                    saw_irap = true;
                    saw_slice = true;
                } else if nalu_type <= 31 {
                    saw_slice = true;
                }
            }
        }
    }

    if saw_slice { Some(saw_irap) } else { None }
}

fn vt_error(context: &str, status: i32) -> BackendError {
    BackendError::Backend(format!("videotoolbox({context}): {status}"))
}

fn cm_error(context: &str, status: i32) -> BackendError {
    BackendError::Backend(format!("coremedia({context}): {status}"))
}

fn cv_error(context: &str, status: i32) -> BackendError {
    BackendError::Backend(format!("corevideo({context}): {status}"))
}

extern "C" fn vt_decode_output_callback(
    decompression_output_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: i32,
    _info_flags: video_toolbox::errors::VTDecodeInfoFlags,
    image_buffer: core_video::image_buffer::CVImageBufferRef,
    _presentation_time_stamp: CMTime,
    _presentation_duration: CMTime,
) {
    if status != 0 || decompression_output_ref_con.is_null() || image_buffer.is_null() {
        return;
    }

    let state = unsafe { &*(decompression_output_ref_con as *const Mutex<DecodeOutputState>) };
    let pixel_buffer = unsafe { CVPixelBuffer::wrap_under_get_rule(image_buffer) };

    if let Ok(mut s) = state.lock() {
        s.decoded_frames += 1;
        if s.width.is_none() {
            s.width = Some(pixel_buffer.get_width());
        }
        if s.height.is_none() {
            s.height = Some(pixel_buffer.get_height());
        }
        if s.pixel_format.is_none() {
            s.pixel_format = Some(pixel_buffer.get_pixel_format());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_h264_keyframe_from_length_prefixed_payload() {
        let mut payload = Vec::new();
        let idr = [0x65, 0x88, 0x84, 0x21];
        payload.extend_from_slice(&(idr.len() as u32).to_be_bytes());
        payload.extend_from_slice(&idr);
        assert_eq!(
            detect_keyframe_from_avcc_hvcc_payload(Codec::H264, &payload),
            Some(true)
        );
    }

    #[test]
    fn detect_h264_non_keyframe_from_length_prefixed_payload() {
        let mut payload = Vec::new();
        let non_idr = [0x41, 0x9a, 0x22];
        payload.extend_from_slice(&(non_idr.len() as u32).to_be_bytes());
        payload.extend_from_slice(&non_idr);
        assert_eq!(
            detect_keyframe_from_avcc_hvcc_payload(Codec::H264, &payload),
            Some(false)
        );
    }

    #[test]
    fn detect_hevc_keyframe_from_length_prefixed_payload() {
        let mut payload = Vec::new();
        // nal_unit_type=19 (IDR_W_RADL): first byte bits[6:1] = 0b010011.
        let idr = [0b0010_0110, 0x01, 0xaa, 0xbb];
        payload.extend_from_slice(&(idr.len() as u32).to_be_bytes());
        payload.extend_from_slice(&idr);
        assert_eq!(
            detect_keyframe_from_avcc_hvcc_payload(Codec::Hevc, &payload),
            Some(true)
        );
    }

    #[test]
    fn vt_switch_immediate_updates_generation_hint() {
        let mut adapter = VtEncoderAdapter::with_config(Codec::H264, 30, false);
        assert_eq!(adapter.pipeline_generation_hint(), Some(1));
        adapter
            .apply_vt_session_switch(
                VtSessionConfig {
                    force_keyframe_on_activate: false,
                },
                SessionSwitchMode::Immediate,
            )
            .unwrap();
        assert_eq!(adapter.pipeline_generation_hint(), Some(2));
        assert!(adapter.session_reconfigure_pending);
    }

    #[test]
    fn vt_switch_on_next_keyframe_stays_pending_when_frames_are_buffered() {
        let mut adapter = VtEncoderAdapter::with_config(Codec::H264, 30, false);
        adapter.pending_frames.push(Frame {
            width: 640,
            height: 360,
            pixel_format: None,
            pts_90k: Some(0),
            argb: None,
            force_keyframe: false,
        });
        adapter
            .apply_vt_session_switch(
                VtSessionConfig {
                    force_keyframe_on_activate: false,
                },
                SessionSwitchMode::OnNextKeyframe,
            )
            .unwrap();
        assert!(adapter.pending_switch.is_some());
        assert!(adapter.force_next_keyframe);
        assert_eq!(adapter.pipeline_generation_hint(), Some(2));
    }
}
