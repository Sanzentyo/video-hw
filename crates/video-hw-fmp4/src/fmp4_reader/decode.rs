use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result, anyhow};
use video_hw::{
    AnyDecodeSession, Backend, BackendError, BackendKind, BitstreamInput, DecodeOutputMode,
    DecodedFrame, DecoderConfig, Timestamp90k,
};

use super::{
    EncodedSample, EncodedSampleIter, Fmp4Reader, GopSegment, MediaTime, SampleId, SampleMeta,
    SampleRange, SyncReading, TrackId, config::Fmp4Track,
};

#[cfg(feature = "serde")]
mod serde_backend {
    use std::str::FromStr;

    use serde::{Deserialize, Deserializer, Serializer, de};
    use video_hw::Backend;

    pub fn serialize<S>(backend: &Backend, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&backend.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Backend, D::Error>
    where
        D: Deserializer<'de>,
    {
        Backend::from_str(&String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[cfg(feature = "serde")]
mod serde_backend_kind {
    use serde::{Deserialize, Deserializer, Serializer, de};
    use video_hw::BackendKind;

    pub fn serialize<S>(backend: &BackendKind, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&backend.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BackendKind, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            "videotoolbox" | "vt" => Ok(BackendKind::VideoToolbox),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            "nvidia" | "nv" => Ok(BackendKind::Nvidia),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            "intel" | "qsv" => Ok(BackendKind::Intel),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            "vulkan" => Ok(BackendKind::Vulkan),
            other => Err(de::Error::custom(format!(
                "backend kind {other:?} is not available with the enabled features"
            ))),
        }
    }
}

#[cfg(feature = "serde")]
mod serde_decode_output_mode {
    use serde::{Deserialize, Deserializer, Serializer, de};
    use video_hw::DecodeOutputMode;

    pub fn serialize<S>(mode: &DecodeOutputMode, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&mode.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<DecodeOutputMode, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "metadata" => Ok(DecodeOutputMode::Metadata),
            "nv12" => Ok(DecodeOutputMode::Nv12),
            "rgb24" => Ok(DecodeOutputMode::Rgb24),
            other => Err(de::Error::unknown_variant(
                other,
                &["metadata", "nv12", "rgb24"],
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FrameDecodeRequest {
    pub track_id: TrackId,
    pub target_sample: SampleId,
    pub backend: Backend,
    pub require_hardware: bool,
    pub output_mode: DecodeOutputMode,
    pub fps: Option<i32>,
}

impl FrameDecodeRequest {
    pub fn new(track_id: TrackId, target_sample: SampleId) -> Self {
        Self {
            track_id,
            target_sample,
            backend: Backend::Auto,
            require_hardware: false,
            output_mode: DecodeOutputMode::Rgb24,
            fps: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecodedSampleFrame {
    pub sample_id: Option<SampleId>,
    pub frame: DecodedFrame,
}

#[derive(Debug, Clone)]
pub struct FrameDecodeRangeRequest {
    pub range: SampleRange,
    pub backend: Backend,
    pub require_hardware: bool,
    pub output_mode: DecodeOutputMode,
    pub fps: Option<i32>,
}

impl FrameDecodeRangeRequest {
    pub fn new(range: SampleRange) -> Self {
        Self {
            range,
            backend: Backend::Auto,
            require_hardware: false,
            output_mode: DecodeOutputMode::Rgb24,
            fps: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FrameDecodeWindowRequest {
    pub track_id: TrackId,
    pub center_sample: SampleId,
    pub before: u32,
    pub after: u32,
    pub backend: Backend,
    pub require_hardware: bool,
    pub output_mode: DecodeOutputMode,
    pub fps: Option<i32>,
}

impl FrameDecodeWindowRequest {
    pub fn new(track_id: TrackId, center_sample: SampleId) -> Self {
        Self {
            track_id,
            center_sample,
            before: 0,
            after: 0,
            backend: Backend::Auto,
            require_hardware: false,
            output_mode: DecodeOutputMode::Rgb24,
            fps: None,
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DecodeDiagnostics {
    #[cfg_attr(feature = "serde", serde(with = "serde_backend"))]
    pub requested_backend: Backend,
    #[cfg_attr(feature = "serde", serde(with = "serde_backend_kind"))]
    pub resolved_backend: BackendKind,
    pub require_hardware: bool,
    #[cfg_attr(feature = "serde", serde(with = "serde_decode_output_mode"))]
    pub output_mode: DecodeOutputMode,
    pub fps: i32,
    pub fallback_used: bool,
    pub fallback_reason: Option<String>,
    pub requested_sample_count: usize,
    pub returned_sample_count: usize,
    pub missing_sample_ids: Vec<SampleId>,
}

#[derive(Debug, Clone)]
pub struct FrameDecodeResult {
    pub target_sample: SampleId,
    pub diagnostics: DecodeDiagnostics,
    pub resolved_backend: BackendKind,
    pub output_mode: DecodeOutputMode,
    pub frames: Vec<DecodedSampleFrame>,
    pub target_frame_index: Option<usize>,
}

impl FrameDecodeResult {
    pub fn target_frame(&self) -> Option<&DecodedSampleFrame> {
        self.target_frame_index
            .and_then(|index| self.frames.get(index))
    }
}

#[derive(Debug, Clone)]
pub struct FrameDecodeRangeResult {
    pub range: SampleRange,
    pub diagnostics: DecodeDiagnostics,
    pub resolved_backend: BackendKind,
    pub output_mode: DecodeOutputMode,
    pub frames: Vec<DecodedSampleFrame>,
}

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
            .keyframe_before(first_sample)
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

pub struct DecodedFrameIter<'a> {
    range: SampleRange,
    diagnostics: DecodeDiagnostics,
    track: Fmp4Track,
    session: AnyDecodeSession,
    decode_samples: EncodedSampleIter<'a>,
    wanted_sample_ids: HashSet<SampleId>,
    pts_to_sample: HashMap<i64, SampleId>,
    buffered_frames: VecDeque<DecodedSampleFrame>,
    target_frame_index: Option<usize>,
    finished: bool,
}

impl DecodedFrameIter<'_> {
    pub fn diagnostics(&self) -> &DecodeDiagnostics {
        &self.diagnostics
    }

    pub fn range(&self) -> SampleRange {
        self.range
    }
}

impl<'a> FrameDecoder<'a> {
    pub fn new(reader: &'a mut Fmp4Reader<SyncReading>) -> Self {
        Self { reader }
    }

    pub fn decode_sample(&mut self, request: FrameDecodeRequest) -> Result<FrameDecodeResult> {
        let target_meta = self
            .reader
            .sample_meta(request.target_sample)
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
        for sample in gop_samples {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.target_sample,
                target_frame_index: &mut target_frame_index,
                filter_sample_ids: None,
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
            };
            push_decoded_frame(frame, &mut collect);
        }
        assign_frames_by_presentation_order(&mut frames, &gop_sample_order);
        sort_frames_by_sample_order(&mut frames, &gop_sample_order);
        target_frame_index = find_frame_index(&frames, request.target_sample);
        finalize_decode_diagnostics(&mut diagnostics, &gop_sample_order, &frames);

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
        let (mut session, mut diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let resolved_backend = diagnostics.resolved_backend;
        let requested_samples = samples_in_range(self.reader, request.range)?.to_vec();
        let wanted_sample_order = ordered_sample_ids_by_pts(&requested_samples);
        let wanted_sample_ids = wanted_sample_order.iter().copied().collect::<HashSet<_>>();
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
                .sample_meta(cursor.decode_start_sample)
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
            };
            push_decoded_frame(frame, &mut collect);
        }
        assign_frames_by_presentation_order(&mut frames, &decode_sample_order);
        retain_requested_frames(&mut frames, &wanted_sample_ids);
        sort_frames_by_sample_order(&mut frames, &wanted_sample_order);
        finalize_decode_diagnostics(&mut diagnostics, &wanted_sample_order, &frames);
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
        let (session, diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let (wanted_sample_ids, end_pts) = {
            let samples = samples_in_range(self.reader, request.range)?;
            let wanted_sample_ids = samples
                .iter()
                .map(|sample| sample.sample_id)
                .collect::<HashSet<_>>();
            let end_pts = samples
                .last()
                .map(|sample| {
                    MediaTime::new(
                        sample.pts.ticks.saturating_add(u64::from(sample.duration)),
                        sample.pts.timescale,
                    )
                })
                .context("sample range is empty")?;
            (wanted_sample_ids, end_pts)
        };
        let decode_segment = GopSegment {
            track_id: request.range.track_id,
            keyframe_sample: cursor.decode_start_sample,
            end_sample_exclusive: request.range.end_sample_exclusive,
            start_pts: self
                .reader
                .sample_meta(cursor.decode_start_sample)
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
        let (decode_first_index, decode_end_index) =
            decode_span_for_sample_ids(&samples, &wanted_sample_ids)?;
        let decode_anchor_index = decode_first_index.min(center_decode_index);
        let decode_start_sample = self
            .reader
            .keyframe_before(samples[decode_anchor_index].sample_id)
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
        let (mut session, mut diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let resolved_backend = diagnostics.resolved_backend;
        let decode_start_pts = self
            .reader
            .sample_meta(decode_start_sample)
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
            };
            push_decoded_frame(frame, &mut collect);
        }
        assign_frames_by_presentation_order(&mut frames, &decode_sample_order);
        retain_requested_frames(&mut frames, &wanted_sample_ids);
        sort_frames_by_sample_order(&mut frames, &wanted_sample_order);
        finalize_decode_diagnostics(&mut diagnostics, &wanted_sample_order, &frames);
        Ok(FrameDecodeRangeResult {
            range,
            diagnostics,
            resolved_backend,
            output_mode: request.output_mode,
            frames: frames.into_iter().collect(),
        })
    }
}

impl Iterator for DecodedFrameIter<'_> {
    type Item = Result<DecodedSampleFrame>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(frame) = self.buffered_frames.pop_front() {
                return Some(Ok(frame));
            }
            if self.finished {
                return None;
            }
            match self.decode_samples.next() {
                Some(Ok(sample)) => {
                    let sample_id = sample.meta.sample_id;
                    let mut collect = DecodeCollectState {
                        pts_to_sample: &mut self.pts_to_sample,
                        frames: &mut self.buffered_frames,
                        target_sample: self.range.start_sample,
                        target_frame_index: &mut self.target_frame_index,
                        filter_sample_ids: Some(&self.wanted_sample_ids),
                    };
                    if let Err(err) =
                        submit_sample(&mut self.session, &self.track, sample, &mut collect)
                    {
                        return Some(Err(err.context(format!(
                            "streaming decode failed at sample {} on track {}",
                            sample_id, self.track.track_id
                        ))));
                    }
                }
                Some(Err(err)) => return Some(Err(err)),
                None => {
                    self.finished = true;
                    match self.session.flush().map_err(anyhow::Error::new) {
                        Ok(frames) => {
                            for frame in frames {
                                let mut collect = DecodeCollectState {
                                    pts_to_sample: &mut self.pts_to_sample,
                                    frames: &mut self.buffered_frames,
                                    target_sample: self.range.start_sample,
                                    target_frame_index: &mut self.target_frame_index,
                                    filter_sample_ids: Some(&self.wanted_sample_ids),
                                };
                                push_decoded_frame(frame, &mut collect);
                            }
                        }
                        Err(err) => return Some(Err(err.context("decoder flush failed"))),
                    }
                }
            }
        }
    }
}

fn create_decode_session(
    requested_backend: Backend,
    config: DecoderConfig,
) -> Result<(AnyDecodeSession, DecodeDiagnostics)> {
    let resolved_backend = requested_backend.resolve_decoder(&config).with_context(|| {
        format!(
            "failed to resolve decoder backend: requested={}, codec={}, fps={}, require_hardware={}",
            requested_backend, config.codec, config.fps, config.require_hardware
        )
    })?;
    match AnyDecodeSession::with_backend_kind(resolved_backend, config.clone()) {
        Ok(session) => Ok((
            session,
            DecodeDiagnostics {
                requested_backend,
                resolved_backend,
                require_hardware: config.require_hardware,
                output_mode: config.output_mode,
                fps: config.fps,
                fallback_used: false,
                fallback_reason: None,
                requested_sample_count: 0,
                returned_sample_count: 0,
                missing_sample_ids: Vec::new(),
            },
        )),
        Err(primary_err) if requested_backend != Backend::Auto && !config.require_hardware => {
            let fallback_backend = Backend::Auto
                .resolve_decoder(&config)
                .context("failed to resolve fallback auto decoder backend")?;
            let fallback_reason =
                format!("requested backend {resolved_backend} failed: {primary_err}");
            let session = AnyDecodeSession::with_backend_kind(fallback_backend, config.clone())
                .with_context(|| {
                    format!(
                        "fallback backend {fallback_backend} also failed after {resolved_backend}"
                    )
                })?;
            Ok((
                session,
                DecodeDiagnostics {
                    requested_backend,
                    resolved_backend: fallback_backend,
                    require_hardware: config.require_hardware,
                    output_mode: config.output_mode,
                    fps: config.fps,
                    fallback_used: fallback_backend != resolved_backend,
                    fallback_reason: Some(fallback_reason),
                    requested_sample_count: 0,
                    returned_sample_count: 0,
                    missing_sample_ids: Vec::new(),
                },
            ))
        }
        Err(err) => Err(anyhow!(err).context(format!(
            "failed to create decoder session with {resolved_backend}"
        ))),
    }
}

fn submit_sample(
    session: &mut AnyDecodeSession,
    track: &Fmp4Track,
    sample: EncodedSample,
    collect: &mut DecodeCollectState<'_>,
) -> Result<()> {
    let sample_id = sample.meta.sample_id;
    let pts_90k = sample_timestamp_to_90k(&sample.meta);
    if let Some(pts) = pts_90k {
        collect.pts_to_sample.insert(pts.0, sample_id);
    }
    let annexb = sample
        .to_annexb()
        .with_context(|| format!("failed to convert sample {sample_id} to Annex-B"))?;
    loop {
        match session.submit(BitstreamInput::AnnexBChunk {
            chunk: annexb.clone(),
            pts_90k,
        }) {
            Ok(()) => break,
            Err(BackendError::TemporaryBackpressure(_)) => drain_ready_frames(session, collect)?,
            Err(err) => {
                return Err(anyhow!(err).context(format!(
                    "decoder submit failed at sample {sample_id} on track {}",
                    track.track_id
                )));
            }
        }
    }
    drain_ready_frames(session, collect)
        .with_context(|| format!("failed to reap decoded frames after sample {sample_id}"))
}

fn drain_ready_frames(
    session: &mut AnyDecodeSession,
    collect: &mut DecodeCollectState<'_>,
) -> Result<()> {
    loop {
        match session.try_reap() {
            Ok(Some(frame)) => {
                push_decoded_frame(frame, collect);
            }
            Ok(None) => return Ok(()),
            Err(err) => return Err(anyhow!(err)),
        }
    }
}

fn push_decoded_frame(frame: DecodedFrame, collect: &mut DecodeCollectState<'_>) {
    let sample_id =
        frame_pts_90k(&frame).and_then(|pts| collect.pts_to_sample.get(&pts.0).copied());
    if let Some(filter) = collect.filter_sample_ids
        && !sample_id.is_some_and(|sample_id| filter.contains(&sample_id))
    {
        return;
    }
    if sample_id == Some(collect.target_sample) && collect.target_frame_index.is_none() {
        *collect.target_frame_index = Some(collect.frames.len());
    }
    collect
        .frames
        .push_back(DecodedSampleFrame { sample_id, frame });
}

struct DecodeCollectState<'a> {
    pts_to_sample: &'a mut HashMap<i64, SampleId>,
    frames: &'a mut VecDeque<DecodedSampleFrame>,
    target_sample: SampleId,
    target_frame_index: &'a mut Option<usize>,
    filter_sample_ids: Option<&'a HashSet<SampleId>>,
}

fn ordered_sample_ids_by_pts(samples: &[SampleMeta]) -> Vec<SampleId> {
    let mut ordered = samples.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|sample| (sample.pts.ticks, sample.sample_id));
    ordered.into_iter().map(|sample| sample.sample_id).collect()
}

fn presentation_window_sample_ids(
    samples: &[SampleMeta],
    center_sample: SampleId,
    before: u32,
    after: u32,
) -> Result<Vec<SampleId>> {
    let ordered = ordered_sample_ids_by_pts(samples);
    let center_index = ordered
        .iter()
        .position(|sample| *sample == center_sample)
        .with_context(|| {
            format!(
                "center sample {} is not present in PTS order",
                center_sample
            )
        })?;
    let before = usize::try_from(before).context("window before overflows usize")?;
    let after = usize::try_from(after).context("window after overflows usize")?;
    let start = center_index.saturating_sub(before);
    let end = center_index
        .saturating_add(after)
        .saturating_add(1)
        .min(ordered.len());
    Ok(ordered[start..end].to_vec())
}

fn decode_span_for_sample_ids(
    samples: &[SampleMeta],
    sample_ids: &HashSet<SampleId>,
) -> Result<(usize, usize)> {
    let mut positions = samples
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| sample_ids.contains(&sample.sample_id).then_some(index));
    let Some(first) = positions.next() else {
        anyhow::bail!("decode sample set is empty");
    };
    let (min_index, max_index) = positions.fold((first, first), |(min_index, max_index), index| {
        (min_index.min(index), max_index.max(index))
    });
    Ok((min_index, max_index.saturating_add(1)))
}

fn sample_after_in_pts_order(samples: &[SampleMeta], sample_id: SampleId) -> Option<SampleId> {
    let ordered = ordered_sample_ids_by_pts(samples);
    let index = ordered.iter().position(|sample| *sample == sample_id)?;
    ordered.get(index.saturating_add(1)).copied()
}

fn sample_end_pts(sample: &SampleMeta) -> MediaTime {
    MediaTime::new(
        sample.pts.ticks.saturating_add(u64::from(sample.duration)),
        sample.pts.timescale,
    )
}

fn sort_frames_by_sample_order(
    frames: &mut VecDeque<DecodedSampleFrame>,
    sample_order: &[SampleId],
) {
    let order = sample_order
        .iter()
        .enumerate()
        .map(|(index, sample)| (*sample, index))
        .collect::<HashMap<_, _>>();
    let mut sorted = frames.drain(..).collect::<Vec<_>>();
    sorted.sort_by_key(|frame| {
        frame
            .sample_id
            .and_then(|sample| order.get(&sample).copied())
            .unwrap_or(usize::MAX)
    });
    *frames = sorted.into();
}

fn assign_frames_by_presentation_order(
    frames: &mut VecDeque<DecodedSampleFrame>,
    sample_order: &[SampleId],
) {
    for (frame, sample_id) in frames.iter_mut().zip(sample_order.iter().copied()) {
        frame.sample_id = Some(sample_id);
    }
}

fn retain_requested_frames(
    frames: &mut VecDeque<DecodedSampleFrame>,
    wanted_sample_ids: &HashSet<SampleId>,
) {
    frames.retain(|frame| {
        frame
            .sample_id
            .is_some_and(|sample_id| wanted_sample_ids.contains(&sample_id))
    });
}

fn find_frame_index(
    frames: &VecDeque<DecodedSampleFrame>,
    target_sample: SampleId,
) -> Option<usize> {
    frames
        .iter()
        .position(|frame| frame.sample_id == Some(target_sample))
}

fn finalize_decode_diagnostics(
    diagnostics: &mut DecodeDiagnostics,
    requested_sample_order: &[SampleId],
    frames: &VecDeque<DecodedSampleFrame>,
) {
    let returned = frames
        .iter()
        .filter_map(|frame| frame.sample_id)
        .collect::<HashSet<_>>();
    diagnostics.requested_sample_count = requested_sample_order.len();
    diagnostics.returned_sample_count = returned.len();
    diagnostics.missing_sample_ids = requested_sample_order
        .iter()
        .copied()
        .filter(|sample| !returned.contains(sample))
        .collect();
}

fn samples_in_range(
    reader: &mut Fmp4Reader<SyncReading>,
    range: SampleRange,
) -> Result<&[SampleMeta]> {
    let samples = reader.samples(range.track_id)?;
    let start = samples
        .iter()
        .position(|sample| sample.sample_id == range.start_sample)
        .with_context(|| {
            format!(
                "range start sample {} does not belong to track {}",
                range.start_sample, range.track_id
            )
        })?;
    let end = samples
        .iter()
        .position(|sample| sample.sample_id == range.end_sample_exclusive)
        .unwrap_or(samples.len());
    anyhow::ensure!(start <= end, "invalid sample range");
    Ok(&samples[start..end])
}

fn frame_pts_90k(frame: &DecodedFrame) -> Option<Timestamp90k> {
    match frame {
        DecodedFrame::Metadata { pts_90k, .. }
        | DecodedFrame::Nv12 { pts_90k, .. }
        | DecodedFrame::Rgb24 { pts_90k, .. } => *pts_90k,
    }
}

fn sample_timestamp_to_90k(sample: &SampleMeta) -> Option<Timestamp90k> {
    let scaled_90k = u128::from(sample.pts.ticks).saturating_mul(90_000)
        / u128::from(sample.pts.timescale.get());
    i64::try_from(scaled_90k).ok().map(Timestamp90k)
}

fn estimate_fps(samples: &[SampleMeta]) -> i32 {
    samples
        .iter()
        .find_map(|sample| {
            if sample.duration == 0 {
                return None;
            }
            let timescale = u64::from(sample.dts.timescale.get());
            let duration = u64::from(sample.duration);
            let fps = (timescale + (duration / 2)) / duration;
            i32::try_from(fps.max(1)).ok()
        })
        .unwrap_or(30)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::fmp4_reader::{Fmp4ReaderConfig, MediaTime, TrackKind};
    #[cfg(all(
        feature = "serde",
        any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                any(
                    feature = "backend-nvidia",
                    feature = "backend-intel",
                    feature = "backend-vulkan"
                ),
                any(target_os = "linux", target_os = "windows")
            )
        )
    ))]
    use video_hw::Codec;

    #[test]
    fn frame_decode_request_defaults_to_auto_rgb() {
        let request = FrameDecodeRequest::new(TrackId(1), SampleId(2));
        assert_eq!(request.track_id, TrackId(1));
        assert_eq!(request.target_sample, SampleId(2));
        assert_eq!(request.backend, Backend::Auto);
        assert_eq!(request.output_mode, DecodeOutputMode::Rgb24);
        assert!(!request.require_hardware);
    }

    #[test]
    fn frame_decode_range_request_defaults_to_auto_rgb() {
        let range = SampleRange {
            track_id: TrackId(1),
            start_sample: SampleId(2),
            end_sample_exclusive: SampleId(4),
        };
        let request = FrameDecodeRangeRequest::new(range);
        assert_eq!(request.range, range);
        assert_eq!(request.backend, Backend::Auto);
        assert_eq!(request.output_mode, DecodeOutputMode::Rgb24);
        assert!(!request.require_hardware);
    }

    #[test]
    fn frame_decode_window_request_defaults_to_auto_rgb() {
        let request = FrameDecodeWindowRequest::new(TrackId(1), SampleId(2));
        assert_eq!(request.track_id, TrackId(1));
        assert_eq!(request.center_sample, SampleId(2));
        assert_eq!(request.before, 0);
        assert_eq!(request.after, 0);
        assert_eq!(request.backend, Backend::Auto);
        assert_eq!(request.output_mode, DecodeOutputMode::Rgb24);
        assert!(!request.require_hardware);
    }

    #[test]
    fn gop_cursor_starts_from_previous_keyframe() {
        let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.mp4");
        let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path))
            .into_sync_session()
            .expect("sample should open");
        let video_track = reader
            .tracks()
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .expect("video track should exist")
            .track_id;
        let samples = reader.samples(video_track).expect("video samples");
        let start = samples
            .iter()
            .find(|sample| !sample.keyframe)
            .expect("sample should contain inter frames")
            .sample_id;
        let range = SampleRange {
            track_id: video_track,
            start_sample: start,
            end_sample_exclusive: SampleId(u64::MAX),
        };
        let cursor = GopCursor::new(&mut reader, range).expect("cursor should resolve");

        assert_eq!(cursor.range, range);
        assert_eq!(
            cursor.decode_start_sample,
            reader
                .keyframe_before(start)
                .expect("previous keyframe should exist")
        );
        assert_eq!(cursor.next_sample, cursor.decode_start_sample);
    }

    #[test]
    fn sample_timestamp_uses_sample_timescale() {
        let meta = SampleMeta {
            sample_id: SampleId(0),
            track_id: TrackId(1),
            offset: 0,
            size: 0,
            dts: MediaTime::new(0, NonZeroU32::new(30_000).expect("timescale")),
            pts: MediaTime::new(15_000, NonZeroU32::new(30_000).expect("timescale")),
            duration: 1_001,
            composition_time_offset: None,
            keyframe: true,
        };
        assert_eq!(sample_timestamp_to_90k(&meta), Some(Timestamp90k(45_000)));
    }

    #[test]
    fn presentation_window_uses_pts_order_not_decode_order() {
        let timescale = NonZeroU32::new(30_000).expect("timescale");
        let samples = [
            (0, 0, 2_002),
            (1, 1_001, 6_006),
            (2, 2_002, 4_004),
            (3, 3_003, 3_003),
            (4, 4_004, 5_005),
        ]
        .into_iter()
        .map(|(sample_id, dts, pts)| SampleMeta {
            sample_id: SampleId(sample_id),
            track_id: TrackId(1),
            offset: 0,
            size: 0,
            dts: MediaTime::new(dts, timescale),
            pts: MediaTime::new(pts, timescale),
            duration: 1_001,
            composition_time_offset: None,
            keyframe: sample_id == 0,
        })
        .collect::<Vec<_>>();

        assert_eq!(
            presentation_window_sample_ids(&samples, SampleId(2), 2, 1)
                .expect("window should resolve"),
            vec![SampleId(0), SampleId(3), SampleId(2), SampleId(4)]
        );
        let wanted = [SampleId(0), SampleId(3), SampleId(2), SampleId(4)]
            .into_iter()
            .collect::<HashSet<_>>();
        assert_eq!(
            decode_span_for_sample_ids(&samples, &wanted).expect("span should resolve"),
            (0, 5)
        );
    }

    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    #[test]
    fn decode_window_returns_presentation_order_for_reordered_mp4() {
        let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("foreman_cif.mp4");
        let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path))
            .into_sync_session()
            .expect("sample should open");
        let video_track = reader
            .tracks()
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .expect("video track should exist")
            .track_id;
        let samples = reader.samples(video_track).expect("video samples");
        assert_eq!(samples[0].sample_id, SampleId(0));
        assert_eq!(samples[0].pts.ticks, 2_002);
        assert_eq!(samples[1].sample_id, SampleId(1));
        assert_eq!(samples[1].pts.ticks, 6_006);
        assert_eq!(samples[2].sample_id, SampleId(2));
        assert_eq!(samples[2].pts.ticks, 4_004);
        assert_eq!(samples[3].sample_id, SampleId(3));
        assert_eq!(samples[3].pts.ticks, 3_003);
        assert_eq!(samples[4].sample_id, SampleId(4));
        assert_eq!(samples[4].pts.ticks, 5_005);

        let mut request = FrameDecodeWindowRequest::new(video_track, SampleId(2));
        request.before = 2;
        request.after = 1;
        request.output_mode = DecodeOutputMode::Metadata;
        let mut decoder = FrameDecoder::new(&mut reader);
        let result = match decoder.decode_window(request) {
            Ok(result) => result,
            Err(err) => {
                let error = format!("{err:#}");
                if error.contains("failed to create decoder session")
                    || error.contains("auto backend selection failed")
                    || error.contains("unsupported config")
                {
                    eprintln!("skipping decode-window reorder test: {error}");
                    return;
                }
                panic!("decode_window failed unexpectedly: {error}");
            }
        };
        let returned = result
            .frames
            .iter()
            .map(|frame| frame.sample_id.expect("decoded frame should map to sample"))
            .collect::<Vec<_>>();
        assert_eq!(
            returned,
            vec![SampleId(0), SampleId(3), SampleId(2), SampleId(4)]
        );
        assert_eq!(result.diagnostics.requested_sample_count, 4);
        assert_eq!(result.diagnostics.returned_sample_count, 4);
        assert!(result.diagnostics.missing_sample_ids.is_empty());
    }

    #[cfg(all(
        feature = "serde",
        any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                any(
                    feature = "backend-nvidia",
                    feature = "backend-intel",
                    feature = "backend-vulkan"
                ),
                any(target_os = "linux", target_os = "windows")
            )
        )
    ))]
    #[test]
    fn serde_roundtrips_decode_diagnostics() {
        let resolved_backend = Backend::Auto
            .resolve_decoder(&DecoderConfig::new(Codec::H264, 30, false))
            .expect("a backend feature is enabled");
        let diagnostics = DecodeDiagnostics {
            requested_backend: Backend::Auto,
            resolved_backend,
            require_hardware: false,
            output_mode: DecodeOutputMode::Rgb24,
            fps: 30,
            fallback_used: true,
            fallback_reason: Some("primary backend failed".to_string()),
            requested_sample_count: 3,
            returned_sample_count: 2,
            missing_sample_ids: vec![SampleId(42)],
        };
        let json = serde_json::to_string(&diagnostics).expect("serialize diagnostics");
        assert!(json.contains("\"requested_backend\":\"auto\""));
        assert!(json.contains("\"output_mode\":\"rgb24\""));
        let roundtrip: DecodeDiagnostics =
            serde_json::from_str(&json).expect("deserialize diagnostics");
        assert_eq!(roundtrip.requested_backend, diagnostics.requested_backend);
        assert_eq!(roundtrip.resolved_backend, diagnostics.resolved_backend);
        assert_eq!(roundtrip.output_mode, diagnostics.output_mode);
        assert_eq!(roundtrip.fallback_used, diagnostics.fallback_used);
        assert_eq!(roundtrip.fallback_reason, diagnostics.fallback_reason);
        assert_eq!(roundtrip.missing_sample_ids, diagnostics.missing_sample_ids);
    }
}
