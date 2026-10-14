use std::{collections::HashMap, num::NonZeroU32};

use anyhow::{Result, ensure};

use super::{MediaTime, SampleId, SampleMeta, TrackId};

/// A video track's samples arranged in presentation order.
///
/// `Fmp4Reader::samples` exposes demux/decode order, which is not necessarily
/// monotonic by PTS when a stream contains reordered frames. This derived view
/// keeps decode order for keyframe lookup while exposing a stable PTS-ordered
/// sequence for seeking and playback.
#[derive(Debug, Clone)]
pub struct PresentationTimeline {
    track_id: TrackId,
    timescale: NonZeroU32,
    samples: Vec<SampleMeta>,
    presentation_index_by_id: HashMap<SampleId, usize>,
    decode_order: Vec<SampleId>,
    decode_index_by_id: HashMap<SampleId, usize>,
    default_duration_ticks: u64,
}

impl PresentationTimeline {
    /// Builds a presentation timeline from samples returned by
    /// `Fmp4Reader::samples` in demux/decode order.
    pub fn from_decode_order(samples: Vec<SampleMeta>) -> Result<Self> {
        let first = samples
            .first()
            .ok_or_else(|| anyhow::anyhow!("cannot build a timeline without samples"))?;
        let track_id = first.track_id;
        let timescale = first.pts.timescale;
        ensure!(
            samples.iter().all(|sample| sample.track_id == track_id),
            "timeline samples must belong to one track"
        );
        ensure!(
            samples.iter().all(|sample| {
                sample.pts.timescale == timescale && sample.dts.timescale == timescale
            }),
            "timeline samples must use one timescale"
        );

        let decode_order = samples
            .iter()
            .map(|sample| sample.sample_id)
            .collect::<Vec<_>>();
        let decode_index_by_id = decode_order
            .iter()
            .copied()
            .enumerate()
            .map(|(index, sample_id)| (sample_id, index))
            .collect::<HashMap<_, _>>();
        ensure!(
            decode_index_by_id.len() == decode_order.len(),
            "timeline sample ids must be unique"
        );

        let default_duration_ticks = average_non_zero_duration_ticks(&samples).unwrap_or(1);
        let mut samples = samples;
        samples.sort_by_key(|sample| (sample.pts.ticks, sample.sample_id));
        let presentation_index_by_id = samples
            .iter()
            .enumerate()
            .map(|(index, sample)| (sample.sample_id, index))
            .collect();

        Ok(Self {
            track_id,
            timescale,
            samples,
            presentation_index_by_id,
            decode_order,
            decode_index_by_id,
            default_duration_ticks,
        })
    }

    pub const fn track_id(&self) -> TrackId {
        self.track_id
    }

    pub const fn timescale(&self) -> NonZeroU32 {
        self.timescale
    }

    pub fn samples(&self) -> &[SampleMeta] {
        &self.samples
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn sample(&self, presentation_index: usize) -> Option<&SampleMeta> {
        self.samples.get(presentation_index)
    }

    pub fn presentation_index(&self, sample_id: SampleId) -> Option<usize> {
        self.presentation_index_by_id.get(&sample_id).copied()
    }

    /// Returns the last sample whose PTS is at or before `time`.
    ///
    /// The requested time is rescaled to the track timescale before lookup.
    /// Times before the first sample select the first sample.
    pub fn index_at(&self, time: MediaTime) -> Option<usize> {
        let target = time.checked_rescale(self.timescale)?;
        let after = self
            .samples
            .partition_point(|sample| sample.pts.ticks <= target.ticks);
        Some(
            after
                .saturating_sub(1)
                .min(self.samples.len().saturating_sub(1)),
        )
    }

    pub fn timestamp(&self, presentation_index: usize) -> Option<MediaTime> {
        self.sample(presentation_index).map(|sample| sample.pts)
    }

    /// Returns the display duration for a presentation-ordered sample.
    ///
    /// The next distinct PTS is authoritative. The container sample duration,
    /// then the track average, are used for the final sample or duplicate PTS.
    pub fn duration_ticks(&self, presentation_index: usize) -> Option<u64> {
        let sample = self.sample(presentation_index)?;
        if let Some(next) = self.sample(presentation_index.saturating_add(1)) {
            let delta = next.pts.ticks.saturating_sub(sample.pts.ticks);
            if delta > 0 {
                return Some(delta);
            }
        }
        Some(if sample.duration > 0 {
            u64::from(sample.duration)
        } else {
            self.default_duration_ticks
        })
    }

    pub fn end_time(&self) -> Option<MediaTime> {
        let last_index = self.samples.len().checked_sub(1)?;
        let last = self.sample(last_index)?;
        Some(MediaTime::new(
            last.pts
                .ticks
                .saturating_add(self.duration_ticks(last_index)?),
            self.timescale,
        ))
    }

    /// Returns the decode-order keyframe needed to start decoding `sample_id`.
    pub fn keyframe_before(&self, sample_id: SampleId) -> Option<SampleId> {
        let decode_index = self.decode_index_by_id.get(&sample_id).copied()?;
        self.decode_order[..=decode_index]
            .iter()
            .rev()
            .copied()
            .find(|candidate| {
                self.presentation_index(*candidate)
                    .and_then(|index| self.sample(index))
                    .is_some_and(|sample| sample.keyframe)
            })
    }
}

fn average_non_zero_duration_ticks(samples: &[SampleMeta]) -> Option<u64> {
    let (sum, count) = samples
        .iter()
        .filter_map(|sample| (sample.duration > 0).then_some(u128::from(sample.duration)))
        .fold((0_u128, 0_u128), |(sum, count), duration| {
            (sum.saturating_add(duration), count.saturating_add(1))
        });
    (count > 0)
        .then(|| sum / count)
        .and_then(|average| u64::try_from(average).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_orders_reordered_frames_by_pts_and_seeks_in_other_timescale() -> Result<()> {
        let timeline = PresentationTimeline::from_decode_order(vec![
            sample(0, 0, 2, true),
            sample(1, 1, 6, false),
            sample(2, 2, 4, false),
            sample(3, 3, 3, false),
            sample(4, 4, 5, false),
        ])?;

        assert_eq!(
            timeline
                .samples()
                .iter()
                .map(|sample| sample.sample_id)
                .collect::<Vec<_>>(),
            [
                SampleId(0),
                SampleId(3),
                SampleId(2),
                SampleId(4),
                SampleId(1)
            ]
        );
        assert_eq!(
            timeline.index_at(MediaTime::new(
                8,
                NonZeroU32::new(2_000).expect("non-zero timescale")
            )),
            Some(2)
        );
        assert_eq!(timeline.keyframe_before(SampleId(4)), Some(SampleId(0)));
        assert_eq!(timeline.duration_ticks(0), Some(1));
        assert_eq!(timeline.end_time().map(|time| time.ticks), Some(7));
        Ok(())
    }

    #[test]
    fn timeline_rejects_mixed_tracks_timescales_and_duplicate_ids() {
        let mut mixed_track = vec![sample(0, 0, 0, true), sample(1, 1, 1, false)];
        mixed_track[1].track_id = TrackId(2);
        assert!(PresentationTimeline::from_decode_order(mixed_track).is_err());

        let mut mixed_timescale = vec![sample(0, 0, 0, true), sample(1, 1, 1, false)];
        mixed_timescale[1].pts.timescale = NonZeroU32::new(90_000).expect("non-zero timescale");
        assert!(PresentationTimeline::from_decode_order(mixed_timescale).is_err());

        assert!(
            PresentationTimeline::from_decode_order(vec![
                sample(0, 0, 0, true),
                sample(0, 1, 1, false),
            ])
            .is_err()
        );
    }

    fn sample(sample_id: u64, dts: u64, pts: u64, keyframe: bool) -> SampleMeta {
        let timescale = NonZeroU32::new(1_000).expect("non-zero timescale");
        SampleMeta {
            sample_id: SampleId(sample_id),
            track_id: TrackId(1),
            offset: 0,
            size: 1,
            dts: MediaTime::new(dts, timescale),
            pts: MediaTime::new(pts, timescale),
            duration: 1,
            composition_time_offset: None,
            keyframe,
        }
    }
}
