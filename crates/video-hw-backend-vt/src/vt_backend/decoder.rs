//! VideoToolbox decoder session ownership and streaming adapter.

use super::*;

#[derive(Debug, Clone, Default)]
pub(super) struct DecodeOutputState {
    pub(super) decoded_frames: usize,
    pub(super) width: Option<usize>,
    pub(super) height: Option<usize>,
    pub(super) pixel_format: Option<u32>,
    pub(super) capture_argb: bool,
    pub(super) pending_frames: VecDeque<Frame>,
    pub(super) callback_error: Option<String>,
}

struct VtDecoderSession {
    session: VTDecompressionSession,
    format_description: CMVideoFormatDescription,
    decode_state: Box<Mutex<DecodeOutputState>>,
    next_pts: Mutex<i64>,
    av1_config_obus: Option<Vec<u8>>,
}

impl Drop for VtDecoderSession {
    fn drop(&mut self) {
        // The callback refcon points into `decode_state`, so quiesce the native session before
        // Rust starts dropping fields and invalidates that address. Drop cannot report errors;
        // explicit completion paths still surface the same failures to callers.
        let _ = self.session.finish_delayed_frames();
        let _ = self.session.wait_for_asynchronous_frames();
    }
}

impl VtDecoderSession {
    fn new(config: &DecoderConfig, parameter_sets: &[Vec<u8>]) -> Result<Self, BackendError> {
        ensure_vt_codec_implemented(config.codec, "decode")?;
        let codec_type = to_cm_codec_type(config.codec);
        if config.require_hardware
            && !VTDecompressionSession::is_hardware_decode_supported(codec_type)
        {
            return Err(BackendError::UnsupportedConfig(format!(
                "{} hardware decode is not supported on this machine",
                codec_label(config.codec)
            )));
        }

        let options = vt_decoder_options(config);
        let format_description = create_format_description(config.codec, parameter_sets, &options)?;

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
        let destination_image_buffer_attributes =
            decode_destination_image_buffer_attributes(config.output_mode);

        let mut decode_state = Box::new(Mutex::new(DecodeOutputState::default()));
        if let Ok(mut state) = decode_state.lock() {
            state.capture_argb = !matches!(config.output_mode, DecodeOutputMode::Metadata);
        }
        let decode_state_ptr =
            (&mut *decode_state as *mut Mutex<DecodeOutputState>).cast::<c_void>();
        let callback = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(vt_decode_output_callback),
            decompressionOutputRefCon: decode_state_ptr,
        };

        // SAFETY: `decode_state_ptr` points into a boxed mutex that remains stable until after
        // the owned decompression session is dropped; every callback field is valid for the call.
        let session = unsafe {
            VTDecompressionSession::new_with_callback(
                format_description.clone(),
                decoder_specification,
                destination_image_buffer_attributes,
                Some(&callback as *const VTDecompressionOutputCallbackRecord),
            )
        }
        .map_err(|status| vt_error("VTDecompressionSession::new_with_callback", status))?;

        Ok(Self {
            session,
            format_description,
            decode_state,
            next_pts: Mutex::new(0),
            av1_config_obus: options.av1_config_obus,
        })
    }

    fn decode_access_units(
        &self,
        access_units: &[AccessUnit],
        fps: i32,
    ) -> Result<(), BackendError> {
        let mut packer = VtSamplePacker::new(self.av1_config_obus.as_deref());
        for access_unit in access_units {
            let packed = packer.pack(access_unit)?;
            if packed.data.is_empty() {
                continue;
            }

            // SAFETY: CoreMedia allocates the block, and every subsequent copy stays within the
            // requested `packed.data.len()` byte range.
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
            // SAFETY: `self.format_description` owns a valid CoreMedia reference; the wrapper
            // retains that reference for the constructed sample buffer.
            let format_description: CMFormatDescription = unsafe {
                CMFormatDescription::wrap_under_get_rule(
                    self.format_description.as_concrete_TypeRef(),
                )
            };
            let presentation_time_stamp = access_unit
                .pts_90k
                .map(cm_time_from_90k)
                .unwrap_or_else(|| CMTime::make(self.next_pts(), fps));
            // SAFETY: this reads CoreMedia's immutable invalid-time sentinel.
            let decode_time_stamp = unsafe { kCMTimeInvalid };
            let timing = CMSampleTimingInfo {
                duration: CMTime::make(1, fps),
                presentationTimeStamp: presentation_time_stamp,
                decodeTimeStamp: decode_time_stamp,
            };
            let sample_buffer = CMSampleBuffer::new_ready(
                &block_buffer,
                Some(&format_description),
                1,
                Some(&[timing]),
                Some(&sample_size),
            )
            .map_err(|status| cm_error("CMSampleBuffer::new_ready", status))?;

            // SAFETY: the sample buffer and session are valid for this call, while the callback
            // refcon continues to point at the boxed state owned by `self`.
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

    fn drain_output_frames(&self) -> Result<Vec<Frame>, BackendError> {
        let mut state = self
            .decode_state
            .lock()
            .map_err(|_| BackendError::Backend("decode output state lock poisoned".to_string()))?;
        if let Some(error) = &state.callback_error {
            return Err(BackendError::Backend(error.clone()));
        }
        Ok(state.pending_frames.drain(..).collect())
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
    pub(super) report_metrics: bool,
    assembler: StatefulBitstreamAssembler,
    decoder: Option<VtDecoderSession>,
    last_summary: DecodeSummary,
    last_output_pts_90k: Option<i64>,
}

impl VtDecoderAdapter {
    pub fn new(config: DecoderConfig) -> Self {
        let options = vt_decoder_options(&config);
        let report_metrics = option_or_env_bool(options.report_metrics, "VIDEO_HW_VT_METRICS");
        Self {
            assembler: StatefulBitstreamAssembler::with_codec(config.codec),
            config,
            report_metrics,
            decoder: None,
            last_summary: DecodeSummary {
                decoded_frames: 0,
                width: None,
                height: None,
                pixel_format: None,
            },
            last_output_pts_90k: None,
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
            let frames = decoder.drain_output_frames()?;
            let summary = decoder.snapshot_summary();
            let delta = frames.len();
            self.last_summary = summary.clone();
            if self.report_metrics {
                let mut jitter_stats = SampleStats::default();
                let expected_frame_ms = if self.config.fps > 0 {
                    1_000.0 / self.config.fps as f64
                } else {
                    33.333
                };
                for frame in &frames {
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
                    frames.len(),
                );
            }
            return Ok(frames);
        }

        Ok(Vec::new())
    }
}

impl VideoDecoder for VtDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(vt_capability_report(codec))
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        ensure_vt_codec_implemented(self.config.codec, "decode")?;
        let submit_start = Instant::now();
        let (access_units, cache) = self
            .assembler
            .push_chunk(chunk, self.config.codec, pts_90k)?;
        let input_copy_bytes = packed_access_units_bytes(&access_units);
        let access_unit_count = access_units.len();
        self.ensure_decoder(&cache)?;

        if let Some(decoder) = self.decoder.as_ref()
            && !access_units.is_empty()
        {
            decoder.decode_access_units(&access_units, self.config.fps)?;
        }
        if self.report_metrics {
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
        ensure_vt_codec_implemented(self.config.codec, "decode")?;
        let submit_start = Instant::now();
        let (access_units, cache) = self.assembler.flush()?;
        let input_copy_bytes = packed_access_units_bytes(&access_units);
        let access_unit_count = access_units.len();
        self.ensure_decoder(&cache)?;

        if let Some(decoder) = self.decoder.as_ref()
            && !access_units.is_empty()
        {
            decoder.decode_access_units(&access_units, self.config.fps)?;
        }
        if self.report_metrics {
            eprintln!(
                "[vt.decode.submit] flush=true, access_units={}, input_copy_bytes={}, submit_ms={:.3}",
                access_unit_count,
                input_copy_bytes,
                submit_start.elapsed().as_secs_f64() * 1_000.0
            );
        }

        self.take_delta(true)
    }

    fn try_reap(&mut self) -> Result<Vec<Frame>, BackendError> {
        self.take_delta(false)
    }

    fn decode_summary(&self) -> DecodeSummary {
        self.last_summary.clone()
    }
}
