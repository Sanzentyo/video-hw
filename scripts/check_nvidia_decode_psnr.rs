#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
anyhow = "1"
---

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow, bail};

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output_dir: PathBuf,
    ffmpeg: PathBuf,
    width: u32,
    height: u32,
    min_psnr_y: f64,
    skip_build: bool,
}

#[derive(Debug)]
struct PsnrSummary {
    frames: usize,
    avg_y: f64,
    min_y: f64,
    min_y_frame: usize,
    avg_all: f64,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("failed to create {}", args.output_dir.display()))?;
    if !args.skip_build {
        run(
            Command::new("cargo").args([
                "build",
                "--release",
                "-p",
                "video-hw",
                "--features",
                "backend-nvidia",
                "--example",
                "decode_to_yuv",
            ]),
            "cargo build decode_to_yuv",
        )?;
    }

    let hw_nv12 = args.output_dir.join("nvidia_hevc.nv12");
    let ref_nv12 = args.output_dir.join("ffmpeg_ref.nv12");
    let stats = args.output_dir.join("psnr_stats.txt");
    decode_with_nvidia(&args, &hw_nv12)?;
    decode_with_ffmpeg(&args, &ref_nv12)?;
    compute_psnr(&args, &hw_nv12, &ref_nv12, &stats)?;
    let summary = parse_psnr_stats(&stats)?;

    println!(
        "nvidia_hevc_psnr frames={} psnr_y_avg={:.4} psnr_y_min={:.4} psnr_y_min_frame={} psnr_avg={:.4} threshold={:.4}",
        summary.frames,
        summary.avg_y,
        summary.min_y,
        summary.min_y_frame,
        summary.avg_all,
        args.min_psnr_y
    );
    if summary.min_y < args.min_psnr_y {
        bail!(
            "NVIDIA HEVC PSNR below threshold: min_y={:.4} dB at frame {}, threshold={:.4} dB",
            summary.min_y,
            summary.min_y_frame,
            args.min_psnr_y
        );
    }
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut input = PathBuf::from("sample-videos/foreman_cif.h265");
    let mut output_dir = PathBuf::from("output/nvidia-hevc-psnr");
    let mut ffmpeg = find_ffmpeg();
    let mut width = 352;
    let mut height = 288;
    let mut min_psnr_y = 40.0;
    let mut skip_build = false;
    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--input" => input = next_path(&mut iter, "--input")?,
            "--output-dir" => output_dir = next_path(&mut iter, "--output-dir")?,
            "--ffmpeg" => ffmpeg = next_path(&mut iter, "--ffmpeg")?,
            "--width" => width = next_value(&mut iter, "--width")?.parse()?,
            "--height" => height = next_value(&mut iter, "--height")?.parse()?,
            "--min-psnr-y" => min_psnr_y = next_value(&mut iter, "--min-psnr-y")?.parse()?,
            "--skip-build" => skip_build = true,
            "-h" | "--help" => {
                println!(
                    "usage: cargo +nightly -Zscript scripts/check_nvidia_decode_psnr.rs [--input FILE] [--output-dir DIR] [--ffmpeg PATH] [--width N] [--height N] [--min-psnr-y DB] [--skip-build]"
                );
                std::process::exit(0);
            }
            other => return Err(anyhow!("unknown option {other}")),
        }
    }
    Ok(Args {
        input,
        output_dir,
        ffmpeg,
        width,
        height,
        min_psnr_y,
        skip_build,
    })
}

fn next_value(iter: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    iter.next().with_context(|| format!("{name} requires a value"))
}

fn next_path(iter: &mut impl Iterator<Item = String>, name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(next_value(iter, name)?))
}

fn find_ffmpeg() -> PathBuf {
    if let Ok(path) = env::var("FFMPEG_PATH") {
        return PathBuf::from(path);
    }
    let local = PathBuf::from(r"D:\git\media-autobuild_suite\local64\bin-video\ffmpeg.exe");
    if local.exists() {
        return local;
    }
    PathBuf::from("ffmpeg")
}

fn decode_example_path() -> PathBuf {
    let exe = if cfg!(windows) {
        "decode_to_yuv.exe"
    } else {
        "decode_to_yuv"
    };
    PathBuf::from("target").join("release").join("examples").join(exe)
}

fn decode_with_nvidia(args: &Args, output: &Path) -> Result<()> {
    let mut command = Command::new(decode_example_path());
    command.args([
        "--backend",
        "nvidia",
        "--codec",
        "hevc",
        "--input",
        path_str(&args.input)?,
        "--output",
        path_str(output)?,
        "--output-mode",
        "nv12",
    ]);
    run(&mut command, "decode_to_yuv NVIDIA HEVC")
}

fn decode_with_ffmpeg(args: &Args, output: &Path) -> Result<()> {
    let mut command = Command::new(&args.ffmpeg);
    command.args([
        "-y",
        "-i",
        path_str(&args.input)?,
        "-f",
        "rawvideo",
        "-pix_fmt",
        "nv12",
        path_str(output)?,
    ]);
    run(&mut command, "ffmpeg reference decode")
}

fn compute_psnr(args: &Args, hw: &Path, reference: &Path, stats: &Path) -> Result<()> {
    let size = format!("{}x{}", args.width, args.height);
    let stats_path = stats.to_string_lossy().replace('\\', "/");
    let filter = format!("psnr=stats_file={stats_path}");
    let mut command = Command::new(&args.ffmpeg);
    command.args([
        "-y",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "nv12",
        "-s",
        &size,
        "-i",
        path_str(hw)?,
        "-f",
        "rawvideo",
        "-pix_fmt",
        "nv12",
        "-s",
        &size,
        "-i",
        path_str(reference)?,
        "-lavfi",
        &filter,
        "-f",
        "null",
        "-",
    ]);
    run(&mut command, "ffmpeg psnr")
}

fn run(command: &mut Command, label: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("failed to spawn {label}"))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{label} failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            stdout,
            stderr
        );
    }
    Ok(())
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not valid UTF-8: {}", path.display()))
}

fn parse_psnr_stats(path: &Path) -> Result<PsnrSummary> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read PSNR stats {}", path.display()))?;
    let mut frames = 0usize;
    let mut sum_y = 0.0f64;
    let mut sum_all = 0.0f64;
    let mut min_y = f64::INFINITY;
    let mut min_y_frame = 0usize;
    for line in text.lines() {
        let Some(frame) = field(line, "n:").and_then(|value| value.parse::<usize>().ok()) else {
            continue;
        };
        let Some(psnr_y) = field(line, "psnr_y:").and_then(parse_finite_psnr) else {
            continue;
        };
        let Some(psnr_avg) = field(line, "psnr_avg:").and_then(parse_finite_psnr) else {
            continue;
        };
        frames = frames.saturating_add(1);
        sum_y += psnr_y;
        sum_all += psnr_avg;
        if psnr_y < min_y {
            min_y = psnr_y;
            min_y_frame = frame;
        }
    }
    if frames == 0 {
        bail!("no finite PSNR rows found in {}", path.display());
    }
    Ok(PsnrSummary {
        frames,
        avg_y: sum_y / frames as f64,
        min_y,
        min_y_frame,
        avg_all: sum_all / frames as f64,
    })
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(key))
}

fn parse_finite_psnr(value: &str) -> Option<f64> {
    match value {
        "inf" | "INFINITY" => Some(100.0),
        _ => value.parse::<f64>().ok().filter(|value| value.is_finite()),
    }
}
