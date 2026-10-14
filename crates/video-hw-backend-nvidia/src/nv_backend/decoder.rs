//! Stateful NVIDIA bitstream assembly and decode adapter.

use super::*;

#[derive(Debug, Default)]
struct AnnexBPacker {
    data: Vec<u8>,
}

impl AnnexBPacker {
    fn pack<'a>(
        &'a mut self,
        codec: Codec,
        access_unit: &AccessUnit,
    ) -> Result<&'a [u8], BackendError> {
        self.data.clear();
        if codec == Codec::Av1 {
            let total_size = access_unit.nalus.iter().try_fold(0usize, |total, obu| {
                total.checked_add(obu.len()).ok_or_else(|| {
                    BackendError::InvalidBitstream("AV1 access unit size overflow".to_string())
                })
            })?;
            self.data
                .reserve(total_size.saturating_sub(self.data.capacity()));
            for obu in &access_unit.nalus {
                self.data.extend_from_slice(obu);
            }
            return Ok(&self.data);
        }
        let total_size = access_unit.nalus.iter().try_fold(0usize, |total, nal| {
            let framed_len = nal.len().checked_add(4).ok_or_else(|| {
                BackendError::InvalidBitstream("Annex B NAL size overflow".to_string())
            })?;
            total.checked_add(framed_len).ok_or_else(|| {
                BackendError::InvalidBitstream("Annex B access unit size overflow".to_string())
            })
        })?;
        self.data
            .reserve(total_size.saturating_sub(self.data.capacity()));

        for nal in &access_unit.nalus {
            self.data.extend_from_slice(&[0, 0, 0, 1]);
            self.data.extend_from_slice(nal);
        }

        Ok(&self.data)
    }
}

#[derive(Debug)]
struct DecodeReapSummary {
    frames: Vec<Frame>,
    map_samples: SampleStats,
    queue_depth_samples: SampleStats,
    jitter_samples: SampleStats,
}

pub struct NvDecoderAdapter {
    config: DecoderConfig,
    report_metrics: bool,
    assembler: StatefulBitstreamAssembler,
    packer: AnnexBPacker,
    decoder: Option<NvMetaDecoder>,
    next_pts_90k: i64,
    last_summary: DecodeSummary,
}

impl NvDecoderAdapter {
    pub fn new(config: DecoderConfig) -> Self {
        let report_metrics = match &config.backend_options {
            BackendDecoderOptions::Nvidia(options) => options
                .report_metrics
                .or_else(|| env_bool("VIDEO_HW_NV_METRICS"))
                .unwrap_or(false),
            BackendDecoderOptions::Default
            | BackendDecoderOptions::VideoToolbox(_)
            | BackendDecoderOptions::Intel(_)
            | BackendDecoderOptions::Vulkan(_)
            | BackendDecoderOptions::Android(_) => env_bool("VIDEO_HW_NV_METRICS").unwrap_or(false),
        };
        Self {
            assembler: StatefulBitstreamAssembler::with_codec(config.codec),
            packer: AnnexBPacker::default(),
            config,
            report_metrics,
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
        let decoder = NvMetaDecoder::new(
            cuda_ctx,
            to_decode_codec(self.config.codec),
            self.config.output_mode,
        )?;

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
            let packed = self.packer.pack(self.config.codec, au)?;
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

        if self.report_metrics {
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
            self.last_summary.pixel_format = last.pixel_format;
        }
    }
}

impl VideoDecoder for NvDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(nvidia_capability_report(codec))
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

    fn try_reap(&mut self) -> Result<Vec<Frame>, BackendError> {
        let Some(decoder) = self.decoder.as_mut() else {
            return Ok(Vec::new());
        };
        let drained = decoder.try_drain()?;
        self.apply_decoded_summary(&drained);
        Ok(drained)
    }

    fn decode_summary(&self) -> DecodeSummary {
        self.last_summary.clone()
    }
}
