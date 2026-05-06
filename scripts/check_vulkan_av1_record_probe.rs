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
    }
    let output = command
        .output()
        .map_err(|err| format!("failed to run Vulkan AV1 record probe: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    fs::create_dir_all(&args.output_dir).map_err(|err| {
        format!(
            "failed to create output dir {}: {err}",
            args.output_dir.display()
        )
    })?;
    let report_path = args.output_dir.join(format!(
        "vulkan-av1-record-probe-{}.md",
        unix_timestamp()
    ));
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
         ## stdout\n\n```text\n{}\n```\n\n\
         ## stderr\n\n```text\n{}\n```\n",
        args.record_command_buffer,
        args.record_mode,
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
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Self, String> {
        let mut output_dir = PathBuf::from("output/vulkan-av1-record-probe");
        let mut skip_build = false;
        let mut record_command_buffer = true;
        let mut record_mode = "full".to_string();
        let mut iter = raw.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--output-dir" => {
                    output_dir = PathBuf::from(next_value(&mut iter, "--output-dir")?);
                }
                "--skip-build" => skip_build = true,
                "--no-record-command-buffer" => record_command_buffer = false,
                "--record-command-buffer" => record_command_buffer = true,
                "--record-mode" => {
                    record_mode = next_value(&mut iter, "--record-mode")?;
                    validate_record_mode(&record_mode)?;
                }
                "--help" | "-h" => {
                    return Err(
                        "usage: cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs [--output-dir DIR] [--skip-build] [--record-command-buffer|--no-record-command-buffer] [--record-mode barrier_only|begin_end|reset_end|first_decode|full]"
                            .to_string(),
                    );
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(Self {
            output_dir,
            skip_build,
            record_command_buffer,
            record_mode,
        })
    }
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

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
