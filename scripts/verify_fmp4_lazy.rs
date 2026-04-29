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
use std::process::Command;
use video_hw_fmp4::{
    Fmp4Reader, Fmp4ReaderConfig, IndexMode, SampleId, SampleLookupMatch, TrackKind,
};

#[derive(Debug)]
struct Args {
    input_path: PathBuf,
    decode_features: Option<String>,
    decode_backend: String,
    require_hardware: bool,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let input_path = args.input_path.clone();

    verify_eager(&input_path)?;
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

    run_optional_decode_smoke(&input_path, &args)?;

    println!("verify_fmp4_lazy=ok");
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut input_path = None;
    let mut decode_features = None;
    let mut decode_backend = String::from("auto");
    let mut require_hardware = false;
    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--decode-features" => {
                decode_features = Some(
                    iter.next()
                        .context("--decode-features requires a feature list")?,
                );
            }
            "--decode-backend" => {
                decode_backend = iter.next().context("--decode-backend requires a backend")?;
            }
            "--require-hardware" => {
                require_hardware = true;
            }
            "-h" | "--help" => {
                println!(
                    "usage: cargo +nightly -Zscript scripts/verify_fmp4_lazy.rs [input.mp4] [--decode-features FEATURES] [--decode-backend BACKEND] [--require-hardware]"
                );
                std::process::exit(0);
            }
            value if value.starts_with('-') => {
                return Err(anyhow!("unknown option {value}"));
            }
            value => {
                if input_path.replace(PathBuf::from(value)).is_some() {
                    return Err(anyhow!("multiple input paths were provided"));
                }
            }
        }
    }
    Ok(Args {
        input_path: input_path.unwrap_or_else(|| PathBuf::from("sample-videos/sample-10s.mp4")),
        decode_features,
        decode_backend,
        require_hardware,
    })
}

fn verify_eager(input_path: &PathBuf) -> Result<()> {
    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path.clone()))
        .into_sync_session()
        .with_context(|| format!("failed to eager-open {}", input_path.display()))?;
    let video_track = reader
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .context("input has no video track")?
        .track_id;
    let sample_count = reader.samples(video_track)?.len();
    let status = reader.status();
    println!(
        "eager_indexed={} track={} samples={}",
        status.samples_indexed, video_track, sample_count
    );
    if sample_count == 0 {
        return Err(anyhow!("eager reader found no video samples"));
    }
    if status.samples_indexed < sample_count as u64 {
        return Err(anyhow!(
            "eager reader indexed fewer samples than the video track exposes"
        ));
    }
    Ok(())
}

fn run_optional_decode_smoke(input_path: &PathBuf, args: &Args) -> Result<()> {
    let Some(features) = &args.decode_features else {
        println!("decode_smoke=skipped");
        return Ok(());
    };
    let mut command = Command::new("cargo");
    command.args([
        "run",
        "-p",
        "video-hw-fmp4",
        "--example",
        "read_fmp4_slider_gui",
        "--features",
        features,
        "--",
    ]);
    command.arg(input_path);
    command.args([
        "--backend",
        args.decode_backend.as_str(),
        "--smoke-test",
    ]);
    if args.require_hardware {
        command.arg("--require-hardware");
    }
    println!(
        "decode_smoke=running features=\"{}\" backend={} require_hardware={}",
        features, args.decode_backend, args.require_hardware
    );
    let status = command.status().context("failed to spawn decode smoke test")?;
    if !status.success() {
        return Err(anyhow!("decode smoke test failed with status {status}"));
    }
    println!("decode_smoke=ok");
    Ok(())
}
