use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use video_hw::{
    AnyDecodeSession, Backend, BackendDecoderOptions, BackendKind, BitstreamInput, Codec,
    DecodeOutputMode, DecoderConfig, IntelDecoderOptions, NvidiaDecoderOptions,
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
    #[arg(long, default_value = "metadata")]
    output_mode: String,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
    #[arg(long)]
    nv_report_metrics: Option<bool>,
    #[arg(long, default_value_t = false)]
    intel_force_software: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let codec = parse_codec(&args.codec)?;
    let output_mode = parse_output_mode(&args.output_mode)?;
    let backend: Backend = args.backend.parse()?;
    let mut config = DecoderConfig {
        codec,
        fps: args.fps,
        require_hardware: args.require_hardware,
        output_mode,
        backend_options: BackendDecoderOptions::Default,
    };
    let resolved_backend = backend
        .resolve_decoder(&config)
        .context("failed to resolve decoder backend")?;
    let input_path = args.input.unwrap_or_else(|| default_decode_input(codec));
    config.backend_options = if backend_is_nvidia(resolved_backend) {
        BackendDecoderOptions::Nvidia(NvidiaDecoderOptions {
            report_metrics: args.nv_report_metrics,
        })
    } else if backend_is_intel(resolved_backend) {
        BackendDecoderOptions::Intel(IntelDecoderOptions {
            force_software: args.intel_force_software,
        })
    } else {
        BackendDecoderOptions::Default
    };

    let data = fs::read(&input_path)
        .with_context(|| format!("failed to read input stream: {}", input_path.display()))?;
    let step = args.chunk_bytes.max(1);
    let (total_decoded, summary) =
        decode_with_backend(resolved_backend, config, &data, step).context("decode failed")?;

    println!(
        "decoded_frames={}, width={:?}, height={:?}, pixel_format={:?}, input={}, chunk_bytes={}, backend={}",
        total_decoded,
        summary.width,
        summary.height,
        summary.pixel_format,
        input_path.display(),
        step,
        resolved_backend
    );

    Ok(())
}

fn decode_with_backend(
    resolved_backend: BackendKind,
    config: DecoderConfig,
    data: &[u8],
    step: usize,
) -> Result<(usize, video_hw::DecodeSummary), video_hw::BackendError> {
    let mut decoder = AnyDecodeSession::with_backend_kind(resolved_backend, config)?;
    let mut total_decoded = 0usize;
    for chunk in data.chunks(step) {
        loop {
            match decoder.submit(BitstreamInput::AnnexBChunk {
                chunk: chunk.to_vec(),
                pts_90k: None,
            }) {
                Ok(()) => break,
                Err(video_hw::BackendError::TemporaryBackpressure(_)) => {
                    while decoder.try_reap()?.is_some() {
                        total_decoded += 1;
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }
    while decoder.try_reap()?.is_some() {
        total_decoded += 1;
    }
    total_decoded += decoder.flush()?.len();
    Ok((total_decoded, decoder.summary()))
}

fn parse_codec(raw: &str) -> Result<Codec> {
    match raw.to_ascii_lowercase().as_str() {
        "h264" => Ok(Codec::H264),
        "hevc" | "h265" => Ok(Codec::Hevc),
        other => anyhow::bail!("unsupported codec: {other}"),
    }
}

fn parse_output_mode(raw: &str) -> Result<DecodeOutputMode> {
    match raw.to_ascii_lowercase().as_str() {
        "metadata" => Ok(DecodeOutputMode::Metadata),
        "nv12" => Ok(DecodeOutputMode::Nv12),
        "rgb24" => Ok(DecodeOutputMode::Rgb24),
        other => anyhow::bail!("unsupported output_mode: {other}"),
    }
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
fn backend_is_nvidia(backend: BackendKind) -> bool {
    matches!(backend, BackendKind::Nvidia)
}

#[cfg(not(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
)))]
fn backend_is_nvidia(_backend: BackendKind) -> bool {
    false
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
fn backend_is_intel(backend: BackendKind) -> bool {
    matches!(backend, BackendKind::Intel)
}

#[cfg(not(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
)))]
fn backend_is_intel(_backend: BackendKind) -> bool {
    false
}

fn default_decode_input(codec: Codec) -> PathBuf {
    match codec {
        Codec::H264 => PathBuf::from("assets/h264_annexb.ts.h264"),
        Codec::Hevc => PathBuf::from("assets/hevc_annexb.ts.h265"),
    }
}
