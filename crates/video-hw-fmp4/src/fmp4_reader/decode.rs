use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use video_hw::{
    AnyDecodeSession, Backend, BackendError, BackendKind, BitstreamInput, DecodeOutputMode,
    DecodedFrame, DecoderConfig, Timestamp90k,
};

use super::{
    EncodedSample, Fmp4Reader, SampleId, SampleMeta, SyncReading, TrackId, config::Fmp4Track,
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
pub struct FrameDecodeResult {
    pub target_sample: SampleId,
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
        let samples = self.reader.samples(request.track_id)?;
        let fps = request.fps.unwrap_or_else(|| estimate_fps(samples)).max(1);
        let mut config = DecoderConfig::new(codec, fps, request.require_hardware);
        config.output_mode = request.output_mode;
        let resolved_backend = request.backend.resolve_decoder(&config).with_context(|| {
            format!(
                "failed to resolve decoder backend: requested={}, codec={}, fps={}, require_hardware={}",
                request.backend, codec, fps, request.require_hardware
            )
        })?;
        let mut session = AnyDecodeSession::with_backend_kind(resolved_backend, config)
            .with_context(|| format!("failed to create decoder session with {resolved_backend}"))?;

        let mut frames = Vec::new();
        let mut target_frame_index = None;
        let mut pts_to_sample = HashMap::<i64, SampleId>::new();
        let gop_samples = self.reader.iter_gop_for_sample(request.target_sample)?;
        for sample in gop_samples {
            let sample = sample?;
            submit_sample(
                &mut session,
                &track,
                sample,
                &mut pts_to_sample,
                &mut frames,
                request.target_sample,
                &mut target_frame_index,
            )?;
        }

        for frame in session
            .flush()
            .map_err(anyhow::Error::new)
            .context("decoder flush failed")?
        {
            push_decoded_frame(
                frame,
                &pts_to_sample,
                &mut frames,
                request.target_sample,
                &mut target_frame_index,
            );
        }

        Ok(FrameDecodeResult {
            target_sample: request.target_sample,
            resolved_backend,
            output_mode: request.output_mode,
            frames,
            target_frame_index,
        })
    }
}

fn submit_sample(
    session: &mut AnyDecodeSession,
    track: &Fmp4Track,
    sample: EncodedSample,
    pts_to_sample: &mut HashMap<i64, SampleId>,
    frames: &mut Vec<DecodedSampleFrame>,
    target_sample: SampleId,
    target_frame_index: &mut Option<usize>,
) -> Result<()> {
    let sample_id = sample.meta.sample_id;
    let pts_90k = sample_timestamp_to_90k(&sample.meta);
    if let Some(pts) = pts_90k {
        pts_to_sample.insert(pts.0, sample_id);
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
            Err(BackendError::TemporaryBackpressure(_)) => drain_ready_frames(
                session,
                pts_to_sample,
                frames,
                target_sample,
                target_frame_index,
            )?,
            Err(err) => {
                return Err(anyhow!(err).context(format!(
                    "decoder submit failed at sample {sample_id} on track {}",
                    track.track_id
                )));
            }
        }
    }
    drain_ready_frames(
        session,
        pts_to_sample,
        frames,
        target_sample,
        target_frame_index,
    )
    .with_context(|| format!("failed to reap decoded frames after sample {sample_id}"))
}

fn drain_ready_frames(
    session: &mut AnyDecodeSession,
    pts_to_sample: &HashMap<i64, SampleId>,
    frames: &mut Vec<DecodedSampleFrame>,
    target_sample: SampleId,
    target_frame_index: &mut Option<usize>,
) -> Result<()> {
    loop {
        match session.try_reap() {
            Ok(Some(frame)) => {
                push_decoded_frame(
                    frame,
                    pts_to_sample,
                    frames,
                    target_sample,
                    target_frame_index,
                );
            }
            Ok(None) => return Ok(()),
            Err(err) => return Err(anyhow!(err)),
        }
    }
}

fn push_decoded_frame(
    frame: DecodedFrame,
    pts_to_sample: &HashMap<i64, SampleId>,
    frames: &mut Vec<DecodedSampleFrame>,
    target_sample: SampleId,
    target_frame_index: &mut Option<usize>,
) {
    let sample_id = frame_pts_90k(&frame).and_then(|pts| pts_to_sample.get(&pts.0).copied());
    if sample_id == Some(target_sample) && target_frame_index.is_none() {
        *target_frame_index = Some(frames.len());
    }
    frames.push(DecodedSampleFrame { sample_id, frame });
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
    use crate::fmp4_reader::MediaTime;

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
