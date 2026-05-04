#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
anyhow = "1"
clap = { version = "4.5", features = ["derive"] }
---

use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Run the fMP4 decode access-pattern benchmark example")]
struct Args {
    /// Features used when compiling video-hw-fmp4.
    #[arg(long)]
    features: Option<String>,

    /// Use the release profile for the benchmark binary.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    release: bool,

    /// Arguments forwarded to benchmark_decode_access after `--`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    forwarded: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let features = args.features.unwrap_or_else(default_features_for_host);
    let mut command = Command::new("cargo");
    command.args([
        "run",
        "-p",
        "video-hw-fmp4",
        "--example",
        "benchmark_decode_access",
        "--features",
        &features,
    ]);
    if args.release {
        command.arg("--release");
    }
    command.arg("--");
    command.args(args.forwarded);
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("spawn benchmark example")?;
    if !status.success() {
        bail!("benchmark example failed: status={status}");
    }
    Ok(())
}

fn default_features_for_host() -> String {
    if cfg!(target_os = "macos") {
        return "backend-vt".to_string();
    }
    if cfg!(target_os = "windows") || cfg!(target_os = "linux") {
        return "backend-nvidia backend-intel backend-vulkan".to_string();
    }
    String::new()
}
