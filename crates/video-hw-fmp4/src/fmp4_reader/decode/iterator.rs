//! Incremental decoded-frame iteration.

use super::*;

pub struct DecodedFrameIter<'a> {
    pub(super) range: SampleRange,
    pub(super) diagnostics: DecodeDiagnostics,
    pub(super) track: Fmp4Track,
    pub(super) session: AnyDecodeSession,
    pub(super) decode_samples: EncodedSampleIter<'a>,
    pub(super) wanted_sample_ids: HashSet<SampleId>,
    pub(super) sample_meta_by_id: HashMap<SampleId, SampleMeta>,
    pub(super) presentation_index_by_id: HashMap<SampleId, usize>,
    pub(super) pts_to_sample: HashMap<i64, SampleId>,
    pub(super) buffered_frames: VecDeque<DecodedSampleFrame>,
    pub(super) target_frame_index: Option<usize>,
    pub(super) finished: bool,
}

impl DecodedFrameIter<'_> {
    pub fn diagnostics(&self) -> &DecodeDiagnostics {
        &self.diagnostics
    }

    pub fn range(&self) -> SampleRange {
        self.range
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
                        sample_meta_by_id: &self.sample_meta_by_id,
                        presentation_index_by_id: &self.presentation_index_by_id,
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
                                    sample_meta_by_id: &self.sample_meta_by_id,
                                    presentation_index_by_id: &self.presentation_index_by_id,
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
