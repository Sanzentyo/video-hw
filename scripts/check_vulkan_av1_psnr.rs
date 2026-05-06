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
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};

#[derive(Debug)]
struct Args {
    input: Option<PathBuf>,
    input_format: Av1InputFormat,
    output_dir: PathBuf,
    ffmpeg: PathBuf,
    width: u32,
    height: u32,
    frames: u32,
    fps: u32,
    min_psnr_y: f64,
    skip_build: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av1InputFormat {
    Obu,
    Fmp4,
}

#[derive(Debug)]
struct PsnrSummary {
    frames: usize,
    min_y: f64,
    avg_y: f64,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create {}", args.output_dir.display()))?;
    let run_id = unix_millis();
    let input = match &args.input {
        Some(path) => absolute_path(path)?,
        None => {
            let extension = match args.input_format {
                Av1InputFormat::Obu => "obu",
                Av1InputFormat::Fmp4 => "mp4",
            };
            let generated = args
                .output_dir
                .join(format!("vulkan-av1-psnr-{run_id}.{extension}"));
            generate_ffmpeg_av1_input(&args, &generated)?;
            absolute_path(&generated)?
        }
    };
    if !args.skip_build {
        run(
            Command::new("cargo").args([
                "build",
                "-p",
                "video-hw",
                "--features",
                "backend-vulkan",
                "--example",
                "decode_to_yuv",
            ]),
            "build decode_to_yuv",
        )?;
    }

    let hw_nv12 = args
        .output_dir
        .join(format!("vulkan-av1-{run_id}.nv12"));
    let ref_nv12 = args.output_dir.join(format!("ffmpeg-av1-{run_id}.nv12"));
    let stats = args.output_dir.join(format!("vulkan-av1-psnr-{run_id}.txt"));
    decode_with_vulkan(&args, &input, &hw_nv12)?;
    decode_with_ffmpeg(&args, &input, &ref_nv12)?;
    compute_psnr(&args, &hw_nv12, &ref_nv12, &stats)?;
    let summary = parse_psnr_stats(&stats)?;
    let report = args
        .output_dir
        .join(format!("vulkan-av1-psnr-{run_id}.md"));
    write_report(&args, &input, &summary, &report)?;
    println!(
        "vulkan_av1_psnr frames={} psnr_y_avg={:.4} psnr_y_min={:.4} threshold={:.4} report={}",
        summary.frames,
        summary.avg_y,
        summary.min_y,
        args.min_psnr_y,
        report.display()
    );
    if summary.min_y < args.min_psnr_y {
        bail!(
            "Vulkan AV1 PSNR below threshold: min_y={:.4} dB, threshold={:.4} dB",
            summary.min_y,
            args.min_psnr_y
        );
    }
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut input = None;
    let mut input_format = Av1InputFormat::Obu;
    let mut output_dir = PathBuf::from("output/vulkan-av1-psnr");
    let mut ffmpeg = env::var("FFMPEG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("ffmpeg"));
    let mut width = 320;
    let mut height = 180;
    let mut frames = 1;
    let mut fps = 30;
    let mut min_psnr_y = 40.0;
    let mut skip_build = false;
    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--input" => input = Some(PathBuf::from(next_value(&mut iter, "--input")?)),
            "--input-format" | "--container" => {
                input_format = parse_input_format(&next_value(&mut iter, "--input-format")?)?
            }
            "--output-dir" => output_dir = PathBuf::from(next_value(&mut iter, "--output-dir")?),
            "--ffmpeg" => ffmpeg = PathBuf::from(next_value(&mut iter, "--ffmpeg")?),
            "--width" => width = next_value(&mut iter, "--width")?.parse()?,
            "--height" => height = next_value(&mut iter, "--height")?.parse()?,
            "--frames" => frames = next_value(&mut iter, "--frames")?.parse()?,
            "--fps" => fps = next_value(&mut iter, "--fps")?.parse()?,
            "--min-psnr-y" => min_psnr_y = next_value(&mut iter, "--min-psnr-y")?.parse()?,
            "--skip-build" => skip_build = true,
            "-h" | "--help" => {
                println!(
                    "usage: cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs [--input PATH] [--input-format obu|fmp4] [--output-dir DIR] [--ffmpeg PATH] [--width N] [--height N] [--frames N] [--fps N] [--min-psnr-y DB] [--skip-build]"
                );
                std::process::exit(0);
            }
            other => return Err(anyhow!("unknown option {other}")),
        }
    }
    if width == 0 || height == 0 || frames == 0 || fps == 0 {
        bail!("--width/--height/--frames/--fps must be non-zero");
    }
    Ok(Args {
        input,
        input_format,
        output_dir,
        ffmpeg,
        width,
        height,
        frames,
        fps,
        min_psnr_y,
        skip_build,
    })
}

fn parse_input_format(raw: &str) -> Result<Av1InputFormat> {
    match raw.to_ascii_lowercase().as_str() {
        "obu" | "annexb" => Ok(Av1InputFormat::Obu),
        "mp4" | "fmp4" => Ok(Av1InputFormat::Fmp4),
        other => Err(anyhow!("unsupported input format {other}")),
    }
}

fn next_value(iter: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    iter.next().with_context(|| format!("{name} requires a value"))
}

fn generate_ffmpeg_av1_input(args: &Args, output: &Path) -> Result<()> {
    let source = format!("testsrc2=size={}x{}:rate={}", args.width, args.height, args.fps);
    let mut command = Command::new(&args.ffmpeg);
    command.args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &source,
            "-frames:v",
            &args.frames.to_string(),
            "-an",
            "-c:v",
            "libaom-av1",
            "-cpu-used",
            "8",
            "-g",
            "1",
            "-lag-in-frames",
            "0",
    ]);
    match args.input_format {
        Av1InputFormat::Obu => {
            command.args([
            "-f",
            "obu",
            &output.display().to_string(),
            ]);
        }
        Av1InputFormat::Fmp4 => {
            command.args([
                "-movflags",
                "+frag_keyframe+empty_moov+delay_moov+default_base_moof",
                "-f",
                "mp4",
                &output.display().to_string(),
            ]);
        }
    }
    run(&mut command, "generate FFmpeg AV1 input")
}

fn decode_with_vulkan(args: &Args, input: &Path, output: &Path) -> Result<()> {
    let executable = decode_to_yuv_executable();
    let input_format = match args.input_format {
        Av1InputFormat::Obu => "annexb",
        Av1InputFormat::Fmp4 => "mp4",
    };
    run(
        Command::new(executable).args([
            "--backend",
            "vulkan",
            "--codec",
            "av1",
            "--input",
            &input.display().to_string(),
            "--input-format",
            input_format,
            "--output-mode",
            "nv12",
            "--output",
            &output.display().to_string(),
            "--fps",
            &args.fps.to_string(),
        ]),
        "decode Vulkan AV1 to NV12",
    )
}

fn decode_with_ffmpeg(args: &Args, input: &Path, output: &Path) -> Result<()> {
    run(
        Command::new(&args.ffmpeg).args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &input.display().to_string(),
            "-frames:v",
            &args.frames.to_string(),
            "-pix_fmt",
            "nv12",
            "-f",
            "rawvideo",
            &output.display().to_string(),
        ]),
        "decode FFmpeg AV1 reference to NV12",
    )
}

fn compute_psnr(args: &Args, hw_nv12: &Path, ref_nv12: &Path, stats: &Path) -> Result<()> {
    let size = format!("{}x{}", args.width, args.height);
    let stats_path = stats.display().to_string().replace('\\', "/");
    run(
        Command::new(&args.ffmpeg).args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-s:v",
            &size,
            "-pix_fmt",
            "nv12",
            "-f",
            "rawvideo",
            "-i",
            &hw_nv12.display().to_string(),
            "-s:v",
            &size,
            "-pix_fmt",
            "nv12",
            "-f",
            "rawvideo",
            "-i",
            &ref_nv12.display().to_string(),
            "-lavfi",
            &format!("psnr=stats_file={stats_path}"),
            "-f",
            "null",
            "-",
        ]),
        "compute Vulkan AV1 PSNR",
    )
}

fn parse_psnr_stats(path: &Path) -> Result<PsnrSummary> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let values = text
        .lines()
        .map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("psnr_y:"))
                .ok_or_else(|| anyhow!("missing psnr_y field in {line:?}"))?
                .parse::<f64>()
                .with_context(|| format!("parse psnr_y from {line:?}"))
        })
        .collect::<Result<Vec<_>>>()?;
    if values.is_empty() {
        bail!("PSNR stats file contained no frames");
    }
    let frames = values.len();
    let min_y = values.iter().copied().fold(f64::INFINITY, f64::min);
    let avg_y = values.iter().sum::<f64>() / frames as f64;
    Ok(PsnrSummary {
        frames,
        min_y,
        avg_y,
    })
}

fn write_report(args: &Args, input: &Path, summary: &PsnrSummary, report: &Path) -> Result<()> {
    let status = if summary.min_y >= args.min_psnr_y {
        "PASS"
    } else {
        "FAIL"
    };
    let body = format!(
        "# Vulkan AV1 PSNR\n\nStatus: {status}\n\ninput: `{}`\n\ninput_format: `{:?}`\n\nframes: `{}`\n\nsize: `{}x{}`\n\npsnr_y_avg: `{:.4}`\n\npsnr_y_min: `{:.4}`\n\nthreshold: `{:.4}`\n",
        input.display(),
        args.input_format,
        summary.frames,
        args.width,
        args.height,
        summary.avg_y,
        summary.min_y,
        args.min_psnr_y
    );
    fs::write(report, body).with_context(|| format!("write {}", report.display()))
}

fn decode_to_yuv_executable() -> PathBuf {
    let exe = if cfg!(windows) {
        "decode_to_yuv.exe"
    } else {
        "decode_to_yuv"
    };
    PathBuf::from("target").join("debug").join("examples").join(exe)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn run(command: &mut Command, label: &str) -> Result<()> {
    let status = command.status().with_context(|| format!("spawn {label}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("{label} failed with status {status}")
    }
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}
