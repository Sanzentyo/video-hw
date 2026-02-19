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
use std::path::PathBuf;
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

    fn ffmpeg_muxer(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Benchmark video-hw (NVDEC/NVENC) vs ffmpeg on NVIDIA")]
struct Args {
    #[arg(long, value_enum, default_value_t = Codec::H264)]
    codec: Codec,

    #[arg(long)]
    release: bool,

    #[arg(long, default_value_t = 65536)]
    chunk_bytes: usize,

    #[arg(long, default_value_t = 300)]
    frame_count: usize,

    #[arg(long, default_value_t = false)]
    verify: bool,
}

#[derive(Debug)]
struct BenchResult {
    label: &'static str,
    seconds: f64,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let output_dir = PathBuf::from("output");
    fs::create_dir_all(&output_dir).context("create output directory")?;

    let codec = args.codec;
    let release_flag = if args.release {
        Some("--release")
    } else {
        None
    };

    let video_hw_output = output_dir.join(format!("video-hw-{}.bin", codec.as_cli()));
    let ffmpeg_output = output_dir.join(format!("ffmpeg-{}.bin", codec.as_cli()));
    let null_sink = if cfg!(windows) { "NUL" } else { "/dev/null" };

    let mut results = Vec::new();

    results.push(run_command(
        "video-hw decode",
        "cargo",
        &cargo_decode_args(codec, args.chunk_bytes, release_flag),
    )?);

    results.push(run_command(
        "video-hw encode",
        "cargo",
        &cargo_encode_args(codec, args.frame_count, &video_hw_output, release_flag),
    )?);

    results.push(run_command(
        "ffmpeg decode",
        "ffmpeg",
        &ffmpeg_decode_args(codec, null_sink),
    )?);

    results.push(run_command(
        "ffmpeg encode",
        "ffmpeg",
        &ffmpeg_encode_args(codec, args.frame_count, &ffmpeg_output),
    )?);

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before UNIX_EPOCH")?
        .as_secs();
    let report_path = output_dir.join(format!(
        "benchmark-nv-{}-{}.txt",
        codec.as_cli(),
        now_secs
    ));

    let mut report = String::new();
    writeln!(&mut report, "benchmark epoch_seconds: {now_secs}")?;
    writeln!(&mut report, "codec: {}", codec.as_cli())?;
    writeln!(&mut report)?;
    for result in &results {
        writeln!(&mut report, "{}: {:.3}s", result.label, result.seconds)?;
    }

    if args.verify {
        writeln!(&mut report)?;
        writeln!(&mut report, "verification: enabled")?;
        let verify_items = [
            ("video-hw", video_hw_output.as_path()),
            ("ffmpeg", ffmpeg_output.as_path()),
        ];
        for (label, path) in verify_items {
            let summary = ffprobe_summary(path, codec, args.frame_count)?;
            run_ffmpeg_decode_verify(path, null_sink)?;
            writeln!(
                &mut report,
                "{} verify: codec={}, {}x{}, frames={}, decode=ok",
                label, summary.codec_name, summary.width, summary.height, summary.nb_read_frames
            )?;
        }
    }

    fs::write(&report_path, report).with_context(|| {
        format!(
            "write benchmark report to {}",
            report_path.to_string_lossy()
        )
    })?;

    println!("saved report: {}", report_path.to_string_lossy());
    Ok(())
}

fn run_command(label: &'static str, program: &str, args: &[String]) -> Result<BenchResult> {
    println!("\n=== {label} ===");
    println!("{} {}", program, args.join(" "));

    let start = Instant::now();
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("spawn command: {program}"))?;

    if !status.success() {
        bail!("{label} failed with status {status}");
    }

    Ok(BenchResult {
        label,
        seconds: start.elapsed().as_secs_f64(),
    })
}

fn cargo_decode_args(codec: Codec, chunk_bytes: usize, release_flag: Option<&str>) -> Vec<String> {
    let mut args = vec!["run".to_string()];
    if let Some(flag) = release_flag {
        args.push(flag.to_string());
    }
    args.extend([
        "--features".to_string(),
        "backend-nvidia".to_string(),
        "--example".to_string(),
        "decode_annexb".to_string(),
        "--".to_string(),
        "--backend".to_string(),
        "nv".to_string(),
        "--codec".to_string(),
        codec.as_cli().to_string(),
        "--input".to_string(),
        codec.sample_input().to_string(),
        "--chunk-bytes".to_string(),
        chunk_bytes.to_string(),
        "--require-hardware".to_string(),
    ]);
    args
}

fn cargo_encode_args(
    codec: Codec,
    frame_count: usize,
    output: &PathBuf,
    release_flag: Option<&str>,
) -> Vec<String> {
    let mut args = vec!["run".to_string()];
    if let Some(flag) = release_flag {
        args.push(flag.to_string());
    }
    args.extend([
        "--features".to_string(),
        "backend-nvidia".to_string(),
        "--example".to_string(),
        "encode_synthetic".to_string(),
        "--".to_string(),
        "--backend".to_string(),
        "nv".to_string(),
        "--codec".to_string(),
        codec.as_cli().to_string(),
        "--fps".to_string(),
        "30".to_string(),
        "--frame-count".to_string(),
        frame_count.to_string(),
        "--require-hardware".to_string(),
        "--output".to_string(),
        output.to_string_lossy().to_string(),
    ]);
    args
}

fn ffmpeg_decode_args(codec: Codec, null_sink: &str) -> Vec<String> {
    vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-benchmark".to_string(),
        "-hwaccel".to_string(),
        "cuda".to_string(),
        "-c:v".to_string(),
        codec.ffmpeg_decode_codec().to_string(),
        "-i".to_string(),
        codec.sample_input().to_string(),
        "-f".to_string(),
        "null".to_string(),
        null_sink.to_string(),
    ]
}

fn ffmpeg_encode_args(codec: Codec, frame_count: usize, output: &PathBuf) -> Vec<String> {
    vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-benchmark".to_string(),
        "-f".to_string(),
        "lavfi".to_string(),
        "-i".to_string(),
        "testsrc2=size=640x360:rate=30".to_string(),
        "-frames:v".to_string(),
        frame_count.to_string(),
        "-c:v".to_string(),
        codec.ffmpeg_encode_codec().to_string(),
        "-preset".to_string(),
        "p1".to_string(),
        "-f".to_string(),
        codec.ffmpeg_muxer().to_string(),
        output.to_string_lossy().to_string(),
    ]
}

#[derive(Debug, Clone)]
struct ProbeSummary {
    codec_name: String,
    width: usize,
    height: usize,
    nb_read_frames: usize,
}

fn ffprobe_summary(path: &std::path::Path, codec: Codec, expected_frames: usize) -> Result<ProbeSummary> {
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

fn run_ffmpeg_decode_verify(path: &std::path::Path, null_sink: &str) -> Result<()> {
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
