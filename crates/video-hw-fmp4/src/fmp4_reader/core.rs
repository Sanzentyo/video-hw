use std::fs;

use anyhow::{Context, Result};
use shiguredo_mp4::demux::{
    DemuxError, Fmp4FileDemuxer, Input, Mp4FileDemuxer, Mp4FileKind, Mp4FileKindDetector,
    RequiredInput, Sample, TrackInfo,
};

use super::config::{Fmp4ReadSample, Fmp4ReaderConfig, Fmp4ReaderStatus, Fmp4Track};

#[derive(Debug)]
enum ReaderDemuxer {
    Fragmented(Fmp4FileDemuxer),
    Mp4(Mp4FileDemuxer),
}

impl ReaderDemuxer {
    fn required_input(&self) -> Option<RequiredInput> {
        match self {
            Self::Fragmented(demuxer) => demuxer.required_input(),
            Self::Mp4(demuxer) => demuxer.required_input(),
        }
    }

    fn handle_input(&mut self, input: Input<'_>) {
        match self {
            Self::Fragmented(demuxer) => demuxer.handle_input(input),
            Self::Mp4(demuxer) => demuxer.handle_input(input),
        }
    }

    fn tracks(&mut self) -> std::result::Result<&[TrackInfo], DemuxError> {
        match self {
            Self::Fragmented(demuxer) => demuxer.tracks(),
            Self::Mp4(demuxer) => demuxer.tracks(),
        }
    }

    fn next_sample(&mut self) -> std::result::Result<Option<Sample<'_>>, DemuxError> {
        match self {
            Self::Fragmented(demuxer) => demuxer.next_sample(),
            Self::Mp4(demuxer) => demuxer.next_sample(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ReaderCore {
    bytes: Vec<u8>,
    demuxer: ReaderDemuxer,
    tracks: Vec<Fmp4Track>,
    status: Fmp4ReaderStatus,
}

impl ReaderCore {
    pub(crate) fn open(config: &Fmp4ReaderConfig) -> Result<Self> {
        let bytes = fs::read(&config.input_path)
            .with_context(|| format!("failed to read {}", config.input_path.display()))?;
        let file_kind = detect_mp4_file_kind(&bytes)?;
        let mut demuxer = match file_kind {
            Mp4FileKind::FragmentedMp4 => ReaderDemuxer::Fragmented(Fmp4FileDemuxer::new()),
            Mp4FileKind::Mp4 => ReaderDemuxer::Mp4(Mp4FileDemuxer::new()),
        };
        while let Some(required) = demuxer.required_input() {
            demuxer.handle_input(required_input_as_input(&bytes, required)?);
        }
        let tracks = demuxer
            .tracks()
            .context("failed to initialize MP4 demuxer")?
            .iter()
            .map(|track| Fmp4Track {
                track_id: track.track_id,
                kind: track.kind,
                duration: track.duration,
                timescale: track.timescale,
                sample_entry: None,
            })
            .collect();
        Ok(Self {
            bytes,
            demuxer,
            tracks,
            status: Fmp4ReaderStatus::default(),
        })
    }

    pub(crate) fn tracks(&self) -> &[Fmp4Track] {
        &self.tracks
    }

    pub(crate) fn status(&self) -> Fmp4ReaderStatus {
        self.status.clone()
    }

    pub(crate) fn next_sample(&mut self) -> Result<Option<Fmp4ReadSample>> {
        let sample = loop {
            match self.demuxer.next_sample() {
                Ok(Some(sample)) => break sample,
                Ok(None) => return Ok(None),
                Err(DemuxError::InputRequired(_)) => self.feed_required_input()?,
                Err(err) => return Err(err).context("failed to demux next sample"),
            }
        };
        let start = usize::try_from(sample.data_offset).context("sample offset exceeds usize")?;
        let end = start.saturating_add(sample.data_size);
        let data = self
            .bytes
            .get(start..end)
            .context("sample data range is outside file contents")?
            .to_vec();
        self.status.samples_read = self.status.samples_read.saturating_add(1);
        if let Some(sample_entry) = sample.sample_entry.cloned()
            && let Some(track) = self
                .tracks
                .iter_mut()
                .find(|track| track.track_id == sample.track.track_id)
            && track.sample_entry.is_none()
        {
            track.sample_entry = Some(sample_entry.clone());
        }
        Ok(Some(Fmp4ReadSample {
            track_id: sample.track.track_id,
            kind: sample.track.kind,
            sample_entry: sample.sample_entry.cloned().or_else(|| {
                self.tracks
                    .iter()
                    .find(|track| track.track_id == sample.track.track_id)
                    .and_then(|track| track.sample_entry.clone())
            }),
            keyframe: sample.keyframe,
            timestamp: sample.timestamp,
            duration: sample.duration,
            composition_time_offset: sample.composition_time_offset,
            data,
        }))
    }

    fn feed_required_input(&mut self) -> Result<()> {
        let Some(required) = self.demuxer.required_input() else {
            return Ok(());
        };
        self.demuxer
            .handle_input(required_input_as_input(&self.bytes, required)?);
        Ok(())
    }
}

fn required_input_as_input<'a>(bytes: &'a [u8], required: RequiredInput) -> Result<Input<'a>> {
    let start =
        usize::try_from(required.position).context("required input offset exceeds usize")?;
    let end = match required.size {
        Some(size) => start.saturating_add(size),
        None => bytes.len(),
    }
    .min(bytes.len());
    Ok(Input {
        position: required.position,
        data: bytes.get(start..end).unwrap_or(&[]),
    })
}

fn detect_mp4_file_kind(bytes: &[u8]) -> Result<Mp4FileKind> {
    let mut detector = Mp4FileKindDetector::new();
    while let Some(required) = detector.required_input() {
        detector.handle_input(required_input_as_input(bytes, required)?);
        if let Some(kind) = detector
            .file_kind()
            .context("failed to detect MP4 file kind")?
        {
            return Ok(kind);
        }
    }
    detector
        .file_kind()
        .context("failed to detect MP4 file kind")?
        .context("failed to detect MP4 file kind before EOF")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn open_regular_mp4_sample_from_workspace() {
        let input_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.mp4");
        let mut core = ReaderCore::open(&Fmp4ReaderConfig {
            input_path: input_path.clone(),
        })
        .unwrap_or_else(|err| {
            panic!("failed to open {}: {err:#}", input_path.display());
        });
        assert!(!core.tracks().is_empty(), "no tracks in sample-10s.mp4");
        let mut sample_count = 0_u64;
        while let Some(sample) = core.next_sample().expect("failed to read sample") {
            assert!(!sample.data.is_empty(), "sample payload must not be empty");
            sample_count = sample_count.saturating_add(1);
            if sample_count > 600 {
                break;
            }
        }
        assert!(sample_count > 0, "no samples read from sample-10s.mp4");
    }
}
