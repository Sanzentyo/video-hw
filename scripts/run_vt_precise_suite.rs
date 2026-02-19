#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
anyhow = "1"
clap = { version = "4.5", features = ["derive"] }
---

use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Run VT precise benchmark suite serially (h264 + hevc)")]
struct Args {
    #[arg(long, default_value_t = true)]
    release: bool,

    #[arg(long, default_value_t = 1)]
    warmup: usize,

    #[arg(long, default_value_t = 3)]
    repeat: usize,

    #[arg(long, default_value_t = true)]
    verify: bool,

    #[arg(long, default_value_t = true)]
    equal_raw_input: bool,

    #[arg(long, default_value_t = true)]
    include_internal_metrics: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    run_one("h264", &args)?;
    run_one("hevc", &args)?;

    println!("[vt-suite] done");
    Ok(())
}

fn run_one(codec: &str, args: &Args) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args([
        "+nightly",
        "-Zscript",
        "scripts/benchmark_ffmpeg_vt_precise.rs",
        "--codec",
        codec,
        "--warmup",
        &args.warmup.to_string(),
        "--repeat",
        &args.repeat.to_string(),
    ]);

    if args.release {
        cmd.arg("--release");
    }
    if args.verify {
        cmd.arg("--verify");
    }
    if args.equal_raw_input {
        cmd.arg("--equal-raw-input");
    }
    if args.include_internal_metrics {
        cmd.arg("--include-internal-metrics");
    }

    println!("[vt-suite] start codec={codec}");
    let status = cmd
        .status()
        .with_context(|| format!("spawn vt precise benchmark ({codec})"))?;
    if !status.success() {
        bail!("vt precise benchmark failed for codec={codec} (status={status})");
    }
    println!("[vt-suite] done  codec={codec}");
    Ok(())
}
