use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Read, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Parser;
use video_hw::{
    AnyEncodeSession, Backend, BackendEncoderOptions, BackendKind, Codec, Dimensions, EncodeFrame,
    EncoderConfig, IntelEncoderOptions, NvidiaEncoderOptions, RawFrameBuffer, Timestamp90k,
    VtEncoderOptions,
};

#[derive(Parser, Debug)]
#[command(about = "Encode raw frames")]
struct Args {
    #[arg(long, default_value = "auto")]
    backend: String,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
    #[arg(long)]
    input_raw: PathBuf,
    #[arg(long, default_value = "argb")]
    input_pix_fmt: String,
    #[arg(long, default_value_t = 640)]
    width: usize,
    #[arg(long, default_value_t = 360)]
    height: usize,
    #[arg(long, default_value_t = 300)]
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
    #[arg(long)]
    vt_report_metrics: Option<bool>,
    #[arg(long)]
    vt_enable_pipeline_scheduler: Option<bool>,
    #[arg(long)]
    vt_pipeline_queue_capacity: Option<usize>,
    #[arg(long, default_value_t = false)]
    intel_force_software: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let codec = parse_codec(&args.codec)?;
    let backend: Backend = args.backend.parse()?;
    let input_pix_fmt = parse_input_pix_fmt(&args.input_pix_fmt)?;

    if matches!(input_pix_fmt, InputPixelFormat::Nv12)
        && (!args.width.is_multiple_of(2) || !args.height.is_multiple_of(2))
    {
        anyhow::bail!("nv12 input requires even width/height");
    }

    let frame_size = frame_size_for_format(input_pix_fmt, args.width, args.height)?;
    let required_size = frame_size
        .checked_mul(args.frame_count)
        .context("required input size overflow")?;
    let input_len = usize::try_from(
        fs::metadata(&args.input_raw)
            .with_context(|| format!("failed to stat raw input: {}", args.input_raw.display()))?
            .len(),
    )
    .context("raw input length does not fit in usize")?;
    if input_len < required_size {
        anyhow::bail!(
            "raw input too small: need {} bytes for {} frames, got {}",
            required_size,
            args.frame_count,
            input_len
        );
    }
    let input_file = File::open(&args.input_raw)
        .with_context(|| format!("failed to open raw input: {}", args.input_raw.display()))?;
    let mut input_reader = BufReader::new(input_file);

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
        let mut frame_argb = vec![0_u8; frame_size];
        input_reader.read_exact(&mut frame_argb).with_context(|| {
            format!("failed to read frame {i} from {}", args.input_raw.display())
        })?;
        let buffer = make_raw_frame_buffer(input_pix_fmt, frame_argb, args.width, args.height)?;

        encoder.submit(EncodeFrame {
            dims,
            pts_90k: Some(Timestamp90k((i as i64) * 3000)),
            buffer,
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
        "packets={}, output_bytes={}, output={}, discard_output={}, backend={}, codec={}, input_raw={}",
        total_packets,
        output_bytes,
        args.output.display(),
        args.discard_output,
        resolved_backend,
        args.codec,
        args.input_raw.display()
    );
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum InputPixelFormat {
    Argb,
    Nv12,
}

fn parse_input_pix_fmt(raw: &str) -> Result<InputPixelFormat> {
    match raw.to_ascii_lowercase().as_str() {
        "argb" => Ok(InputPixelFormat::Argb),
        "nv12" => Ok(InputPixelFormat::Nv12),
        other => anyhow::bail!("unsupported input_pix_fmt: {other}"),
    }
}

fn frame_size_for_format(
    input_pix_fmt: InputPixelFormat,
    width: usize,
    height: usize,
) -> Result<usize> {
    match input_pix_fmt {
        InputPixelFormat::Argb => width
            .checked_mul(height)
            .and_then(|px| px.checked_mul(4))
            .context("ARGB frame size overflow"),
        InputPixelFormat::Nv12 => width
            .checked_mul(height)
            .and_then(|px| px.checked_mul(3))
            .and_then(|bytes| bytes.checked_div(2))
            .context("NV12 frame size overflow"),
    }
}

fn make_raw_frame_buffer(
    input_pix_fmt: InputPixelFormat,
    frame_bytes: Vec<u8>,
    width: usize,
    height: usize,
) -> Result<RawFrameBuffer> {
    match input_pix_fmt {
        InputPixelFormat::Argb => Ok(RawFrameBuffer::Argb8888(frame_bytes)),
        InputPixelFormat::Nv12 => make_nv12_buffer(frame_bytes, width, height),
    }
}

#[cfg(feature = "unstable-raw-inputs")]
fn make_nv12_buffer(frame_bytes: Vec<u8>, width: usize, height: usize) -> Result<RawFrameBuffer> {
    let y_size = width
        .checked_mul(height)
        .context("nv12 Y plane size overflow")?;
    if frame_bytes.len() != y_size.saturating_mul(3) / 2 {
        anyhow::bail!(
            "nv12 payload size mismatch: expected {}, got {}",
            y_size.saturating_mul(3) / 2,
            frame_bytes.len()
        );
    }
    Ok(RawFrameBuffer::Nv12 {
        pitch: width,
        data: frame_bytes,
    })
}

#[cfg(not(feature = "unstable-raw-inputs"))]
fn make_nv12_buffer(
    _frame_bytes: Vec<u8>,
    _width: usize,
    _height: usize,
) -> Result<RawFrameBuffer> {
    anyhow::bail!("nv12 input requires building with --features unstable-raw-inputs");
}

fn parse_codec(raw: &str) -> Result<Codec> {
    match raw.to_ascii_lowercase().as_str() {
        "h264" => Ok(Codec::H264),
        "hevc" | "h265" => Ok(Codec::Hevc),
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

fn dims(width: u32, height: u32) -> Result<Dimensions> {
    let width = std::num::NonZeroU32::new(width).context("width must be > 0")?;
    let height = std::num::NonZeroU32::new(height).context("height must be > 0")?;
    Ok(Dimensions { width, height })
}
