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
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
enum Codec {
    #[default]
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

    fn ffmpeg_decode_codec(self, hardware: bool) -> &'static str {
        if hardware {
            return match self {
                Self::H264 => "h264_qsv",
                Self::Hevc => "hevc_qsv",
            };
        }
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
        }
    }

    fn ffmpeg_encode_codec(self, hardware: bool) -> &'static str {
        if hardware {
            return match self {
                Self::H264 => "h264_qsv",
                Self::Hevc => "hevc_qsv",
            };
        }
        match self {
            Self::H264 => "libx264",
            Self::Hevc => "libx265",
        }
    }

    fn muxer(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
enum RawInputPixFmt {
    #[default]
    Argb,
    Nv12,
}

impl RawInputPixFmt {
    fn as_cli(self) -> &'static str {
        match self {
            Self::Argb => "argb",
            Self::Nv12 => "nv12",
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
#[command(about = "Precise repeated benchmark for video-hw (Intel oneVPL) vs ffmpeg (QSV)")]
struct Args {
    #[arg(long, value_enum, default_value_t = Codec::H264)]
    codec: Codec,

    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    release: bool,

    #[arg(long, default_value_t = 2)]
    warmup: usize,

    #[arg(long, default_value_t = 7)]
    repeat: usize,

    #[arg(long, default_value_t = 65_536)]
    chunk_bytes: usize,

    #[arg(long, default_value = "metadata")]
    decode_output_mode: String,

    #[arg(long, default_value_t = 10)]
    decode_loops: usize,

    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    require_hardware: bool,

    #[arg(long, default_value_t = 300)]
    frame_count: usize,

    #[arg(long, default_value_t = 640)]
    width: usize,

    #[arg(long, default_value_t = 360)]
    height: usize,

    #[arg(long, default_value_t = false)]
    verify: bool,

    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    equal_raw_input: bool,

    #[arg(long, value_enum, default_value_t = RawInputPixFmt::Argb)]
    raw_input_pix_fmt: RawInputPixFmt,

    #[arg(long, default_value_t = false)]
    allow_case_failures: bool,

    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    grouped_cases: bool,

    #[arg(long, default_value_t = 0)]
    settle_ms: u64,

    #[arg(long)]
    intel_decode_async_depth: Option<u16>,

    #[arg(long, default_value_t = false)]
    intel_force_software: bool,
}

#[derive(Debug, Clone)]
struct CaseSamples {
    case: Case,
    seconds: Vec<f64>,
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
        let cv_percent = if mean > 0.0 {
            (stddev / mean) * 100.0
        } else {
            0.0
        };

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

#[derive(Debug, Clone, Copy)]
struct CaseRun {
    seconds: f64,
}

#[derive(Debug, Clone)]
struct ProbeSummary {
    codec_name: String,
    width: usize,
    height: usize,
    nb_read_frames: usize,
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
    if args_missing_windows_or_linux_guard() {
        bail!("this benchmark is intended for Windows/Linux (Intel oneVPL + ffmpeg QSV)");
    }

    let args = Args::parse();
    if args.repeat == 0 {
        bail!("--repeat must be >= 1");
    }
    if args.decode_loops == 0 {
        bail!("--decode-loops must be >= 1");
    }
    if let Some(depth) = args.intel_decode_async_depth
        && !(1..=16).contains(&depth)
    {
        bail!("--intel-decode-async-depth must be in 1..=16");
    }
    if args.intel_force_software && args.require_hardware {
        bail!("--intel-force-software requires --require-hardware false");
    }

    let profile = if args.release { "release" } else { "debug" };
    let output_dir = PathBuf::from("output");
    fs::create_dir_all(&output_dir).context("create output directory")?;

    build_examples(profile, args.raw_input_pix_fmt)?;

    let decode_bin = example_bin_path(profile, "decode_annexb");
    let encode_bin = example_bin_path(profile, "encode_synthetic");
    let encode_raw_bin = example_bin_path(profile, "encode_raw_argb");
    let decode_input = prepare_decode_input(args.codec, args.decode_loops, &output_dir)?;
    let video_hw_output = output_dir.join(format!("video-hw-intel-{}-precise.bin", args.codec.as_cli()));
    let ffmpeg_output = output_dir.join(format!("ffmpeg-qsv-{}-precise.bin", args.codec.as_cli()));
    let raw_input = output_dir.join(format!(
        "benchmark-input-{}-{}x{}-{}f.raw",
        args.raw_input_pix_fmt.as_cli(),
        args.width,
        args.height,
        args.frame_count
    ));
    let null_sink = if cfg!(windows) { "NUL" } else { "/dev/null" };

    if args.equal_raw_input {
        write_raw_input(
            &raw_input,
            args.raw_input_pix_fmt,
            args.width,
            args.height,
            args.frame_count,
        )?;
    }

    let decode_frames = probe_stream_frame_count(&decode_input.to_string_lossy()).unwrap_or(args.frame_count);

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
    let mut failures = Vec::<String>::new();

    let total_rounds = args.warmup + args.repeat;
    if args.grouped_cases {
        for case in &cases {
            println!("case={}, mode=grouped", case.label());
            for i in 0..total_rounds {
                let is_warmup = i < args.warmup;
                let round = i + 1;
                let label = if is_warmup { "warmup" } else { "measure" };
                println!("  round {round}/{total_rounds}, phase={label}");

                let run = match run_case(
                    *case,
                    &args,
                    &decode_bin,
                    &decode_input,
                    &encode_bin,
                    &encode_raw_bin,
                    &video_hw_output,
                    &ffmpeg_output,
                    &raw_input,
                    null_sink,
                ) {
                    Ok(run) => run,
                    Err(err) => {
                        let message = format!(
                            "case {} round {round} failed: {err}",
                            case.label()
                        );
                        if args.allow_case_failures {
                            println!("    failed ({err})");
                            if !is_warmup {
                                failures.push(message);
                            }
                            continue;
                        }
                        return Err(err);
                    }
                };
                println!("    {:.3}s", run.seconds);
                if !is_warmup {
                    samples[case_index(*case)].push(run.seconds);
                }
                if args.settle_ms > 0 {
                    std::thread::sleep(Duration::from_millis(args.settle_ms));
                }
            }
        }
    } else {
        for i in 0..total_rounds {
            let is_warmup = i < args.warmup;
            let round = i + 1;
            let label = if is_warmup { "warmup" } else { "measure" };
            println!("round {round}/{total_rounds}, phase={label}");

            for case in &cases {
                let run = match run_case(
                    *case,
                    &args,
                    &decode_bin,
                    &decode_input,
                    &encode_bin,
                    &encode_raw_bin,
                    &video_hw_output,
                    &ffmpeg_output,
                    &raw_input,
                    null_sink,
                ) {
                    Ok(run) => run,
                    Err(err) => {
                        let message = format!("round {round} {} failed: {err}", case.label());
                        if args.allow_case_failures {
                            println!("  {:<16} failed ({err})", case.label());
                            if !is_warmup {
                                failures.push(message);
                            }
                            continue;
                        }
                        return Err(err);
                    }
                };
                println!("  {:<16} {:.3}s", case.label(), run.seconds);
                if !is_warmup {
                    samples[case_index(*case)].push(run.seconds);
                }
                if args.settle_ms > 0 {
                    std::thread::sleep(Duration::from_millis(args.settle_ms));
                }
            }
        }
    }

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before UNIX_EPOCH")?
        .as_secs();
    let report_path = output_dir.join(format!(
        "benchmark-intel-precise-{}-{}.md",
        args.codec.as_cli(),
        now_secs
    ));

    let decode_video_hw = samples[0].summarize();
    let encode_video_hw = samples[1].summarize();
    let decode_ffmpeg = samples[2].summarize();
    let encode_ffmpeg = samples[3].summarize();

    let decode_parity = if !samples[0].seconds.is_empty() && !samples[2].seconds.is_empty() {
        let video_hw_fps = throughput_fps(decode_frames, decode_video_hw.mean);
        let ffmpeg_fps = throughput_fps(decode_frames, decode_ffmpeg.mean);
        let delta_percent = percent_delta(video_hw_fps, ffmpeg_fps);
        let pass = delta_percent.abs() <= 10.0;
        Some((video_hw_fps, ffmpeg_fps, delta_percent, pass))
    } else {
        None
    };

    let encode_parity = if !samples[1].seconds.is_empty() && !samples[3].seconds.is_empty() {
        let video_hw_fps = throughput_fps(args.frame_count, encode_video_hw.mean);
        let ffmpeg_fps = throughput_fps(args.frame_count, encode_ffmpeg.mean);
        let delta_percent = percent_delta(video_hw_fps, ffmpeg_fps);
        let pass = delta_percent.abs() <= 10.0;
        Some((video_hw_fps, ffmpeg_fps, delta_percent, pass))
    } else {
        None
    };

    let mut report = String::new();
    writeln!(&mut report, "# Intel Precise Benchmark Report")?;
    writeln!(&mut report, "epoch_seconds: {now_secs}")?;
    writeln!(&mut report, "codec: {}", args.codec.as_cli())?;
    writeln!(&mut report, "warmup: {}", args.warmup)?;
    writeln!(&mut report, "repeat: {}", args.repeat)?;
    writeln!(&mut report, "width: {}", args.width)?;
    writeln!(&mut report, "height: {}", args.height)?;
    writeln!(&mut report, "decode_frames: {decode_frames}")?;
    writeln!(&mut report, "decode_loops: {}", args.decode_loops)?;
    writeln!(&mut report, "decode_output_mode: {}", args.decode_output_mode)?;
    writeln!(&mut report, "encode_frames: {}", args.frame_count)?;
    writeln!(&mut report, "require_hardware: {}", args.require_hardware)?;
    writeln!(
        &mut report,
        "intel_force_software: {}",
        args.intel_force_software
    )?;
    writeln!(
        &mut report,
        "intel_decode_async_depth: {}",
        args.intel_decode_async_depth
            .map(|depth| depth.to_string())
            .unwrap_or_else(|| "backend default (16)".to_string())
    )?;
    writeln!(&mut report, "equal_raw_input: {}", args.equal_raw_input)?;
    writeln!(
        &mut report,
        "raw_input_pix_fmt: {}",
        args.raw_input_pix_fmt.as_cli()
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
        if case_samples.seconds.is_empty() {
            writeln!(
                &mut report,
                "| {} | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |",
                case_samples.case.label()
            )?;
        } else {
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
    }
    writeln!(&mut report)?;
    writeln!(&mut report, "## Parity Check (mean throughput)")?;
    writeln!(
        &mut report,
        "- Target: average throughput delta within ±10% vs ffmpeg"
    )?;
    match decode_parity {
        Some((video_hw_fps, ffmpeg_fps, delta_percent, pass)) => writeln!(
            &mut report,
            "- Decode: video-hw={:.2} fps, ffmpeg={:.2} fps, delta={:+.2}% => {}",
            video_hw_fps,
            ffmpeg_fps,
            delta_percent,
            pass_fail(pass)
        )?,
        None => writeln!(
            &mut report,
            "- Decode: unavailable (missing measured samples due runtime failures)"
        )?,
    }
    match encode_parity {
        Some((video_hw_fps, ffmpeg_fps, delta_percent, pass)) => writeln!(
            &mut report,
            "- Encode: video-hw={:.2} fps, ffmpeg={:.2} fps, delta={:+.2}% => {}",
            video_hw_fps,
            ffmpeg_fps,
            delta_percent,
            pass_fail(pass)
        )?,
        None => writeln!(
            &mut report,
            "- Encode: unavailable (missing measured samples due runtime failures)"
        )?,
    }
    let overall_status = match (decode_parity, encode_parity) {
        (Some((_, _, _, decode_ok)), Some((_, _, _, encode_ok))) if decode_ok && encode_ok => {
            "PASS (both encode/decode within ±10%)"
        }
        (Some((_, _, _, _)), Some((_, _, _, _))) => {
            "FAIL (at least one path exceeds ±10%)"
        }
        _ => "BLOCKED (at least one comparison could not be measured)",
    };
    writeln!(&mut report, "- Overall: {overall_status}")?;

    if !failures.is_empty() {
        writeln!(&mut report)?;
        writeln!(&mut report, "## Runtime Failures")?;
        for failure in &failures {
            writeln!(&mut report, "- {failure}")?;
        }
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

    if args.verify {
        writeln!(&mut report)?;
        writeln!(&mut report, "## Verification")?;
        let verify_items = [
            ("video-hw", video_hw_output.as_path()),
            ("ffmpeg", ffmpeg_output.as_path()),
        ];
        for (label, path) in verify_items {
            if !path.exists() {
                if args.allow_case_failures {
                    writeln!(
                        &mut report,
                        "- {label}: skipped (missing output due earlier runtime failure: {})",
                        path.display()
                    )?;
                    continue;
                }
                bail!("verification output missing: {}", path.display());
            }
            let summary = match ffprobe_summary(path, args.codec, args.frame_count) {
                Ok(summary) => summary,
                Err(err) => {
                    if args.allow_case_failures {
                        writeln!(
                            &mut report,
                            "- {label}: skipped (ffprobe verification failed: {err})"
                        )?;
                        continue;
                    }
                    return Err(err);
                }
            };
            if let Err(err) = run_ffmpeg_decode_verify(path, null_sink) {
                if args.allow_case_failures {
                    writeln!(
                        &mut report,
                        "- {label}: skipped (ffmpeg decode verification failed: {err})"
                    )?;
                    continue;
                }
                return Err(err);
            }
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
    println!("parity overall: {overall_status}");
    if !failures.is_empty() {
        println!("runtime_failures: {}", failures.len());
    }
    Ok(())
}

fn case_index(case: Case) -> usize {
    match case {
        Case::VideoHwDecode => 0,
        Case::VideoHwEncode => 1,
        Case::FfmpegDecode => 2,
        Case::FfmpegEncode => 3,
    }
}

fn args_missing_windows_or_linux_guard() -> bool {
    !(cfg!(target_os = "windows") || cfg!(target_os = "linux"))
}

fn pass_fail(ok: bool) -> &'static str {
    if ok { "PASS" } else { "FAIL" }
}

fn throughput_fps(frames: usize, seconds: f64) -> f64 {
    let safe_seconds = seconds.max(f64::EPSILON);
    frames as f64 / safe_seconds
}

fn percent_delta(video_hw: f64, ffmpeg: f64) -> f64 {
    if ffmpeg <= f64::EPSILON {
        return 0.0;
    }
    ((video_hw / ffmpeg) - 1.0) * 100.0
}

fn prepare_decode_input(codec: Codec, loops: usize, output_dir: &Path) -> Result<PathBuf> {
    let source = PathBuf::from(codec.sample_input());
    if loops <= 1 {
        return Ok(source);
    }

    let data = fs::read(&source).with_context(|| {
        format!(
            "read source decode input for loop expansion: {}",
            source.display()
        )
    })?;
    let total_size = data
        .len()
        .checked_mul(loops)
        .context("decode loop input size overflow")?;
    let mut repeated = Vec::with_capacity(total_size);
    for _ in 0..loops {
        repeated.extend_from_slice(&data);
    }

    let extension = source.extension().and_then(|ext| ext.to_str()).unwrap_or("bin");
    let expanded = output_dir.join(format!(
        "benchmark-decode-input-{}-{}x.{}",
        codec.as_cli(),
        loops,
        extension
    ));
    fs::write(&expanded, repeated)
        .with_context(|| format!("write expanded decode input: {}", expanded.display()))?;
    Ok(expanded)
}

fn build_examples(profile: &str, raw_input_pix_fmt: RawInputPixFmt) -> Result<()> {
    let features = match raw_input_pix_fmt {
        RawInputPixFmt::Argb => "backend-intel",
        RawInputPixFmt::Nv12 => "backend-intel,unstable-raw-inputs",
    };
    let mut args = vec![
        "build",
        "--features",
        features,
        "--examples",
        "--profile",
        profile,
    ];
    if profile == "release" {
        args = vec!["build", "--features", features, "--examples", "--release"];
    }
    run_command("cargo", &args)?;
    Ok(())
}

fn run_command(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("spawn command: {program} {}", args.join(" ")))?;
    if !status.success() {
        bail!("command failed: {program} (status={status})");
    }
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
    decode_input: &Path,
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
            let decode_input_arg = decode_input.to_string_lossy().to_string();
            cmd.args([
                "--backend",
                "intel",
                "--codec",
                args.codec.as_cli(),
                "--input",
                &decode_input_arg,
                "--chunk-bytes",
                &args.chunk_bytes.to_string(),
                "--output-mode",
                &args.decode_output_mode,
            ]);
            if args.require_hardware {
                cmd.arg("--require-hardware");
            }
            if args.intel_force_software {
                cmd.arg("--intel-force-software");
            }
            if let Some(depth) = args.intel_decode_async_depth {
                cmd.env("VIDEO_HW_INTEL_DECODE_ASYNC_DEPTH", depth.to_string());
            }
            run_timed_command(cmd, true)
        }
        Case::VideoHwEncode => {
            let _ = fs::remove_file(video_hw_output);
            let mut cmd = if args.equal_raw_input {
                let mut c = Command::new(encode_raw_bin);
                c.args([
                    "--backend",
                    "intel",
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
                    "--input-pix-fmt",
                    args.raw_input_pix_fmt.as_cli(),
                    "--output",
                    &video_hw_output.to_string_lossy(),
                    "--discard-output",
                ]);
                c
            } else {
                let mut c = Command::new(encode_bin);
                c.args([
                    "--backend",
                    "intel",
                    "--codec",
                    args.codec.as_cli(),
                    "--fps",
                    "30",
                    "--frame-count",
                    &args.frame_count.to_string(),
                    "--output",
                    &video_hw_output.to_string_lossy(),
                    "--discard-output",
                ]);
                c
            };
            if args.require_hardware {
                cmd.arg("--require-hardware");
            }
            if args.intel_force_software {
                cmd.arg("--intel-force-software");
            }
            run_timed_command(cmd, true)
        }
        Case::FfmpegDecode => {
            let ffmpeg_hw = !args.intel_force_software;
            let mut cmd = Command::new("ffmpeg");
            let decode_input_arg = decode_input.to_string_lossy().to_string();
            cmd.args(["-y", "-hide_banner", "-benchmark"]);
            if ffmpeg_hw {
                cmd.args(["-hwaccel", "qsv"]);
            }
            cmd.args([
                "-c:v",
                args.codec.ffmpeg_decode_codec(ffmpeg_hw),
                "-i",
                &decode_input_arg,
                "-f",
                "null",
                null_sink,
            ]);
            run_timed_command(cmd, true)
        }
        Case::FfmpegEncode => {
            let ffmpeg_hw = !args.intel_force_software;
            if args.verify {
                let _ = fs::remove_file(ffmpeg_output);
            }
            let mut cmd = Command::new("ffmpeg");
            if args.equal_raw_input {
                cmd.args([
                    "-y",
                    "-hide_banner",
                    "-benchmark",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    args.raw_input_pix_fmt.as_cli(),
                    "-s:v",
                    &format!("{}x{}", args.width, args.height),
                    "-r",
                    "30",
                    "-i",
                    &raw_input.to_string_lossy(),
                    "-frames:v",
                    &args.frame_count.to_string(),
                    "-c:v",
                    args.codec.ffmpeg_encode_codec(ffmpeg_hw),
                ]);
                if args.verify {
                    cmd.args(["-f", args.codec.muxer(), &ffmpeg_output.to_string_lossy()]);
                } else {
                    cmd.args(["-f", "null", null_sink]);
                }
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
                    args.codec.ffmpeg_encode_codec(ffmpeg_hw),
                ]);
                if args.verify {
                    cmd.args(["-f", args.codec.muxer(), &ffmpeg_output.to_string_lossy()]);
                } else {
                    cmd.args(["-f", "null", null_sink]);
                }
            }
            run_timed_command(cmd, true)
        }
    }
}

fn run_timed_command(mut cmd: Command, _quiet: bool) -> Result<CaseRun> {
    let start = Instant::now();
    let output = cmd.output().context("spawn benchmark command")?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if !stderr.trim().is_empty() {
            tail_text(stderr.as_ref(), 400)
        } else {
            tail_text(stdout.as_ref(), 400)
        };
        bail!(
            "benchmark command failed: status={}, detail={}",
            output.status,
            detail.trim()
        );
    }
    let seconds = start.elapsed().as_secs_f64();
    Ok(CaseRun { seconds })
}

fn tail_text(input: &str, limit: usize) -> String {
    let len = input.chars().count();
    if len <= limit {
        return input.to_string();
    }
    input
        .chars()
        .skip(len.saturating_sub(limit))
        .collect::<String>()
}

fn write_raw_input(
    path: &Path,
    input_pix_fmt: RawInputPixFmt,
    width: usize,
    height: usize,
    frame_count: usize,
) -> Result<()> {
    match input_pix_fmt {
        RawInputPixFmt::Argb => write_raw_argb_input(path, width, height, frame_count),
        RawInputPixFmt::Nv12 => write_raw_nv12_input(path, width, height, frame_count),
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
                out[offset] = 255;
                out[offset + 1] = ((x + frame_index) % 256) as u8;
                out[offset + 2] = ((y + frame_index * 2) % 256) as u8;
                out[offset + 3] = ((frame_index * 5) % 256) as u8;
            }
        }
    }
    fs::write(path, out).with_context(|| format!("write raw input: {}", path.display()))?;
    Ok(())
}

fn write_raw_nv12_input(path: &Path, width: usize, height: usize, frame_count: usize) -> Result<()> {
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        bail!("nv12 raw input requires even width/height");
    }
    let y_size = width.checked_mul(height).context("NV12 Y size overflow")?;
    let uv_size = y_size.checked_div(2).context("NV12 UV size overflow")?;
    let frame_size = y_size.checked_add(uv_size).context("NV12 frame size overflow")?;
    let total_size = frame_size
        .checked_mul(frame_count)
        .context("raw input total size overflow")?;
    let mut out = vec![0_u8; total_size];

    for frame_index in 0..frame_count {
        let frame_base = frame_index * frame_size;
        let y_base = frame_base;
        let uv_base = frame_base + y_size;

        for y in 0..height {
            for x in 0..width {
                let y_offset = y_base + (y * width + x);
                out[y_offset] = ((x * 3 + y * 5 + frame_index * 7) % 256) as u8;
            }
        }

        for y in 0..(height / 2) {
            for x in (0..width).step_by(2) {
                let uv_offset = uv_base + (y * width + x);
                out[uv_offset] = (128 + ((x + frame_index * 3) % 32) as u8).saturating_sub(16);
                out[uv_offset + 1] =
                    (128 + ((y * 2 + frame_index * 5) % 32) as u8).saturating_sub(16);
            }
        }
    }

    fs::write(path, out).with_context(|| format!("write raw input: {}", path.display()))?;
    Ok(())
}

fn probe_stream_frame_count(path: &str) -> Result<usize> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .with_context(|| format!("spawn ffprobe for {path}"))?;
    if !output.status.success() {
        bail!("ffprobe failed for {path}: status={}", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let value = text.trim().parse::<usize>().context("parse nb_read_frames")?;
    Ok(value)
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

    if summary.codec_name != codec.as_cli() {
        bail!(
            "unexpected codec for {}: expected {}, got {}",
            path.display(),
            codec.as_cli(),
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
