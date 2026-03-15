use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::IntelDecoderAdapter;
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::NvDecoderAdapter;
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use video_hw::VtDecoderAdapter;
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::VulkanDecoderAdapter;
use video_hw::{
    Backend, BackendDecoderOptions, BackendError, BitstreamInput, Codec, DecodeOutputMode,
    DecodeSession, DecoderConfig, IntelDecoderOptions, NvidiaDecoderOptions,
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
    #[arg(long, default_value_t = false)]
    intel_force_software: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let codec = parse_codec(&args.codec)?;
    let backend = resolve_backend(parse_backend(&args.backend)?)?;
    let input_path = args.input.unwrap_or_else(|| default_decode_input(codec));
    let backend_options = if backend_is_nvidia(backend) {
        BackendDecoderOptions::Nvidia(NvidiaDecoderOptions {
            report_metrics: args.nv_report_metrics,
        })
    } else if backend_is_intel(backend) {
        BackendDecoderOptions::Intel(IntelDecoderOptions {
            force_software: args.intel_force_software,
        })
    } else {
        BackendDecoderOptions::Default
    };
    let config = DecoderConfig {
        codec,
        fps: args.fps,
        require_hardware: args.require_hardware,
        output_mode: DecodeOutputMode::Metadata,
        backend_options,
    };

    let data = fs::read(&input_path)
        .with_context(|| format!("failed to read input stream: {}", input_path.display()))?;
    let step = args.chunk_bytes.max(1);

    let (total_decoded, summary) = match backend {
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        Backend::Nvidia => decode_with_nvidia(config, &data, step).context("decode failed")?,
        #[cfg(all(
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        Backend::Intel => decode_with_intel(config, &data, step).context("decode failed")?,
        #[cfg(all(
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        Backend::Vulkan => decode_with_vulkan(config, &data, step).context("decode failed")?,
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        Backend::VideoToolbox => decode_with_vt(config, &data, step).context("decode failed")?,
        Backend::Auto => unreachable!("auto is resolved in resolve_backend"),
    };

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
                any(
                    feature = "backend-nvidia",
                    feature = "backend-intel",
                    feature = "backend-vulkan"
                ),
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
        #[cfg(all(
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        "intel" | "qsv" => Ok(Backend::Intel),
        #[cfg(all(
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        "vulkan" | "vk" => Ok(Backend::Vulkan),
        other => anyhow::bail!("unsupported backend: {other}"),
    }
}

fn resolve_backend(backend: Backend) -> Result<Backend> {
    if !matches!(backend, Backend::Auto) {
        return Ok(backend);
    }
    let mut selected = None;
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    if selected.is_none() {
        selected = Some(Backend::Nvidia);
    }
    #[cfg(all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    if selected.is_none() {
        selected = Some(Backend::Intel);
    }
    #[cfg(all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    ))]
    if selected.is_none() {
        selected = Some(Backend::Vulkan);
    }
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    if selected.is_none() {
        selected = Some(Backend::VideoToolbox);
    }

    selected.ok_or_else(|| {
        anyhow::anyhow!("Backend::Auto is not available for this build target/feature set")
    })
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
fn decode_with_nvidia(
    config: DecoderConfig,
    data: &[u8],
    step: usize,
) -> Result<(usize, video_hw::DecodeSummary), BackendError> {
    let mut decoder = DecodeSession::<NvDecoderAdapter>::new(config);
    let mut total_decoded = 0usize;
    for chunk in data.chunks(step) {
        loop {
            match decoder.submit(BitstreamInput::AnnexBChunk {
                chunk: chunk.to_vec(),
                pts_90k: None,
            }) {
                Ok(()) => break,
                Err(BackendError::TemporaryBackpressure(_)) => {
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

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
fn decode_with_intel(
    config: DecoderConfig,
    data: &[u8],
    step: usize,
) -> Result<(usize, video_hw::DecodeSummary), BackendError> {
    let mut decoder = DecodeSession::<IntelDecoderAdapter>::new(config);
    let mut total_decoded = 0usize;
    for chunk in data.chunks(step) {
        loop {
            match decoder.submit(BitstreamInput::AnnexBChunk {
                chunk: chunk.to_vec(),
                pts_90k: None,
            }) {
                Ok(()) => break,
                Err(BackendError::TemporaryBackpressure(_)) => {
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

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
fn decode_with_vulkan(
    config: DecoderConfig,
    data: &[u8],
    step: usize,
) -> Result<(usize, video_hw::DecodeSummary), BackendError> {
    let mut decoder = DecodeSession::<VulkanDecoderAdapter>::new(config);
    let mut total_decoded = 0usize;
    for chunk in data.chunks(step) {
        loop {
            match decoder.submit(BitstreamInput::AnnexBChunk {
                chunk: chunk.to_vec(),
                pts_90k: None,
            }) {
                Ok(()) => break,
                Err(BackendError::TemporaryBackpressure(_)) => {
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

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn decode_with_vt(
    config: DecoderConfig,
    data: &[u8],
    step: usize,
) -> Result<(usize, video_hw::DecodeSummary), BackendError> {
    let mut decoder = DecodeSession::<VtDecoderAdapter>::new(config);
    let mut total_decoded = 0usize;
    for chunk in data.chunks(step) {
        loop {
            match decoder.submit(BitstreamInput::AnnexBChunk {
                chunk: chunk.to_vec(),
                pts_90k: None,
            }) {
                Ok(()) => break,
                Err(BackendError::TemporaryBackpressure(_)) => {
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

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
fn backend_is_nvidia(backend: Backend) -> bool {
    matches!(backend, Backend::Nvidia)
}

#[cfg(not(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
)))]
fn backend_is_nvidia(_backend: Backend) -> bool {
    false
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
fn backend_is_intel(backend: Backend) -> bool {
    matches!(backend, Backend::Intel)
}

#[cfg(not(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
)))]
fn backend_is_intel(_backend: Backend) -> bool {
    false
}

fn default_decode_input(codec: Codec) -> PathBuf {
    match codec {
        Codec::H264 => PathBuf::from("sample-videos/sample-10s.h264"),
        Codec::Hevc => PathBuf::from("sample-videos/sample-10s.h265"),
    }
}
