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

    #[arg(long, default_value_t = 65536)]
    chunk_bytes: usize,

    #[arg(long, default_value_t = 300)]
    frame_count: usize,

    #[arg(long, default_value_t = 640)]
    width: usize,

    #[arg(long, default_value_t = 360)]
    height: usize,

    #[arg(long, default_value_t = false)]
    include_internal_metrics: bool,

    #[arg(long)]
    nv_max_in_flight: Option<usize>,

    #[arg(long, default_value_t = false)]
    verify: bool,

    #[arg(long, default_value_t = false)]
    equal_raw_input: bool,
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
    map_ms: Vec<f64>,
    pack_p95_ms: Vec<f64>,
    pack_p99_ms: Vec<f64>,
    sdk_p95_ms: Vec<f64>,
    sdk_p99_ms: Vec<f64>,
    map_p95_ms: Vec<f64>,
    map_p99_ms: Vec<f64>,
    queue_depth_peak: Vec<f64>,
    queue_depth_p95: Vec<f64>,
    queue_depth_p99: Vec<f64>,
    jitter_ms_mean: Vec<f64>,
    jitter_ms_p95: Vec<f64>,
    jitter_ms_p99: Vec<f64>,
}

#[derive(Debug, Default, Clone)]
struct EncodeMetricSamples {
    synth_ms: Vec<f64>,
    upload_ms: Vec<f64>,
    submit_ms: Vec<f64>,
    reap_ms: Vec<f64>,
    encode_ms: Vec<f64>,
    lock_ms: Vec<f64>,
    queue_peak: Vec<f64>,
    queue_p95: Vec<f64>,
    queue_p99: Vec<f64>,
    jitter_ms_mean: Vec<f64>,
    jitter_ms_p95: Vec<f64>,
    jitter_ms_p99: Vec<f64>,
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
    let encode_raw_bin = example_bin_path(profile, "encode_raw_argb");
    let video_hw_output = output_dir.join(format!("video-hw-{}-precise.bin", args.codec.as_cli()));
    let ffmpeg_output = output_dir.join(format!("ffmpeg-{}-precise.bin", args.codec.as_cli()));
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
                            pack_ms,
                            sdk_ms,
                            map_ms,
                            pack_p95_ms,
                            pack_p99_ms,
                            sdk_p95_ms,
                            sdk_p99_ms,
                            map_p95_ms,
                            map_p99_ms,
                            queue_depth_peak,
                            queue_depth_p95,
                            queue_depth_p99,
                            jitter_ms_mean,
                            jitter_ms_p95,
                            jitter_ms_p99,
                        } => {
                            decode_metrics.pack_ms.push(pack_ms);
                            decode_metrics.sdk_ms.push(sdk_ms);
                            decode_metrics.map_ms.push(map_ms);
                            decode_metrics.pack_p95_ms.push(pack_p95_ms);
                            decode_metrics.pack_p99_ms.push(pack_p99_ms);
                            decode_metrics.sdk_p95_ms.push(sdk_p95_ms);
                            decode_metrics.sdk_p99_ms.push(sdk_p99_ms);
                            decode_metrics.map_p95_ms.push(map_p95_ms);
                            decode_metrics.map_p99_ms.push(map_p99_ms);
                            decode_metrics.queue_depth_peak.push(queue_depth_peak);
                            decode_metrics.queue_depth_p95.push(queue_depth_p95);
                            decode_metrics.queue_depth_p99.push(queue_depth_p99);
                            decode_metrics.jitter_ms_mean.push(jitter_ms_mean);
                            decode_metrics.jitter_ms_p95.push(jitter_ms_p95);
                            decode_metrics.jitter_ms_p99.push(jitter_ms_p99);
                        }
                        InternalMetrics::Encode {
                            synth_ms,
                            upload_ms,
                            submit_ms,
                            reap_ms,
                            encode_ms,
                            lock_ms,
                            queue_peak,
                            queue_p95,
                            queue_p99,
                            jitter_ms_mean,
                            jitter_ms_p95,
                            jitter_ms_p99,
                        } => {
                            encode_metrics.synth_ms.push(synth_ms);
                            encode_metrics.upload_ms.push(upload_ms);
                            encode_metrics.submit_ms.push(submit_ms);
                            encode_metrics.reap_ms.push(reap_ms);
                            encode_metrics.encode_ms.push(encode_ms);
                            encode_metrics.lock_ms.push(lock_ms);
                            encode_metrics.queue_peak.push(queue_peak);
                            encode_metrics.queue_p95.push(queue_p95);
                            encode_metrics.queue_p99.push(queue_p99);
                            encode_metrics.jitter_ms_mean.push(jitter_ms_mean);
                            encode_metrics.jitter_ms_p95.push(jitter_ms_p95);
                            encode_metrics.jitter_ms_p99.push(jitter_ms_p99);
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
    writeln!(&mut report, "width: {}", args.width)?;
    writeln!(&mut report, "height: {}", args.height)?;
    writeln!(&mut report, "equal_raw_input: {}", args.equal_raw_input)?;
    writeln!(
        &mut report,
        "internal_metrics: {}",
        args.include_internal_metrics
    )?;
    writeln!(&mut report, "verify: {}", args.verify)?;
    writeln!(&mut report)?;
    writeln!(
        &mut report,
        "| Case | min(s) | mean(s) | p50(s) | p95(s) | p99(s) | max(s) | stddev(s) | CV(%) |"
    )?;
    writeln!(
        &mut report,
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|"
    )?;
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
        if !decode_metrics.pack_ms.is_empty() {
            let pack = Stats::from_samples(&decode_metrics.pack_ms);
            let sdk = Stats::from_samples(&decode_metrics.sdk_ms);
            let map = Stats::from_samples(&decode_metrics.map_ms);
            let pack_p95 = Stats::from_samples(&decode_metrics.pack_p95_ms);
            let pack_p99 = Stats::from_samples(&decode_metrics.pack_p99_ms);
            let sdk_p95 = Stats::from_samples(&decode_metrics.sdk_p95_ms);
            let sdk_p99 = Stats::from_samples(&decode_metrics.sdk_p99_ms);
            let map_p95 = Stats::from_samples(&decode_metrics.map_p95_ms);
            let map_p99 = Stats::from_samples(&decode_metrics.map_p99_ms);
            let queue_peak = Stats::from_samples(&decode_metrics.queue_depth_peak);
            let queue_p95 = Stats::from_samples(&decode_metrics.queue_depth_p95);
            let queue_p99 = Stats::from_samples(&decode_metrics.queue_depth_p99);
            let jitter_mean = Stats::from_samples(&decode_metrics.jitter_ms_mean);
            let jitter_p95 = Stats::from_samples(&decode_metrics.jitter_ms_p95);
            let jitter_p99 = Stats::from_samples(&decode_metrics.jitter_ms_p99);
            writeln!(&mut report, "### decode")?;
            writeln!(
                &mut report,
                "- pack_ms mean={:.3}, p95={:.3}, p99={:.3}",
                pack.mean, pack.p95, pack.p99
            )?;
            writeln!(
                &mut report,
                "- sdk_ms mean={:.3}, p95={:.3}, p99={:.3}",
                sdk.mean, sdk.p95, sdk.p99
            )?;
            writeln!(
                &mut report,
                "- map_ms mean={:.3}, p95={:.3}, p99={:.3}",
                map.mean, map.p95, map.p99
            )?;
            writeln!(
                &mut report,
                "- pack_stage_p95_ms mean={:.3}, pack_stage_p99_ms mean={:.3}",
                pack_p95.mean, pack_p99.mean
            )?;
            writeln!(
                &mut report,
                "- sdk_stage_p95_ms mean={:.3}, sdk_stage_p99_ms mean={:.3}",
                sdk_p95.mean, sdk_p99.mean
            )?;
            writeln!(
                &mut report,
                "- map_stage_p95_ms mean={:.3}, map_stage_p99_ms mean={:.3}",
                map_p95.mean, map_p99.mean
            )?;
            writeln!(
                &mut report,
                "- queue_depth_peak mean={:.3}, p95={:.3}, p99={:.3}",
                queue_peak.mean, queue_peak.p95, queue_peak.p99
            )?;
            writeln!(
                &mut report,
                "- queue_depth_p95 mean={:.3}, queue_depth_p99 mean={:.3}",
                queue_p95.mean, queue_p99.mean
            )?;
            writeln!(
                &mut report,
                "- jitter_ms_mean mean={:.3}, jitter_ms_p95 mean={:.3}, jitter_ms_p99 mean={:.3}",
                jitter_mean.mean, jitter_p95.mean, jitter_p99.mean
            )?;
        }
        if !encode_metrics.synth_ms.is_empty() {
            let synth = Stats::from_samples(&encode_metrics.synth_ms);
            let upload = Stats::from_samples(&encode_metrics.upload_ms);
            let submit = Stats::from_samples(&encode_metrics.submit_ms);
            let reap = Stats::from_samples(&encode_metrics.reap_ms);
            let encode = Stats::from_samples(&encode_metrics.encode_ms);
            let lock = Stats::from_samples(&encode_metrics.lock_ms);
            let queue = Stats::from_samples(&encode_metrics.queue_peak);
            let queue_p95 = Stats::from_samples(&encode_metrics.queue_p95);
            let queue_p99 = Stats::from_samples(&encode_metrics.queue_p99);
            let jitter_mean = Stats::from_samples(&encode_metrics.jitter_ms_mean);
            let jitter_p95 = Stats::from_samples(&encode_metrics.jitter_ms_p95);
            let jitter_p99 = Stats::from_samples(&encode_metrics.jitter_ms_p99);
            writeln!(&mut report, "### encode")?;
            writeln!(
                &mut report,
                "- synth_ms mean={:.3}, p95={:.3}, p99={:.3}",
                synth.mean, synth.p95, synth.p99
            )?;
            writeln!(
                &mut report,
                "- upload_ms mean={:.3}, p95={:.3}, p99={:.3}",
                upload.mean, upload.p95, upload.p99
            )?;
            writeln!(
                &mut report,
                "- submit_ms mean={:.3}, p95={:.3}, p99={:.3}",
                submit.mean, submit.p95, submit.p99
            )?;
            writeln!(
                &mut report,
                "- reap_ms mean={:.3}, p95={:.3}, p99={:.3}",
                reap.mean, reap.p95, reap.p99
            )?;
            writeln!(
                &mut report,
                "- encode_ms mean={:.3}, p95={:.3}, p99={:.3}",
                encode.mean, encode.p95, encode.p99
            )?;
            writeln!(
                &mut report,
                "- lock_ms mean={:.3}, p95={:.3}, p99={:.3}",
                lock.mean, lock.p95, lock.p99
            )?;
            writeln!(
                &mut report,
                "- queue_peak mean={:.3}, p95={:.3}, p99={:.3}",
                queue.mean, queue.p95, queue.p99
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
        }
    }

    if args.verify {
        writeln!(&mut report)?;
        writeln!(&mut report, "## Verification")?;
        let verify_items = [
            ("video-hw", video_hw_output.as_path()),
            ("ffmpeg", ffmpeg_output.as_path()),
        ];
        for (label, path) in verify_items {
            let summary = ffprobe_summary(path, args.codec, args.frame_count)?;
            run_ffmpeg_decode_verify(path, null_sink)?;
            writeln!(
                &mut report,
                "- {}: codec={}, {}x{}, frames={} (decode=ok)",
                label, summary.codec_name, summary.width, summary.height, summary.nb_read_frames
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
            let mut cmd = if args.equal_raw_input {
                let mut c = Command::new(encode_raw_bin);
                c.args([
                    "--backend",
                    "nv",
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
                    "--require-hardware",
                    "--output",
                    &video_hw_output.to_string_lossy(),
                ]);
                c
            } else {
                let mut c = Command::new(encode_bin);
                c.args([
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
                c
            };
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
            if args.equal_raw_input {
                cmd.args([
                    "-y",
                    "-hide_banner",
                    "-benchmark",
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
                    "-preset",
                    "p1",
                    "-f",
                    muxer,
                    &ffmpeg_output.to_string_lossy(),
                ]);
            } else {
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
            }
            run_timed_command(cmd, true)
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

    for frame_index in 0..frame_count {
        let frame_base = frame_index * frame_size;
        for y in 0..height {
            for x in 0..width {
                let offset = frame_base + (y * width + x) * 4;
                out[offset] = ((x + frame_index) % 256) as u8;
                out[offset + 1] = ((y + frame_index * 2) % 256) as u8;
                out[offset + 2] = ((frame_index * 5) % 256) as u8;
                out[offset + 3] = 255;
            }
        }
    }
    fs::write(path, out).with_context(|| format!("write raw input: {}", path.display()))?;
    Ok(())
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
        map_ms: f64,
        pack_p95_ms: f64,
        pack_p99_ms: f64,
        sdk_p95_ms: f64,
        sdk_p99_ms: f64,
        map_p95_ms: f64,
        map_p99_ms: f64,
        queue_depth_peak: f64,
        queue_depth_p95: f64,
        queue_depth_p99: f64,
        jitter_ms_mean: f64,
        jitter_ms_p95: f64,
        jitter_ms_p99: f64,
    },
    Encode {
        synth_ms: f64,
        upload_ms: f64,
        submit_ms: f64,
        reap_ms: f64,
        encode_ms: f64,
        lock_ms: f64,
        queue_peak: f64,
        queue_p95: f64,
        queue_p99: f64,
        jitter_ms_mean: f64,
        jitter_ms_p95: f64,
        jitter_ms_p99: f64,
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
        let map_ms = parse_metric_value(line, "map_ms").unwrap_or(0.0);
        let pack_p95_ms = parse_metric_value(line, "pack_p95_ms").unwrap_or(0.0);
        let pack_p99_ms = parse_metric_value(line, "pack_p99_ms").unwrap_or(0.0);
        let sdk_p95_ms = parse_metric_value(line, "sdk_p95_ms").unwrap_or(0.0);
        let sdk_p99_ms = parse_metric_value(line, "sdk_p99_ms").unwrap_or(0.0);
        let map_p95_ms = parse_metric_value(line, "map_p95_ms").unwrap_or(0.0);
        let map_p99_ms = parse_metric_value(line, "map_p99_ms").unwrap_or(0.0);
        let queue_depth_peak = parse_metric_value(line, "queue_depth_peak").unwrap_or(0.0);
        let queue_depth_p95 = parse_metric_value(line, "queue_depth_p95").unwrap_or(0.0);
        let queue_depth_p99 = parse_metric_value(line, "queue_depth_p99").unwrap_or(0.0);
        let jitter_ms_mean = parse_metric_value(line, "jitter_ms_mean").unwrap_or(0.0);
        let jitter_ms_p95 = parse_metric_value(line, "jitter_ms_p95").unwrap_or(0.0);
        let jitter_ms_p99 = parse_metric_value(line, "jitter_ms_p99").unwrap_or(0.0);
        return Some(InternalMetrics::Decode {
            pack_ms,
            sdk_ms,
            map_ms,
            pack_p95_ms,
            pack_p99_ms,
            sdk_p95_ms,
            sdk_p99_ms,
            map_p95_ms,
            map_p99_ms,
            queue_depth_peak,
            queue_depth_p95,
            queue_depth_p99,
            jitter_ms_mean,
            jitter_ms_p95,
            jitter_ms_p99,
        });
    }

    let encode_line = logs
        .lines()
        .find(|line| line.trim_start().starts_with("[nv.encode]"));
    if let Some(line) = encode_line {
        let synth_ms = parse_metric_value(line, "synth_ms")?;
        let upload_ms = parse_metric_value(line, "upload_ms")?;
        let encode_ms = parse_metric_value(line, "encode_ms")
            .or_else(|| parse_metric_value(line, "submit_ms"))?;
        let submit_ms = parse_metric_value(line, "submit_ms").unwrap_or(encode_ms);
        let reap_ms = parse_metric_value(line, "reap_ms")
            .or_else(|| parse_metric_value(line, "lock_ms"))
            .unwrap_or(0.0);
        let lock_ms = parse_metric_value(line, "lock_ms").unwrap_or(reap_ms);
        let queue_peak = parse_metric_value(line, "queue_peak")?;
        let queue_p95 = parse_metric_value(line, "queue_p95").unwrap_or(queue_peak);
        let queue_p99 = parse_metric_value(line, "queue_p99").unwrap_or(queue_peak);
        let jitter_ms_mean = parse_metric_value(line, "jitter_ms_mean").unwrap_or(0.0);
        let jitter_ms_p95 = parse_metric_value(line, "jitter_ms_p95").unwrap_or(0.0);
        let jitter_ms_p99 = parse_metric_value(line, "jitter_ms_p99").unwrap_or(0.0);
        return Some(InternalMetrics::Encode {
            synth_ms,
            upload_ms,
            submit_ms,
            reap_ms,
            encode_ms,
            lock_ms,
            queue_peak,
            queue_p95,
            queue_p99,
            jitter_ms_mean,
            jitter_ms_p95,
            jitter_ms_p99,
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

#[derive(Debug, Clone)]
struct ProbeSummary {
    codec_name: String,
    width: usize,
    height: usize,
    nb_read_frames: usize,
}

fn ffprobe_summary(path: &Path, codec: Codec, expected_frames: usize) -> Result<ProbeSummary> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_entries",
            "stream=codec_name,width,height,nb_read_frames",
            "-of",
            "default=noprint_wrappers=1",
            &path.to_string_lossy(),
        ])
        .output()
        .with_context(|| format!("spawn ffprobe for {}", path.display()))?;
    if !output.status.success() {
        bail!("ffprobe failed for {}: status={}", path.display(), output.status);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut codec_name = None;
    let mut width = None;
    let mut height = None;
    let mut nb_read_frames = None;
    for line in text.lines() {
        let mut parts = line.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("").trim();
        match key {
            "codec_name" => codec_name = Some(value.to_string()),
            "width" => width = value.parse::<usize>().ok(),
            "height" => height = value.parse::<usize>().ok(),
            "nb_read_frames" => nb_read_frames = value.parse::<usize>().ok(),
            _ => {}
        }
    }

    let summary = ProbeSummary {
        codec_name: codec_name.unwrap_or_default(),
        width: width.unwrap_or(0),
        height: height.unwrap_or(0),
        nb_read_frames: nb_read_frames.unwrap_or(0),
    };

    let expected_codec = codec.as_cli();
    if summary.codec_name != expected_codec {
        bail!(
            "unexpected codec for {}: expected {}, got {}",
            path.display(),
            expected_codec,
            summary.codec_name
        );
    }
    if summary.nb_read_frames != expected_frames {
        bail!(
            "unexpected frame count for {}: expected {}, got {}",
            path.display(),
            expected_frames,
            summary.nb_read_frames
        );
    }
    if summary.width == 0 || summary.height == 0 {
        bail!("unexpected dimensions for {}", path.display());
    }

    Ok(summary)
}

fn run_ffmpeg_decode_verify(path: &Path, null_sink: &str) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-i", &path.to_string_lossy(), "-f", "null", null_sink])
        .status()
        .with_context(|| format!("spawn ffmpeg decode verify for {}", path.display()))?;
    if !status.success() {
        bail!(
            "ffmpeg decode verify failed for {}: status={status}",
            path.display()
        );
    }
    Ok(())
}
