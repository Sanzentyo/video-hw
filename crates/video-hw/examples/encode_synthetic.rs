use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Parser;
use video_hw::{
    AnyEncodeSession, Backend, BackendEncoderOptions, BackendKind, Codec, Dimensions, EncodeFrame,
    EncoderConfig, IntelEncoderOptions, NvidiaEncoderOptions, RawFrameBuffer, Timestamp90k,
    VtEncoderOptions, VulkanEncoderOptions,
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
    #[arg(long, default_value_t = 640)]
    width: usize,
    #[arg(long, default_value_t = 360)]
    height: usize,
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
    #[arg(long)]
    vt_report_metrics: Option<bool>,
    #[arg(long)]
    vt_enable_pipeline_scheduler: Option<bool>,
    #[arg(long)]
    vt_pipeline_queue_capacity: Option<usize>,
    #[arg(long, default_value_t = false)]
    intel_force_software: bool,
    #[arg(long)]
    vulkan_adapter_index: Option<usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let codec = parse_codec(&args.codec)?;
    let backend: Backend = args.backend.parse()?;

    let mut config = EncoderConfig::new(codec, args.fps, args.require_hardware);
    let resolved_backend = backend
        .resolve_encoder(&config)
        .context("failed to resolve encoder backend")?;
    if backend_is_nvidia(resolved_backend) {
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
    } else if backend_is_vt(resolved_backend) {
        config.backend_options = BackendEncoderOptions::VideoToolbox(VtEncoderOptions {
            report_metrics: args.vt_report_metrics,
            enable_pipeline_scheduler: args.vt_enable_pipeline_scheduler,
            pipeline_queue_capacity: args.vt_pipeline_queue_capacity,
        });
    } else if backend_is_intel(resolved_backend) {
        config.backend_options = BackendEncoderOptions::Intel(IntelEncoderOptions {
            force_software: args.intel_force_software,
            hevc_use_vpp: None,
            ..Default::default()
        });
    } else if backend_is_vulkan(resolved_backend) {
        config.backend_options = BackendEncoderOptions::Vulkan(VulkanEncoderOptions {
            adapter_index: args.vulkan_adapter_index,
            ..Default::default()
        });
    }
    let mut encoder = AnyEncodeSession::with_backend_kind(resolved_backend, config)?;
    let mut output_writer = if args.discard_output {
        None
    } else {
        let output_file = File::create(&args.output)
            .with_context(|| format!("failed to create output: {}", args.output.display()))?;
        Some(BufWriter::new(output_file))
    };

    let mut total_packets = 0usize;
    let mut output_bytes = 0usize;
    let dims = dims(args.width as u32, args.height as u32)?;

    for i in 0..args.frame_count {
        let argb = synthetic_argb(args.width, args.height, i);
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
        resolved_backend,
        args.codec
    );

    Ok(())
}

fn parse_codec(raw: &str) -> Result<Codec> {
    match raw.to_ascii_lowercase().as_str() {
        "h264" => Ok(Codec::H264),
        "hevc" | "h265" => Ok(Codec::Hevc),
        "av1" => Ok(Codec::Av1),
        other => anyhow::bail!("unsupported codec: {other}"),
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

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn backend_is_vt(backend: BackendKind) -> bool {
    matches!(backend, BackendKind::VideoToolbox)
}

#[cfg(not(all(target_os = "macos", feature = "backend-vt")))]
fn backend_is_vt(_backend: BackendKind) -> bool {
    false
}

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
fn backend_is_vulkan(backend: BackendKind) -> bool {
    matches!(backend, BackendKind::Vulkan)
}

#[cfg(not(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
)))]
fn backend_is_vulkan(_backend: BackendKind) -> bool {
    false
}

fn dims(width: u32, height: u32) -> Result<Dimensions> {
    let width = std::num::NonZeroU32::new(width).context("width must be > 0")?;
    let height = std::num::NonZeroU32::new(height).context("height must be > 0")?;
    Ok(Dimensions { width, height })
}

fn synthetic_argb(width: usize, height: usize, frame_index: usize) -> Vec<u8> {
    let mut buffer = vec![0_u8; width.saturating_mul(height).saturating_mul(4)];
    let frame_shift = frame_index as u8;
    let green_shift = frame_shift.wrapping_mul(2);
    let blue = frame_shift.wrapping_mul(5);

    for (y, row) in buffer.chunks_exact_mut(width.saturating_mul(4)).enumerate() {
        let mut red = frame_shift;
        let green = (y as u8).wrapping_add(green_shift);
        for px in row.chunks_exact_mut(4) {
            px[0] = 255;
            px[1] = red;
            px[2] = green;
            px[3] = blue;
            red = red.wrapping_add(1);
        }
    }
    buffer
}
