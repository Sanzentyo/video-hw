use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use video_hw_fmp4::{Fmp4Reader, Fmp4ReaderConfig, SampleId, TrackKind};

#[derive(Debug, Parser)]
struct Cli {
    input: PathBuf,
    #[arg(long)]
    sample_id: Option<u64>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let reader = Fmp4Reader::new(Fmp4ReaderConfig::new(cli.input));
    let mut reader = reader.into_sync_session()?;
    println!("tracks={}", reader.tracks().len());
    for track in reader.tracks() {
        println!(
            "track id={} kind={:?} duration={} timescale={}",
            track.track_id, track.kind, track.duration, track.timescale
        );
        let samples = reader.samples(track.track_id)?;
        println!("  indexed_samples={}", samples.len());
        for sample in samples.iter().take(5) {
            println!(
                "  meta sample={} dts={} pts={} dur={} keyframe={} range={}..{}",
                sample.sample_id,
                sample.dts.ticks,
                sample.pts.ticks,
                sample.duration,
                sample.keyframe,
                sample.offset,
                sample.offset.saturating_add(sample.size as u64)
            );
        }
    }

    let sample_id = cli
        .sample_id
        .map(SampleId)
        .or_else(|| {
            reader
                .tracks()
                .iter()
                .find(|track| track.kind == TrackKind::Video)
                .and_then(|track| reader.samples(track.track_id).ok()?.first().cloned())
                .map(|sample| sample.sample_id)
        })
        .or_else(|| {
            reader
                .tracks()
                .first()
                .and_then(|track| reader.samples(track.track_id).ok()?.first().cloned())
                .map(|sample| sample.sample_id)
        });

    if let Some(sample_id) = sample_id {
        let sample = reader.read_sample(sample_id)?;
        println!(
            "read sample={} track={} dts={} pts={} dur={} keyframe={} bytes={} annexb={}",
            sample.meta.sample_id,
            sample.meta.track_id,
            sample.meta.dts.ticks,
            sample.meta.pts.ticks,
            sample.meta.duration,
            sample.meta.keyframe,
            sample.data.len(),
            sample.to_annexb().map(|annexb| annexb.len()).unwrap_or(0)
        );
    }
    let finished = reader.finish();
    println!("samples_read={}", finished.status().samples_read);
    Ok(())
}
