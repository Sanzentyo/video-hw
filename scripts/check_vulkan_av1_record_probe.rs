#!/usr/bin/env -S cargo +nightly -Zscript
---
[package]
edition = "2024"
---

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> Result<(), String> {
    let args = Args::parse(env::args().skip(1).collect())?;
    fs::create_dir_all(&args.output_dir).map_err(|err| {
        format!(
            "failed to create output dir {}: {err}",
            args.output_dir.display()
        )
    })?;
    let run_id = unix_millis();
    let generated_input = if args.generate_ffmpeg_obu {
        let path = args.output_dir.join(format!("ffmpeg-av1-probe-{run_id}.obu"));
        generate_ffmpeg_av1_obu(&args, &path)?;
        Some(path)
    } else {
        None
    };
    let input_path = args
        .input
        .as_ref()
        .or(generated_input.as_ref())
        .map(|path| absolute_path(path))
        .transpose()?;
    if !args.skip_build {
        run(
            Command::new("cargo").args([
                "build",
                "-p",
                "video-hw-backend-vulkan",
                "--features",
                "backend-vulkan",
                "--tests",
            ]),
            "build Vulkan backend tests",
        )?;
    }

    let mut command = Command::new("cargo");
    command.args([
        "test",
        "-p",
        "video-hw-backend-vulkan",
        "--features",
        "backend-vulkan",
        "live_av1_decode_command_record_probe_reports_status",
        "--",
        "--ignored",
        "--nocapture",
    ]);
    if args.record_command_buffer {
        command.env("VIDEO_HW_VULKAN_AV1_RECORD_COMMAND_BUFFER", "1");
        command.env("VIDEO_HW_VULKAN_AV1_RECORD_MODE", &args.record_mode);
        if args.submit_command_buffer {
            command.env("VIDEO_HW_VULKAN_AV1_SUBMIT_COMMAND_BUFFER", "1");
        }
        if args.readback {
            command.env("VIDEO_HW_VULKAN_AV1_READBACK", "1");
        }
    }
    if let Some(input_path) = &input_path {
        command.env("VIDEO_HW_VULKAN_AV1_PROBE_BITSTREAM_PATH", input_path);
    }
    let output = command
        .output()
        .map_err(|err| format!("failed to run Vulkan AV1 record probe: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let report_path = args
        .output_dir
        .join(format!("vulkan-av1-record-probe-{run_id}.md"));
    let status = if output.status.success() {
        "PASS"
    } else {
        "FAIL"
    };
    let report = format!(
        "# Vulkan AV1 Command Record Probe\n\n\
         Status: {status}\n\n\
         record_command_buffer: `{}`\n\n\
         record_mode: `{}`\n\n\
         submit_command_buffer: `{}`\n\n\
         readback: `{}`\n\n\
         input: `{}`\n\n\
         generated_ffmpeg_obu: `{}`\n\n\
         ## stdout\n\n```text\n{}\n```\n\n\
         ## stderr\n\n```text\n{}\n```\n",
        args.record_command_buffer,
        args.record_mode,
        args.submit_command_buffer,
        args.readback,
        input_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "synthetic-reduced-still".to_string()),
        args.generate_ffmpeg_obu,
        stdout.trim(),
        stderr.trim()
    );
    fs::write(&report_path, report)
        .map_err(|err| format!("failed to write report {}: {err}", report_path.display()))?;
    println!("report: {}", report_path.display());

    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Vulkan AV1 record probe failed with status {}",
            output.status
        ))
    }
}

struct Args {
    output_dir: PathBuf,
    skip_build: bool,
    record_command_buffer: bool,
    record_mode: String,
    submit_command_buffer: bool,
    readback: bool,
    input: Option<PathBuf>,
    generate_ffmpeg_obu: bool,
    ffmpeg: PathBuf,
    width: u32,
    height: u32,
    frames: u32,
    fps: u32,
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Self, String> {
        let mut output_dir = PathBuf::from("output/vulkan-av1-record-probe");
        let mut skip_build = false;
        let mut record_command_buffer = true;
        let mut record_mode = "full".to_string();
        let mut submit_command_buffer = false;
        let mut readback = false;
        let mut input = None;
        let mut generate_ffmpeg_obu = false;
        let mut ffmpeg = env::var("FFMPEG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("ffmpeg"));
        let mut width = 320;
        let mut height = 180;
        let mut frames = 1;
        let mut fps = 30;
        let mut iter = raw.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--output-dir" => {
                    output_dir = PathBuf::from(next_value(&mut iter, "--output-dir")?);
                }
                "--input" => {
                    input = Some(PathBuf::from(next_value(&mut iter, "--input")?));
                }
                "--generate-ffmpeg-obu" => generate_ffmpeg_obu = true,
                "--ffmpeg" => {
                    ffmpeg = PathBuf::from(next_value(&mut iter, "--ffmpeg")?);
                }
                "--width" => width = parse_u32(next_value(&mut iter, "--width")?, "--width")?,
                "--height" => height = parse_u32(next_value(&mut iter, "--height")?, "--height")?,
                "--frames" => frames = parse_u32(next_value(&mut iter, "--frames")?, "--frames")?,
                "--fps" => fps = parse_u32(next_value(&mut iter, "--fps")?, "--fps")?,
                "--skip-build" => skip_build = true,
                "--no-record-command-buffer" => record_command_buffer = false,
                "--record-command-buffer" => record_command_buffer = true,
                "--submit-command-buffer" => submit_command_buffer = true,
                "--no-submit-command-buffer" => submit_command_buffer = false,
                "--readback" => {
                    readback = true;
                    record_command_buffer = true;
                    submit_command_buffer = true;
                }
                "--no-readback" => readback = false,
                "--record-mode" => {
                    record_mode = next_value(&mut iter, "--record-mode")?;
                    validate_record_mode(&record_mode)?;
                }
                "--help" | "-h" => {
                    return Err(
                        "usage: cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs [--output-dir DIR] [--input OBU|--generate-ffmpeg-obu] [--ffmpeg PATH] [--width N] [--height N] [--frames N] [--fps N] [--skip-build] [--record-command-buffer|--no-record-command-buffer] [--record-mode barrier_only|begin_end|reset_end|first_decode|full] [--submit-command-buffer|--no-submit-command-buffer] [--readback|--no-readback]"
                            .to_string(),
                    );
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        if input.is_some() && generate_ffmpeg_obu {
            return Err("--input and --generate-ffmpeg-obu are mutually exclusive".to_string());
        }
        if width == 0 || height == 0 || frames == 0 || fps == 0 {
            return Err("--width/--height/--frames/--fps must be non-zero".to_string());
        }
        if readback {
            record_command_buffer = true;
            submit_command_buffer = true;
            if record_mode != "full" {
                return Err("--readback requires --record-mode full".to_string());
            }
        }
        Ok(Self {
            output_dir,
            skip_build,
            record_command_buffer,
            record_mode,
            submit_command_buffer,
            readback,
            input,
            generate_ffmpeg_obu,
            ffmpeg,
            width,
            height,
            frames,
            fps,
        })
    }
}

fn parse_u32(value: String, name: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|err| format!("invalid {name} value {value:?}: {err}"))
}

fn validate_record_mode(mode: &str) -> Result<(), String> {
    match mode {
        "barrier_only" | "begin_end" | "reset_end" | "first_decode" | "full" => Ok(()),
        other => Err(format!("unsupported --record-mode: {other}")),
    }
}

fn next_value(
    iter: &mut impl Iterator<Item = String>,
    name: &str,
) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("missing value for {name}"))
}

fn absolute_path(path: &PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.clone())
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|err| format!("failed to resolve current directory: {err}"))
    }
}

fn run(command: &mut Command, label: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|err| format!("failed to run {label}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} failed with status {status}"))
    }
}

fn generate_ffmpeg_av1_obu(args: &Args, output: &PathBuf) -> Result<(), String> {
    let size = format!("{}x{}", args.width, args.height);
    let source = format!("testsrc2=size={size}:rate={}", args.fps);
    let frames = args.frames.to_string();
    let output = output.display().to_string();
    run(
        Command::new(&args.ffmpeg).args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &source,
            "-frames:v",
            &frames,
            "-an",
            "-c:v",
            "libaom-av1",
            "-cpu-used",
            "8",
            "-g",
            "1",
            "-lag-in-frames",
            "0",
            "-f",
            "obu",
            &output,
        ]),
        "generate FFmpeg AV1 OBU probe input",
    )
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}
