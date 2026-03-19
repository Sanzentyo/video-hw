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
    let reader = Fmp4Reader::new(Fmp4ReaderConfig {
        input_path: cli.input,
    });
    let mut reader = reader.into_sync_session()?;
    println!("tracks={}", reader.tracks().len());
    for track in reader.tracks() {
        println!(
            "track id={} kind={:?} duration={} timescale={}",
            track.track_id, track.kind, track.duration, track.timescale
        );
    }
    while let Some(sample) = reader.next_sample()? {
        println!(
            "sample track={} ts={} dur={} keyframe={} bytes={}",
            sample.track_id,
            sample.timestamp,
            sample.duration,
            sample.keyframe,
            sample.data.len()
        );
    }
    let finished = reader.finish();
    println!("samples_read={}", finished.status().samples_read);
    Ok(())
}
