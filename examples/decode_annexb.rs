use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use video_hw::{
    Backend, BackendDecoderOptions, BitstreamInput, Codec, DecodeSession, DecoderConfig,
    NvidiaDecoderOptions,
};

#[derive(Parser, Debug)]
#[command(about = "Decode Annex-B stream")]
struct Args {
    #[arg(long, default_value = "auto")]
    backend: String,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long)]
    input: Option<PathBuf>,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long, default_value_t = 65536)]
    chunk_bytes: usize,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
    #[arg(long)]
    nv_report_metrics: Option<bool>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let codec = parse_codec(&args.codec)?;
    let backend = parse_backend(&args.backend)?;
    let input_path = args.input.unwrap_or_else(|| default_decode_input(codec));
    let backend_options = if backend_is_nvidia(backend) {
        BackendDecoderOptions::Nvidia(NvidiaDecoderOptions {
            report_metrics: args.nv_report_metrics,
        })
    } else {
        BackendDecoderOptions::Default
    };

    let mut decoder = DecodeSession::new(
        backend,
        DecoderConfig {
            codec,
            fps: args.fps,
            require_hardware: args.require_hardware,
            backend_options,
        },
    );

    let data = fs::read(&input_path)
        .with_context(|| format!("failed to read input stream: {}", input_path.display()))?;
    let step = args.chunk_bytes.max(1);

    let mut total_decoded = 0usize;
    for chunk in data.chunks(step) {
        decoder
            .submit(BitstreamInput::AnnexBChunk {
                chunk: chunk.to_vec(),
                pts_90k: None,
            })
            .context("decode submit failed")?;
        while decoder.try_reap().context("try_reap failed")?.is_some() {
            total_decoded += 1;
        }
    }

    total_decoded += decoder.flush().context("flush failed")?.len();
    let summary = decoder.summary();

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

fn parse_backend(raw: &str) -> Result<Backend> {
    match raw.to_ascii_lowercase().as_str() {
        #[cfg(any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            )
        ))]
        "auto" => Ok(Backend::Auto),
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        "vt" | "videotoolbox" => Ok(Backend::VideoToolbox),
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        "nvidia" | "nv" => Ok(Backend::Nvidia),
        other => anyhow::bail!("unsupported backend: {other}"),
    }
}

fn backend_is_nvidia(backend: Backend) -> bool {
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    {
        return matches!(backend, Backend::Nvidia);
    }
    {
        let _ = backend;
        false
    }
}

fn default_decode_input(codec: Codec) -> PathBuf {
    match codec {
        Codec::H264 => PathBuf::from("sample-videos/sample-10s.h264"),
        Codec::Hevc => PathBuf::from("sample-videos/sample-10s.h265"),
    }
}
