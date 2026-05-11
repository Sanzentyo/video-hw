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

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    fn as_cli(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
        }
    }

    fn sample_input(self) -> &'static str {
        match self {
            Self::H264 => "sample-videos/sample-10s.h264",
            Self::Hevc => "sample-videos/sample-10s.h265",
            Self::Av1 => "output/benchmark-vt-av1-decode-input.mp4",
        }
    }

    fn ffmpeg_encode_codec(self) -> &'static str {
        match self {
            Self::H264 => "h264_videotoolbox",
            Self::Hevc => "hevc_videotoolbox",
            Self::Av1 => "av1_videotoolbox",
        }
    }

    fn muxer(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "obu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
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

    #[arg(long)]
    vt_enable_pipeline_scheduler: Option<bool>,

    #[arg(long)]
    vt_pipeline_queue_capacity: Option<usize>,

    #[arg(long, default_value_t = 40.0)]
    min_psnr_y: f64,
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
    let rank = ((percentile / 100.0) * n as f64)
        .ceil()
        .clamp(1.0, n as f64) as usize;
    sorted[rank - 1]
}

fn main() -> Result<()> {
    let args = Args::parse();

    if !cfg!(target_os = "macos") {
        bail!("this benchmark is intended for macOS (VideoToolbox)");
    }

    if args.repeat == 0 {
        bail!("--repeat must be >= 1");
    }
    let profile = if args.release { "release" } else { "debug" };
    let output_dir = PathBuf::from("output");
    fs::create_dir_all(&output_dir).context("create output directory")?;

    build_examples(profile)?;

    if args.codec == Codec::Av1 {
        generate_av1_fmp4_decode_input(&args, Path::new(args.codec.sample_input()))?;
    }

    let decode_bin = if args.codec == Codec::Av1 {
        example_bin_path(profile, "decode_to_yuv")
    } else {
        example_bin_path(profile, "decode_annexb")
    };
    let encode_bin = example_bin_path(profile, "encode_synthetic");
    let encode_raw_bin = example_bin_path(profile, "encode_raw_argb");
    let video_hw_output =
        output_dir.join(format!("video-hw-vt-{}-precise.bin", args.codec.as_cli()));
    let ffmpeg_output = output_dir.join(format!("ffmpeg-vt-{}-precise.bin", args.codec.as_cli()));
    let raw_input = output_dir.join(format!(
        "benchmark-input-argb-{}x{}-{}f.raw",
        args.width, args.height, args.frame_count
    ));
    let null_sink = if cfg!(windows) { "NUL" } else { "/dev/null" };

    if args.equal_raw_input {
        write_raw_argb_input(&raw_input, args.width, args.height, args.frame_count)?;
    }

    let cases: Vec<Case> = if args.codec == Codec::Av1 {
        vec![Case::VideoHwDecode, Case::FfmpegDecode]
    } else {
        vec![
            Case::VideoHwDecode,
            Case::VideoHwEncode,
            Case::FfmpegDecode,
            Case::FfmpegEncode,
        ]
    };
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
                if let Some(case_samples) = samples.iter_mut().find(|s| s.case == *case) {
                    case_samples.push(run.seconds);
                }
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
    writeln!(
        &mut report,
        "vt_enable_pipeline_scheduler: {:?}",
        args.vt_enable_pipeline_scheduler
    )?;
    writeln!(
        &mut report,
        "vt_pipeline_queue_capacity: {:?}",
        args.vt_pipeline_queue_capacity
    )?;
    if args.codec == Codec::Av1 {
        writeln!(
            &mut report,
            "av1_note: decode-only; VideoToolbox AV1 encode remains unsupported"
        )?;
        writeln!(&mut report, "decode_input: {}", args.codec.sample_input())?;
        writeln!(&mut report, "min_psnr_y: {:.4}", args.min_psnr_y)?;
    }
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

    if args.verify && args.codec == Codec::Av1 {
        writeln!(&mut report)?;
        writeln!(&mut report, "## Verification")?;
        let ffmpeg_ref_nv12 = output_dir.join(format!(
            "ffmpeg-vt-av1-reference-nv12-{}x{}-{}f.raw",
            args.width, args.height, args.frame_count
        ));
        generate_ffmpeg_nv12_reference(
            Path::new(args.codec.sample_input()),
            &ffmpeg_ref_nv12,
            args.frame_count,
        )?;
        let bytes = fs::metadata(&video_hw_output).map(|m| m.len()).unwrap_or(0);
        let expected_min = args
            .width
            .saturating_mul(args.height)
            .saturating_mul(3)
            .saturating_div(2)
            .saturating_mul(args.frame_count.saturating_div(10).max(1));
        writeln!(
            &mut report,
            "- video-hw decode raw bytes: {bytes} (expected_min={expected_min})"
        )?;
        if bytes < expected_min as u64 {
            bail!("video-hw AV1 decode output is smaller than expected");
        }
        let psnr = compare_nv12_psnr_y(
            &video_hw_output,
            &ffmpeg_ref_nv12,
            args.width,
            args.height,
            args.frame_count,
        )?;
        writeln!(
            &mut report,
            "- video-hw vs ffmpeg software NV12 PSNR-Y: avg={:.4}, min={:.4}, frames={}",
            psnr.avg_y, psnr.min_y, psnr.frames
        )?;
        if psnr.min_y < args.min_psnr_y {
            bail!(
                "VideoToolbox AV1 PSNR-Y below threshold: min={:.4}, threshold={:.4}",
                psnr.min_y,
                args.min_psnr_y
            );
        }
        let summary = ffprobe_summary(
            Path::new(args.codec.sample_input()),
            args.codec,
            args.frame_count,
        )?;
        writeln!(
            &mut report,
            "- input: codec={}, {}x{}, frames={}",
            summary.codec_name, summary.width, summary.height, summary.nb_read_frames
        )?;
    } else if args.verify {
        writeln!(&mut report)?;
        writeln!(&mut report, "## Verification")?;
        convert_length_prefixed_to_annexb(&video_hw_output, &video_hw_verify_input).with_context(
            || {
                format!(
                    "convert video-hw output to annexb: {}",
                    video_hw_output.display()
                )
            },
        )?;
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
    let mut args = vec!["build", "--examples", "--features", "backend-vt"];
    if profile == "release" {
        args.push("--release");
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
            if args.codec == Codec::Av1 {
                cmd.args([
                    "--backend",
                    "vt",
                    "--codec",
                    "av1",
                    "--input",
                    args.codec.sample_input(),
                    "--input-format",
                    "mp4",
                    "--output-mode",
                    "nv12",
                    "--output",
                    &video_hw_output.to_string_lossy(),
                    "--fps",
                    "30",
                ]);
            } else {
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
            }
            apply_vt_options(&mut cmd, args);
            run_timed_command(cmd)
        }
        Case::VideoHwEncode => {
            let cmd = if args.equal_raw_input {
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
                apply_vt_options(&mut c, args);
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
                apply_vt_options(&mut c, args);
                c
            };
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

fn apply_vt_options(cmd: &mut Command, args: &Args) {
    if args.include_internal_metrics {
        cmd.args(["--vt-report-metrics", "true"]);
    }
    if let Some(enabled) = args.vt_enable_pipeline_scheduler {
        cmd.arg("--vt-enable-pipeline-scheduler")
            .arg(enabled.to_string());
    }
    if let Some(capacity) = args.vt_pipeline_queue_capacity {
        cmd.arg("--vt-pipeline-queue-capacity")
            .arg(capacity.to_string());
    }
}

fn generate_av1_fmp4_decode_input(args: &Args, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create input dir: {}", parent.display()))?;
    }
    let encoder = select_ffmpeg_av1_encoder()?;
    let mut command = Command::new("ffmpeg");
    command.args([
        "-y",
        "-hide_banner",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc2=size={}x{}:rate=30", args.width, args.height),
        "-frames:v",
        &args.frame_count.to_string(),
        "-c:v",
        encoder.name,
    ]);
    command.args(encoder.extra_args);
    command.args([
        "-g",
        "30",
        "-movflags",
        "frag_keyframe+empty_moov+default_base_moof+delay_moov",
        "-f",
        "mp4",
        &path.to_string_lossy(),
    ]);
    let output = command
        .output()
        .with_context(|| format!("generate AV1 fMP4 input: {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "ffmpeg AV1 fMP4 input generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[derive(Debug)]
struct FfmpegAv1Encoder {
    name: &'static str,
    extra_args: &'static [&'static str],
}

fn select_ffmpeg_av1_encoder() -> Result<FfmpegAv1Encoder> {
    let encoders = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .context("list FFmpeg encoders")?;
    let listing = format!(
        "{}\n{}",
        String::from_utf8_lossy(&encoders.stdout),
        String::from_utf8_lossy(&encoders.stderr)
    );
    for encoder in [
        FfmpegAv1Encoder {
            name: "libaom-av1",
            extra_args: &["-cpu-used", "8", "-lag-in-frames", "0"],
        },
        FfmpegAv1Encoder {
            name: "libsvtav1",
            extra_args: &["-preset", "13"],
        },
        FfmpegAv1Encoder {
            name: "rav1e",
            extra_args: &["-speed", "10"],
        },
    ] {
        if listing.contains(encoder.name) {
            return Ok(encoder);
        }
    }
    bail!("ffmpeg does not list a supported AV1 encoder (tried libaom-av1, libsvtav1, rav1e)")
}

fn generate_ffmpeg_nv12_reference(
    input: &Path,
    output_path: &Path,
    frame_count: usize,
) -> Result<()> {
    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-v",
            "error",
            "-i",
            &input.to_string_lossy(),
            "-frames:v",
            &frame_count.to_string(),
            "-pix_fmt",
            "nv12",
            "-f",
            "rawvideo",
            &output_path.to_string_lossy(),
        ])
        .output()
        .with_context(|| format!("generate FFmpeg NV12 reference: {}", output_path.display()))?;
    if !output.status.success() {
        bail!(
            "ffmpeg NV12 reference generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct PsnrSummary {
    frames: usize,
    avg_y: f64,
    min_y: f64,
}

fn compare_nv12_psnr_y(
    actual_path: &Path,
    reference_path: &Path,
    width: usize,
    height: usize,
    requested_frames: usize,
) -> Result<PsnrSummary> {
    let actual = fs::read(actual_path)
        .with_context(|| format!("read video-hw NV12 output: {}", actual_path.display()))?;
    let reference = fs::read(reference_path)
        .with_context(|| format!("read FFmpeg NV12 reference: {}", reference_path.display()))?;
    let y_size = width
        .checked_mul(height)
        .context("NV12 luma plane size overflow")?;
    let frame_size = y_size
        .checked_mul(3)
        .and_then(|v| v.checked_div(2))
        .context("NV12 frame size overflow")?;
    let frames = actual
        .len()
        .min(reference.len())
        .checked_div(frame_size)
        .unwrap_or(0)
        .min(requested_frames);
    if frames == 0 {
        bail!(
            "no full NV12 frames to compare: actual_bytes={}, reference_bytes={}, frame_size={frame_size}",
            actual.len(),
            reference.len()
        );
    }

    let mut values = Vec::with_capacity(frames);
    for frame in 0..frames {
        let offset = frame * frame_size;
        let actual_y = &actual[offset..offset + y_size];
        let reference_y = &reference[offset..offset + y_size];
        values.push(psnr_y(actual_y, reference_y));
    }
    let avg_y = values.iter().sum::<f64>() / values.len() as f64;
    let min_y = values.iter().copied().fold(f64::INFINITY, f64::min);
    Ok(PsnrSummary {
        frames,
        avg_y,
        min_y,
    })
}

fn psnr_y(actual: &[u8], reference: &[u8]) -> f64 {
    let mse = actual
        .iter()
        .zip(reference.iter())
        .map(|(a, b)| {
            let diff = f64::from(*a) - f64::from(*b);
            diff * diff
        })
        .sum::<f64>()
        / actual.len().max(1) as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * ((255.0 * 255.0) / mse).log10()
    }
}

fn write_raw_argb_input(
    path: &Path,
    width: usize,
    height: usize,
    frame_count: usize,
) -> Result<()> {
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
    let output = cmd.output().context("spawn command for benchmark case")?;
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
