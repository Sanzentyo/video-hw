//! GOP/window decode orchestration and eager frame decoding.

use super::*;

#[derive(Debug, Clone)]
pub struct GopCursor {
    pub range: SampleRange,
    pub decode_start_sample: SampleId,
    pub next_sample: SampleId,
}

impl GopCursor {
    pub fn new(reader: &mut Fmp4Reader<SyncReading>, range: SampleRange) -> Result<Self> {
        let first_sample = {
            let samples = samples_in_range(reader, range)?;
            let Some(first) = samples.first() else {
                anyhow::bail!("sample range is empty");
            };
            first.sample_id
        };
        let decode_start_sample = reader
            .keyframe_before(first_sample)?
            .with_context(|| format!("no keyframe before sample {}", first_sample))?;
        Ok(Self {
            range,
            decode_start_sample,
            next_sample: decode_start_sample,
        })
    }
}

pub struct FrameDecoder<'a> {
    reader: &'a mut Fmp4Reader<SyncReading>,
}

impl<'a> FrameDecoder<'a> {
    pub fn new(reader: &'a mut Fmp4Reader<SyncReading>) -> Self {
        Self { reader }
    }

    pub fn decode_sample(&mut self, request: FrameDecodeRequest) -> Result<FrameDecodeResult> {
        let target_meta = self
            .reader
            .sample_meta(request.target_sample)?
            .cloned()
            .with_context(|| format!("unknown target sample {}", request.target_sample))?;
        anyhow::ensure!(
            target_meta.track_id == request.track_id,
            "target sample {} belongs to track {}, not requested track {}",
            request.target_sample,
            target_meta.track_id,
            request.track_id
        );
        let track = self
            .reader
            .tracks()
            .iter()
            .find(|track| track.track_id == request.track_id)
            .cloned()
            .with_context(|| format!("unknown track {}", request.track_id))?;
        let codec = track
            .codec()
            .with_context(|| format!("track {} has no supported video codec", track.track_id))?;
        let fps = {
            let samples = self.reader.samples(request.track_id)?;
            request.fps.unwrap_or_else(|| estimate_fps(samples)).max(1)
        };
        let mut config = DecoderConfig::new(codec, fps, request.require_hardware);
        config.output_mode = request.output_mode;
        apply_track_decoder_options(&mut config, &track);
        let (mut session, mut diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let resolved_backend = diagnostics.resolved_backend;
        let mut frames = VecDeque::new();
        let mut target_frame_index = None;
        let mut pts_to_sample = HashMap::<i64, SampleId>::new();
        let gop_samples = self
            .reader
            .iter_gop_for_sample(request.target_sample)?
            .collect::<Result<Vec<_>>>()?;
        let gop_metas = gop_samples
            .iter()
            .map(|sample| sample.meta.clone())
            .collect::<Vec<_>>();
        let gop_sample_order = ordered_sample_ids_by_pts(&gop_metas);
        let gop_meta_by_id = sample_meta_by_id(&gop_metas);
        let gop_presentation_index_by_id = presentation_index_by_id(&gop_sample_order);
        for sample in gop_samples {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.target_sample,
                target_frame_index: &mut target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &gop_meta_by_id,
                presentation_index_by_id: &gop_presentation_index_by_id,
            };
            submit_sample(&mut session, &track, sample, &mut collect)?;
        }

        for frame in session
            .flush()
            .map_err(anyhow::Error::new)
            .context("decoder flush failed")?
        {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.target_sample,
                target_frame_index: &mut target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &gop_meta_by_id,
                presentation_index_by_id: &gop_presentation_index_by_id,
            };
            push_decoded_frame(frame, &mut collect);
        }
        let ambiguous_sample_association_count =
            align_frames_to_presentation_order(&mut frames, &gop_sample_order);
        sort_frames_by_sample_order(&mut frames, &gop_sample_order);
        let decoded_frame_count = frames.len();
        attach_sample_metadata(&mut frames, &gop_meta_by_id, &gop_sample_order);
        target_frame_index = find_frame_index(&frames, request.target_sample);
        finalize_decode_diagnostics(
            &mut diagnostics,
            &gop_sample_order,
            decoded_frame_count,
            ambiguous_sample_association_count,
            ReturnedFrameOrder::Presentation,
            &frames,
        );

        Ok(FrameDecodeResult {
            target_sample: request.target_sample,
            diagnostics,
            resolved_backend,
            output_mode: request.output_mode,
            frames: frames.into_iter().collect(),
            target_frame_index,
        })
    }

    pub fn decode_range(
        &mut self,
        request: FrameDecodeRangeRequest,
    ) -> Result<FrameDecodeRangeResult> {
        let cursor = GopCursor::new(self.reader, request.range)?;
        let track = self
            .reader
            .tracks()
            .iter()
            .find(|track| track.track_id == request.range.track_id)
            .cloned()
            .with_context(|| format!("unknown track {}", request.range.track_id))?;
        let codec = track
            .codec()
            .with_context(|| format!("track {} has no supported video codec", track.track_id))?;
        let fps = {
            let samples = self.reader.samples(request.range.track_id)?;
            request.fps.unwrap_or_else(|| estimate_fps(samples)).max(1)
        };
        let mut config = DecoderConfig::new(codec, fps, request.require_hardware);
        config.output_mode = request.output_mode;
        apply_track_decoder_options(&mut config, &track);
        let (mut session, mut diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let resolved_backend = diagnostics.resolved_backend;
        let requested_samples = samples_in_range(self.reader, request.range)?.to_vec();
        let wanted_sample_order = ordered_sample_ids_by_pts(&requested_samples);
        let wanted_sample_ids = wanted_sample_order.iter().copied().collect::<HashSet<_>>();
        let requested_meta_by_id = sample_meta_by_id(&requested_samples);
        let requested_presentation_index_by_id = presentation_index_by_id(&wanted_sample_order);
        let end_pts = requested_samples
            .last()
            .map(sample_end_pts)
            .context("sample range is empty")?;
        let mut frames = VecDeque::new();
        let mut ignored_target_frame_index = None;
        let mut pts_to_sample = HashMap::<i64, SampleId>::new();
        let decode_segment = GopSegment {
            track_id: request.range.track_id,
            keyframe_sample: cursor.decode_start_sample,
            end_sample_exclusive: request.range.end_sample_exclusive,
            start_pts: self
                .reader
                .sample_meta(cursor.decode_start_sample)?
                .context("decode start sample disappeared")?
                .pts,
            end_pts,
        };

        let decode_samples = self
            .reader
            .iter_encoded(decode_segment)?
            .collect::<Result<Vec<_>>>()?;
        let decode_sample_order = ordered_sample_ids_by_pts(
            &decode_samples
                .iter()
                .map(|sample| sample.meta.clone())
                .collect::<Vec<_>>(),
        );
        for sample in decode_samples {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.range.start_sample,
                target_frame_index: &mut ignored_target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &requested_meta_by_id,
                presentation_index_by_id: &requested_presentation_index_by_id,
            };
            submit_sample(&mut session, &track, sample, &mut collect)?;
        }
        for frame in session
            .flush()
            .map_err(anyhow::Error::new)
            .context("decoder flush failed")?
        {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.range.start_sample,
                target_frame_index: &mut ignored_target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &requested_meta_by_id,
                presentation_index_by_id: &requested_presentation_index_by_id,
            };
            push_decoded_frame(frame, &mut collect);
        }
        let decoded_frame_count = frames.len();
        let ambiguous_sample_association_count =
            align_frames_to_presentation_order(&mut frames, &decode_sample_order);
        retain_requested_frames(&mut frames, &wanted_sample_ids);
        sort_frames_by_sample_order(&mut frames, &wanted_sample_order);
        attach_sample_metadata(&mut frames, &requested_meta_by_id, &wanted_sample_order);
        finalize_decode_diagnostics(
            &mut diagnostics,
            &wanted_sample_order,
            decoded_frame_count,
            ambiguous_sample_association_count,
            ReturnedFrameOrder::Presentation,
            &frames,
        );
        Ok(FrameDecodeRangeResult {
            range: request.range,
            diagnostics,
            resolved_backend,
            output_mode: request.output_mode,
            frames: frames.into_iter().collect(),
        })
    }

    pub fn decode_range_iter(
        &mut self,
        request: FrameDecodeRangeRequest,
    ) -> Result<DecodedFrameIter<'_>> {
        let cursor = GopCursor::new(self.reader, request.range)?;
        let track = self
            .reader
            .tracks()
            .iter()
            .find(|track| track.track_id == request.range.track_id)
            .cloned()
            .with_context(|| format!("unknown track {}", request.range.track_id))?;
        let codec = track
            .codec()
            .with_context(|| format!("track {} has no supported video codec", track.track_id))?;
        let fps = {
            let samples = self.reader.samples(request.range.track_id)?;
            request.fps.unwrap_or_else(|| estimate_fps(samples)).max(1)
        };
        let mut config = DecoderConfig::new(codec, fps, request.require_hardware);
        config.output_mode = request.output_mode;
        apply_track_decoder_options(&mut config, &track);
        let (session, diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let (wanted_sample_ids, sample_meta_by_id, presentation_index_by_id, end_pts) = {
            let samples = samples_in_range(self.reader, request.range)?;
            let wanted_sample_ids = samples
                .iter()
                .map(|sample| sample.sample_id)
                .collect::<HashSet<_>>();
            let sample_order = ordered_sample_ids_by_pts(samples);
            let sample_meta_by_id = sample_meta_by_id(samples);
            let presentation_index_by_id = sample_order
                .iter()
                .enumerate()
                .map(|(index, sample_id)| (*sample_id, index))
                .collect::<HashMap<_, _>>();
            let end_pts = samples
                .last()
                .map(|sample| {
                    MediaTime::new(
                        sample.pts.ticks.saturating_add(u64::from(sample.duration)),
                        sample.pts.timescale,
                    )
                })
                .context("sample range is empty")?;
            (
                wanted_sample_ids,
                sample_meta_by_id,
                presentation_index_by_id,
                end_pts,
            )
        };
        let decode_segment = GopSegment {
            track_id: request.range.track_id,
            keyframe_sample: cursor.decode_start_sample,
            end_sample_exclusive: request.range.end_sample_exclusive,
            start_pts: self
                .reader
                .sample_meta(cursor.decode_start_sample)?
                .context("decode start sample disappeared")?
                .pts,
            end_pts,
        };

        let decode_samples = self.reader.iter_encoded(decode_segment)?;
        Ok(DecodedFrameIter {
            range: request.range,
            diagnostics,
            track,
            session,
            decode_samples,
            wanted_sample_ids,
            sample_meta_by_id,
            presentation_index_by_id,
            pts_to_sample: HashMap::new(),
            buffered_frames: VecDeque::new(),
            target_frame_index: None,
            finished: false,
        })
    }

    pub fn decode_window(
        &mut self,
        request: FrameDecodeWindowRequest,
    ) -> Result<FrameDecodeRangeResult> {
        let samples = self.reader.samples(request.track_id)?.to_vec();
        let center_decode_index = samples
            .iter()
            .position(|sample| sample.sample_id == request.center_sample)
            .with_context(|| {
                format!(
                    "center sample {} does not belong to track {}",
                    request.center_sample, request.track_id
                )
            })?;
        let wanted_sample_order = presentation_window_sample_ids(
            &samples,
            request.center_sample,
            request.before,
            request.after,
        )?;
        let wanted_sample_ids = wanted_sample_order.iter().copied().collect::<HashSet<_>>();
        let sample_meta_by_id = sample_meta_by_id(&samples);
        let presentation_index_by_id = presentation_index_by_id(&wanted_sample_order);
        let (decode_first_index, decode_end_index) =
            decode_span_for_sample_ids(&samples, &wanted_sample_ids)?;
        let decode_anchor_index = decode_first_index.min(center_decode_index);
        let decode_start_sample = self
            .reader
            .keyframe_before(samples[decode_anchor_index].sample_id)?
            .with_context(|| {
                format!(
                    "no keyframe before sample {}",
                    samples[decode_anchor_index].sample_id
                )
            })?;
        let decode_end_exclusive = samples
            .get(decode_end_index)
            .map_or(SampleId(u64::MAX), |sample| sample.sample_id);
        let decode_end_pts = samples
            .get(decode_end_index.saturating_sub(1))
            .map(sample_end_pts)
            .context("decode window is empty")?;
        let range = SampleRange {
            track_id: request.track_id,
            start_sample: *wanted_sample_order
                .first()
                .context("presentation window is empty")?,
            end_sample_exclusive: wanted_sample_order
                .last()
                .and_then(|last| sample_after_in_pts_order(&samples, *last))
                .unwrap_or(SampleId(u64::MAX)),
        };

        let track = self
            .reader
            .tracks()
            .iter()
            .find(|track| track.track_id == request.track_id)
            .cloned()
            .with_context(|| format!("unknown track {}", request.track_id))?;
        let codec = track
            .codec()
            .with_context(|| format!("track {} has no supported video codec", track.track_id))?;
        let fps = request.fps.unwrap_or_else(|| estimate_fps(&samples)).max(1);
        let mut config = DecoderConfig::new(codec, fps, request.require_hardware);
        config.output_mode = request.output_mode;
        apply_track_decoder_options(&mut config, &track);
        let (mut session, mut diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let resolved_backend = diagnostics.resolved_backend;
        let decode_start_pts = self
            .reader
            .sample_meta(decode_start_sample)?
            .context("decode start sample disappeared")?
            .pts;
        let decode_segment = GopSegment {
            track_id: request.track_id,
            keyframe_sample: decode_start_sample,
            end_sample_exclusive: decode_end_exclusive,
            start_pts: decode_start_pts,
            end_pts: decode_end_pts,
        };
        let decode_samples = self
            .reader
            .iter_encoded(decode_segment)?
            .collect::<Result<Vec<_>>>()?;
        let decode_sample_order = ordered_sample_ids_by_pts(
            &decode_samples
                .iter()
                .map(|sample| sample.meta.clone())
                .collect::<Vec<_>>(),
        );
        let mut frames = VecDeque::new();
        let mut target_frame_index = None;
        let mut pts_to_sample = HashMap::<i64, SampleId>::new();
        for sample in decode_samples {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.center_sample,
                target_frame_index: &mut target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &sample_meta_by_id,
                presentation_index_by_id: &presentation_index_by_id,
            };
            submit_sample(&mut session, &track, sample, &mut collect)?;
        }
        for frame in session
            .flush()
            .map_err(anyhow::Error::new)
            .context("decoder flush failed")?
        {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.center_sample,
                target_frame_index: &mut target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &sample_meta_by_id,
                presentation_index_by_id: &presentation_index_by_id,
            };
            push_decoded_frame(frame, &mut collect);
        }
        let decoded_frame_count = frames.len();
        let ambiguous_sample_association_count =
            align_frames_to_presentation_order(&mut frames, &decode_sample_order);
        retain_requested_frames(&mut frames, &wanted_sample_ids);
        sort_frames_by_sample_order(&mut frames, &wanted_sample_order);
        attach_sample_metadata(&mut frames, &sample_meta_by_id, &wanted_sample_order);
        finalize_decode_diagnostics(
            &mut diagnostics,
            &wanted_sample_order,
            decoded_frame_count,
            ambiguous_sample_association_count,
            ReturnedFrameOrder::Presentation,
            &frames,
        );
        Ok(FrameDecodeRangeResult {
            range,
            diagnostics,
            resolved_backend,
            output_mode: request.output_mode,
            frames: frames.into_iter().collect(),
        })
    }
}
