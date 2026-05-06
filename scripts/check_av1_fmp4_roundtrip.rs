#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
anyhow = "1"
clap = { version = "4.5", features = ["derive"] }
---

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "AV1 fMP4 roundtrip smoke for video-hw-fmp4 backends")]
struct Args {
    #[arg(long, default_value = "nvidia,intel")]
    backends: String,
    #[arg(long, default_value_t = 320)]
    width: u32,
    #[arg(long, default_value_t = 180)]
    height: u32,
    #[arg(long, default_value_t = 30)]
    frames: u32,
    #[arg(long, default_value_t = 30)]
    fps: u32,
    #[arg(long, default_value_t = 10)]
    fragment_frames: u32,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    release: bool,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    require_hardware: bool,
    #[arg(long, default_value = "output/av1-fmp4-roundtrip")]
    output_dir: PathBuf,
    #[arg(long, default_value = "ffprobe")]
    ffprobe: PathBuf,
    #[arg(long, default_value = "ffmpeg")]
    ffmpeg: PathBuf,
    #[arg(long, default_value_t = 40.0)]
    min_decode_psnr_y: f64,
}

#[derive(Debug)]
struct CaseResult {
    backend: String,
    output: PathBuf,
    bytes: u64,
    reader_samples: usize,
    metadata_frames: usize,
    decode_psnr: PsnrSummary,
    ffprobe_codec: String,
    ffprobe_tag: String,
    ffprobe_duration: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.width == 0 || args.height == 0 || args.frames == 0 || args.fps == 0 {
        bail!("--width/--height/--frames/--fps must be non-zero");
    }
    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create {}", args.output_dir.display()))?;
    let run_id = epoch_seconds();
    let backends = args
        .backends
        .split(',')
        .map(str::trim)
        .filter(|backend| !backend.is_empty())
        .collect::<Vec<_>>();
    if backends.is_empty() {
        bail!("--backends must name at least one backend");
    }

    let mut results = Vec::new();
    for backend in backends {
        results.push(run_case(&args, backend, run_id)?);
    }

    let report = args
        .output_dir
        .join(format!("av1-fmp4-roundtrip-{run_id}.md"));
    write_report(&args, &results, &report)?;
    println!("report: {}", report.display());
    for result in &results {
        println!(
            "{} output={} samples={} metadata_frames={} decode_psnr_min={:.4} codec={} tag={} duration={}",
            result.backend,
            result.output.display(),
            result.reader_samples,
            result.metadata_frames,
            result.decode_psnr.psnr_y_min,
            result.ffprobe_codec,
            result.ffprobe_tag,
            result.ffprobe_duration
        );
    }
    Ok(())
}

fn run_case(args: &Args, backend: &str, run_id: u64) -> Result<CaseResult> {
    let output = args
        .output_dir
        .join(format!("synthetic-{backend}-av1-fmp4-{run_id}.mp4"));
    write_synthetic_fmp4(args, backend, &output)?;
    let reader_samples = read_fmp4_file(args, backend, &output)?;
    let probe = ffprobe_av1_mp4(args, &output)?;
    ffmpeg_decode(args, &output)?;
    let metadata_frames = decode_to_yuv_metadata(args, backend, &output)?;
    let decode_psnr = decode_rgb_and_compare(args, backend, &output, run_id)?;
    if reader_samples != args.frames as usize {
        bail!(
            "{backend} reader sample count mismatch: got {reader_samples}, expected {}",
            args.frames
        );
    }
    if metadata_frames != args.frames as usize {
        bail!(
            "{backend} decode_to_yuv frame count mismatch: got {metadata_frames}, expected {}",
            args.frames
        );
    }
    if probe.codec != "av1" || probe.tag != "av01" {
        bail!(
            "{backend} ffprobe codec/tag mismatch: codec={}, tag={}",
            probe.codec,
            probe.tag
        );
    }
    if decode_psnr.psnr_y_min < args.min_decode_psnr_y {
        bail!(
            "{backend} fMP4 decode PSNR below threshold: min={:.4}, threshold={:.4}",
            decode_psnr.psnr_y_min,
            args.min_decode_psnr_y
        );
    }
    Ok(CaseResult {
        backend: backend.to_string(),
        bytes: fs::metadata(&output)
            .with_context(|| format!("stat {}", output.display()))?
            .len(),
        output,
        reader_samples,
        metadata_frames,
        decode_psnr,
        ffprobe_codec: probe.codec,
        ffprobe_tag: probe.tag,
        ffprobe_duration: probe.duration,
    })
}

fn write_synthetic_fmp4(args: &Args, backend: &str, output: &Path) -> Result<()> {
    let mut command = cargo_example_command(args, "video-hw-fmp4", backend, "write_synthetic_fmp4")?;
    command.args([
        "--output",
        &path_str(output)?,
        "--codec",
        "av1",
        "--backend",
        backend,
        "--width",
        &args.width.to_string(),
        "--height",
        &args.height.to_string(),
        "--frames",
        &args.frames.to_string(),
        "--fps",
        &args.fps.to_string(),
        "--fragment-frames",
        &args.fragment_frames.to_string(),
    ]);
    if args.require_hardware {
        command.arg("--require-hardware");
    }
    run(&mut command, &format!("write {backend} AV1 fMP4"))
}

fn read_fmp4_file(args: &Args, backend: &str, input: &Path) -> Result<usize> {
    let mut command = cargo_example_command(args, "video-hw-fmp4", backend, "read_fmp4_file")?;
    command.arg(path_str(input)?);
    let output = run_capture(&mut command, &format!("read {backend} AV1 fMP4"))?;
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("indexed_samples="))
        .with_context(|| format!("read_fmp4_file did not report indexed_samples for {backend}"))?
        .parse()
        .with_context(|| format!("parse read_fmp4_file indexed_samples for {backend}"))
}

fn ffmpeg_decode(args: &Args, input: &Path) -> Result<()> {
    run(
        Command::new(&args.ffmpeg).args([
            "-hide_banner",
            "-v",
            "error",
            "-i",
            &path_str(input)?,
            "-f",
            "null",
            "-",
        ]),
        "FFmpeg decode AV1 fMP4",
    )
}

fn decode_to_yuv_metadata(args: &Args, backend: &str, input: &Path) -> Result<usize> {
    let mut command = cargo_example_command(args, "video-hw", backend, "decode_to_yuv")?;
    command.args([
        "--backend",
        backend,
        "--codec",
        "av1",
        "--input",
        &path_str(input)?,
        "--input-format",
        "mp4",
        "--output-mode",
        "metadata",
    ]);
    if args.require_hardware {
        command.arg("--require-hardware");
    }
    let output = run_capture(&mut command, &format!("decode_to_yuv {backend} AV1 fMP4"))?;
    output
        .lines()
        .find_map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("frames="))
        })
        .with_context(|| format!("decode_to_yuv did not report frames for {backend}"))?
        .parse()
        .with_context(|| format!("parse decode_to_yuv frame count for {backend}"))
}

fn decode_rgb_and_compare(
    args: &Args,
    backend: &str,
    input: &Path,
    run_id: u64,
) -> Result<PsnrSummary> {
    let hw_rgb = args
        .output_dir
        .join(format!("decode-{backend}-av1-fmp4-{run_id}.rgb"));
    let ffmpeg_rgb = args
        .output_dir
        .join(format!("ffmpeg-{backend}-av1-fmp4-{run_id}.rgb"));
    let stats = args
        .output_dir
        .join(format!("psnr-decode-{backend}-{run_id}.txt"));

    let mut command = cargo_example_command(args, "video-hw", backend, "decode_to_yuv")?;
    command.args([
        "--backend",
        backend,
        "--codec",
        "av1",
        "--input",
        &path_str(input)?,
        "--input-format",
        "mp4",
        "--output-mode",
        "rgb24",
        "--output",
        &path_str(&hw_rgb)?,
    ]);
    if args.require_hardware {
        command.arg("--require-hardware");
    }
    run(
        &mut command,
        &format!("decode_to_yuv rgb24 {backend} AV1 fMP4"),
    )?;
    run(
        Command::new(&args.ffmpeg).args([
            "-y",
            "-hide_banner",
            "-v",
            "error",
            "-i",
            &path_str(input)?,
            "-frames:v",
            &args.frames.to_string(),
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            &path_str(&ffmpeg_rgb)?,
        ]),
        "FFmpeg RGB24 reference decode",
    )?;
    compute_rgb_psnr(args, &hw_rgb, &ffmpeg_rgb, &stats)?;
    parse_psnr_stats(&stats)
}

fn compute_rgb_psnr(args: &Args, hw_rgb: &Path, ffmpeg_rgb: &Path, stats: &Path) -> Result<()> {
    let size = format!("{}x{}", args.width, args.height);
    let stats_path = path_str(stats)?.replace('\\', "/");
    run(
        Command::new(&args.ffmpeg).args([
            "-y",
            "-hide_banner",
            "-v",
            "error",
            "-s:v",
            &size,
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-i",
            &path_str(hw_rgb)?,
            "-s:v",
            &size,
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-i",
            &path_str(ffmpeg_rgb)?,
            "-lavfi",
            &format!("psnr=stats_file={stats_path}"),
            "-f",
            "null",
            "-",
        ]),
        "compute fMP4 decode PSNR",
    )
}

#[derive(Debug, Clone, Copy)]
struct PsnrSummary {
    psnr_y_avg: f64,
    psnr_y_min: f64,
}

fn parse_psnr_stats(path: &Path) -> Result<PsnrSummary> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let values = text
        .lines()
        .map(|line| {
            line.split_whitespace()
                .find_map(|field| {
                    field
                        .strip_prefix("psnr_y:")
                        .or_else(|| field.strip_prefix("psnr_avg:"))
                })
                .with_context(|| format!("missing psnr_y/psnr_avg field in {line:?}"))?
                .parse::<f64>()
                .with_context(|| format!("parse psnr_y/psnr_avg from {line:?}"))
        })
        .collect::<Result<Vec<_>>>()?;
    if values.is_empty() {
        bail!("PSNR stats file contained no frames");
    }
    Ok(PsnrSummary {
        psnr_y_avg: values.iter().sum::<f64>() / values.len() as f64,
        psnr_y_min: values.iter().copied().fold(f64::INFINITY, f64::min),
    })
}

#[derive(Debug)]
struct ProbeResult {
    codec: String,
    tag: String,
    duration: String,
}

fn ffprobe_av1_mp4(args: &Args, input: &Path) -> Result<ProbeResult> {
    let output = run_capture(
        Command::new(&args.ffprobe).args([
            "-hide_banner",
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,codec_tag_string,width,height",
            "-show_entries",
            "format=duration",
            "-of",
            "default=nw=1",
            &path_str(input)?,
        ]),
        "ffprobe AV1 fMP4",
    )?;
    let codec = value_from_probe(&output, "codec_name").unwrap_or_default();
    let tag = value_from_probe(&output, "codec_tag_string").unwrap_or_default();
    let duration = value_from_probe(&output, "duration").unwrap_or_default();
    Ok(ProbeResult {
        codec,
        tag,
        duration,
    })
}

fn value_from_probe(output: &str, name: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}=")))
        .map(ToOwned::to_owned)
}

fn cargo_example_command(
    args: &Args,
    package: &str,
    backend: &str,
    example: &str,
) -> Result<Command> {
    let mut command = Command::new("cargo");
    command.args([
        "run",
        "-p",
        package,
        "--features",
        package_features(package, backend)?,
        "--example",
        example,
    ]);
    if args.release {
        command.arg("--release");
    }
    command.arg("--");
    Ok(command)
}

fn package_features(package: &str, backend: &str) -> Result<&'static str> {
    match (package, backend) {
        ("video-hw-fmp4", "nvidia" | "nv") => Ok("backend-nvidia async-session"),
        ("video-hw-fmp4", "intel") => Ok("backend-intel async-session"),
        ("video-hw", "nvidia" | "nv") => Ok("backend-nvidia"),
        ("video-hw", "intel") => Ok("backend-intel"),
        (_, other) => bail!("unsupported backend {other}; expected nvidia or intel"),
    }
}

fn run(command: &mut Command, label: &str) -> Result<()> {
    let _ = run_capture(command, label)?;
    Ok(())
}

fn run_capture(command: &mut Command, label: &str) -> Result<String> {
    let output = command.output().with_context(|| format!("spawn {label}"))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{label} failed: status={}; stdout_tail={}; stderr_tail={}",
            output.status,
            tail_for_report(&stdout),
            tail_for_report(&stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn write_report(args: &Args, results: &[CaseResult], report: &Path) -> Result<()> {
    let mut body = String::new();
    writeln!(&mut body, "# AV1 fMP4 Roundtrip")?;
    writeln!(&mut body)?;
    writeln!(&mut body, "width: {}", args.width)?;
    writeln!(&mut body, "height: {}", args.height)?;
    writeln!(&mut body, "frames: {}", args.frames)?;
    writeln!(&mut body, "fps: {}", args.fps)?;
    writeln!(&mut body, "release: {}", args.release)?;
    writeln!(&mut body, "min_decode_psnr_y: {:.4}", args.min_decode_psnr_y)?;
    writeln!(&mut body)?;
    writeln!(
        &mut body,
        "| Backend | Output | Bytes | Reader samples | Metadata frames | Decode PSNR avg | Decode PSNR min | Codec | Tag | Duration |"
    )?;
    writeln!(
        &mut body,
        "|---|---|---:|---:|---:|---:|---:|---|---|---:|"
    )?;
    for result in results {
        writeln!(
            &mut body,
            "| {} | {} | {} | {} | {} | {:.4} | {:.4} | {} | {} | {} |",
            result.backend,
            result.output.display(),
            result.bytes,
            result.reader_samples,
            result.metadata_frames,
            result.decode_psnr.psnr_y_avg,
            result.decode_psnr.psnr_y_min,
            result.ffprobe_codec,
            result.ffprobe_tag,
            result.ffprobe_duration
        )?;
    }
    fs::write(report, body).with_context(|| format!("write {}", report.display()))
}

fn path_str(path: &Path) -> Result<String> {
    Ok(path
        .to_str()
        .with_context(|| format!("path is not UTF-8: {}", path.display()))?
        .to_string())
}

fn tail_for_report(text: &str) -> String {
    let lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(8)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        "-".to_string()
    } else {
        lines.into_iter().rev().collect::<Vec<_>>().join(" / ")
    }
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
