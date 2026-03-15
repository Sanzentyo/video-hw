use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Parser;
use video_hw::{
    Backend, BackendEncoderOptions, Codec, Dimensions, EncodeFrame, EncodeSession, EncoderConfig,
    IntelEncoderOptions, NvidiaEncoderOptions, RawFrameBuffer, Timestamp90k,
};

#[derive(Parser, Debug)]
#[command(about = "Encode synthetic frames")]
struct Args {
    #[arg(long, default_value = "auto")]
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
    #[arg(long, default_value_t = false)]
    discard_output: bool,

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
    #[arg(long, default_value_t = false)]
    intel_force_software: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let codec = parse_codec(&args.codec)?;
    let backend = parse_backend(&args.backend)?;

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
    } else if backend_is_intel(backend) {
        config.backend_options = BackendEncoderOptions::Intel(IntelEncoderOptions {
            force_software: args.intel_force_software,
            ..Default::default()
        });
    }
    let mut encoder = EncodeSession::new(backend, config);
    let mut output_writer = if args.discard_output {
        None
    } else {
        let output_file = File::create(&args.output)
            .with_context(|| format!("failed to create output: {}", args.output.display()))?;
        Some(BufWriter::new(output_file))
    };

    let mut total_packets = 0usize;
    let mut output_bytes = 0usize;
    let dims = dims(640, 360)?;

    for i in 0..args.frame_count {
        let argb = synthetic_argb(640, 360, i);
        encoder.submit(EncodeFrame {
            dims,
            pts_90k: Some(Timestamp90k((i as i64) * 3000)),
            buffer: RawFrameBuffer::Argb8888(argb),
            force_keyframe: i == 0,
        })?;
        while let Some(packet) = encoder.try_reap()? {
            total_packets += 1;
            if let Some(writer) = output_writer.as_mut() {
                writer.write_all(&packet.data).with_context(|| {
                    format!("failed to write output: {}", args.output.display())
                })?;
            }
            output_bytes = output_bytes.saturating_add(packet.data.len());
        }
    }

    for packet in encoder.flush()? {
        total_packets += 1;
        if let Some(writer) = output_writer.as_mut() {
            writer
                .write_all(&packet.data)
                .with_context(|| format!("failed to write output: {}", args.output.display()))?;
        }
        output_bytes = output_bytes.saturating_add(packet.data.len());
    }
    if let Some(writer) = output_writer.as_mut() {
        writer
            .flush()
            .with_context(|| format!("failed to flush output: {}", args.output.display()))?;
    }

    println!(
        "packets={}, output_bytes={}, output={}, discard_output={}, backend={}, codec={}",
        total_packets,
        output_bytes,
        args.output.display(),
        args.discard_output,
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

fn dims(width: u32, height: u32) -> Result<Dimensions> {
    let width = std::num::NonZeroU32::new(width).context("width must be > 0")?;
    let height = std::num::NonZeroU32::new(height).context("height must be > 0")?;
    Ok(Dimensions { width, height })
}

fn synthetic_argb(width: usize, height: usize, frame_index: usize) -> Vec<u8> {
    let mut buffer = vec![0_u8; width.saturating_mul(height).saturating_mul(4)];
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;
            buffer[offset] = 255;
            buffer[offset + 1] = ((x + frame_index) % 256) as u8;
            buffer[offset + 2] = ((y + frame_index * 2) % 256) as u8;
            buffer[offset + 3] = ((frame_index * 5) % 256) as u8;
        }
    }
    buffer
}
