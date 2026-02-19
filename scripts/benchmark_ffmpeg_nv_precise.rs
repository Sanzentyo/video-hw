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

    fn ffmpeg_decode_codec(self) -> &'static str {
        match self {
            Self::H264 => "h264_cuvid",
            Self::Hevc => "hevc_cuvid",
        }
    }

    fn ffmpeg_encode_codec(self) -> &'static str {
        match self {
            Self::H264 => "h264_nvenc",
            Self::Hevc => "hevc_nvenc",
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
#[command(about = "Precise repeated benchmark for video-hw (NV) vs ffmpeg (NV)")]
struct Args {
    #[arg(long, value_enum, default_value_t = Codec::H264)]
    codec: Codec,

    #[arg(long, default_value_t = true)]
    release: bool,

    #[arg(long, default_value_t = 1)]
    warmup: usize,

    #[arg(long, default_value_t = 7)]
    repeat: usize,

    #[arg(long, default_value_t = 4096)]
    chunk_bytes: usize,

    #[arg(long, default_value_t = 300)]
    frame_count: usize,

    #[arg(long, default_value_t = false)]
    include_internal_metrics: bool,

    #[arg(long)]
    nv_max_in_flight: Option<usize>,
}

#[derive(Debug, Clone)]
struct CaseSamples {
    case: Case,
    seconds: Vec<f64>,
}

#[derive(Debug, Default, Clone)]
struct DecodeMetricSamples {
    pack_ms: Vec<f64>,
    sdk_ms: Vec<f64>,
}

#[derive(Debug, Default, Clone)]
struct EncodeMetricSamples {
    synth_ms: Vec<f64>,
    upload_ms: Vec<f64>,
    encode_ms: Vec<f64>,
    lock_ms: Vec<f64>,
    queue_peak: Vec<f64>,
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
            stddev,
            cv_percent,
        }
    }
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
    let video_hw_output = output_dir.join(format!("video-hw-{}-precise.bin", args.codec.as_cli()));
    let ffmpeg_output = output_dir.join(format!("ffmpeg-{}-precise.bin", args.codec.as_cli()));
    let null_sink = if cfg!(windows) { "NUL" } else { "/dev/null" };

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
                &video_hw_output,
                &ffmpeg_output,
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
                        InternalMetrics::Decode { pack_ms, sdk_ms } => {
                            decode_metrics.pack_ms.push(pack_ms);
                            decode_metrics.sdk_ms.push(sdk_ms);
                        }
                        InternalMetrics::Encode {
                            synth_ms,
                            upload_ms,
                            encode_ms,
                            lock_ms,
                            queue_peak,
                        } => {
                            encode_metrics.synth_ms.push(synth_ms);
                            encode_metrics.upload_ms.push(upload_ms);
                            encode_metrics.encode_ms.push(encode_ms);
                            encode_metrics.lock_ms.push(lock_ms);
                            encode_metrics.queue_peak.push(queue_peak);
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
        "benchmark-nv-precise-{}-{}.md",
        args.codec.as_cli(),
        now_secs
    ));

    let mut report = String::new();
    writeln!(&mut report, "# NV Precise Benchmark Report")?;
    writeln!(&mut report, "epoch_seconds: {now_secs}")?;
    writeln!(&mut report, "codec: {}", args.codec.as_cli())?;
    writeln!(&mut report, "warmup: {}", args.warmup)?;
    writeln!(&mut report, "repeat: {}", args.repeat)?;
    writeln!(
        &mut report,
        "internal_metrics: {}",
        args.include_internal_metrics
    )?;
    writeln!(&mut report)?;
    writeln!(
        &mut report,
        "| Case | min(s) | mean(s) | p50(s) | p95(s) | max(s) | stddev(s) | CV(%) |"
    )?;
    writeln!(
        &mut report,
        "|---|---:|---:|---:|---:|---:|---:|---:|"
    )?;
    for case_samples in &samples {
        let s = case_samples.summarize();
        writeln!(
            &mut report,
            "| {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.2} |",
            case_samples.case.label(),
            s.min,
            s.mean,
            s.p50,
            s.p95,
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
        if !decode_metrics.pack_ms.is_empty() {
            let pack = Stats::from_samples(&decode_metrics.pack_ms);
            let sdk = Stats::from_samples(&decode_metrics.sdk_ms);
            writeln!(&mut report, "### decode")?;
            writeln!(&mut report, "- pack_ms mean={:.3}, p95={:.3}", pack.mean, pack.p95)?;
            writeln!(&mut report, "- sdk_ms mean={:.3}, p95={:.3}", sdk.mean, sdk.p95)?;
        }
        if !encode_metrics.synth_ms.is_empty() {
            let synth = Stats::from_samples(&encode_metrics.synth_ms);
            let upload = Stats::from_samples(&encode_metrics.upload_ms);
            let encode = Stats::from_samples(&encode_metrics.encode_ms);
            let lock = Stats::from_samples(&encode_metrics.lock_ms);
            let queue = Stats::from_samples(&encode_metrics.queue_peak);
            writeln!(&mut report, "### encode")?;
            writeln!(&mut report, "- synth_ms mean={:.3}, p95={:.3}", synth.mean, synth.p95)?;
            writeln!(
                &mut report,
                "- upload_ms mean={:.3}, p95={:.3}",
                upload.mean, upload.p95
            )?;
            writeln!(
                &mut report,
                "- encode_ms mean={:.3}, p95={:.3}",
                encode.mean, encode.p95
            )?;
            writeln!(&mut report, "- lock_ms mean={:.3}, p95={:.3}", lock.mean, lock.p95)?;
            writeln!(
                &mut report,
                "- queue_peak mean={:.3}, p95={:.3}",
                queue.mean, queue.p95
            )?;
        }
    }

    fs::write(&report_path, report)
        .with_context(|| format!("write report: {}", report_path.display()))?;
    println!("saved report: {}", report_path.display());
    Ok(())
}

fn build_examples(profile: &str) -> Result<()> {
    let mut args = vec![
        "build",
        "--features",
        "backend-nvidia",
        "--examples",
        "--profile",
        profile,
    ];
    if profile == "release" {
        args = vec!["build", "--features", "backend-nvidia", "--examples", "--release"];
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
    video_hw_output: &Path,
    ffmpeg_output: &Path,
    null_sink: &str,
) -> Result<CaseRun> {
    match case {
        Case::VideoHwDecode => {
            let mut cmd = Command::new(decode_bin);
            cmd.args([
                "--backend",
                "nv",
                "--codec",
                args.codec.as_cli(),
                "--input",
                args.codec.sample_input(),
                "--chunk-bytes",
                &args.chunk_bytes.to_string(),
                "--require-hardware",
            ]);
            if args.include_internal_metrics {
                cmd.env("VIDEO_HW_NV_METRICS", "1");
            }
            run_timed_command(cmd, !args.include_internal_metrics)
        }
        Case::VideoHwEncode => {
            let mut cmd = Command::new(encode_bin);
            cmd.args([
                "--backend",
                "nv",
                "--codec",
                args.codec.as_cli(),
                "--fps",
                "30",
                "--frame-count",
                &args.frame_count.to_string(),
                "--require-hardware",
                "--output",
                &video_hw_output.to_string_lossy(),
            ]);
            if let Some(value) = args.nv_max_in_flight {
                cmd.args(["--nv-max-in-flight", &value.to_string()]);
            }
            if args.include_internal_metrics {
                cmd.env("VIDEO_HW_NV_METRICS", "1");
            }
            run_timed_command(cmd, !args.include_internal_metrics)
        }
        Case::FfmpegDecode => {
            let mut cmd = Command::new("ffmpeg");
            cmd.args([
                "-y",
                "-hide_banner",
                "-benchmark",
                "-hwaccel",
                "cuda",
                "-c:v",
                args.codec.ffmpeg_decode_codec(),
                "-i",
                args.codec.sample_input(),
                "-f",
                "null",
                null_sink,
            ]);
            run_timed_command(cmd, true)
        }
        Case::FfmpegEncode => {
            let muxer = match args.codec {
                Codec::H264 => "h264",
                Codec::Hevc => "hevc",
            };
            let mut cmd = Command::new("ffmpeg");
            cmd.args([
                "-y",
                "-hide_banner",
                "-benchmark",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x360:rate=30",
                "-frames:v",
                &args.frame_count.to_string(),
                "-c:v",
                args.codec.ffmpeg_encode_codec(),
                "-preset",
                "p1",
                "-f",
                muxer,
                &ffmpeg_output.to_string_lossy(),
            ]);
            run_timed_command(cmd, true)
        }
    }
}

fn run_command(program: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<()> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn command: {program} {}", args.join(" ")))?;
    if !status.success() {
        bail!("command failed: {program} (status={status})");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum InternalMetrics {
    Decode {
        pack_ms: f64,
        sdk_ms: f64,
    },
    Encode {
        synth_ms: f64,
        upload_ms: f64,
        encode_ms: f64,
        lock_ms: f64,
        queue_peak: f64,
    },
}

#[derive(Debug, Clone, Copy)]
struct CaseRun {
    seconds: f64,
    metrics: Option<InternalMetrics>,
}

fn run_timed_command(mut cmd: Command, quiet: bool) -> Result<CaseRun> {
    let start = Instant::now();
    if quiet {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let output = cmd.output().context("spawn benchmark command")?;
    if !output.status.success() {
        bail!("benchmark command failed: status={}", output.status);
    }
    let seconds = start.elapsed().as_secs_f64();
    let logs = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let metrics = parse_internal_metrics(&logs);
    Ok(CaseRun { seconds, metrics })
}

fn parse_internal_metrics(logs: &str) -> Option<InternalMetrics> {
    let decode_line = logs
        .lines()
        .find(|line| line.trim_start().starts_with("[nv.decode]"));
    if let Some(line) = decode_line {
        let pack_ms = parse_metric_value(line, "pack_ms")?;
        let sdk_ms = parse_metric_value(line, "sdk_ms")?;
        return Some(InternalMetrics::Decode { pack_ms, sdk_ms });
    }

    let encode_line = logs
        .lines()
        .find(|line| line.trim_start().starts_with("[nv.encode]"));
    if let Some(line) = encode_line {
        let synth_ms = parse_metric_value(line, "synth_ms")?;
        let upload_ms = parse_metric_value(line, "upload_ms")?;
        let encode_ms = parse_metric_value(line, "encode_ms")?;
        let lock_ms = parse_metric_value(line, "lock_ms")?;
        let queue_peak = parse_metric_value(line, "queue_peak")?;
        return Some(InternalMetrics::Encode {
            synth_ms,
            upload_ms,
            encode_ms,
            lock_ms,
            queue_peak,
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
