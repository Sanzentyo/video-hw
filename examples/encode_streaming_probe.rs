use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;
use video_hw::{
    Backend, BackendEncoderOptions, Codec, Dimensions, EncodeFrame, EncodeSession, EncoderConfig,
    NvidiaEncoderOptions, RawFrameBuffer, Timestamp90k,
};

#[derive(Parser, Debug)]
#[command(about = "Probe streaming suitability of video-hw encoder backends")]
struct Args {
    #[arg(long, default_value = "auto")]
    backend: String,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
    #[arg(long, default_value_t = 640)]
    width: usize,
    #[arg(long, default_value_t = 360)]
    height: usize,
    #[arg(long, default_value_t = 120)]
    frame_count: usize,

    #[arg(long)]
    nv_max_in_flight: Option<usize>,
    #[arg(long)]
    nv_report_metrics: Option<bool>,
    #[arg(long)]
    nv_safe_lifetime_mode: Option<bool>,
    #[arg(long)]
    nv_pipeline_queue_capacity: Option<usize>,
}

#[derive(Default)]
struct ProbeSummary {
    pushed: usize,
    push_non_empty: usize,
    flush_calls: usize,
    flush_empty: usize,
    packets: usize,
    bytes: usize,
    push_ms: Vec<f64>,
    flush_ms: Vec<f64>,
}

impl ProbeSummary {
    fn record_push(&mut self, elapsed_ms: f64, packets_len: usize) {
        self.pushed += 1;
        self.push_ms.push(elapsed_ms);
        if packets_len > 0 {
            self.push_non_empty += 1;
        }
        self.packets += packets_len;
    }

    fn record_flush(&mut self, elapsed_ms: f64, packets_len: usize, bytes: usize) {
        self.flush_calls += 1;
        self.flush_ms.push(elapsed_ms);
        if packets_len == 0 {
            self.flush_empty += 1;
        }
        self.packets += packets_len;
        self.bytes += bytes;
    }

    fn print(&self, label: &str) {
        println!(
            "[{label}] pushed={}, push_non_empty={}, flush_calls={}, flush_empty={}, packets={}, bytes={}, push_mean_ms={:.3}, push_p95_ms={:.3}, flush_mean_ms={:.3}, flush_p95_ms={:.3}",
            self.pushed,
            self.push_non_empty,
            self.flush_calls,
            self.flush_empty,
            self.packets,
            self.bytes,
            mean(&self.push_ms),
            percentile(&self.push_ms, 0.95),
            mean(&self.flush_ms),
            percentile(&self.flush_ms, 0.95)
        );
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let backend = parse_backend(&args.backend)?;
    let codec = parse_codec(&args.codec)?;

    let batch_summary = run_batch_flush_probe(&args, backend, codec)?;
    let streaming_summary = run_per_frame_flush_probe(&args, backend, codec)?;

    println!(
        "backend={:?}, codec={:?}, fps={}, size={}x{}, frames={}",
        backend, codec, args.fps, args.width, args.height, args.frame_count
    );
    batch_summary.print("batch_flush_once");
    streaming_summary.print("streaming_flush_each_frame");

    Ok(())
}

fn run_batch_flush_probe(args: &Args, backend: Backend, codec: Codec) -> Result<ProbeSummary> {
    let mut encoder = build_encoder(args, backend, codec);
    let mut summary = ProbeSummary::default();

    for i in 0..args.frame_count {
        let frame = make_frame(args.width, args.height, i, args.fps)?;
        let push_start = Instant::now();
        encoder.submit(frame)?;
        let packets_from_push = reap_all(&mut encoder)?;
        let push_elapsed = push_start.elapsed().as_secs_f64() * 1_000.0;
        summary.record_push(push_elapsed, packets_from_push.len());
        summary.bytes += packets_from_push
            .iter()
            .map(|p| p.data.len())
            .sum::<usize>();
    }

    let flush_start = Instant::now();
    let packets = encoder.flush()?;
    let flush_elapsed = flush_start.elapsed().as_secs_f64() * 1_000.0;
    let bytes = packets.iter().map(|p| p.data.len()).sum::<usize>();
    summary.record_flush(flush_elapsed, packets.len(), bytes);

    Ok(summary)
}

fn run_per_frame_flush_probe(args: &Args, backend: Backend, codec: Codec) -> Result<ProbeSummary> {
    let mut encoder = build_encoder(args, backend, codec);
    let mut summary = ProbeSummary::default();

    for i in 0..args.frame_count {
        let frame = make_frame(args.width, args.height, i, args.fps)?;

        let push_start = Instant::now();
        encoder.submit(frame)?;
        let packets_from_push = reap_all(&mut encoder)?;
        let push_elapsed = push_start.elapsed().as_secs_f64() * 1_000.0;
        summary.record_push(push_elapsed, packets_from_push.len());
        summary.bytes += packets_from_push
            .iter()
            .map(|p| p.data.len())
            .sum::<usize>();

        let flush_start = Instant::now();
        let packets = encoder.flush()?;
        let flush_elapsed = flush_start.elapsed().as_secs_f64() * 1_000.0;
        let bytes = packets.iter().map(|p| p.data.len()).sum::<usize>();
        summary.record_flush(flush_elapsed, packets.len(), bytes);
    }

    let flush_start = Instant::now();
    let packets = encoder.flush()?;
    let flush_elapsed = flush_start.elapsed().as_secs_f64() * 1_000.0;
    let bytes = packets.iter().map(|p| p.data.len()).sum::<usize>();
    summary.record_flush(flush_elapsed, packets.len(), bytes);

    Ok(summary)
}

fn reap_all(encoder: &mut EncodeSession) -> Result<Vec<video_hw::EncodedChunk>> {
    let mut out = Vec::new();
    while let Some(packet) = encoder.try_reap()? {
        out.push(packet);
    }
    Ok(out)
}

fn build_encoder(args: &Args, backend: Backend, codec: Codec) -> EncodeSession {
    let mut config = EncoderConfig::new(codec, args.fps, args.require_hardware);
    if backend_is_nvidia(backend) {
        let mut options = NvidiaEncoderOptions::default();
        if let Some(value) = args.nv_max_in_flight {
            options.max_in_flight_outputs = value.clamp(1, 64);
        }
        options.report_metrics = args.nv_report_metrics;
        options.safe_lifetime_mode = args.nv_safe_lifetime_mode;
        options.pipeline_queue_capacity = args.nv_pipeline_queue_capacity;
        config.backend_options = BackendEncoderOptions::Nvidia(options);
    }
    EncodeSession::new(backend, config)
}

fn make_frame(width: usize, height: usize, index: usize, fps: i32) -> Result<EncodeFrame> {
    let dims = dims(width as u32, height as u32)?;
    let frame_size = width.saturating_mul(height).saturating_mul(4);
    let mut argb = vec![0u8; frame_size];

    for px in argb.chunks_exact_mut(4) {
        px[0] = 255;
        px[1] = (index.wrapping_mul(13) % 255) as u8;
        px[2] = 96;
        px[3] = 192;
    }

    let pts_step_90k = (90_000 / fps.max(1)) as i64;
    Ok(EncodeFrame {
        dims,
        pts_90k: Some(Timestamp90k((index as i64).saturating_mul(pts_step_90k))),
        buffer: RawFrameBuffer::Argb8888(argb),
        force_keyframe: index == 0,
    })
}

fn dims(width: u32, height: u32) -> Result<Dimensions> {
    let width = std::num::NonZeroU32::new(width).context("width must be > 0")?;
    let height = std::num::NonZeroU32::new(height).context("height must be > 0")?;
    Ok(Dimensions { width, height })
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
        "nv" | "nvidia" => Ok(Backend::Nvidia),
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

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((sorted.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize;
    sorted[idx]
}
