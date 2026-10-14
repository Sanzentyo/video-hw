use std::{num::NonZeroU32, path::PathBuf};

use anyhow::{Context, Result};
use shiguredo_mp4::TrackKind;
use video_hw_fmp4::{
    Fmp4Reader, Fmp4ReaderConfig, Fmp4Track, MediaTime, PresentationTimeline, SampleMeta,
};

#[derive(Debug, Clone)]
pub(super) struct LoadedVideo {
    pub(super) input_path: PathBuf,
    pub(super) track: Fmp4Track,
    pub(super) timeline: PresentationTimeline,
    pub(super) timeline_start_ticks: u64,
    pub(super) total_duration_ticks: u64,
    pub(super) estimated_fps: i32,
}

impl LoadedVideo {
    pub(super) fn total_duration_seconds(&self) -> f64 {
        ticks_to_seconds(self.total_duration_ticks, self.track.timescale)
    }

    pub(super) fn sample_timestamp_seconds(&self, sample_index: usize) -> f64 {
        self.timeline.timestamp(sample_index).map_or(0.0, |time| {
            ticks_to_seconds(
                time.ticks.saturating_sub(self.timeline_start_ticks),
                time.timescale,
            )
        })
    }

    pub(super) fn sample_duration_ticks(&self, sample_index: usize) -> u64 {
        self.timeline.duration_ticks(sample_index).unwrap_or(1)
    }

    pub(super) fn sample_duration_seconds(&self, sample_index: usize) -> f64 {
        ticks_to_seconds(
            self.sample_duration_ticks(sample_index),
            self.track.timescale,
        )
    }

    pub(super) fn sample_index_for_seconds(&self, seconds: f64) -> usize {
        let relative_ticks = seconds_to_ticks(seconds.max(0.0), self.track.timescale);
        let target_ticks = self.timeline_start_ticks.saturating_add(relative_ticks);
        self.timeline
            .index_at(MediaTime::new(target_ticks, self.track.timescale))
            .unwrap_or(0)
    }

    pub(super) fn keyframe_start_for(&self, sample_index: usize) -> usize {
        self.sample(sample_index)
            .and_then(|sample| self.timeline.keyframe_before(sample.sample_id))
            .and_then(|sample_id| self.timeline.presentation_index(sample_id))
            .unwrap_or(0)
    }

    pub(super) fn samples(&self) -> &[SampleMeta] {
        self.timeline.samples()
    }

    pub(super) fn sample(&self, sample_index: usize) -> Option<&SampleMeta> {
        self.timeline.sample(sample_index)
    }

    pub(super) fn sample_count(&self) -> usize {
        self.timeline.len()
    }
}

pub(super) fn load_video_samples(input_path: PathBuf) -> Result<LoadedVideo> {
    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path.clone()))
        .into_sync_session()
        .with_context(|| format!("failed to open {}", input_path.display()))?;

    let track_id = reader
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .context("failed to find video track in input")?
        .track_id;
    let samples = reader
        .samples(track_id)
        .with_context(|| format!("failed to get samples for track {}", track_id))?
        .to_vec();
    let track = reader
        .tracks()
        .iter()
        .find(|track| track.track_id == track_id)
        .cloned()
        .context("video track disappeared after indexing samples")?;
    let _finished = reader.finish();

    anyhow::ensure!(
        !samples.is_empty(),
        "input has no samples for video track {}",
        track.track_id
    );

    let estimated_fps = estimate_fps(track.timescale, &samples);
    let timeline = PresentationTimeline::from_decode_order(samples)
        .context("failed to build presentation timeline")?;
    let timeline_start_ticks = timeline
        .samples()
        .first()
        .map_or(0, |sample| sample.pts.ticks);
    let timeline_end_ticks = timeline.end_time().map_or(0, |time| time.ticks);
    let presentation_duration_ticks = timeline_end_ticks.saturating_sub(timeline_start_ticks);
    let total_duration_ticks = track.duration.max(presentation_duration_ticks);
    track
        .codec()
        .context("failed to determine video codec from sample entry")?;

    Ok(LoadedVideo {
        input_path,
        track,
        timeline,
        timeline_start_ticks,
        total_duration_ticks,
        estimated_fps,
    })
}

fn estimate_fps(timescale: NonZeroU32, samples: &[SampleMeta]) -> i32 {
    let non_zero_durations: Vec<u32> = samples
        .iter()
        .map(|sample| sample.duration)
        .filter(|duration| *duration > 0)
        .collect();
    if non_zero_durations.is_empty() {
        return 30;
    }
    let average_duration = non_zero_durations
        .iter()
        .map(|duration| f64::from(*duration))
        .sum::<f64>()
        / non_zero_durations.len() as f64;
    let fps = (f64::from(timescale.get()) / average_duration).round();
    fps.clamp(1.0, 120.0) as i32
}

fn ticks_to_seconds(ticks: u64, timescale: NonZeroU32) -> f64 {
    ticks as f64 / f64::from(timescale.get())
}

fn seconds_to_ticks(seconds: f64, timescale: NonZeroU32) -> u64 {
    if seconds <= 0.0 {
        return 0;
    }
    let scaled = seconds * f64::from(timescale.get());
    if !scaled.is_finite() || scaled <= 0.0 {
        return 0;
    }
    scaled.round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository_sample_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.mp4")
    }

    #[test]
    fn repository_bframe_sample_loads_in_presentation_order() -> Result<()> {
        let input_path = repository_sample_path();
        let mut reader =
            Fmp4Reader::new(Fmp4ReaderConfig::new(input_path.clone())).into_sync_session()?;
        let video_track = reader
            .tracks()
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .context("sample video has no video track")?
            .track_id;
        let decode_order = reader.samples(video_track)?.to_vec();
        assert!(
            decode_order
                .windows(2)
                .any(|window| window[0].pts.ticks > window[1].pts.ticks),
            "fixture must exercise reordered presentation timestamps"
        );

        let loaded = load_video_samples(input_path)?;
        assert!(
            loaded
                .samples()
                .windows(2)
                .all(|window| window[0].pts.ticks <= window[1].pts.ticks)
        );
        assert!(loaded.samples()[0].pts.ticks > 0);
        assert_eq!(loaded.sample_timestamp_seconds(0), 0.0);
        assert_eq!(loaded.total_duration_ticks, loaded.track.duration);
        assert_eq!(
            loaded.sample_index_for_seconds(loaded.total_duration_seconds()),
            loaded.sample_count() - 1
        );
        let middle = loaded.sample_count() / 2;
        let middle_time = loaded.sample_timestamp_seconds(middle);
        assert_eq!(loaded.sample_index_for_seconds(middle_time), middle);
        Ok(())
    }
}
