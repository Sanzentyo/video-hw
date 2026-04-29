#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
anyhow = "1"
video-hw-fmp4 = { path = "../crates/video-hw-fmp4" }
---

use anyhow::{Context, Result, anyhow};
use std::env;
use std::path::PathBuf;
use video_hw_fmp4::{
    Fmp4Reader, Fmp4ReaderConfig, IndexMode, SampleId, SampleLookupMatch, TrackKind,
};

fn main() -> Result<()> {
    let input_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("sample-videos/sample-10s.mp4"));

    let mut config = Fmp4ReaderConfig::new(input_path.clone());
    config.index_mode = IndexMode::Lazy;
    let mut reader = Fmp4Reader::new(config)
        .into_sync_session()
        .with_context(|| format!("failed to open {}", input_path.display()))?;

    println!("input={}", input_path.display());
    println!("initial_indexed={}", reader.status().samples_indexed);
    if reader.status().samples_indexed != 0 {
        return Err(anyhow!("Lazy reader indexed samples during open"));
    }

    let first = reader
        .next_sample()
        .context("next_sample failed")?
        .context("input had no samples")?;
    println!(
        "next_sample sample={} track={} bytes={} indexed={}",
        first.meta.sample_id,
        first.meta.track_id,
        first.data.len(),
        reader.status().samples_indexed
    );
    if reader.status().samples_indexed == 0 {
        return Err(anyhow!("next_sample did not advance the lazy metadata index"));
    }

    let target = SampleId(10);
    let tenth = reader
        .read_sample(target)
        .with_context(|| format!("read_sample({target}) failed"))?;
    let after_point_read = reader.status().samples_indexed;
    println!(
        "read_sample sample={} bytes={} indexed={}",
        tenth.meta.sample_id,
        tenth.data.len(),
        after_point_read
    );
    if tenth.meta.sample_id != target {
        return Err(anyhow!("read_sample returned the wrong sample"));
    }

    let video_track = reader
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .context("input has no video track")?
        .track_id;
    let sample_count = reader.samples(video_track)?.len();
    let after_full_index = reader.status().samples_indexed;
    println!(
        "samples track={} count={} indexed={}",
        video_track, sample_count, after_full_index
    );
    if after_full_index < after_point_read {
        return Err(anyhow!("full indexing moved the indexed count backwards"));
    }
    if sample_count == 0 {
        return Err(anyhow!("video track had no indexed samples"));
    }
    let checkpoints = {
        let samples = reader.samples(video_track)?;
        [
            samples[0].sample_id,
            samples[samples.len() / 2].sample_id,
            samples[samples.len() - 1].sample_id,
        ]
    };
    for sample_id in checkpoints {
        let meta = reader
            .sample_meta(sample_id)
            .cloned()
            .with_context(|| format!("sample metadata disappeared for {sample_id}"))?;
        let lookup = reader
            .sample_at_pts_with_delta(video_track, meta.pts)
            .with_context(|| format!("sample_at_pts_with_delta failed for {sample_id}"))?;
        if lookup.match_type != SampleLookupMatch::Exact {
            return Err(anyhow!(
                "expected exact PTS match for sample {}, got {:?}",
                sample_id,
                lookup.match_type
            ));
        }
        let gop = reader
            .gop_for_sample(sample_id)
            .with_context(|| format!("gop_for_sample failed for {sample_id}"))?;
        println!(
            "checkpoint sample={} pts={} lookup={:?} gop_start={} gop_end={}",
            sample_id,
            meta.pts.ticks,
            lookup.match_type,
            gop.keyframe_sample,
            gop.end_sample_exclusive
        );
    }
    let snapshot = reader.index_snapshot()?;
    println!(
        "snapshot tracks={} samples={} cache_resident={}",
        snapshot.tracks.len(),
        snapshot.samples.len(),
        reader.status().cache_resident_bytes
    );
    reader.clear_cache();
    println!("cache_after_clear={}", reader.status().cache_resident_bytes);

    println!("verify_fmp4_lazy=ok");
    Ok(())
}
