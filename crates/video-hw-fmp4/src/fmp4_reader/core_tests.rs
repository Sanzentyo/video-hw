use std::io::Write;
use std::num::NonZeroU32;
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
fn lazy_index_mode_advances_metadata_on_demand() {
    let input_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.mp4");
    let mut config = Fmp4ReaderConfig::new(input_path);
    config.index_mode = IndexMode::Lazy;
    let mut core = ReaderCore::open(&config).expect("lazy mode should open");
    assert_eq!(core.status().samples_indexed, 0);

    let first = core
        .next_sample()
        .expect("lazy next_sample should index and read")
        .expect("sample should exist");
    assert_eq!(first.meta.sample_id, SampleId(0));
    assert!(core.status().samples_indexed >= 1);

    let tenth = core
        .read_sample(SampleId(10))
        .expect("lazy read_sample should index through the requested sample");
    assert_eq!(tenth.meta.sample_id, SampleId(10));
    assert!(
        core.status().samples_indexed < 303,
        "point sample read should not force full video-track indexing"
    );

    let video_track = core
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .expect("video track should exist")
        .track_id;
    assert_eq!(core.samples(video_track).expect("video samples").len(), 303);
    assert!(core.status().samples_indexed >= 303);
}

#[test]
fn next_sample_cursor_does_not_advance_when_payload_read_fails() {
    let input_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.mp4");
    let mut core = ReaderCore::open(&Fmp4ReaderConfig::new(input_path))
        .expect("repository sample should open");
    let sample_id = core.sample_order[0];
    let (track_id, track_index) = core.sample_lookup[&sample_id];
    core.samples_by_track
        .get_mut(&track_id)
        .expect("sample track should exist")[track_index]
        .offset = u64::MAX;

    assert!(core.next_sample().is_err());
    assert_eq!(core.next_sample_index, 0);
    assert!(core.next_sample().is_err());
    assert_eq!(core.next_sample_index, 0);
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
        core.sample_meta(first.sample_id)
            .expect("sample metadata lookup should succeed"),
        Some(&first),
        "sample_meta should resolve indexed metadata"
    );
    assert_eq!(
        core.sample_at_pts(video_track, first.pts)
            .expect("sample PTS lookup should succeed"),
        Some(first.sample_id),
        "sample_at_pts should find exact PTS"
    );
    let exact_lookup = core
        .sample_at_pts_with_delta(video_track, first.pts)
        .expect("exact PTS lookup should succeed")
        .expect("exact PTS should match a sample");
    assert_eq!(exact_lookup.matched_sample, first.sample_id);
    assert_eq!(exact_lookup.match_type, SampleLookupMatch::Exact);
    assert_eq!(exact_lookup.delta_ticks, 0);
    let doubled_timescale = NonZeroU32::new(
        first
            .pts
            .timescale
            .get()
            .checked_mul(2)
            .expect("sample timescale can be doubled"),
    )
    .expect("non-zero doubled timescale");
    let equivalent_pts = MediaTime::new(
        first
            .pts
            .ticks
            .checked_mul(2)
            .expect("sample PTS can be doubled"),
        doubled_timescale,
    );
    let rescaled_lookup = core
        .sample_at_pts_with_delta(video_track, equivalent_pts)
        .expect("equivalent PTS lookup should succeed")
        .expect("equivalent PTS should match a sample");
    assert_eq!(rescaled_lookup.matched_sample, first.sample_id);
    assert_eq!(rescaled_lookup.requested_pts, first.pts);
    assert_eq!(rescaled_lookup.delta_ticks, 0);
    assert!(
        core.sample_at_pts_with_delta(
            video_track,
            MediaTime::new(u64::MAX, NonZeroU32::new(1).expect("non-zero timescale")),
        )
        .is_err(),
        "unrepresentable PTS rescaling must not look like a missing sample"
    );
    let first_after_lookup = core
        .sample_at_pts_with_delta(video_track, MediaTime::new(0, first.pts.timescale))
        .expect("pre-roll PTS lookup should succeed")
        .expect("pre-roll PTS should select the first sample");
    assert_eq!(first_after_lookup.match_type, SampleLookupMatch::FirstAfter);
    assert!(first_after_lookup.delta_ticks > 0);
    let previous_lookup = core
        .sample_at_pts_with_delta(
            video_track,
            MediaTime::new(first.pts.ticks.saturating_add(1), first.pts.timescale),
        )
        .expect("inexact PTS lookup should succeed")
        .expect("inexact PTS should select a sample");
    assert_eq!(previous_lookup.match_type, SampleLookupMatch::Previous);
    assert_eq!(previous_lookup.matched_sample, first.sample_id);
    assert_eq!(previous_lookup.delta_ticks, -1);
    assert_eq!(
        core.keyframe_before(first.sample_id)
            .expect("keyframe lookup should succeed"),
        Some(first.sample_id),
        "the first sample should be a keyframe for repository sample"
    );

    let gop = core
        .gop_for_sample(first.sample_id)
        .expect("GOP lookup should succeed")
        .expect("first sample should produce GOP segment");
    assert_eq!(gop.track_id, video_track);
    assert_eq!(gop.keyframe_sample, first.sample_id);
    let snapshot = core.index_snapshot().expect("index snapshot");
    assert_eq!(snapshot.tracks.len(), core.tracks().len());
    assert_eq!(
        snapshot.samples.len(),
        core.status().samples_indexed as usize
    );

    let encoded = core
        .read_sample(first.sample_id)
        .expect("on-demand sample read should succeed");
    assert_eq!(encoded.meta, first);
    assert_eq!(encoded.data.len(), encoded.meta.size as usize);
    assert!(!encoded.to_annexb().expect("annexb").is_empty());
}

#[test]
fn fragmented_mp4_sample_supports_metadata_gop_and_on_demand_reads() {
    let input_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("foreman_cif_fmp4.mp4");
    let mut core =
        ReaderCore::open(&Fmp4ReaderConfig::new(input_path)).expect("fMP4 sample should open");
    let video_track = core
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .expect("video track should exist")
        .track_id;
    let samples = core.samples(video_track).expect("video samples");
    assert_eq!(samples.len(), 300);
    assert_eq!(samples[0].dts.ticks, 0);
    assert_eq!(samples[0].pts.ticks, 2002);
    assert_eq!(samples[0].duration, 1001);
    assert!(samples[0].keyframe);
    assert!(samples.iter().all(|sample| sample.size > 0));
    let first_sample_id = samples[0].sample_id;
    let first_sample_size = samples[0].size as usize;
    let inter_sample_id = samples[4].sample_id;

    let gop = core
        .gop_for_sample(inter_sample_id)
        .expect("GOP lookup should succeed")
        .expect("inter frame should produce GOP segment");
    assert_eq!(gop.keyframe_sample, first_sample_id);

    let encoded = core
        .read_sample(first_sample_id)
        .expect("on-demand fMP4 sample read should succeed");
    assert_eq!(encoded.data.len(), first_sample_size);
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
        .expect("GOP lookup should succeed")
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

#[test]
fn range_cache_read_ahead_loads_next_chunk() {
    let input_path = std::env::temp_dir().join(format!(
        "video-hw-fmp4-range-cache-read-ahead-{}.bin",
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
        max_bytes: 16,
        read_ahead_chunks: 1,
    });

    assert_eq!(
        cache
            .read_range(&mut file, 16, 0, 2)
            .expect("range read should succeed"),
        vec![0, 1]
    );
    assert!(cache.chunks.contains_key(&0));
    assert!(cache.chunks.contains_key(&1));
    assert_eq!(cache.stats.misses, 2);

    let _ = std::fs::remove_file(input_path);
}

#[test]
fn reader_exposes_range_cache_stats() {
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

    let before = core.cache_stats();
    core.read_sample(first_sample)
        .expect("on-demand sample read should succeed");
    let after = core.cache_stats();
    let status = core.status();
    let track_stats = status
        .track_reads
        .get(&video_track)
        .expect("track read stats should be recorded");
    assert_eq!(track_stats.samples_read, 1);
    assert_eq!(track_stats.bytes_read, status.bytes_read);
    let sample_stats = status
        .sample_reads
        .get(&first_sample)
        .expect("sample read stats should be recorded");
    assert_eq!(sample_stats.reads, 1);
    assert_eq!(sample_stats.bytes_read, status.bytes_read);

    assert!(after.misses >= before.misses);
    assert!(after.resident_bytes >= before.resident_bytes);
    assert_eq!(core.status().range_cache_config.chunk_size, 8 * 1024 * 1024);
    core.clear_cache();
    assert_eq!(core.cache_stats().resident_bytes, 0);
}
