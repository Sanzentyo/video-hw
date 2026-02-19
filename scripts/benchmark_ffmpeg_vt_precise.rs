#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
anyhow = "1"
clap = { version = "4.5", features = ["derive"] }
---

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Codec {
    H264,
    Hevc,
}

impl Codec {
    fn as_cli(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
        }
    }

    fn sample_input(self) -> &'static str {
        match self {
            Self::H264 => "sample-videos/sample-10s.h264",
            Self::Hevc => "sample-videos/sample-10s.h265",
        }
    }

    fn ffmpeg_encode_codec(self) -> &'static str {
        match self {
            Self::H264 => "h264_videotoolbox",
            Self::Hevc => "hevc_videotoolbox",
        }
    }

    fn muxer(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Case {
    VideoHwDecode,
    VideoHwEncode,
    FfmpegDecode,
    FfmpegEncode,
}

impl Case {
    fn label(self) -> &'static str {
        match self {
            Self::VideoHwDecode => "video-hw decode",
            Self::VideoHwEncode => "video-hw encode",
            Self::FfmpegDecode => "ffmpeg decode",
            Self::FfmpegEncode => "ffmpeg encode",
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Precise repeated benchmark for video-hw (VT) vs ffmpeg (VT)")]
struct Args {
    #[arg(long, value_enum, default_value_t = Codec::H264)]
    codec: Codec,

    #[arg(long, default_value_t = true)]
    release: bool,

    #[arg(long, default_value_t = 1)]
    warmup: usize,

    #[arg(long, default_value_t = 7)]
    repeat: usize,

    #[arg(long, default_value_t = 65536)]
    chunk_bytes: usize,

    #[arg(long, default_value_t = 300)]
    frame_count: usize,

    #[arg(long, default_value_t = 640)]
    width: usize,

    #[arg(long, default_value_t = 360)]
    height: usize,

    #[arg(long, default_value_t = false)]
    verify: bool,

    #[arg(long, default_value_t = false)]
    equal_raw_input: bool,

    #[arg(long, default_value_t = false)]
    include_internal_metrics: bool,
}

#[derive(Debug, Clone)]
struct CaseSamples {
    case: Case,
    seconds: Vec<f64>,
}

#[derive(Debug, Default, Clone)]
struct DecodeMetricSamples {
    submit_ms: Vec<f64>,
    elapsed_ms: Vec<f64>,
    jitter_ms_mean: Vec<f64>,
    jitter_ms_p95: Vec<f64>,
    jitter_ms_p99: Vec<f64>,
    input_copy_bytes: Vec<f64>,
    output_copy_frames: Vec<f64>,
}

#[derive(Debug, Default, Clone)]
struct EncodeMetricSamples {
    frame_prep_ms: Vec<f64>,
    submit_ms: Vec<f64>,
    complete_ms: Vec<f64>,
    total_ms: Vec<f64>,
    queue_peak: Vec<f64>,
    queue_p95: Vec<f64>,
    queue_p99: Vec<f64>,
    jitter_ms_mean: Vec<f64>,
    jitter_ms_p95: Vec<f64>,
    jitter_ms_p99: Vec<f64>,
    input_copy_bytes: Vec<f64>,
    output_copy_bytes: Vec<f64>,
}

impl CaseSamples {
    fn new(case: Case) -> Self {
        Self {
            case,
            seconds: Vec::new(),
        }
    }

    fn push(&mut self, value: f64) {
        self.seconds.push(value);
    }

    fn summarize(&self) -> Stats {
        Stats::from_samples(&self.seconds)
    }
}

#[derive(Debug, Clone, Copy)]
struct Stats {
    min: f64,
    max: f64,
    mean: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    stddev: f64,
    cv_percent: f64,
}

impl Stats {
    fn from_samples(samples: &[f64]) -> Self {
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);

        let count = sorted.len().max(1);
        let mean = sorted.iter().sum::<f64>() / count as f64;
        let variance = sorted.iter().map(|x| (*x - mean).powi(2)).sum::<f64>() / count as f64;
        let stddev = variance.sqrt();
        let cv_percent = if mean > 0.0 { (stddev / mean) * 100.0 } else { 0.0 };

        Self {
            min: *sorted.first().unwrap_or(&0.0),
            max: *sorted.last().unwrap_or(&0.0),
            mean,
            p50: percentile_nearest_rank(&sorted, 50.0),
            p95: percentile_nearest_rank(&sorted, 95.0),
            p99: percentile_nearest_rank(&sorted, 99.0),
            stddev,
            cv_percent,
        }
    }
}

#[derive(Debug)]
struct CaseRun {
    seconds: f64,
    metrics: Option<InternalMetrics>,
}

fn percentile_nearest_rank(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let n = sorted.len();
    let rank = ((percentile / 100.0) * n as f64).ceil().clamp(1.0, n as f64) as usize;
    sorted[rank - 1]
}

fn main() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("this benchmark is intended for macOS (VideoToolbox)");
    }

    let args = Args::parse();
    if args.repeat == 0 {
        bail!("--repeat must be >= 1");
    }

    let profile = if args.release { "release" } else { "debug" };
    let output_dir = PathBuf::from("output");
    fs::create_dir_all(&output_dir).context("create output directory")?;

    build_examples(profile)?;

    let decode_bin = example_bin_path(profile, "decode_annexb");
    let encode_bin = example_bin_path(profile, "encode_synthetic");
    let encode_raw_bin = example_bin_path(profile, "encode_raw_argb");
    let video_hw_output = output_dir.join(format!("video-hw-vt-{}-precise.bin", args.codec.as_cli()));
    let ffmpeg_output = output_dir.join(format!("ffmpeg-vt-{}-precise.bin", args.codec.as_cli()));
    let raw_input = output_dir.join(format!(
        "benchmark-input-argb-{}x{}-{}f.raw",
        args.width, args.height, args.frame_count
    ));
    let null_sink = if cfg!(windows) { "NUL" } else { "/dev/null" };

    if args.equal_raw_input {
        write_raw_argb_input(&raw_input, args.width, args.height, args.frame_count)?;
    }

    let cases = [
        Case::VideoHwDecode,
        Case::VideoHwEncode,
        Case::FfmpegDecode,
        Case::FfmpegEncode,
    ];
    let mut samples = cases
        .iter()
        .copied()
        .map(CaseSamples::new)
        .collect::<Vec<_>>();
    let mut decode_metrics = DecodeMetricSamples::default();
    let mut encode_metrics = EncodeMetricSamples::default();

    for i in 0..(args.warmup + args.repeat) {
        let is_warmup = i < args.warmup;
        let round = i + 1;
        let label = if is_warmup { "warmup" } else { "measure" };
        println!("round {round}/{}, phase={label}", args.warmup + args.repeat);

        for case in &cases {
            let run = run_case(
                *case,
                &args,
                &decode_bin,
                &encode_bin,
                &encode_raw_bin,
                &video_hw_output,
                &ffmpeg_output,
                &raw_input,
                null_sink,
            )?;
            println!("  {:<16} {:.3}s", case.label(), run.seconds);
            if !is_warmup {
                let idx = match case {
                    Case::VideoHwDecode => 0,
                    Case::VideoHwEncode => 1,
                    Case::FfmpegDecode => 2,
                    Case::FfmpegEncode => 3,
                };
                samples[idx].push(run.seconds);
                if let Some(metrics) = run.metrics {
                    match metrics {
                        InternalMetrics::Decode {
                            submit_ms,
                            elapsed_ms,
                            jitter_ms_mean,
                            jitter_ms_p95,
                            jitter_ms_p99,
                            input_copy_bytes,
                            output_copy_frames,
                        } => {
                            decode_metrics.submit_ms.push(submit_ms);
                            decode_metrics.elapsed_ms.push(elapsed_ms);
                            decode_metrics.jitter_ms_mean.push(jitter_ms_mean);
                            decode_metrics.jitter_ms_p95.push(jitter_ms_p95);
                            decode_metrics.jitter_ms_p99.push(jitter_ms_p99);
                            decode_metrics.input_copy_bytes.push(input_copy_bytes);
                            decode_metrics.output_copy_frames.push(output_copy_frames);
                        }
                        InternalMetrics::Encode {
                            frame_prep_ms,
                            submit_ms,
                            complete_ms,
                            total_ms,
                            queue_peak,
                            queue_p95,
                            queue_p99,
                            jitter_ms_mean,
                            jitter_ms_p95,
                            jitter_ms_p99,
                            input_copy_bytes,
                            output_copy_bytes,
                        } => {
                            encode_metrics.frame_prep_ms.push(frame_prep_ms);
                            encode_metrics.submit_ms.push(submit_ms);
                            encode_metrics.complete_ms.push(complete_ms);
                            encode_metrics.total_ms.push(total_ms);
                            encode_metrics.queue_peak.push(queue_peak);
                            encode_metrics.queue_p95.push(queue_p95);
                            encode_metrics.queue_p99.push(queue_p99);
                            encode_metrics.jitter_ms_mean.push(jitter_ms_mean);
                            encode_metrics.jitter_ms_p95.push(jitter_ms_p95);
                            encode_metrics.jitter_ms_p99.push(jitter_ms_p99);
                            encode_metrics.input_copy_bytes.push(input_copy_bytes);
                            encode_metrics.output_copy_bytes.push(output_copy_bytes);
                        }
                    }
                }
            }
        }
    }

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before UNIX_EPOCH")?
        .as_secs();
    let report_path = output_dir.join(format!(
        "benchmark-vt-precise-{}-{}.md",
        args.codec.as_cli(),
        now_secs
    ));
    let video_hw_verify_input = output_dir.join(format!(
        "video-hw-vt-{}-precise-annexb-{}.bin",
        args.codec.as_cli(),
        now_secs
    ));

    let mut report = String::new();
    writeln!(&mut report, "# VT Precise Benchmark Report")?;
    writeln!(&mut report, "epoch_seconds: {now_secs}")?;
    writeln!(&mut report, "codec: {}", args.codec.as_cli())?;
    writeln!(&mut report, "warmup: {}", args.warmup)?;
    writeln!(&mut report, "repeat: {}", args.repeat)?;
    writeln!(&mut report, "width: {}", args.width)?;
    writeln!(&mut report, "height: {}", args.height)?;
    writeln!(&mut report, "equal_raw_input: {}", args.equal_raw_input)?;
    writeln!(&mut report, "verify: {}", args.verify)?;
    writeln!(&mut report, "internal_metrics: {}", args.include_internal_metrics)?;
    writeln!(&mut report)?;
    writeln!(
        &mut report,
        "| Case | min(s) | mean(s) | p50(s) | p95(s) | p99(s) | max(s) | stddev(s) | CV(%) |"
    )?;
    writeln!(&mut report, "|---|---:|---:|---:|---:|---:|---:|---:|---:|")?;
    for case_samples in &samples {
        let s = case_samples.summarize();
        writeln!(
            &mut report,
            "| {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.2} |",
            case_samples.case.label(),
            s.min,
            s.mean,
            s.p50,
            s.p95,
            s.p99,
            s.max,
            s.stddev,
            s.cv_percent
        )?;
    }
    writeln!(&mut report)?;
    writeln!(&mut report, "## Raw Samples")?;
    for case_samples in &samples {
        write!(&mut report, "- {}: ", case_samples.case.label())?;
        for (i, sec) in case_samples.seconds.iter().enumerate() {
            if i > 0 {
                write!(&mut report, ", ")?;
            }
            write!(&mut report, "{sec:.3}")?;
        }
        writeln!(&mut report)?;
    }

    if args.include_internal_metrics {
        writeln!(&mut report)?;
        writeln!(&mut report, "## Internal Metrics (video-hw)")?;
        if !decode_metrics.submit_ms.is_empty() {
            let submit = Stats::from_samples(&decode_metrics.submit_ms);
            let elapsed = Stats::from_samples(&decode_metrics.elapsed_ms);
            let jitter_mean = Stats::from_samples(&decode_metrics.jitter_ms_mean);
            let jitter_p95 = Stats::from_samples(&decode_metrics.jitter_ms_p95);
            let jitter_p99 = Stats::from_samples(&decode_metrics.jitter_ms_p99);
            let input_copy = Stats::from_samples(&decode_metrics.input_copy_bytes);
            let output_copy_frames = Stats::from_samples(&decode_metrics.output_copy_frames);
            writeln!(&mut report, "### decode")?;
            writeln!(
                &mut report,
                "- submit_ms mean={:.3}, p95={:.3}, p99={:.3}",
                submit.mean, submit.p95, submit.p99
            )?;
            writeln!(
                &mut report,
                "- elapsed_ms mean={:.3}, p95={:.3}, p99={:.3}",
                elapsed.mean, elapsed.p95, elapsed.p99
            )?;
            writeln!(
                &mut report,
                "- jitter_ms_mean mean={:.3}, jitter_ms_p95 mean={:.3}, jitter_ms_p99 mean={:.3}",
                jitter_mean.mean, jitter_p95.mean, jitter_p99.mean
            )?;
            writeln!(
                &mut report,
                "- input_copy_bytes mean={:.3}, p95={:.3}, p99={:.3}",
                input_copy.mean, input_copy.p95, input_copy.p99
            )?;
            writeln!(
                &mut report,
                "- output_copy_frames mean={:.3}, p95={:.3}, p99={:.3}",
                output_copy_frames.mean, output_copy_frames.p95, output_copy_frames.p99
            )?;
        }
        if !encode_metrics.frame_prep_ms.is_empty() {
            let frame_prep = Stats::from_samples(&encode_metrics.frame_prep_ms);
            let submit = Stats::from_samples(&encode_metrics.submit_ms);
            let complete = Stats::from_samples(&encode_metrics.complete_ms);
            let total = Stats::from_samples(&encode_metrics.total_ms);
            let queue_peak = Stats::from_samples(&encode_metrics.queue_peak);
            let queue_p95 = Stats::from_samples(&encode_metrics.queue_p95);
            let queue_p99 = Stats::from_samples(&encode_metrics.queue_p99);
            let jitter_mean = Stats::from_samples(&encode_metrics.jitter_ms_mean);
            let jitter_p95 = Stats::from_samples(&encode_metrics.jitter_ms_p95);
            let jitter_p99 = Stats::from_samples(&encode_metrics.jitter_ms_p99);
            let input_copy = Stats::from_samples(&encode_metrics.input_copy_bytes);
            let output_copy = Stats::from_samples(&encode_metrics.output_copy_bytes);
            writeln!(&mut report, "### encode")?;
            writeln!(
                &mut report,
                "- frame_prep_ms mean={:.3}, p95={:.3}, p99={:.3}",
                frame_prep.mean, frame_prep.p95, frame_prep.p99
            )?;
            writeln!(
                &mut report,
                "- submit_ms mean={:.3}, p95={:.3}, p99={:.3}",
                submit.mean, submit.p95, submit.p99
            )?;
            writeln!(
                &mut report,
                "- complete_ms mean={:.3}, p95={:.3}, p99={:.3}",
                complete.mean, complete.p95, complete.p99
            )?;
            writeln!(
                &mut report,
                "- total_ms mean={:.3}, p95={:.3}, p99={:.3}",
                total.mean, total.p95, total.p99
            )?;
            writeln!(
                &mut report,
                "- queue_peak mean={:.3}, p95={:.3}, p99={:.3}",
                queue_peak.mean, queue_peak.p95, queue_peak.p99
            )?;
            writeln!(
                &mut report,
                "- queue_p95 mean={:.3}, queue_p99 mean={:.3}",
                queue_p95.mean, queue_p99.mean
            )?;
            writeln!(
                &mut report,
                "- jitter_ms_mean mean={:.3}, jitter_ms_p95 mean={:.3}, jitter_ms_p99 mean={:.3}",
                jitter_mean.mean, jitter_p95.mean, jitter_p99.mean
            )?;
            writeln!(
                &mut report,
                "- input_copy_bytes mean={:.3}, p95={:.3}, p99={:.3}",
                input_copy.mean, input_copy.p95, input_copy.p99
            )?;
            writeln!(
                &mut report,
                "- output_copy_bytes mean={:.3}, p95={:.3}, p99={:.3}",
                output_copy.mean, output_copy.p95, output_copy.p99
            )?;
        }
    }

    if args.verify {
        writeln!(&mut report)?;
        writeln!(&mut report, "## Verification")?;
        convert_length_prefixed_to_annexb(&video_hw_output, &video_hw_verify_input)
            .with_context(|| {
                format!(
                    "convert video-hw output to annexb: {}",
                    video_hw_output.display()
                )
            })?;
        match ffprobe_summary(&video_hw_verify_input, args.codec, args.frame_count) {
            Ok(summary) => {
                if let Err(err) = run_ffmpeg_decode_verify(&video_hw_verify_input, null_sink) {
                    writeln!(
                        &mut report,
                        "- video-hw: ffprobe=ok (codec={}, {}x{}, frames={}), decode=ng ({err})",
                        summary.codec_name, summary.width, summary.height, summary.nb_read_frames
                    )?;
                } else {
                    writeln!(
                        &mut report,
                        "- video-hw: codec={}, {}x{}, frames={} (decode=ok)",
                        summary.codec_name, summary.width, summary.height, summary.nb_read_frames
                    )?;
                }
            }
            Err(err) => {
                let bytes = fs::metadata(&video_hw_output).map(|m| m.len()).unwrap_or(0);
                writeln!(
                    &mut report,
                    "- video-hw: ffprobe=ng ({err}); fallback=output_bytes={bytes} (>0 expected)"
                )?;
                if bytes == 0 {
                    bail!("video-hw output is empty and ffprobe verification failed");
                }
            }
        }

        let summary = ffprobe_summary(&ffmpeg_output, args.codec, args.frame_count)?;
        run_ffmpeg_decode_verify(&ffmpeg_output, null_sink)?;
        writeln!(
            &mut report,
            "- ffmpeg: codec={}, {}x{}, frames={} (decode=ok)",
            summary.codec_name, summary.width, summary.height, summary.nb_read_frames
        )?;
    }

    fs::write(&report_path, report)
        .with_context(|| format!("write report: {}", report_path.display()))?;
    println!("saved report: {}", report_path.display());
    Ok(())
}

fn build_examples(profile: &str) -> Result<()> {
    let mut args = vec!["build", "--examples", "--features", "backend-vt", "--profile", profile];
    if profile == "release" {
        args = vec!["build", "--examples", "--features", "backend-vt", "--release"];
    }
    run_command("cargo", &args, &[])?;
    Ok(())
}

fn example_bin_path(profile: &str, name: &str) -> PathBuf {
    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
    PathBuf::from("target")
        .join(profile)
        .join("examples")
        .join(format!("{name}{exe_suffix}"))
}

fn run_case(
    case: Case,
    args: &Args,
    decode_bin: &Path,
    encode_bin: &Path,
    encode_raw_bin: &Path,
    video_hw_output: &Path,
    ffmpeg_output: &Path,
    raw_input: &Path,
    null_sink: &str,
) -> Result<CaseRun> {
    match case {
        Case::VideoHwDecode => {
            let mut cmd = Command::new(decode_bin);
            cmd.args([
                "--backend",
                "vt",
                "--codec",
                args.codec.as_cli(),
                "--input",
                args.codec.sample_input(),
                "--chunk-bytes",
                &args.chunk_bytes.to_string(),
            ]);
            if args.include_internal_metrics {
                cmd.env("VIDEO_HW_VT_METRICS", "1");
            }
            run_timed_command(cmd)
        }
        Case::VideoHwEncode => {
            let mut cmd = if args.equal_raw_input {
                let mut c = Command::new(encode_raw_bin);
                c.args([
                    "--backend",
                    "vt",
                    "--codec",
                    args.codec.as_cli(),
                    "--fps",
                    "30",
                    "--frame-count",
                    &args.frame_count.to_string(),
                    "--width",
                    &args.width.to_string(),
                    "--height",
                    &args.height.to_string(),
                    "--input-raw",
                    &raw_input.to_string_lossy(),
                    "--output",
                    &video_hw_output.to_string_lossy(),
                ]);
                c
            } else {
                let mut c = Command::new(encode_bin);
                c.args([
                    "--backend",
                    "vt",
                    "--codec",
                    args.codec.as_cli(),
                    "--fps",
                    "30",
                    "--frame-count",
                    &args.frame_count.to_string(),
                    "--output",
                    &video_hw_output.to_string_lossy(),
                ]);
                c
            };
            if args.include_internal_metrics {
                cmd.env("VIDEO_HW_VT_METRICS", "1");
            }
            run_timed_command(cmd)
        }
        Case::FfmpegDecode => {
            let mut cmd = Command::new("ffmpeg");
            cmd.args([
                "-y",
                "-hide_banner",
                "-benchmark",
                "-v",
                "error",
                "-hwaccel",
                "videotoolbox",
                "-i",
                args.codec.sample_input(),
                "-f",
                "null",
                null_sink,
            ]);
            run_timed_command(cmd)
        }
        Case::FfmpegEncode => {
            let mut cmd = Command::new("ffmpeg");
            if args.equal_raw_input {
                cmd.args([
                    "-y",
                    "-hide_banner",
                    "-benchmark",
                    "-v",
                    "error",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "argb",
                    "-s:v",
                    &format!("{}x{}", args.width, args.height),
                    "-r",
                    "30",
                    "-i",
                    &raw_input.to_string_lossy(),
                    "-frames:v",
                    &args.frame_count.to_string(),
                    "-c:v",
                    args.codec.ffmpeg_encode_codec(),
                    "-f",
                    args.codec.muxer(),
                    &ffmpeg_output.to_string_lossy(),
                ]);
            } else {
                cmd.args([
                    "-y",
                    "-hide_banner",
                    "-benchmark",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=640x360:rate=30",
                    "-frames:v",
                    &args.frame_count.to_string(),
                    "-c:v",
                    args.codec.ffmpeg_encode_codec(),
                    "-f",
                    args.codec.muxer(),
                    &ffmpeg_output.to_string_lossy(),
                ]);
            }
            run_timed_command(cmd)
        }
    }
}

fn write_raw_argb_input(path: &Path, width: usize, height: usize, frame_count: usize) -> Result<()> {
    let frame_size = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .context("frame size overflow")?;
    let total_size = frame_size
        .checked_mul(frame_count)
        .context("raw input total size overflow")?;

    let mut out = vec![0_u8; total_size];
    for frame_idx in 0..frame_count {
        let base = frame_idx * frame_size;
        for y in 0..height {
            for x in 0..width {
                let off = base + (y * width + x) * 4;
                out[off] = 255;
                out[off + 1] = ((x + frame_idx) % 256) as u8;
                out[off + 2] = ((y + frame_idx * 2) % 256) as u8;
                out[off + 3] = ((frame_idx * 5) % 256) as u8;
            }
        }
    }

    fs::write(path, out).with_context(|| format!("write raw input: {}", path.display()))?;
    Ok(())
}

#[derive(Debug)]
struct VerifySummary {
    codec_name: String,
    width: String,
    height: String,
    nb_read_frames: String,
}

fn ffprobe_summary(path: &Path, codec: Codec, expected_min_frames: usize) -> Result<VerifySummary> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,width,height,nb_read_frames",
            "-of",
            "default=noprint_wrappers=1:nokey=0",
            &path.to_string_lossy(),
        ])
        .output()
        .with_context(|| format!("run ffprobe: {}", path.display()))?;

    if !output.status.success() {
        bail!(
            "ffprobe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut codec_name = String::new();
    let mut width = String::new();
    let mut height = String::new();
    let mut nb_read_frames = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("codec_name=") {
            codec_name = v.to_string();
        } else if let Some(v) = line.strip_prefix("width=") {
            width = v.to_string();
        } else if let Some(v) = line.strip_prefix("height=") {
            height = v.to_string();
        } else if let Some(v) = line.strip_prefix("nb_read_frames=") {
            nb_read_frames = v.to_string();
        }
    }

    if codec_name.is_empty() {
        bail!("ffprobe missing codec_name for {}", path.display());
    }

    let frames = nb_read_frames.parse::<usize>().unwrap_or(0);
    if frames == 0 || frames < expected_min_frames.saturating_div(10) {
        bail!(
            "ffprobe suspicious frame count for {} (codec={}): {}",
            path.display(),
            codec.as_cli(),
            nb_read_frames
        );
    }

    Ok(VerifySummary {
        codec_name,
        width,
        height,
        nb_read_frames,
    })
}

fn run_ffmpeg_decode_verify(path: &Path, null_sink: &str) -> Result<()> {
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            &path.to_string_lossy(),
            "-f",
            "null",
            null_sink,
        ])
        .output()
        .with_context(|| format!("run ffmpeg verify decode: {}", path.display()))?;

    if !output.status.success() {
        bail!(
            "ffmpeg decode verify failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn run_timed_command(mut cmd: Command) -> Result<CaseRun> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let start = Instant::now();
    let output = cmd
        .output()
        .context("spawn command for benchmark case")?;
    let elapsed = start.elapsed().as_secs_f64();

    if !output.status.success() {
        bail!(
            "command failed (status={:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let logs = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let metrics = parse_internal_metrics(&logs);
    Ok(CaseRun {
        seconds: elapsed,
        metrics,
    })
}

#[derive(Debug, Clone, Copy)]
enum InternalMetrics {
    Decode {
        submit_ms: f64,
        elapsed_ms: f64,
        jitter_ms_mean: f64,
        jitter_ms_p95: f64,
        jitter_ms_p99: f64,
        input_copy_bytes: f64,
        output_copy_frames: f64,
    },
    Encode {
        frame_prep_ms: f64,
        submit_ms: f64,
        complete_ms: f64,
        total_ms: f64,
        queue_peak: f64,
        queue_p95: f64,
        queue_p99: f64,
        jitter_ms_mean: f64,
        jitter_ms_p95: f64,
        jitter_ms_p99: f64,
        input_copy_bytes: f64,
        output_copy_bytes: f64,
    },
}

fn parse_internal_metrics(logs: &str) -> Option<InternalMetrics> {
    let decode_line = logs
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with("[vt.decode]"));
    if let Some(line) = decode_line {
        let submit_line = logs
            .lines()
            .rev()
            .find(|l| l.trim_start().starts_with("[vt.decode.submit]"));
        let submit_ms = submit_line
            .and_then(|l| parse_metric_value(l, "submit_ms"))
            .unwrap_or(0.0);
        let input_copy_bytes = submit_line
            .and_then(|l| parse_metric_value(l, "input_copy_bytes"))
            .unwrap_or(0.0);
        let elapsed_ms = parse_metric_value(line, "elapsed_ms").unwrap_or(0.0);
        let jitter_ms_mean = parse_metric_value(line, "jitter_ms_mean").unwrap_or(0.0);
        let jitter_ms_p95 = parse_metric_value(line, "jitter_ms_p95").unwrap_or(0.0);
        let jitter_ms_p99 = parse_metric_value(line, "jitter_ms_p99").unwrap_or(0.0);
        let output_copy_frames = parse_metric_value(line, "output_copy_frames").unwrap_or(0.0);
        return Some(InternalMetrics::Decode {
            submit_ms,
            elapsed_ms,
            jitter_ms_mean,
            jitter_ms_p95,
            jitter_ms_p99,
            input_copy_bytes,
            output_copy_frames,
        });
    }

    let encode_line = logs
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with("[vt.encode]"));
    if let Some(line) = encode_line {
        let frame_prep_ms = parse_metric_value(line, "frame_prep_ms").unwrap_or(0.0);
        let submit_ms = parse_metric_value(line, "submit_ms").unwrap_or(0.0);
        let complete_ms = parse_metric_value(line, "complete_ms").unwrap_or(0.0);
        let total_ms = parse_metric_value(line, "total_ms").unwrap_or(0.0);
        let queue_peak = parse_metric_value(line, "queue_peak").unwrap_or(0.0);
        let queue_p95 = parse_metric_value(line, "queue_p95").unwrap_or(0.0);
        let queue_p99 = parse_metric_value(line, "queue_p99").unwrap_or(0.0);
        let jitter_ms_mean = parse_metric_value(line, "jitter_ms_mean").unwrap_or(0.0);
        let jitter_ms_p95 = parse_metric_value(line, "jitter_ms_p95").unwrap_or(0.0);
        let jitter_ms_p99 = parse_metric_value(line, "jitter_ms_p99").unwrap_or(0.0);
        let input_copy_bytes = parse_metric_value(line, "input_copy_bytes").unwrap_or(0.0);
        let output_copy_bytes = parse_metric_value(line, "output_copy_bytes").unwrap_or(0.0);
        return Some(InternalMetrics::Encode {
            frame_prep_ms,
            submit_ms,
            complete_ms,
            total_ms,
            queue_peak,
            queue_p95,
            queue_p99,
            jitter_ms_mean,
            jitter_ms_p95,
            jitter_ms_p99,
            input_copy_bytes,
            output_copy_bytes,
        });
    }

    None
}

fn parse_metric_value(line: &str, key: &str) -> Option<f64> {
    for token in line.split(',') {
        let t = token.trim();
        if let Some(value) = t.strip_prefix(&format!("{key}=")) {
            return value.parse::<f64>().ok();
        }
    }
    None
}

fn run_command(cmd: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<()> {
    let mut command = Command::new(cmd);
    command.args(args);
    for (k, v) in envs {
        command.env(k, v);
    }
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    let status = command
        .status()
        .with_context(|| format!("run command: {} {:?}", cmd, args))?;
    if !status.success() {
        bail!("command failed: {} {:?} (status={status})", cmd, args);
    }
    Ok(())
}

fn convert_length_prefixed_to_annexb(input: &Path, output: &Path) -> Result<()> {
    let data = fs::read(input).with_context(|| format!("read input: {}", input.display()))?;
    if data.is_empty() {
        bail!("input is empty: {}", input.display());
    }

    let mut out = Vec::with_capacity(data.len() + 1024);
    let mut offset = 0usize;
    while offset.saturating_add(4) <= data.len() {
        let len = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset = offset.saturating_add(4);
        if len == 0 || offset.saturating_add(len) > data.len() {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&data[offset..offset + len]);
        offset = offset.saturating_add(len);
    }

    if out.is_empty() {
        bail!(
            "failed to parse length-prefixed payload from {}",
            input.display()
        );
    }

    fs::write(output, out).with_context(|| format!("write output: {}", output.display()))?;
    Ok(())
}
