use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use video_hw::{BackendKind, Codec, Decoder, DecoderConfig};

#[derive(Parser, Debug)]
#[command(about = "Decode Annex-B stream")]
struct Args {
    #[arg(long, default_value = "vt")]
    backend: String,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long)]
    input: Option<PathBuf>,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long, default_value_t = 4096)]
    chunk_bytes: usize,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let codec = parse_codec(&args.codec)?;
    let backend = parse_backend(&args.backend)?;
    let input_path = args.input.unwrap_or_else(|| default_decode_input(codec));

    let mut decoder = Decoder::new(
        backend,
        DecoderConfig {
            codec,
            fps: args.fps,
            require_hardware: args.require_hardware,
        },
    );

    let data = fs::read(&input_path)
        .with_context(|| format!("failed to read input stream: {}", input_path.display()))?;
    let step = args.chunk_bytes.max(1);

    let mut total_decoded = 0usize;
    for chunk in data.chunks(step) {
        let frames = decoder
            .push_bitstream_chunk(chunk, None)
            .context("push_bitstream_chunk failed")?;
        total_decoded += frames.len();
    }

    total_decoded += decoder.flush().context("flush failed")?.len();
    let summary = decoder.decode_summary();

    println!(
        "decoded_frames={}, width={:?}, height={:?}, pixel_format={:?}, input={}, chunk_bytes={}, backend={}",
        total_decoded,
        summary.width,
        summary.height,
        summary.pixel_format,
        input_path.display(),
        step,
        args.backend
    );

    Ok(())
}

fn parse_codec(raw: &str) -> Result<Codec> {
    match raw.to_ascii_lowercase().as_str() {
        "h264" => Ok(Codec::H264),
        "hevc" | "h265" => Ok(Codec::Hevc),
        other => anyhow::bail!("unsupported codec: {other}"),
    }
}

fn parse_backend(raw: &str) -> Result<BackendKind> {
    match raw.to_ascii_lowercase().as_str() {
        "vt" | "videotoolbox" => Ok(BackendKind::VideoToolbox),
        "nvidia" | "nv" => Ok(BackendKind::Nvidia),
        other => anyhow::bail!("unsupported backend: {other}"),
    }
}

fn default_decode_input(codec: Codec) -> PathBuf {
    match codec {
        Codec::H264 => PathBuf::from("sample-videos/sample-10s.h264"),
        Codec::Hevc => PathBuf::from("sample-videos/sample-10s.h265"),
    }
}
