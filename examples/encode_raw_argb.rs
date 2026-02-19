use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use video_hw::{
    BackendEncoderOptions, BackendKind, Codec, Encoder, EncoderConfig, Frame, NvidiaEncoderOptions,
};

#[derive(Parser, Debug)]
#[command(about = "Encode raw ARGB frames")]
struct Args {
    #[arg(long, default_value = "nv")]
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
    if matches!(backend, BackendKind::Nvidia) {
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
    let mut encoder = Encoder::with_config(backend, config);

    let mut total_packets = 0usize;
    let mut out = Vec::new();
    for i in 0..args.frame_count {
        let start = i * frame_size;
        let end = start + frame_size;
        let frame = Frame {
            width: args.width,
            height: args.height,
            pixel_format: None,
            pts_90k: Some((i as i64) * 3000),
            argb: Some(input[start..end].to_vec()),
            force_keyframe: false,
        };
        let packets = encoder.push_frame(frame)?;
        total_packets += packets.len();
        for p in packets {
            out.extend_from_slice(&p.data);
        }
    }

    let packets = encoder.flush()?;
    total_packets += packets.len();
    for p in packets {
        out.extend_from_slice(&p.data);
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

fn parse_backend(raw: &str) -> Result<BackendKind> {
    match raw.to_ascii_lowercase().as_str() {
        "vt" | "videotoolbox" => Ok(BackendKind::VideoToolbox),
        "nvidia" | "nv" => Ok(BackendKind::Nvidia),
        other => anyhow::bail!("unsupported backend: {other}"),
    }
}
