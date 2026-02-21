use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use video_hw::{
    Backend, BackendEncoderOptions, Codec, Dimensions, EncodeFrame, EncodeSession, EncoderConfig,
    NvidiaEncoderOptions, RawFrameBuffer, Timestamp90k,
};

#[derive(Parser, Debug)]
#[command(about = "Encode raw ARGB frames")]
struct Args {
    #[arg(long, default_value = "auto")]
    backend: String,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long, default_value_t = true)]
    require_hardware: bool,
    #[arg(long)]
    input_raw: PathBuf,
    #[arg(long, default_value_t = 640)]
    width: usize,
    #[arg(long, default_value_t = 360)]
    height: usize,
    #[arg(long, default_value_t = 300)]
    frame_count: usize,
    #[arg(long, default_value = "./encoded-output.bin")]
    output: PathBuf,
    #[arg(long)]
    nv_max_in_flight: Option<usize>,
    #[arg(long)]
    nv_gop_length: Option<u32>,
    #[arg(long)]
    nv_frame_interval_p: Option<i32>,
    #[arg(long)]
    nv_report_metrics: Option<bool>,
    #[arg(long)]
    nv_safe_lifetime_mode: Option<bool>,
    #[arg(long)]
    nv_enable_pipeline_scheduler: Option<bool>,
    #[arg(long)]
    nv_pipeline_queue_capacity: Option<usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let codec = parse_codec(&args.codec)?;
    let backend = parse_backend(&args.backend)?;

    let frame_size = args
        .width
        .checked_mul(args.height)
        .and_then(|px| px.checked_mul(4))
        .context("frame size overflow")?;
    let required_size = frame_size
        .checked_mul(args.frame_count)
        .context("required input size overflow")?;
    let input = fs::read(&args.input_raw)
        .with_context(|| format!("failed to read raw input: {}", args.input_raw.display()))?;
    if input.len() < required_size {
        anyhow::bail!(
            "raw input too small: need {} bytes for {} frames, got {}",
            required_size,
            args.frame_count,
            input.len()
        );
    }

    let mut config = EncoderConfig::new(codec, args.fps, args.require_hardware);
    if backend_is_nvidia(backend) {
        let mut options = NvidiaEncoderOptions::default();
        if let Some(value) = args.nv_max_in_flight {
            options.max_in_flight_outputs = value.clamp(1, 64);
        }
        options.gop_length = args.nv_gop_length;
        options.frame_interval_p = args.nv_frame_interval_p;
        options.report_metrics = args.nv_report_metrics;
        options.safe_lifetime_mode = args.nv_safe_lifetime_mode;
        options.enable_pipeline_scheduler = args.nv_enable_pipeline_scheduler;
        options.pipeline_queue_capacity = args.nv_pipeline_queue_capacity;
        config.backend_options = BackendEncoderOptions::Nvidia(options);
    }
    let mut encoder = EncodeSession::new(backend, config);

    let mut total_packets = 0usize;
    let mut out = Vec::new();
    let dims = dims(args.width as u32, args.height as u32)?;
    for i in 0..args.frame_count {
        let start = i * frame_size;
        let end = start + frame_size;

        encoder.submit(EncodeFrame {
            dims,
            pts_90k: Some(Timestamp90k((i as i64) * 3000)),
            buffer: RawFrameBuffer::Argb8888(input[start..end].to_vec()),
            force_keyframe: i == 0,
        })?;

        while let Some(packet) = encoder.try_reap()? {
            total_packets += 1;
            out.extend_from_slice(&packet.data);
        }
    }

    for packet in encoder.flush()? {
        total_packets += 1;
        out.extend_from_slice(&packet.data);
    }

    fs::write(&args.output, &out)
        .with_context(|| format!("failed to write output: {}", args.output.display()))?;
    println!(
        "packets={}, output_bytes={}, output={}, backend={}, codec={}, input_raw={}",
        total_packets,
        out.len(),
        args.output.display(),
        args.backend,
        args.codec,
        args.input_raw.display()
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

fn dims(width: u32, height: u32) -> Result<Dimensions> {
    let width = std::num::NonZeroU32::new(width).context("width must be > 0")?;
    let height = std::num::NonZeroU32::new(height).context("height must be > 0")?;
    Ok(Dimensions { width, height })
}
