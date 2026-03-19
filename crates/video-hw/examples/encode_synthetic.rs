use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Parser;
use video_hw::BackendError;
#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::IntelEncoderAdapter;
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::NvEncoderAdapter;
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use video_hw::VtEncoderAdapter;
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::VulkanEncoderAdapter;
use video_hw::{
    Backend, BackendEncoderOptions, BackendKind, Codec, Dimensions, EncodeFrame, EncodeSession,
    EncodedChunk, EncoderConfig, IntelEncoderOptions, NvidiaEncoderOptions, RawFrameBuffer,
    Timestamp90k,
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
    } else if backend_is_intel(resolved_backend) {
        config.backend_options = BackendEncoderOptions::Intel(IntelEncoderOptions {
            force_software: args.intel_force_software,
            ..Default::default()
        });
    }
    let mut encoder = BackendEncoderSession::new(resolved_backend, config)?;
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
        resolved_backend,
        args.codec
    );

    Ok(())
}

enum BackendEncoderSession {
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    Nvidia(Box<EncodeSession<NvEncoderAdapter>>),
    #[cfg(all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    Intel(Box<EncodeSession<IntelEncoderAdapter>>),
    #[cfg(all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    ))]
    Vulkan(Box<EncodeSession<VulkanEncoderAdapter>>),
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    VideoToolbox(Box<EncodeSession<VtEncoderAdapter>>),
}

impl BackendEncoderSession {
    fn new(backend: BackendKind, config: EncoderConfig) -> Result<Self> {
        #[allow(unreachable_patterns)]
        let session = match backend {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Nvidia => {
                Self::Nvidia(Box::new(EncodeSession::<NvEncoderAdapter>::new(config)))
            }
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Intel => {
                Self::Intel(Box::new(EncodeSession::<IntelEncoderAdapter>::new(config)))
            }
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Vulkan => {
                Self::Vulkan(Box::new(EncodeSession::<VulkanEncoderAdapter>::new(config)))
            }
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            BackendKind::VideoToolbox => {
                Self::VideoToolbox(Box::new(EncodeSession::<VtEncoderAdapter>::new(config)))
            }
            _ => anyhow::bail!("resolved backend is not enabled in this build: {backend}"),
        };
        Ok(session)
    }

    fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError> {
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.submit(frame),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.submit(frame),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.submit(frame),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.submit(frame),
            _ => unreachable!("no encoder backend variants are enabled in this build"),
        }
    }

    fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.try_reap(),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.try_reap(),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.try_reap(),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.try_reap(),
            _ => unreachable!("no encoder backend variants are enabled in this build"),
        }
    }

    fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.flush(),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.flush(),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.flush(),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.flush(),
            _ => unreachable!("no encoder backend variants are enabled in this build"),
        }
    }
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
