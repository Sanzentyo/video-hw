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
    EncodedSample, Fmp4ReaderConfig, Fmp4ReaderStatus, Fmp4Track, GopSegment, IndexMode, MediaTime,
    RangeCacheConfig, SampleId, SampleMeta, TrackId,
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

#[derive(Debug, Clone, Copy, Default)]
struct RangeCacheStats {
    hits: u64,
    misses: u64,
    evictions: u64,
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
            if chunk_index * chunk_size >= file_len {
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

    fn status_fields(&self) -> (u64, u64, u64, usize) {
        (
            self.cache.stats.hits,
            self.cache.stats.misses,
            self.cache.stats.evictions,
            self.cache.resident_bytes,
        )
    }
}

#[derive(Debug)]
pub(crate) struct ReaderCore {
    store: SampleStore,
    tracks: Vec<Fmp4Track>,
    samples_by_track: HashMap<TrackId, Vec<SampleMeta>>,
    sample_pts_order: HashMap<TrackId, Vec<SampleId>>,
    sample_lookup: HashMap<SampleId, (TrackId, usize)>,
    sample_entries: HashMap<SampleId, SampleEntry>,
    sample_order: Vec<SampleId>,
    next_sample_index: usize,
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
        let mut tracks = demuxer
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

        let mut current_entries = HashMap::<TrackId, SampleEntry>::new();
        let mut samples_by_track = HashMap::<TrackId, Vec<SampleMeta>>::new();
        let mut sample_lookup = HashMap::<SampleId, (TrackId, usize)>::new();
        let mut sample_entries = HashMap::<SampleId, SampleEntry>::new();
        let mut sample_order = Vec::new();
        let mut next_sample_id = 0_u64;

        match config.index_mode {
            IndexMode::Eager | IndexMode::Lazy => {
                while let Some(sample) = read_next_demux_sample(&mut store, &mut demuxer)? {
                    let sample_id = SampleId(next_sample_id);
                    next_sample_id = next_sample_id.saturating_add(1);
                    let track_id = TrackId(sample.track.track_id);
                    if let Some(sample_entry) = sample.sample_entry.clone() {
                        current_entries.insert(track_id, sample_entry.clone());
                        if let Some(track) =
                            tracks.iter_mut().find(|track| track.track_id == track_id)
                            && track.sample_entry.is_none()
                        {
                            track.sample_entry = Some(sample_entry);
                        }
                    }
                    if let Some(sample_entry) = current_entries.get(&track_id).cloned() {
                        sample_entries.insert(sample_id, sample_entry);
                    }
                    let meta = sample_to_meta(sample_id, sample)?;
                    let track_samples = samples_by_track.entry(track_id).or_default();
                    let index = track_samples.len();
                    track_samples.push(meta);
                    sample_lookup.insert(sample_id, (track_id, index));
                    sample_order.push(sample_id);
                }
            }
        }

        let sample_pts_order = samples_by_track
            .iter()
            .map(|(track_id, samples)| {
                let mut ordered = samples
                    .iter()
                    .map(|sample| sample.sample_id)
                    .collect::<Vec<_>>();
                ordered.sort_by_key(|sample_id| {
                    sample_lookup
                        .get(sample_id)
                        .and_then(|(track_id, index)| samples_by_track.get(track_id)?.get(*index))
                        .map_or(0, |sample| sample.pts.ticks)
                });
                (*track_id, ordered)
            })
            .collect::<HashMap<_, _>>();

        let mut status = Fmp4ReaderStatus {
            samples_indexed: sample_order.len() as u64,
            ..Fmp4ReaderStatus::default()
        };
        apply_cache_status(&mut status, &store);
        Ok(Self {
            store,
            tracks,
            samples_by_track,
            sample_pts_order,
            sample_lookup,
            sample_entries,
            sample_order,
            next_sample_index: 0,
            status,
        })
    }

    pub(crate) fn tracks(&self) -> &[Fmp4Track] {
        &self.tracks
    }

    pub(crate) fn status(&self) -> Fmp4ReaderStatus {
        let mut status = self.status.clone();
        apply_cache_status(&mut status, &self.store);
        status
    }

    pub(crate) fn samples(&self, track: TrackId) -> Result<&[SampleMeta]> {
        self.samples_by_track
            .get(&track)
            .map(Vec::as_slice)
            .ok_or_else(|| anyhow!("unknown track id {}", track.0))
    }

    pub(crate) fn sample_meta(&self, sample: SampleId) -> Option<&SampleMeta> {
        let (track_id, index) = self.sample_lookup.get(&sample).copied()?;
        self.samples_by_track.get(&track_id)?.get(index)
    }

    pub(crate) fn iter_samples(&self, track: TrackId) -> Result<std::slice::Iter<'_, SampleMeta>> {
        Ok(self.samples(track)?.iter())
    }

    pub(crate) fn sample_at_pts(&self, track: TrackId, pts: MediaTime) -> Option<SampleId> {
        let ordered = self.sample_pts_order.get(&track)?;
        if ordered.is_empty() {
            return None;
        }
        match ordered.binary_search_by_key(&pts.ticks, |sample_id| {
            self.sample_meta(*sample_id)
                .map_or(0, |sample| sample.pts.ticks)
        }) {
            Ok(index) => Some(ordered[index]),
            Err(0) => Some(ordered[0]),
            Err(index) => Some(ordered[index.saturating_sub(1)]),
        }
    }

    pub(crate) fn keyframe_before(&self, sample: SampleId) -> Option<SampleId> {
        let (track_id, index) = self.sample_lookup.get(&sample).copied()?;
        self.samples_by_track
            .get(&track_id)?
            .get(..=index)?
            .iter()
            .rev()
            .find_map(|meta| meta.keyframe.then_some(meta.sample_id))
    }

    pub(crate) fn gop_for_sample(&self, sample: SampleId) -> Option<GopSegment> {
        let (track_id, index) = self.sample_lookup.get(&sample).copied()?;
        let samples = self.samples_by_track.get(&track_id)?;
        let keyframe_sample = self.keyframe_before(sample)?;
        let keyframe_meta = self.sample_meta(keyframe_sample)?;
        let target = samples.get(index)?;
        let end_sample_exclusive = samples
            .get(index.saturating_add(1))
            .map_or(SampleId(u64::MAX), |meta| meta.sample_id);
        let end_pts = MediaTime::new(
            target.pts.ticks.saturating_add(u64::from(target.duration)),
            target.pts.timescale,
        );
        Some(GopSegment {
            track_id,
            keyframe_sample,
            end_sample_exclusive,
            start_pts: keyframe_meta.pts,
            end_pts,
        })
    }

    pub(crate) fn read_sample(&mut self, sample: SampleId) -> Result<EncodedSample> {
        let meta = self
            .sample_meta(sample)
            .cloned()
            .with_context(|| format!("unknown sample id {}", sample.0))?;
        let size = usize::try_from(meta.size).context("sample size exceeds usize")?;
        let data = self.store.read_range(meta.offset, size)?;
        self.status.samples_read = self.status.samples_read.saturating_add(1);
        self.status.bytes_read = self.status.bytes_read.saturating_add(u64::from(meta.size));
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
        let Some(sample_id) = self.sample_order.get(self.next_sample_index).copied() else {
            return Ok(None);
        };
        self.next_sample_index = self.next_sample_index.saturating_add(1);
        self.read_sample(sample_id).map(Some)
    }

    pub(crate) fn read_gop(&mut self, sample: SampleId) -> Result<Vec<EncodedSample>> {
        self.iter_gop_for_sample(sample)?.collect()
    }

    pub(crate) fn iter_gop_for_sample(
        &mut self,
        sample: SampleId,
    ) -> Result<EncodedSampleIter<'_>> {
        let segment = self
            .gop_for_sample(sample)
            .with_context(|| format!("unknown sample id {}", sample.0))?;
        self.encoded_iter(segment)
    }

    pub(crate) fn encoded_iter(&mut self, segment: GopSegment) -> Result<EncodedSampleIter<'_>> {
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
    let size = required
        .size
        .map(|size| size.min(remaining as usize))
        .unwrap_or(remaining as usize);
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
    let (hits, misses, evictions, resident_bytes) = store.status_fields();
    status.cache_hits = hits;
    status.cache_misses = misses;
    status.cache_evictions = evictions;
    status.cache_resident_bytes = resident_bytes;
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;

    use super::*;
    use shiguredo_mp4::TrackKind;

    #[test]
    fn open_regular_mp4_sample_from_workspace() {
        let input_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.mp4");
        let mut core =
            ReaderCore::open(&Fmp4ReaderConfig::new(input_path.clone())).unwrap_or_else(|err| {
                panic!("failed to open {}: {err:#}", input_path.display());
            });
        assert!(!core.tracks().is_empty(), "no tracks in sample-10s.mp4");
        let video_track = core
            .tracks()
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .expect("video track should exist")
            .track_id;
        assert!(
            !core.samples(video_track).expect("video samples").is_empty(),
            "sample index must not be empty"
        );
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

    #[test]
    fn metadata_index_supports_lookup_gop_and_on_demand_reads() {
        let input_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.mp4");
        let mut core =
            ReaderCore::open(&Fmp4ReaderConfig::new(input_path)).expect("sample should open");
        let video_track = core
            .tracks()
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .expect("video track should exist")
            .track_id;
        let samples = core.samples(video_track).expect("video samples");
        assert!(
            samples.len() > 1,
            "sample index should have multiple entries"
        );

        let first = samples[0].clone();
        assert_eq!(
            core.sample_meta(first.sample_id),
            Some(&first),
            "sample_meta should resolve indexed metadata"
        );
        assert_eq!(
            core.sample_at_pts(video_track, first.pts),
            Some(first.sample_id),
            "sample_at_pts should find exact PTS"
        );
        assert_eq!(
            core.keyframe_before(first.sample_id),
            Some(first.sample_id),
            "the first sample should be a keyframe for repository sample"
        );

        let gop = core
            .gop_for_sample(first.sample_id)
            .expect("first sample should produce GOP segment");
        assert_eq!(gop.track_id, video_track);
        assert_eq!(gop.keyframe_sample, first.sample_id);

        let encoded = core
            .read_sample(first.sample_id)
            .expect("on-demand sample read should succeed");
        assert_eq!(encoded.meta, first);
        assert_eq!(encoded.data.len(), encoded.meta.size as usize);
        assert!(!encoded.to_annexb().expect("annexb").is_empty());
    }

    #[test]
    fn encoded_iter_reads_gop_on_demand() {
        let input_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.mp4");
        let mut core =
            ReaderCore::open(&Fmp4ReaderConfig::new(input_path)).expect("sample should open");
        let video_track = core
            .tracks()
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .expect("video track should exist")
            .track_id;
        let first_sample = core.samples(video_track).expect("video samples")[0].sample_id;
        let gop = core
            .gop_for_sample(first_sample)
            .expect("first sample should produce GOP segment");
        let mut iter = core.encoded_iter(gop).expect("encoded iterator");
        let first = iter
            .next()
            .expect("iterator should produce first sample")
            .expect("sample read should succeed");
        assert_eq!(first.meta.sample_id, first_sample);
        assert!(first.meta.keyframe);

        let mut iter = core
            .iter_gop_for_sample(first_sample)
            .expect("sample GOP iterator");
        let first = iter
            .next()
            .expect("sample GOP iterator should produce first sample")
            .expect("sample GOP read should succeed");
        assert_eq!(first.meta.sample_id, first_sample);
        assert!(first.meta.keyframe);
        drop(iter);

        let samples = core.read_gop(first_sample).expect("eager GOP read");
        assert!(!samples.is_empty());
        assert_eq!(samples[0].meta.sample_id, first_sample);
        assert!(samples[0].meta.keyframe);
    }

    #[test]
    fn range_cache_reads_boundaries_and_evicts_lru_chunks() {
        let input_path = std::env::temp_dir().join(format!(
            "video-hw-fmp4-range-cache-{}.bin",
            std::process::id()
        ));
        {
            let mut file = File::create(&input_path).expect("create temp range-cache file");
            file.write_all(&(0_u8..16).collect::<Vec<_>>())
                .expect("write temp range-cache file");
        }

        let mut file = File::open(&input_path).expect("open temp range-cache file");
        let mut cache = RangeCache::new(RangeCacheConfig {
            chunk_size: 4,
            max_bytes: 8,
            read_ahead_chunks: 0,
        });

        assert_eq!(
            cache
                .read_range(&mut file, 16, 2, 6)
                .expect("cross-chunk range read"),
            vec![2, 3, 4, 5, 6, 7]
        );
        assert_eq!(cache.stats.misses, 2);
        assert_eq!(cache.stats.hits, 0);
        assert_eq!(cache.resident_bytes, 8);

        assert_eq!(
            cache
                .read_range(&mut file, 16, 2, 6)
                .expect("cached cross-chunk range read"),
            vec![2, 3, 4, 5, 6, 7]
        );
        assert_eq!(cache.stats.hits, 2);
        assert_eq!(cache.stats.misses, 2);

        assert_eq!(
            cache
                .read_range(&mut file, 16, 8, 4)
                .expect("third chunk read"),
            vec![8, 9, 10, 11]
        );
        assert_eq!(cache.stats.evictions, 1);
        assert_eq!(cache.resident_bytes, 8);

        let _ = std::fs::remove_file(input_path);
    }
}
