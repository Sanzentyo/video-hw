use std::{fs, path::PathBuf};

use anyhow::Result;
use clap::Parser;
use video_hw::{
    BackendEncoderOptions, BackendKind, Codec, Encoder, EncoderConfig, Frame, NvidiaEncoderOptions,
};

#[derive(Parser, Debug)]
#[command(about = "Encode synthetic frames")]
struct Args {
    #[arg(long, default_value = "vt")]
    backend: String,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
    #[arg(long, default_value_t = 30)]
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
        let frame = Frame {
            width: 640,
            height: 360,
            pixel_format: None,
            pts_90k: Some((i as i64) * 3000),
            argb: None,
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

    fs::write(&args.output, &out)?;

    println!(
        "packets={}, output_bytes={}, output={}, backend={}, codec={}",
        total_packets,
        out.len(),
        args.output.display(),
        args.backend,
        args.codec
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
