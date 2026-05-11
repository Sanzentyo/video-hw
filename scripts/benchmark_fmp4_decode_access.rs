#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
anyhow = "1"
clap = { version = "4.5", features = ["derive"] }
---

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

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

    /// Generate a synthetic fMP4 input before running the access benchmark.
    #[arg(long)]
    generate_codec: Option<String>,

    /// Backend used for generated synthetic fMP4 input.
    #[arg(long, default_value = "auto")]
    generate_backend: String,

    /// Output path for generated synthetic fMP4 input.
    #[arg(long)]
    generate_output: Option<PathBuf>,

    /// Width of generated synthetic fMP4 input.
    #[arg(long, default_value_t = 320)]
    generate_width: u32,

    /// Height of generated synthetic fMP4 input.
    #[arg(long, default_value_t = 180)]
    generate_height: u32,

    /// Frame count of generated synthetic fMP4 input.
    #[arg(long, default_value_t = 90)]
    generate_frames: u32,

    /// Fragment size in frames for generated synthetic fMP4 input.
    #[arg(long, default_value_t = 30)]
    generate_fragment_frames: u32,

    /// Require hardware when generating synthetic fMP4 input.
    #[arg(long, default_value_t = false)]
    generate_require_hardware: bool,

    /// Arguments forwarded to benchmark_decode_access after `--`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    forwarded: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let features = args
        .features
        .clone()
        .unwrap_or_else(default_features_for_host);
    let generated_input = if let Some(codec) = args.generate_codec.as_deref() {
        ensure_no_forwarded_input(&args.forwarded)?;
        let path = generated_output_path(&args, codec);
        run_generate_input(&args, &features, codec, &path)?;
        Some(path)
    } else {
        None
    };

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
    if let Some(path) = &generated_input {
        command.args(["--input", &path.to_string_lossy()]);
    }
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

fn run_generate_input(args: &Args, features: &str, codec: &str, output: &Path) -> Result<()> {
    if let Some(parent) = output.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("create generated input dir: {}", parent.display()))?;
    }
    let mut command = Command::new("cargo");
    command.args([
        "run",
        "-p",
        "video-hw-fmp4",
        "--example",
        "write_synthetic_fmp4",
        "--features",
        features,
    ]);
    if args.release {
        command.arg("--release");
    }
    command.args([
        "--",
        "--output",
        &output.to_string_lossy(),
        "--backend",
        &args.generate_backend,
        "--codec",
        codec,
        "--width",
        &args.generate_width.to_string(),
        "--height",
        &args.generate_height.to_string(),
        "--frames",
        &args.generate_frames.to_string(),
        "--fragment-frames",
        &args.generate_fragment_frames.to_string(),
    ]);
    if args.generate_require_hardware {
        command.arg("--require-hardware");
    }
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("spawn synthetic fMP4 generator")?;
    if !status.success() {
        bail!("synthetic fMP4 generation failed: status={status}");
    }
    Ok(())
}

fn generated_output_path(args: &Args, codec: &str) -> PathBuf {
    args.generate_output.clone().unwrap_or_else(|| {
        PathBuf::from("output").join(format!(
            "benchmark-fmp4-decode-access-input-{}-{}-{}x{}-{}f.mp4",
            codec,
            args.generate_backend,
            args.generate_width,
            args.generate_height,
            args.generate_frames
        ))
    })
}

fn ensure_no_forwarded_input(forwarded: &[String]) -> Result<()> {
    let has_input = forwarded
        .iter()
        .any(|arg| arg == "--input" || arg.starts_with("--input="));
    if has_input {
        bail!("--generate-codec cannot be combined with forwarded --input");
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
