use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use video_hw_fmp4::{Fmp4Reader, Fmp4ReaderConfig};

#[derive(Debug, Parser)]
struct Cli {
    input: PathBuf,
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
    }
    while let Some(sample) = reader.next_sample()? {
        println!(
            "sample id={} track={} dts={} pts={} dur={} keyframe={} bytes={}",
            sample.meta.sample_id,
            sample.meta.track_id,
            sample.meta.dts.ticks,
            sample.meta.pts.ticks,
            sample.meta.duration,
            sample.meta.keyframe,
            sample.data.len()
        );
    }
    let finished = reader.finish();
    println!("samples_read={}", finished.status().samples_read);
    Ok(())
}
