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

use anyhow::{anyhow, bail, Context, Result};

#[derive(Debug)]
struct Args {
    output_dir: PathBuf,
    decode_bin: PathBuf,
    vulkan_adapter_index: Option<usize>,
    min_psnr_y: f64,
    skip_build: bool,
    quick: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputFormat {
    Obu,
    Fmp4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    Pass,
    UnsupportedAlias,
}

#[derive(Debug, Clone, Copy)]
struct Case {
    name: &'static str,
    input_format: InputFormat,
    frames: u32,
    gop_size: u32,
    lag_in_frames: u32,
    expectation: Expectation,
}

#[derive(Debug)]
struct CaseResult {
    case: Case,
    success: bool,
    status: String,
    report: Option<PathBuf>,
    stdout_tail: String,
    stderr_tail: String,
}

const CASES: &[Case] = &[
    Case {
        name: "obu-keyframe-8f",
        input_format: InputFormat::Obu,
        frames: 8,
        gop_size: 1,
        lag_in_frames: 0,
        expectation: Expectation::Pass,
    },
    Case {
        name: "fmp4-keyframe-8f",
        input_format: InputFormat::Fmp4,
        frames: 8,
        gop_size: 1,
        lag_in_frames: 0,
        expectation: Expectation::Pass,
    },
    Case {
        name: "obu-gop30-8f",
        input_format: InputFormat::Obu,
        frames: 8,
        gop_size: 30,
        lag_in_frames: 0,
        expectation: Expectation::Pass,
    },
    Case {
        name: "fmp4-gop30-8f",
        input_format: InputFormat::Fmp4,
        frames: 8,
        gop_size: 30,
        lag_in_frames: 0,
        expectation: Expectation::Pass,
    },
    Case {
        name: "obu-gop30-lag25-16f",
        input_format: InputFormat::Obu,
        frames: 16,
        gop_size: 30,
        lag_in_frames: 25,
        expectation: Expectation::Pass,
    },
    Case {
        name: "fmp4-gop30-lag25-16f",
        input_format: InputFormat::Fmp4,
        frames: 16,
        gop_size: 30,
        lag_in_frames: 25,
        expectation: Expectation::Pass,
    },
    Case {
        name: "obu-gop16-lag8-32f",
        input_format: InputFormat::Obu,
        frames: 32,
        gop_size: 16,
        lag_in_frames: 8,
        expectation: Expectation::Pass,
    },
    Case {
        name: "fmp4-gop16-lag8-32f",
        input_format: InputFormat::Fmp4,
        frames: 32,
        gop_size: 16,
        lag_in_frames: 8,
        expectation: Expectation::Pass,
    },
    Case {
        name: "obu-gop30-lag25-32f-alias",
        input_format: InputFormat::Obu,
        frames: 32,
        gop_size: 30,
        lag_in_frames: 25,
        expectation: Expectation::UnsupportedAlias,
    },
    Case {
        name: "fmp4-gop30-lag25-32f-alias",
        input_format: InputFormat::Fmp4,
        frames: 32,
        gop_size: 30,
        lag_in_frames: 25,
        expectation: Expectation::UnsupportedAlias,
    },
];

fn main() -> Result<()> {
    let args = parse_args()?;
    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create output dir: {}", args.output_dir.display()))?;
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
                "--release",
            ]),
            "build release decode_to_yuv",
        )?;
    }

    let run_id = unix_millis();
    let cases = if args.quick { &CASES[..6] } else { CASES };
    let mut results = Vec::with_capacity(cases.len());
    for case in cases {
        println!(
            "vulkan_av1_corpus case={} expectation={:?}",
            case.name, case.expectation
        );
        results.push(run_case(&args, *case)?);
    }
    let report = write_report(&args, run_id, &results)?;
    let failed = results.iter().filter(|result| !result.success).count();
    println!(
        "vulkan_av1_corpus_matrix cases={} failed={} report={}",
        results.len(),
        failed,
        report.display()
    );
    if failed != 0 {
        bail!("Vulkan AV1 corpus matrix had {failed} failing case(s)");
    }
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut output_dir = PathBuf::from("output/vulkan-av1-corpus-matrix");
    let mut decode_bin = PathBuf::from("target/release/examples/decode_to_yuv.exe");
    let mut vulkan_adapter_index = None;
    let mut min_psnr_y = 60.0;
    let mut skip_build = false;
    let mut quick = false;
    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--output-dir" => output_dir = PathBuf::from(next_value(&mut iter, "--output-dir")?),
            "--decode-bin" => decode_bin = PathBuf::from(next_value(&mut iter, "--decode-bin")?),
            "--vulkan-adapter-index" => {
                vulkan_adapter_index =
                    Some(next_value(&mut iter, "--vulkan-adapter-index")?.parse()?)
            }
            "--min-psnr-y" => min_psnr_y = next_value(&mut iter, "--min-psnr-y")?.parse()?,
            "--skip-build" => skip_build = true,
            "--quick" => quick = true,
            "-h" | "--help" => {
                println!(
                    "usage: cargo +nightly -Zscript scripts/check_vulkan_av1_corpus_matrix.rs [--output-dir DIR] [--decode-bin PATH] [--vulkan-adapter-index N] [--min-psnr-y DB] [--skip-build] [--quick]"
                );
                std::process::exit(0);
            }
            other => return Err(anyhow!("unknown option {other}")),
        }
    }
    Ok(Args {
        output_dir,
        decode_bin,
        vulkan_adapter_index,
        min_psnr_y,
        skip_build,
        quick,
    })
}

fn run_case(args: &Args, case: Case) -> Result<CaseResult> {
    let before = latest_psnr_report()?;
    let mut command = Command::new("cargo");
    command.args([
        "+nightly",
        "-Zscript",
        "scripts/check_vulkan_av1_psnr.rs",
        "--frames",
        &case.frames.to_string(),
        "--width",
        "320",
        "--height",
        "180",
        "--gop-size",
        &case.gop_size.to_string(),
        "--lag-in-frames",
        &case.lag_in_frames.to_string(),
        "--min-psnr-y",
        &args.min_psnr_y.to_string(),
        "--decode-bin",
    ]);
    command.arg(&args.decode_bin);
    if matches!(case.input_format, InputFormat::Fmp4) {
        command.args(["--input-format", "fmp4"]);
    }
    if let Some(index) = args.vulkan_adapter_index {
        command.args(["--vulkan-adapter-index", &index.to_string()]);
    }
    let output = command
        .output()
        .with_context(|| format!("spawn case {}", case.name))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{stdout}\n{stderr}");
    let report = latest_psnr_report()?.filter(|path| Some(path) != before.as_ref());
    let success = match case.expectation {
        Expectation::Pass => output.status.success(),
        Expectation::UnsupportedAlias => {
            !output.status.success() && combined.contains("aliases Vulkan DPB slot")
        }
    };
    let status = if success {
        match case.expectation {
            Expectation::Pass => "passed".to_string(),
            Expectation::UnsupportedAlias => "expected unsupported alias".to_string(),
        }
    } else {
        format!("unexpected result: process_status={}", output.status)
    };
    Ok(CaseResult {
        case,
        success,
        status,
        report,
        stdout_tail: tail_line(&stdout),
        stderr_tail: tail_line(&stderr),
    })
}

fn latest_psnr_report() -> Result<Option<PathBuf>> {
    let dir = Path::new("output/vulkan-av1-psnr");
    if !dir.exists() {
        return Ok(None);
    }
    let latest = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "md")
        })
        .map(|entry| {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);
            (modified, entry.path())
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path);
    Ok(latest)
}

fn write_report(args: &Args, run_id: u128, results: &[CaseResult]) -> Result<PathBuf> {
    let path = args
        .output_dir
        .join(format!("vulkan-av1-corpus-matrix-{run_id}.md"));
    let mut text = String::new();
    text.push_str("# Vulkan AV1 Corpus Matrix\n\n");
    text.push_str(&format!("epoch_millis: {run_id}\n\n"));
    text.push_str(&format!("decode_bin: `{}`\n\n", args.decode_bin.display()));
    text.push_str(&format!("min_psnr_y: `{:.4}`\n\n", args.min_psnr_y));
    text.push_str("| Case | Input | Frames | GOP | Lag | Expectation | Status | Report |\n");
    text.push_str("|---|---|---:|---:|---:|---|---|---|\n");
    for result in results {
        let report = result
            .report
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        text.push_str(&format!(
            "| {} | {:?} | {} | {} | {} | {:?} | {} | {} |\n",
            result.case.name,
            result.case.input_format,
            result.case.frames,
            result.case.gop_size,
            result.case.lag_in_frames,
            result.case.expectation,
            result.status,
            report
        ));
    }
    text.push_str("\n## Tails\n\n");
    for result in results {
        text.push_str(&format!("### {}\n\n", result.case.name));
        text.push_str(&format!(
            "stdout_tail: `{}`\n\n",
            escape_tick(&result.stdout_tail)
        ));
        text.push_str(&format!(
            "stderr_tail: `{}`\n\n",
            escape_tick(&result.stderr_tail)
        ));
    }
    fs::write(&path, text).with_context(|| format!("write report: {}", path.display()))?;
    Ok(path)
}

fn next_value(iter: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    iter.next()
        .with_context(|| format!("{name} requires a value"))
}

fn run(command: &mut Command, label: &str) -> Result<()> {
    let output = command.output().with_context(|| format!("spawn {label}"))?;
    if !output.status.success() {
        bail!(
            "{label} failed: status={}; stdout_tail={}; stderr_tail={}",
            output.status,
            tail_line(&String::from_utf8_lossy(&output.stdout)),
            tail_line(&String::from_utf8_lossy(&output.stderr))
        );
    }
    Ok(())
}

fn tail_line(text: &str) -> String {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("-")
        .chars()
        .take(600)
        .collect()
}

fn escape_tick(text: &str) -> String {
    text.replace('`', "'")
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}
