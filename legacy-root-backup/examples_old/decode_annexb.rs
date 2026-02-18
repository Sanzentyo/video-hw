use std::fs;

use anyhow::Context;
use clap::Parser;
use video_hw::{default_decode_input, Codec, DecodeOptions, VtBitstreamDecoder};

#[derive(Parser, Debug)]
#[command(about = "Decode Annex-B stream with VideoToolbox")]
struct Args {
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long)]
    input: Option<String>,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long, default_value_t = 4096)]
    chunk_bytes: usize,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let codec = Codec::from_str(&args.codec)?;
    let input_path = args
        .input
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| default_decode_input(codec));

    let options = DecodeOptions {
        codec,
        require_hardware: args.require_hardware,
    };
    let mut decoder = VtBitstreamDecoder::new(options, args.fps);

    let data = fs::read(&input_path)
        .with_context(|| format!("failed to read input stream: {}", input_path.display()))?;
    let step = args.chunk_bytes.max(1);

    let mut total_decoded = 0usize;
    let mut width = None;
    let mut height = None;
    let mut pixel_format = None;

    for chunk in data.chunks(step) {
        let summary = decoder.push_bitstream_chunk(chunk)?;
        total_decoded += summary.decoded_frames;
        if width.is_none() {
            width = summary.width;
        }
        if height.is_none() {
            height = summary.height;
        }
        if pixel_format.is_none() {
            pixel_format = summary.pixel_format;
        }
    }

    let flush_summary = decoder.flush()?;
    total_decoded += flush_summary.decoded_frames;
    if width.is_none() {
        width = flush_summary.width;
    }
    if height.is_none() {
        height = flush_summary.height;
    }
    if pixel_format.is_none() {
        pixel_format = flush_summary.pixel_format;
    }

    println!(
        "decoded_frames={}, width={:?}, height={:?}, pixel_format={:?}, input={}, chunk_bytes={}",
        total_decoded,
        width,
        height,
        pixel_format,
        input_path.display(),
        step
    );

    if total_decoded == 0 {
        eprintln!(
            "hint: input may be missing parameter sets/AUD; regenerate stream with repeat headers (aud=1:repeat-headers=1)"
        );
    }

    Ok(())
}
