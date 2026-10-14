//! Sample/frame submission, presentation alignment, and decode diagnostics.

use super::*;

pub(super) fn submit_sample(
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
    let payload = sample
        .to_decode_payload()
        .with_context(|| format!("failed to prepare sample {sample_id} decoder payload"))?;
    loop {
        match session.submit(BitstreamInput::AnnexBChunk {
            chunk: payload.clone(),
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

pub(super) fn push_decoded_frame(frame: DecodedFrame, collect: &mut DecodeCollectState<'_>) {
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
    collect.frames.push_back(DecodedSampleFrame {
        sample_id,
        sample_meta: sample_id
            .and_then(|sample_id| collect.sample_meta_by_id.get(&sample_id).cloned()),
        presentation_index: sample_id
            .and_then(|sample_id| collect.presentation_index_by_id.get(&sample_id).copied()),
        frame,
    });
}

pub(super) struct DecodeCollectState<'a> {
    pub(super) pts_to_sample: &'a mut HashMap<i64, SampleId>,
    pub(super) frames: &'a mut VecDeque<DecodedSampleFrame>,
    pub(super) target_sample: SampleId,
    pub(super) target_frame_index: &'a mut Option<usize>,
    pub(super) filter_sample_ids: Option<&'a HashSet<SampleId>>,
    pub(super) sample_meta_by_id: &'a HashMap<SampleId, SampleMeta>,
    pub(super) presentation_index_by_id: &'a HashMap<SampleId, usize>,
}

pub(super) fn ordered_sample_ids_by_pts(samples: &[SampleMeta]) -> Vec<SampleId> {
    let mut ordered = samples.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|sample| (sample.pts.ticks, sample.sample_id));
    ordered.into_iter().map(|sample| sample.sample_id).collect()
}

pub(super) fn sample_meta_by_id(samples: &[SampleMeta]) -> HashMap<SampleId, SampleMeta> {
    samples
        .iter()
        .map(|sample| (sample.sample_id, sample.clone()))
        .collect()
}

pub(super) fn presentation_index_by_id(sample_order: &[SampleId]) -> HashMap<SampleId, usize> {
    sample_order
        .iter()
        .enumerate()
        .map(|(index, sample_id)| (*sample_id, index))
        .collect()
}

pub(super) fn presentation_window_sample_ids(
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

pub(super) fn decode_span_for_sample_ids(
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

pub(super) fn sample_after_in_pts_order(
    samples: &[SampleMeta],
    sample_id: SampleId,
) -> Option<SampleId> {
    let ordered = ordered_sample_ids_by_pts(samples);
    let index = ordered.iter().position(|sample| *sample == sample_id)?;
    ordered.get(index.saturating_add(1)).copied()
}

pub(super) fn sample_end_pts(sample: &SampleMeta) -> MediaTime {
    MediaTime::new(
        sample.pts.ticks.saturating_add(u64::from(sample.duration)),
        sample.pts.timescale,
    )
}

pub(super) fn sort_frames_by_sample_order(
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

pub(super) fn align_frames_to_presentation_order(
    frames: &mut VecDeque<DecodedSampleFrame>,
    sample_order: &[SampleId],
) -> usize {
    let order = sample_order
        .iter()
        .enumerate()
        .map(|(index, sample_id)| (*sample_id, index))
        .collect::<HashMap<_, _>>();
    let mapped_indices = frames
        .iter()
        .map(|frame| {
            frame
                .sample_id
                .and_then(|sample_id| order.get(&sample_id).copied())
        })
        .collect::<Vec<_>>();
    let all_frames_mapped = mapped_indices.iter().all(Option::is_some);
    let mapped_sequence_is_presentation_order = mapped_indices
        .iter()
        .flatten()
        .try_fold(0_usize, |previous, index| {
            (*index >= previous).then_some(*index)
        })
        .is_some();

    if all_frames_mapped && mapped_sequence_is_presentation_order {
        return 0;
    }

    let mut ambiguous = 0;
    for (frame, sample_id) in frames.iter_mut().zip(sample_order.iter().copied()) {
        if frame.sample_id != Some(sample_id) {
            ambiguous += 1;
        }
        frame.sample_id = Some(sample_id);
    }
    ambiguous
}

pub(super) fn retain_requested_frames(
    frames: &mut VecDeque<DecodedSampleFrame>,
    wanted_sample_ids: &HashSet<SampleId>,
) {
    frames.retain(|frame| {
        frame
            .sample_id
            .is_some_and(|sample_id| wanted_sample_ids.contains(&sample_id))
    });
}

pub(super) fn attach_sample_metadata(
    frames: &mut VecDeque<DecodedSampleFrame>,
    sample_meta_by_id: &HashMap<SampleId, SampleMeta>,
    sample_order: &[SampleId],
) {
    let presentation_index_by_id = presentation_index_by_id(sample_order);
    for frame in frames {
        frame.sample_meta = frame
            .sample_id
            .and_then(|sample_id| sample_meta_by_id.get(&sample_id).cloned());
        frame.presentation_index = frame
            .sample_id
            .and_then(|sample_id| presentation_index_by_id.get(&sample_id).copied());
    }
}

pub(super) fn find_frame_index(
    frames: &VecDeque<DecodedSampleFrame>,
    target_sample: SampleId,
) -> Option<usize> {
    frames
        .iter()
        .position(|frame| frame.sample_id == Some(target_sample))
}

pub(super) fn finalize_decode_diagnostics(
    diagnostics: &mut DecodeDiagnostics,
    requested_sample_order: &[SampleId],
    decoded_frame_count: usize,
    ambiguous_sample_association_count: usize,
    returned_frame_order: ReturnedFrameOrder,
    frames: &VecDeque<DecodedSampleFrame>,
) {
    let returned = frames
        .iter()
        .filter_map(|frame| frame.sample_id)
        .collect::<HashSet<_>>();
    diagnostics.requested_sample_count = requested_sample_order.len();
    diagnostics.decoded_frame_count = decoded_frame_count;
    diagnostics.returned_sample_count = frames.len();
    diagnostics.frames_with_sample_metadata_count = frames
        .iter()
        .filter(|frame| frame.sample_meta.is_some())
        .count();
    diagnostics.dropped_or_unmatched_frame_count = decoded_frame_count.saturating_sub(frames.len());
    diagnostics.ambiguous_sample_association_count = ambiguous_sample_association_count;
    diagnostics.returned_frame_order = returned_frame_order;
    diagnostics.missing_sample_ids = requested_sample_order
        .iter()
        .copied()
        .filter(|sample| !returned.contains(sample))
        .collect();
}

pub(super) fn samples_in_range(
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

pub(super) fn sample_timestamp_to_90k(sample: &SampleMeta) -> Option<Timestamp90k> {
    let scaled_90k = u128::from(sample.pts.ticks).saturating_mul(90_000)
        / u128::from(sample.pts.timescale.get());
    i64::try_from(scaled_90k).ok().map(Timestamp90k)
}

pub(super) fn estimate_fps(samples: &[SampleMeta]) -> i32 {
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
