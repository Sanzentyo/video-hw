use std::fs;

use anyhow::{Context, Result};
use shiguredo_mp4::demux::{DemuxError, Fmp4FileDemuxer, Input};

use super::config::{Fmp4ReadSample, Fmp4ReaderConfig, Fmp4ReaderStatus, Fmp4Track};

#[derive(Debug)]
pub(crate) struct ReaderCore {
    bytes: Vec<u8>,
    demuxer: Fmp4FileDemuxer,
    tracks: Vec<Fmp4Track>,
    status: Fmp4ReaderStatus,
}

impl ReaderCore {
    pub(crate) fn open(config: &Fmp4ReaderConfig) -> Result<Self> {
        let bytes = fs::read(&config.input_path)
            .with_context(|| format!("failed to read {}", config.input_path.display()))?;
        let mut demuxer = Fmp4FileDemuxer::new();
        while let Some(required) = demuxer.required_input() {
            let start = usize::try_from(required.position)
                .context("required input offset exceeds usize")?;
            let end = match required.size {
                Some(size) => start.saturating_add(size),
                None => bytes.len(),
            }
            .min(bytes.len());
            demuxer.handle_input(Input {
                position: required.position,
                data: bytes.get(start..end).unwrap_or(&[]),
            });
        }
        let tracks = demuxer
            .tracks()
            .context("failed to initialize fMP4 demuxer")?
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
        let start =
            usize::try_from(required.position).context("required input offset exceeds usize")?;
        let end = match required.size {
            Some(size) => start.saturating_add(size),
            None => self.bytes.len(),
        }
        .min(self.bytes.len());
        self.demuxer.handle_input(Input {
            position: required.position,
            data: self.bytes.get(start..end).unwrap_or(&[]),
        });
        Ok(())
    }
}
