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
pub struct DecodeDiagnostics {
    pub requested_backend: Backend,
    pub resolved_backend: BackendKind,
    pub require_hardware: bool,
    pub output_mode: DecodeOutputMode,
    pub fps: i32,
    pub fallback_used: bool,
    pub fallback_reason: Option<String>,
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
        let (mut session, diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let resolved_backend = diagnostics.resolved_backend;
        let mut frames = VecDeque::new();
        let mut target_frame_index = None;
        let mut pts_to_sample = HashMap::<i64, SampleId>::new();
        let gop_samples = self.reader.iter_gop_for_sample(request.target_sample)?;
        for sample in gop_samples {
            let sample = sample?;
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
        let (mut session, diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let resolved_backend = diagnostics.resolved_backend;
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

        let decode_samples = self.reader.iter_encoded(decode_segment)?;
        for sample in decode_samples {
            let sample = sample?;
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.range.start_sample,
                target_frame_index: &mut ignored_target_frame_index,
                filter_sample_ids: Some(&wanted_sample_ids),
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
                filter_sample_ids: Some(&wanted_sample_ids),
            };
            push_decoded_frame(frame, &mut collect);
        }
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
        let range = {
            let samples = self.reader.samples(request.track_id)?;
            let center_index = samples
                .iter()
                .position(|sample| sample.sample_id == request.center_sample)
                .with_context(|| {
                    format!(
                        "center sample {} does not belong to track {}",
                        request.center_sample, request.track_id
                    )
                })?;
            let before =
                usize::try_from(request.before).context("window before overflows usize")?;
            let after = usize::try_from(request.after).context("window after overflows usize")?;
            let start_index = center_index.saturating_sub(before);
            let end_index = center_index
                .saturating_add(after)
                .saturating_add(1)
                .min(samples.len());
            SampleRange {
                track_id: request.track_id,
                start_sample: samples[start_index].sample_id,
                end_sample_exclusive: samples
                    .get(end_index)
                    .map_or(SampleId(u64::MAX), |sample| sample.sample_id),
            }
        };
        self.decode_range(FrameDecodeRangeRequest {
            range,
            backend: request.backend,
            require_hardware: request.require_hardware,
            output_mode: request.output_mode,
            fps: request.fps,
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
}
