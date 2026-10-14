use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use anyhow::{Context, Result, anyhow};
use shiguredo_mp4::{
    boxes::SampleEntry,
    demux::{
        DemuxError, Fmp4FileDemuxer, Input, Mp4FileDemuxer, Mp4FileKind, Mp4FileKindDetector,
        RequiredInput, Sample, TrackInfo,
    },
};

use super::config::{
    EncodedSample, Fmp4ReaderConfig, Fmp4ReaderStatus, Fmp4Track, Fmp4TrackDescription, GopSegment,
    IndexMode, MediaTime, Mp4IndexSnapshot, RangeCacheConfig, RangeCacheStats, SampleId,
    SampleLookup, SampleLookupMatch, SampleMeta, TrackId,
};

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
struct RangeCache {
    config: RangeCacheConfig,
    chunks: HashMap<u64, Vec<u8>>,
    lru: VecDeque<u64>,
    resident_bytes: usize,
    stats: RangeCacheStats,
}

impl RangeCache {
    fn new(config: RangeCacheConfig) -> Self {
        let mut config = config;
        config.chunk_size = config.chunk_size.max(1);
        config.max_bytes = config.max_bytes.max(config.chunk_size);
        Self {
            config,
            chunks: HashMap::new(),
            lru: VecDeque::new(),
            resident_bytes: 0,
            stats: RangeCacheStats::default(),
        }
    }

    fn read_range(
        &mut self,
        file: &mut File,
        file_len: u64,
        offset: u64,
        size: usize,
    ) -> Result<Vec<u8>> {
        if size == 0 {
            return Ok(Vec::new());
        }
        let end = offset
            .checked_add(u64::try_from(size).context("range size exceeds u64")?)
            .context("range end overflows u64")?;
        anyhow::ensure!(
            end <= file_len,
            "requested range {}..{} is outside file length {}",
            offset,
            end,
            file_len
        );

        let chunk_size = self.config.chunk_size as u64;
        let first_chunk = offset / chunk_size;
        let last_chunk = (end.saturating_sub(1)) / chunk_size;
        let mut output = Vec::with_capacity(size);
        for chunk_index in first_chunk..=last_chunk {
            self.ensure_chunk(file, file_len, chunk_index)?;
            self.touch(chunk_index);
            let chunk = self.chunks.get(&chunk_index).expect("chunk should exist");
            let chunk_start = chunk_index * chunk_size;
            let slice_start = offset.saturating_sub(chunk_start) as usize;
            let slice_end = (end.saturating_sub(chunk_start) as usize).min(chunk.len());
            if slice_start < slice_end {
                output.extend_from_slice(&chunk[slice_start..slice_end]);
            }
        }

        for read_ahead in 1..=self.config.read_ahead_chunks {
            let Some(chunk_index) = last_chunk.checked_add(read_ahead as u64) else {
                break;
            };
            let Some(chunk_offset) = chunk_index.checked_mul(chunk_size) else {
                break;
            };
            if chunk_offset >= file_len {
                break;
            }
            self.ensure_chunk(file, file_len, chunk_index)?;
        }

        Ok(output)
    }

    fn ensure_chunk(&mut self, file: &mut File, file_len: u64, chunk_index: u64) -> Result<()> {
        if self.chunks.contains_key(&chunk_index) {
            self.stats.hits = self.stats.hits.saturating_add(1);
            return Ok(());
        }

        self.stats.misses = self.stats.misses.saturating_add(1);
        let chunk_size = self.config.chunk_size as u64;
        let offset = chunk_index
            .checked_mul(chunk_size)
            .context("chunk offset overflows u64")?;
        let remaining = file_len.saturating_sub(offset);
        let read_size = remaining.min(chunk_size) as usize;
        let mut data = vec![0_u8; read_size];
        file.seek(SeekFrom::Start(offset))
            .with_context(|| format!("failed to seek to byte {offset}"))?;
        file.read_exact(&mut data)
            .with_context(|| format!("failed to read chunk {chunk_index}"))?;

        self.resident_bytes = self.resident_bytes.saturating_add(data.len());
        self.chunks.insert(chunk_index, data);
        self.touch(chunk_index);
        self.evict_to_budget();
        Ok(())
    }

    fn touch(&mut self, chunk_index: u64) {
        self.lru.retain(|existing| *existing != chunk_index);
        self.lru.push_back(chunk_index);
    }

    fn evict_to_budget(&mut self) {
        while self.resident_bytes > self.config.max_bytes {
            let Some(chunk_index) = self.lru.pop_front() else {
                break;
            };
            if let Some(chunk) = self.chunks.remove(&chunk_index) {
                self.resident_bytes = self.resident_bytes.saturating_sub(chunk.len());
                self.stats.evictions = self.stats.evictions.saturating_add(1);
            }
        }
    }

    fn clear(&mut self) {
        self.chunks.clear();
        self.lru.clear();
        self.resident_bytes = 0;
    }
}

#[derive(Debug)]
struct SampleStore {
    file: File,
    file_len: u64,
    cache: RangeCache,
}

impl SampleStore {
    fn open(config: &Fmp4ReaderConfig) -> Result<Self> {
        let file = File::open(&config.input_path)
            .with_context(|| format!("failed to open {}", config.input_path.display()))?;
        let file_len = file
            .metadata()
            .with_context(|| format!("failed to stat {}", config.input_path.display()))?
            .len();
        Ok(Self {
            file,
            file_len,
            cache: RangeCache::new(config.range_cache.clone()),
        })
    }

    fn read_range(&mut self, offset: u64, size: usize) -> Result<Vec<u8>> {
        self.cache
            .read_range(&mut self.file, self.file_len, offset, size)
    }

    fn cache_stats(&self) -> RangeCacheStats {
        RangeCacheStats {
            hits: self.cache.stats.hits,
            misses: self.cache.stats.misses,
            evictions: self.cache.stats.evictions,
            resident_bytes: self.cache.resident_bytes,
        }
    }

    fn cache_config(&self) -> RangeCacheConfig {
        self.cache.config.clone()
    }

    fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

#[derive(Debug)]
pub(crate) struct ReaderCore {
    store: SampleStore,
    demuxer: ReaderDemuxer,
    tracks: Vec<Fmp4Track>,
    samples_by_track: HashMap<TrackId, Vec<SampleMeta>>,
    sample_pts_order: HashMap<TrackId, Vec<SampleId>>,
    sample_lookup: HashMap<SampleId, (TrackId, usize)>,
    sample_entries: HashMap<SampleId, SampleEntry>,
    current_entries: HashMap<TrackId, SampleEntry>,
    sample_order: Vec<SampleId>,
    next_sample_id: u64,
    next_sample_index: usize,
    index_complete: bool,
    status: Fmp4ReaderStatus,
}

impl ReaderCore {
    pub(crate) fn open(config: &Fmp4ReaderConfig) -> Result<Self> {
        let mut store = SampleStore::open(config)?;
        let file_kind = detect_mp4_file_kind(&mut store)?;
        let mut demuxer = match file_kind {
            Mp4FileKind::FragmentedMp4 => ReaderDemuxer::Fragmented(Fmp4FileDemuxer::new()),
            Mp4FileKind::Mp4 => ReaderDemuxer::Mp4(Mp4FileDemuxer::new()),
        };
        feed_required_inputs(&mut store, &mut demuxer)?;
        let tracks = demuxer
            .tracks()
            .context("failed to initialize MP4 demuxer")?
            .iter()
            .map(|track| Fmp4Track {
                track_id: TrackId(track.track_id),
                kind: track.kind,
                duration: track.duration,
                timescale: track.timescale,
                sample_entry: None,
            })
            .collect::<Vec<_>>();

        let mut core = Self {
            store,
            demuxer,
            tracks,
            samples_by_track: HashMap::new(),
            sample_pts_order: HashMap::new(),
            sample_lookup: HashMap::new(),
            sample_entries: HashMap::new(),
            current_entries: HashMap::new(),
            sample_order: Vec::new(),
            next_sample_id: 0,
            next_sample_index: 0,
            index_complete: false,
            status: Fmp4ReaderStatus::default(),
        };
        if matches!(config.index_mode, IndexMode::Eager) {
            core.index_to_end()?;
        }
        let mut status = core.status.clone();
        apply_cache_status(&mut status, &core.store);
        core.status = status;
        Ok(core)
    }

    pub(crate) fn tracks(&self) -> &[Fmp4Track] {
        &self.tracks
    }

    pub(crate) fn status(&self) -> Fmp4ReaderStatus {
        let mut status = self.status.clone();
        apply_cache_status(&mut status, &self.store);
        status
    }

    pub(crate) fn cache_stats(&self) -> RangeCacheStats {
        self.store.cache_stats()
    }

    pub(crate) fn clear_cache(&mut self) {
        self.store.clear_cache();
        apply_cache_status(&mut self.status, &self.store);
    }

    pub(crate) fn samples(&mut self, track: TrackId) -> Result<&[SampleMeta]> {
        self.index_to_end()?;
        self.samples_by_track
            .get(&track)
            .map(Vec::as_slice)
            .ok_or_else(|| anyhow!("unknown track id {}", track.0))
    }

    pub(crate) fn sample_meta(&mut self, sample: SampleId) -> Result<Option<&SampleMeta>> {
        if !self.index_until_sample(sample)? {
            return Ok(None);
        }
        let sample_meta = self
            .sample_lookup
            .get(&sample)
            .and_then(|(track_id, index)| self.samples_by_track.get(track_id)?.get(*index));
        Ok(sample_meta)
    }

    pub(crate) fn iter_samples(
        &mut self,
        track: TrackId,
    ) -> Result<std::slice::Iter<'_, SampleMeta>> {
        Ok(self.samples(track)?.iter())
    }

    pub(crate) fn sample_at_pts(
        &mut self,
        track: TrackId,
        pts: MediaTime,
    ) -> Result<Option<SampleId>> {
        Ok(self
            .sample_at_pts_with_delta(track, pts)?
            .map(|lookup| lookup.matched_sample))
    }

    pub(crate) fn sample_at_pts_with_delta(
        &mut self,
        track: TrackId,
        pts: MediaTime,
    ) -> Result<Option<SampleLookup>> {
        self.index_to_end()?;
        let Some(track_timescale) = self
            .tracks
            .iter()
            .find(|candidate| candidate.track_id == track)
            .map(|track| track.timescale)
        else {
            return Ok(None);
        };
        let requested_pts = pts.checked_rescale(track_timescale).with_context(|| {
            format!(
                "requested PTS {}@{} cannot be represented at track timescale {}",
                pts.ticks, pts.timescale, track_timescale
            )
        })?;
        let Some(ordered) = self.sample_pts_order.get(&track) else {
            return Ok(None);
        };
        if ordered.is_empty() {
            return Ok(None);
        }
        let (matched_sample, match_type) = match ordered
            .binary_search_by_key(&requested_pts.ticks, |sample_id| {
                self.indexed_sample_pts(*sample_id).unwrap_or(0)
            }) {
            Ok(index) => (ordered[index], SampleLookupMatch::Exact),
            Err(0) => (ordered[0], SampleLookupMatch::FirstAfter),
            Err(index) => (
                ordered[index.saturating_sub(1)],
                SampleLookupMatch::Previous,
            ),
        };
        let matched_meta = self
            .indexed_sample(matched_sample)
            .context("sample PTS index references missing metadata")?;
        let delta_ticks = i128::from(matched_meta.pts.ticks) - i128::from(requested_pts.ticks);
        Ok(Some(SampleLookup {
            requested_pts,
            matched_sample,
            matched_pts: matched_meta.pts,
            delta_ticks,
            delta_seconds: delta_ticks as f64 / f64::from(track_timescale.get()),
            match_type,
        }))
    }

    pub(crate) fn keyframe_before(&mut self, sample: SampleId) -> Result<Option<SampleId>> {
        if !self.index_until_sample(sample)? {
            return Ok(None);
        }
        let Some((track_id, index)) = self.sample_lookup.get(&sample).copied() else {
            return Ok(None);
        };
        Ok(self
            .samples_by_track
            .get(&track_id)
            .and_then(|samples| samples.get(..=index))
            .and_then(|samples| {
                samples
                    .iter()
                    .rev()
                    .find_map(|meta| meta.keyframe.then_some(meta.sample_id))
            }))
    }

    pub(crate) fn gop_for_sample(&mut self, sample: SampleId) -> Result<Option<GopSegment>> {
        if !self.index_until_sample(sample)? {
            return Ok(None);
        }
        let Some((track_id, index)) = self.sample_lookup.get(&sample).copied() else {
            return Ok(None);
        };
        self.ensure_track_sample_after(track_id, index)?;
        let Some(samples) = self.samples_by_track.get(&track_id) else {
            return Ok(None);
        };
        let Some(keyframe_sample) = samples.get(..=index).and_then(|samples| {
            samples
                .iter()
                .rev()
                .find_map(|meta| meta.keyframe.then_some(meta.sample_id))
        }) else {
            return Ok(None);
        };
        let Some(keyframe_index) = self
            .sample_lookup
            .get(&keyframe_sample)
            .map(|(_, index)| *index)
        else {
            return Ok(None);
        };
        let Some(keyframe_meta) = samples.get(keyframe_index) else {
            return Ok(None);
        };
        let Some(target) = samples.get(index) else {
            return Ok(None);
        };
        let end_sample_exclusive = samples
            .get(index.saturating_add(1))
            .map_or(SampleId(u64::MAX), |meta| meta.sample_id);
        let end_pts = MediaTime::new(
            target.pts.ticks.saturating_add(u64::from(target.duration)),
            target.pts.timescale,
        );
        Ok(Some(GopSegment {
            track_id,
            keyframe_sample,
            end_sample_exclusive,
            start_pts: keyframe_meta.pts,
            end_pts,
        }))
    }

    pub(crate) fn read_sample(&mut self, sample: SampleId) -> Result<EncodedSample> {
        self.ensure_sample_indexed(sample)?;
        let meta = self
            .sample_meta(sample)?
            .cloned()
            .with_context(|| format!("unknown sample id {}", sample.0))?;
        let size = usize::try_from(meta.size).context("sample size exceeds usize")?;
        let data = self.store.read_range(meta.offset, size)?;
        self.status.samples_read = self.status.samples_read.saturating_add(1);
        self.status.bytes_read = self.status.bytes_read.saturating_add(u64::from(meta.size));
        let track_stats = self.status.track_reads.entry(meta.track_id).or_default();
        track_stats.samples_read = track_stats.samples_read.saturating_add(1);
        track_stats.bytes_read = track_stats.bytes_read.saturating_add(u64::from(meta.size));
        let sample_stats = self.status.sample_reads.entry(sample).or_default();
        sample_stats.reads = sample_stats.reads.saturating_add(1);
        sample_stats.bytes_read = sample_stats.bytes_read.saturating_add(u64::from(meta.size));
        apply_cache_status(&mut self.status, &self.store);
        let kind = self
            .tracks
            .iter()
            .find(|track| track.track_id == meta.track_id)
            .map(|track| track.kind)
            .context("sample track disappeared from reader")?;
        Ok(EncodedSample {
            sample_entry: self.sample_entries.get(&sample).cloned(),
            meta,
            kind,
            data,
        })
    }

    pub(crate) fn next_sample(&mut self) -> Result<Option<EncodedSample>> {
        if self.next_sample_index >= self.sample_order.len() && !self.index_complete {
            self.index_next_sample()?;
        }
        let Some(sample_id) = self.sample_order.get(self.next_sample_index).copied() else {
            return Ok(None);
        };
        let sample = self.read_sample(sample_id)?;
        self.next_sample_index = self.next_sample_index.saturating_add(1);
        Ok(Some(sample))
    }

    pub(crate) fn read_gop(&mut self, sample: SampleId) -> Result<Vec<EncodedSample>> {
        self.iter_gop_for_sample(sample)?.collect()
    }

    pub(crate) fn read_segment(&mut self, segment: GopSegment) -> Result<Vec<EncodedSample>> {
        self.encoded_iter(segment)?.collect()
    }

    pub(crate) fn iter_gop_for_sample(
        &mut self,
        sample: SampleId,
    ) -> Result<EncodedSampleIter<'_>> {
        let segment = self
            .gop_for_sample(sample)?
            .with_context(|| format!("unknown sample id {}", sample.0))?;
        self.encoded_iter(segment)
    }

    pub(crate) fn encoded_iter(&mut self, segment: GopSegment) -> Result<EncodedSampleIter<'_>> {
        self.index_to_end()?;
        let samples = self.samples(segment.track_id)?;
        let start = samples
            .iter()
            .position(|meta| meta.sample_id == segment.keyframe_sample)
            .context("GOP keyframe sample does not belong to track")?;
        let end = samples
            .iter()
            .position(|meta| meta.sample_id == segment.end_sample_exclusive)
            .unwrap_or(samples.len());
        anyhow::ensure!(start <= end, "invalid GOP segment sample range");
        let sample_ids = samples[start..end]
            .iter()
            .map(|meta| meta.sample_id)
            .collect();
        Ok(EncodedSampleIter {
            core: self,
            sample_ids,
            next_index: 0,
        })
    }

    pub(crate) fn index_snapshot(&mut self) -> Result<Mp4IndexSnapshot> {
        self.index_to_end()?;
        let samples = self
            .sample_order
            .iter()
            .filter_map(|sample_id| self.indexed_sample(*sample_id).cloned())
            .collect();
        Ok(Mp4IndexSnapshot {
            track_descriptions: self.tracks.iter().map(Fmp4TrackDescription::from).collect(),
            tracks: self.tracks.clone(),
            samples,
        })
    }

    fn index_to_end(&mut self) -> Result<()> {
        while !self.index_complete {
            if self.index_next_sample()?.is_none() {
                break;
            }
        }
        Ok(())
    }

    fn ensure_sample_indexed(&mut self, sample: SampleId) -> Result<()> {
        self.index_until_sample(sample)?;
        anyhow::ensure!(
            self.sample_lookup.contains_key(&sample),
            "unknown sample id {}",
            sample.0
        );
        Ok(())
    }

    fn index_until_sample(&mut self, sample: SampleId) -> Result<bool> {
        while !self.sample_lookup.contains_key(&sample) && !self.index_complete {
            if self.index_next_sample()?.is_none() {
                break;
            }
        }
        Ok(self.sample_lookup.contains_key(&sample))
    }

    fn ensure_track_sample_after(&mut self, track: TrackId, index: usize) -> Result<()> {
        while self
            .samples_by_track
            .get(&track)
            .is_none_or(|samples| samples.len() <= index.saturating_add(1))
            && !self.index_complete
        {
            if self.index_next_sample()?.is_none() {
                break;
            }
        }
        Ok(())
    }

    fn index_next_sample(&mut self) -> Result<Option<SampleId>> {
        if self.index_complete {
            return Ok(None);
        }
        let Some(sample) = read_next_demux_sample(&mut self.store, &mut self.demuxer)? else {
            self.index_complete = true;
            self.rebuild_sample_pts_order();
            apply_cache_status(&mut self.status, &self.store);
            return Ok(None);
        };
        let sample_id = SampleId(self.next_sample_id);
        self.next_sample_id = self.next_sample_id.saturating_add(1);
        let track_id = TrackId(sample.track.track_id);
        if let Some(sample_entry) = sample.sample_entry.clone() {
            self.current_entries.insert(track_id, sample_entry.clone());
            if let Some(track) = self
                .tracks
                .iter_mut()
                .find(|track| track.track_id == track_id)
                && track.sample_entry.is_none()
            {
                track.sample_entry = Some(sample_entry);
            }
        }
        if let Some(sample_entry) = self.current_entries.get(&track_id).cloned() {
            self.sample_entries.insert(sample_id, sample_entry);
        }
        let meta = sample_to_meta(sample_id, sample)?;
        let pts_ticks = meta.pts.ticks;
        let track_samples = self.samples_by_track.entry(track_id).or_default();
        let index = track_samples.len();
        track_samples.push(meta);
        self.sample_lookup.insert(sample_id, (track_id, index));
        self.sample_order.push(sample_id);
        let insert_index = self
            .sample_pts_order
            .get(&track_id)
            .map(|ordered| {
                ordered.partition_point(|existing| {
                    self.indexed_sample_pts(*existing)
                        .is_some_and(|existing_pts| existing_pts <= pts_ticks)
                })
            })
            .unwrap_or(0);
        self.sample_pts_order
            .entry(track_id)
            .or_default()
            .insert(insert_index, sample_id);
        self.status.samples_indexed = self.sample_order.len() as u64;
        apply_cache_status(&mut self.status, &self.store);
        Ok(Some(sample_id))
    }

    fn indexed_sample_pts(&self, sample: SampleId) -> Option<u64> {
        self.indexed_sample(sample).map(|sample| sample.pts.ticks)
    }

    fn indexed_sample(&self, sample: SampleId) -> Option<&SampleMeta> {
        let (track_id, index) = self.sample_lookup.get(&sample).copied()?;
        self.samples_by_track.get(&track_id)?.get(index)
    }

    fn rebuild_sample_pts_order(&mut self) {
        self.sample_pts_order = self
            .samples_by_track
            .iter()
            .map(|(track_id, samples)| {
                let mut ordered = samples
                    .iter()
                    .map(|sample| sample.sample_id)
                    .collect::<Vec<_>>();
                ordered.sort_by_key(|sample_id| {
                    self.sample_lookup
                        .get(sample_id)
                        .and_then(|(track_id, index)| {
                            self.samples_by_track.get(track_id)?.get(*index)
                        })
                        .map_or(0, |sample| sample.pts.ticks)
                });
                (*track_id, ordered)
            })
            .collect();
    }
}

pub struct EncodedSampleIter<'a> {
    core: &'a mut ReaderCore,
    sample_ids: Vec<SampleId>,
    next_index: usize,
}

impl Iterator for EncodedSampleIter<'_> {
    type Item = Result<EncodedSample>;

    fn next(&mut self) -> Option<Self::Item> {
        let sample_id = self.sample_ids.get(self.next_index).copied()?;
        self.next_index = self.next_index.saturating_add(1);
        Some(self.core.read_sample(sample_id))
    }
}

fn sample_to_meta(sample_id: SampleId, sample: OwnedSample) -> Result<SampleMeta> {
    let track_id = TrackId(sample.track.track_id);
    let timescale = sample.track.timescale;
    let dts = MediaTime::new(sample.timestamp, timescale);
    let pts_ticks =
        i128::from(sample.timestamp) + i128::from(sample.composition_time_offset.unwrap_or(0));
    let pts_ticks = u64::try_from(pts_ticks).context("sample PTS is outside u64 range")?;
    let size = u32::try_from(sample.data_size).context("sample size exceeds u32")?;
    Ok(SampleMeta {
        sample_id,
        track_id,
        offset: sample.data_offset,
        size,
        dts,
        pts: MediaTime::new(pts_ticks, timescale),
        duration: sample.duration,
        composition_time_offset: sample.composition_time_offset,
        keyframe: sample.keyframe,
    })
}

fn read_next_demux_sample(
    store: &mut SampleStore,
    demuxer: &mut ReaderDemuxer,
) -> Result<Option<OwnedSample>> {
    loop {
        match demuxer.next_sample() {
            Ok(Some(sample)) => {
                return Ok(Some(OwnedSample::from_sample(sample)));
            }
            Ok(None) => return Ok(None),
            Err(DemuxError::InputRequired(_)) => feed_required_inputs(store, demuxer)?,
            Err(err) => return Err(err).context("failed to demux next sample"),
        }
    }
}

#[derive(Debug)]
struct OwnedSample {
    track: TrackInfo,
    sample_entry: Option<SampleEntry>,
    keyframe: bool,
    timestamp: u64,
    duration: u32,
    data_offset: u64,
    data_size: usize,
    composition_time_offset: Option<i64>,
}

impl OwnedSample {
    fn from_sample(sample: Sample<'_>) -> Self {
        Self {
            track: sample.track.clone(),
            sample_entry: sample.sample_entry.cloned(),
            keyframe: sample.keyframe,
            timestamp: sample.timestamp,
            duration: sample.duration,
            data_offset: sample.data_offset,
            data_size: sample.data_size,
            composition_time_offset: sample.composition_time_offset,
        }
    }
}

fn feed_required_inputs(store: &mut SampleStore, demuxer: &mut ReaderDemuxer) -> Result<()> {
    while let Some(required) = demuxer.required_input() {
        let data = read_required_input(store, required)?;
        demuxer.handle_input(required.to_input(&data));
    }
    Ok(())
}

fn read_required_input(store: &mut SampleStore, required: RequiredInput) -> Result<Vec<u8>> {
    let remaining = store.file_len.saturating_sub(required.position);
    let size = match required.size {
        Some(size) => {
            let requested = u64::try_from(size).context("required input size exceeds u64")?;
            usize::try_from(requested.min(remaining))
                .context("required input size exceeds addressable memory")?
        }
        None => {
            usize::try_from(remaining).context("remaining MP4 input exceeds addressable memory")?
        }
    };
    store.read_range(required.position, size)
}

fn detect_mp4_file_kind(store: &mut SampleStore) -> Result<Mp4FileKind> {
    let mut detector = Mp4FileKindDetector::new();
    while let Some(required) = detector.required_input() {
        let data = read_required_input(store, required)?;
        detector.handle_input(required.to_input(&data));
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

fn apply_cache_status(status: &mut Fmp4ReaderStatus, store: &SampleStore) {
    let cache = store.cache_stats();
    status.cache_hits = cache.hits;
    status.cache_misses = cache.misses;
    status.cache_evictions = cache.evictions;
    status.cache_resident_bytes = cache.resident_bytes;
    status.range_cache_config = store.cache_config();
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
