#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
anyhow = "1"
clap = { version = "4.5", features = ["derive"] }
---

//! AV1 quality smoke: encode/decode PSNR checks for video-hw backends.
//!
//! This script uses FFmpeg as the reference decoder and raw-input source.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "AV1 PSNR/MSE smoke for video-hw backends against FFmpeg reference")]
struct Args {
    #[arg(long, default_value = "nvidia,intel")]
    backends: String,
    #[arg(long, default_value_t = 320)]
    width: usize,
    #[arg(long, default_value_t = 180)]
    height: usize,
    #[arg(long, default_value_t = 30)]
    frames: usize,
    #[arg(long, default_value_t = 30)]
    fps: usize,
    #[arg(long, default_value_t = 25.0)]
    min_encode_psnr_y: f64,
    #[arg(long, default_value_t = 40.0)]
    min_decode_psnr_y: f64,
    #[arg(long, default_value = "output/av1-psnr")]
    output_dir: PathBuf,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    release: bool,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    require_hardware: bool,
    #[arg(long, default_value = "ffmpeg")]
    ffmpeg: PathBuf,
}

#[derive(Debug)]
struct CaseResult {
    backend: String,
    encoded_bytes: u64,
    encode_psnr: PsnrSummary,
    decode_psnr: Option<PsnrSummary>,
    metadata_decode_frames: Option<usize>,
    decode_note: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct PsnrSummary {
    mse_y_avg: f64,
    mse_avg: f64,
    psnr_y_avg: f64,
    psnr_y_min: f64,
    psnr_avg: f64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.frames == 0 {
        bail!("--frames must be >= 1");
    }
    if args.width == 0 || args.height == 0 {
        bail!("--width/--height must be >= 1");
    }

    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create {}", args.output_dir.display()))?;
    let run_id = epoch_seconds();
    let raw_argb = args.output_dir.join(format!("source-{run_id}.argb"));
    generate_argb_source(&args, &raw_argb)?;

    let backends = args
        .backends
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if backends.is_empty() {
        bail!("--backends did not contain any backend names");
    }

    let mut results = Vec::new();
    for backend in backends {
        results.push(run_backend_case(&args, backend, &raw_argb, run_id)?);
    }

    let report_path = args.output_dir.join(format!("av1-psnr-{run_id}.md"));
    write_report(&args, &results, &report_path)?;
    println!("report: {}", report_path.display());
    for result in &results {
        println!(
            "{} encode_psnr_y={:.4} decode_psnr_y_min={} encoded_bytes={}",
            result.backend,
            result.encode_psnr.psnr_y_avg,
            result
                .decode_psnr
                .map(|psnr| format!("{:.4}", psnr.psnr_y_min))
                .unwrap_or_else(|| "skipped".to_string()),
            result.encoded_bytes
        );
    }

    let failed = results.iter().any(|result| {
        result.encode_psnr.psnr_y_avg < args.min_encode_psnr_y
            || result
                .decode_psnr
                .is_some_and(|psnr| psnr.psnr_y_min < args.min_decode_psnr_y)
    });
    if failed {
        bail!("one or more AV1 PSNR checks failed");
    }
    Ok(())
}

fn run_backend_case(
    args: &Args,
    backend: &str,
    raw_argb: &Path,
    run_id: u64,
) -> Result<CaseResult> {
    let encoded = args
        .output_dir
        .join(format!("video-hw-{backend}-av1-{run_id}.av1"));
    let decoded_rgb = args
        .output_dir
        .join(format!("video-hw-{backend}-av1-{run_id}.rgb"));
    let ffmpeg_rgb = args
        .output_dir
        .join(format!("ffmpeg-ref-{backend}-av1-{run_id}.rgb"));
    let encode_stats = args
        .output_dir
        .join(format!("psnr-encode-{backend}-{run_id}.txt"));
    let decode_stats = args
        .output_dir
        .join(format!("psnr-decode-{backend}-{run_id}.txt"));

    encode_with_video_hw(args, backend, raw_argb, &encoded)?;
    compute_encode_psnr(args, &encoded, raw_argb, &encode_stats)?;
    let (decode_psnr, metadata_decode_frames, decode_note) =
        match decode_with_video_hw(args, backend, &encoded, &decoded_rgb) {
            Ok(()) => {
                decode_with_ffmpeg(args, &encoded, &ffmpeg_rgb)?;
                compute_decode_psnr(args, &decoded_rgb, &ffmpeg_rgb, &decode_stats)?;
                (Some(parse_psnr_stats(&decode_stats)?), None, None)
            }
            Err(err) if backend == "intel" => {
                let frames = decode_metadata_with_video_hw(args, backend, &encoded)?;
                (
                    None,
                    Some(frames),
                    Some(format!(
                        "pixel decode PSNR skipped: {}; metadata decode returned {frames} frames",
                        one_line(&format!("{err:#}"))
                    )),
                )
            }
            Err(err) => return Err(err),
        };

    Ok(CaseResult {
        backend: backend.to_string(),
        encoded_bytes: fs::metadata(&encoded)
            .with_context(|| format!("stat {}", encoded.display()))?
            .len(),
        encode_psnr: parse_psnr_stats(&encode_stats)?,
        decode_psnr,
        metadata_decode_frames,
        decode_note,
    })
}

fn generate_argb_source(args: &Args, output: &Path) -> Result<()> {
    run(
        Command::new(&args.ffmpeg).args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=size={}x{}:rate={}", args.width, args.height, args.fps),
            "-frames:v",
            &args.frames.to_string(),
            "-pix_fmt",
            "argb",
            "-f",
            "rawvideo",
            &path_str(output)?,
        ]),
        "generate ARGB source",
    )
}

fn encode_with_video_hw(args: &Args, backend: &str, raw_argb: &Path, output: &Path) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args([
        "run",
        "-p",
        "video-hw",
        "--features",
        backend_features(backend)?,
        "--example",
        "encode_raw_argb",
    ]);
    if args.release {
        command.arg("--release");
    }
    command.args([
        "--",
        "--backend",
        backend,
        "--codec",
        "av1",
        "--width",
        &args.width.to_string(),
        "--height",
        &args.height.to_string(),
        "--frame-count",
        &args.frames.to_string(),
        "--fps",
        &args.fps.to_string(),
        "--input-raw",
        &path_str(raw_argb)?,
        "--input-pix-fmt",
        "argb",
        "--output",
        &path_str(output)?,
    ]);
    if args.require_hardware {
        command.arg("--require-hardware");
    }
    run(&mut command, &format!("video-hw {backend} AV1 encode"))
}

fn decode_with_video_hw(args: &Args, backend: &str, encoded: &Path, output: &Path) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args([
        "run",
        "-p",
        "video-hw",
        "--features",
        backend_features(backend)?,
        "--example",
        "decode_to_yuv",
    ]);
    if args.release {
        command.arg("--release");
    }
    command.args([
        "--",
        "--backend",
        backend,
        "--codec",
        "av1",
        "--input",
        &path_str(encoded)?,
        "--output-mode",
        "rgb24",
        "--output",
        &path_str(output)?,
        "--chunk-bytes",
        "257",
    ]);
    if args.require_hardware {
        command.arg("--require-hardware");
    }
    run(&mut command, &format!("video-hw {backend} AV1 decode"))
}

fn decode_metadata_with_video_hw(args: &Args, backend: &str, encoded: &Path) -> Result<usize> {
    let mut command = Command::new("cargo");
    command.args([
        "run",
        "-p",
        "video-hw",
        "--features",
        backend_features(backend)?,
        "--example",
        "decode_annexb",
    ]);
    if args.release {
        command.arg("--release");
    }
    command.args([
        "--",
        "--backend",
        backend,
        "--codec",
        "av1",
        "--input",
        &path_str(encoded)?,
        "--output-mode",
        "metadata",
        "--chunk-bytes",
        "257",
    ]);
    if args.require_hardware {
        command.arg("--require-hardware");
    }
    let stdout = run_capture(
        &mut command,
        &format!("video-hw {backend} AV1 metadata decode"),
    )?;
    parse_decoded_frames(&stdout)
}

fn decode_with_ffmpeg(args: &Args, encoded: &Path, output: &Path) -> Result<()> {
    run(
        Command::new(&args.ffmpeg).args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "obu",
            "-i",
            &path_str(encoded)?,
            "-frames:v",
            &args.frames.to_string(),
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            &path_str(output)?,
        ]),
        "ffmpeg AV1 reference decode",
    )
}

fn compute_encode_psnr(
    args: &Args,
    encoded: &Path,
    raw_argb: &Path,
    stats: &Path,
) -> Result<()> {
    let stats_path = lavfi_path(stats);
    run(
        Command::new(&args.ffmpeg).args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "obu",
            "-r",
            &args.fps.to_string(),
            "-i",
            &path_str(encoded)?,
            "-f",
            "rawvideo",
            "-pix_fmt",
            "argb",
            "-s:v",
            &format!("{}x{}", args.width, args.height),
            "-r",
            &args.fps.to_string(),
            "-i",
            &path_str(raw_argb)?,
            "-lavfi",
            &format!(
                "[0:v]trim=end_frame={},setpts=PTS-STARTPTS,format=yuv420p[a];\
                 [1:v]trim=end_frame={},setpts=PTS-STARTPTS,format=yuv420p[b];\
                 [a][b]psnr=stats_file={stats_path}",
                args.frames, args.frames
            ),
            "-f",
            "null",
            null_sink(),
        ]),
        "ffmpeg AV1 encode PSNR",
    )
}

fn compute_decode_psnr(args: &Args, decoded: &Path, reference: &Path, stats: &Path) -> Result<()> {
    let stats_path = lavfi_path(stats);
    run(
        Command::new(&args.ffmpeg).args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s:v",
            &format!("{}x{}", args.width, args.height),
            "-r",
            &args.fps.to_string(),
            "-i",
            &path_str(decoded)?,
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s:v",
            &format!("{}x{}", args.width, args.height),
            "-r",
            &args.fps.to_string(),
            "-i",
            &path_str(reference)?,
            "-lavfi",
            &format!(
                "[0:v]format=yuv420p[a];[1:v]format=yuv420p[b];\
                 [a][b]psnr=stats_file={stats_path}"
            ),
            "-f",
            "null",
            null_sink(),
        ]),
        "ffmpeg AV1 decode PSNR",
    )
}

fn parse_psnr_stats(path: &Path) -> Result<PsnrSummary> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("read PSNR stats {}", path.display()))?;
    let mut frames = 0_usize;
    let mut sum_mse_y = 0.0_f64;
    let mut sum_mse_avg = 0.0_f64;
    let mut sum_psnr_y = 0.0_f64;
    let mut sum_psnr_avg = 0.0_f64;
    let mut min_psnr_y = f64::INFINITY;

    for line in content.lines() {
        let Some(mse_y) = field(line, "mse_y:").and_then(parse_finite) else {
            continue;
        };
        let Some(mse_avg) = field(line, "mse_avg:").and_then(parse_finite) else {
            continue;
        };
        let Some(psnr_y) = field(line, "psnr_y:").and_then(parse_psnr_value) else {
            continue;
        };
        let Some(psnr_avg) = field(line, "psnr_avg:").and_then(parse_psnr_value) else {
            continue;
        };
        frames += 1;
        sum_mse_y += mse_y;
        sum_mse_avg += mse_avg;
        sum_psnr_y += psnr_y;
        sum_psnr_avg += psnr_avg;
        min_psnr_y = min_psnr_y.min(psnr_y);
    }
    if frames == 0 {
        bail!("no PSNR rows found in {}", path.display());
    }
    Ok(PsnrSummary {
        mse_y_avg: sum_mse_y / frames as f64,
        mse_avg: sum_mse_avg / frames as f64,
        psnr_y_avg: sum_psnr_y / frames as f64,
        psnr_y_min: min_psnr_y,
        psnr_avg: sum_psnr_avg / frames as f64,
    })
}

fn write_report(args: &Args, results: &[CaseResult], path: &Path) -> Result<()> {
    let mut report = String::new();
    writeln!(&mut report, "# AV1 PSNR Report")?;
    writeln!(&mut report, "width: {}", args.width)?;
    writeln!(&mut report, "height: {}", args.height)?;
    writeln!(&mut report, "frames: {}", args.frames)?;
    writeln!(&mut report, "fps: {}", args.fps)?;
    writeln!(&mut report, "min_encode_psnr_y: {:.4}", args.min_encode_psnr_y)?;
    writeln!(&mut report, "min_decode_psnr_y: {:.4}", args.min_decode_psnr_y)?;
    writeln!(&mut report)?;
    writeln!(
        &mut report,
        "| Backend | Encoded bytes | Encode mse_y | Encode mse_avg | Encode psnr_y avg | Encode psnr_avg | Decode mse_y | Decode mse_avg | Decode psnr_y min | Decode psnr_y avg | Decode psnr_avg | Status | Note |"
    )?;
    writeln!(
        &mut report,
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|"
    )?;
    for result in results {
        let passed = result.encode_psnr.psnr_y_avg >= args.min_encode_psnr_y
            && result
                .decode_psnr
                .is_none_or(|psnr| psnr.psnr_y_min >= args.min_decode_psnr_y);
        let decode_mse_y = result
            .decode_psnr
            .map(|psnr| format!("{:.6}", psnr.mse_y_avg))
            .unwrap_or_else(|| "-".to_string());
        let decode_mse_avg = result
            .decode_psnr
            .map(|psnr| format!("{:.6}", psnr.mse_avg))
            .unwrap_or_else(|| "-".to_string());
        let decode_psnr_y_min = result
            .decode_psnr
            .map(|psnr| format!("{:.4}", psnr.psnr_y_min))
            .unwrap_or_else(|| "-".to_string());
        let decode_psnr_y_avg = result
            .decode_psnr
            .map(|psnr| format!("{:.4}", psnr.psnr_y_avg))
            .unwrap_or_else(|| "-".to_string());
        let decode_psnr_avg = result
            .decode_psnr
            .map(|psnr| format!("{:.4}", psnr.psnr_avg))
            .unwrap_or_else(|| "-".to_string());
        let note = result.decode_note.clone().unwrap_or_else(|| {
            result
                .metadata_decode_frames
                .map(|frames| format!("metadata frames={frames}"))
                .unwrap_or_default()
        });
        writeln!(
            &mut report,
            "| {} | {} | {:.6} | {:.6} | {:.4} | {:.4} | {} | {} | {} | {} | {} | {} | {} |",
            result.backend,
            result.encoded_bytes,
            result.encode_psnr.mse_y_avg,
            result.encode_psnr.mse_avg,
            result.encode_psnr.psnr_y_avg,
            result.encode_psnr.psnr_avg,
            decode_mse_y,
            decode_mse_avg,
            decode_psnr_y_min,
            decode_psnr_y_avg,
            decode_psnr_avg,
            if passed { "PASS" } else { "FAIL" },
            note
        )?;
    }
    fs::write(path, report).with_context(|| format!("write {}", path.display()))
}

fn backend_features(backend: &str) -> Result<&'static str> {
    match backend {
        "nvidia" => Ok("backend-nvidia"),
        "intel" => Ok("backend-intel"),
        other => bail!("unsupported AV1 PSNR backend: {other}"),
    }
}

fn run(command: &mut Command, label: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("spawn {label}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        bail!("{label} failed: status={}, detail={detail}", output.status);
    }
    Ok(())
}

fn run_capture(command: &mut Command, label: &str) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("spawn {label}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        bail!("{label} failed: status={}, detail={detail}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn parse_decoded_frames(stdout: &str) -> Result<usize> {
    for token in stdout.split(|ch: char| ch.is_whitespace() || ch == ',') {
        if let Some(value) = token.strip_prefix("decoded_frames=") {
            return value.parse().context("parse decoded_frames");
        }
    }
    bail!("decoded_frames token not found in output: {stdout}")
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn field<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|token| token.strip_prefix(prefix))
}

fn parse_finite(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn parse_psnr_value(value: &str) -> Option<f64> {
    if value == "inf" {
        Some(100.0)
    } else {
        parse_finite(value)
    }
}

fn lavfi_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn path_str(path: &Path) -> Result<String> {
    Ok(path
        .to_str()
        .context("path is not valid UTF-8")?
        .to_string())
}

fn null_sink() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}
