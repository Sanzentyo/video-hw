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

use anyhow::{Context, Result, bail};

#[derive(Debug)]
struct Args {
    ffmpeg: PathBuf,
    adapter_index: usize,
    width: u32,
    height: u32,
    output_dir: PathBuf,
    min_psnr_y: f64,
}

fn main() -> Result<()> {
    let mut args = parse_args()?;
    if args.width % 2 != 0 || args.height % 2 != 0 {
        bail!("NV12 probe dimensions must be even, got {}x{}", args.width, args.height);
    }
    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create {}", args.output_dir.display()))?;
    args.output_dir = fs::canonicalize(&args.output_dir)
        .with_context(|| format!("canonicalize {}", args.output_dir.display()))?;

    let sample = args.output_dir.join("ffmpeg-hevc-vulkan-parameter-sample.h265");
    let slice = args.output_dir.join("vulkan-hevc-probe-slice.h265");
    let combined = args.output_dir.join("vulkan-hevc-probe-combined.h265");
    let decoded = args.output_dir.join("vulkan-hevc-probe-combined.nv12");

    generate_ffmpeg_parameter_sample(&args, &sample)?;
    run_live_probe(&args, &sample, &slice)?;
    combine_parameter_prefix_and_slice(&sample, &slice, &combined)?;
    decode_with_ffmpeg(&args, &combined, &decoded)?;
    let summary = compare_flat_nv12(&args, &decoded)?;

    println!(
        "vulkan_hevc_encode_probe width={} height={} adapter={} slice_bytes={} mse_y={:.6} psnr_y={} mse_uv={:.6} psnr_uv={} mse_all={:.6} psnr_all={}",
        args.width,
        args.height,
        args.adapter_index,
        fs::metadata(&slice)?.len(),
        summary.mse_y,
        format_psnr(summary.psnr_y),
        summary.mse_uv,
        format_psnr(summary.psnr_uv),
        summary.mse_all,
        format_psnr(summary.psnr_all),
    );

    if summary.psnr_y < args.min_psnr_y {
        bail!(
            "Vulkan HEVC encode probe PSNR below threshold: psnr_y={} threshold={:.4}",
            format_psnr(summary.psnr_y),
            args.min_psnr_y
        );
    }
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut ffmpeg = PathBuf::from("ffmpeg");
    let mut adapter_index = 1_usize;
    let mut width = 320_u32;
    let mut height = 180_u32;
    let mut output_dir = PathBuf::from("output/vulkan-hevc-encode-probe");
    let mut min_psnr_y = 60.0_f64;

    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ffmpeg" => ffmpeg = PathBuf::from(next_value(&mut iter, "--ffmpeg")?),
            "--adapter-index" => {
                adapter_index = next_value(&mut iter, "--adapter-index")?.parse()?
            }
            "--width" => width = next_value(&mut iter, "--width")?.parse()?,
            "--height" => height = next_value(&mut iter, "--height")?.parse()?,
            "--output-dir" => output_dir = PathBuf::from(next_value(&mut iter, "--output-dir")?),
            "--min-psnr-y" => min_psnr_y = next_value(&mut iter, "--min-psnr-y")?.parse()?,
            "-h" | "--help" => {
                println!(
                    "usage: cargo +nightly -Zscript scripts/check_vulkan_hevc_encode_probe.rs [--ffmpeg PATH] [--adapter-index N] [--width N] [--height N] [--output-dir DIR] [--min-psnr-y DB]"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    Ok(Args {
        ffmpeg,
        adapter_index,
        width,
        height,
        output_dir,
        min_psnr_y,
    })
}

fn next_value(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    iter.next()
        .with_context(|| format!("missing value after {flag}"))
}

fn generate_ffmpeg_parameter_sample(args: &Args, sample: &Path) -> Result<()> {
    let duration = (1.0_f64 / 30.0).max(0.001);
    run(
        Command::new(&args.ffmpeg).args([
            "-v",
            "error",
            "-y",
            "-init_hw_device",
            &format!("vulkan:{}", args.adapter_index),
            "-f",
            "lavfi",
            "-i",
            &format!(
                "testsrc2=size={}x{}:rate=30:duration={duration}",
                args.width, args.height
            ),
            "-frames:v",
            "1",
            "-vf",
            "format=nv12,hwupload",
            "-c:v",
            "hevc_vulkan",
            "-f",
            "hevc",
            &sample.to_string_lossy(),
        ]),
        "ffmpeg hevc_vulkan parameter sample",
    )
}

fn run_live_probe(args: &Args, sample: &Path, slice: &Path) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args([
        "test",
        "-p",
        "video-hw-backend-vulkan",
        "--features",
        "backend-vulkan",
        "live_hevc_encode_session_bootstrap_reports_submit_feedback",
        "--",
        "--ignored",
        "--nocapture",
    ]);
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_LIVE_WIDTH", args.width.to_string());
    command.env(
        "VIDEO_HW_VULKAN_HEVC_ENCODE_LIVE_HEIGHT",
        args.height.to_string(),
    );
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SAMPLE_PATH", sample);
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_MODE", "sample");
    command.env(
        "VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_VUI_SAFETY",
        "preserve",
    );
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SIZE_MODE", "sample");
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_IMAGE_VIEW_MODE", "no-ycbcr");
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_DST_PREFIX_BYTES", "256");
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_DST_PREFIX_MODE", "ffmpeg");
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_CONTROL_MODE", "ffmpeg");
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_PNEXT_MODE", "ffmpeg");
    command.env(
        "VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_REFERENCE_SLOT_MODE",
        "ffmpeg",
    );
    command.env(
        "VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_SLOT_POINTER_MODE",
        "ffmpeg",
    );
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_DPB_BARRIER_MODE", "none");
    command.env(
        "VIDEO_HW_VULKAN_HEVC_ENCODE_SOURCE_PICTURE_RESOURCE_EXTENT_MODE",
        "coded",
    );
    command.env(
        "VIDEO_HW_VULKAN_HEVC_ENCODE_SESSION_H265_CREATE_INFO_MODE",
        "ffmpeg",
    );
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_DST_RANGE_MODE", "ffmpeg");
    command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_OUTPUT_PATH", slice);
    run(&mut command, "cargo live Vulkan HEVC encode probe")
}

fn combine_parameter_prefix_and_slice(sample: &Path, slice: &Path, combined: &Path) -> Result<()> {
    let sample_bytes = fs::read(sample).with_context(|| format!("read {}", sample.display()))?;
    let slice_bytes = fs::read(slice).with_context(|| format!("read {}", slice.display()))?;
    let header = leading_non_vcl_nalus(&sample_bytes)?;
    let mut output = Vec::with_capacity(header.len() + slice_bytes.len());
    output.extend_from_slice(header);
    output.extend_from_slice(&slice_bytes);
    fs::write(combined, output).with_context(|| format!("write {}", combined.display()))
}

fn leading_non_vcl_nalus(bytes: &[u8]) -> Result<&[u8]> {
    let mut cursor = 0_usize;
    let mut end = 0_usize;
    while let Some((start, prefix_len)) = find_start_code(bytes, cursor) {
        let nalu_start = start + prefix_len;
        if nalu_start >= bytes.len() {
            break;
        }
        let next_start = find_start_code(bytes, nalu_start)
            .map(|(next, _)| next)
            .unwrap_or(bytes.len());
        let nal_type = (bytes[nalu_start] & 0x7e) >> 1;
        if nal_type <= 31 {
            break;
        }
        end = next_start;
        cursor = next_start;
    }
    if end == 0 {
        bail!("parameter sample did not contain leading HEVC non-VCL NAL units");
    }
    Ok(&bytes[..end])
}

fn find_start_code(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut index = from;
    while index + 3 <= bytes.len() {
        if bytes[index..].starts_with(&[0, 0, 1]) {
            return Some((index, 3));
        }
        if index + 4 <= bytes.len() && bytes[index..].starts_with(&[0, 0, 0, 1]) {
            return Some((index, 4));
        }
        index += 1;
    }
    None
}

fn decode_with_ffmpeg(args: &Args, input: &Path, output: &Path) -> Result<()> {
    run(
        Command::new(&args.ffmpeg).args([
            "-v",
            "error",
            "-y",
            "-f",
            "hevc",
            "-i",
            &input.to_string_lossy(),
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "nv12",
            &output.to_string_lossy(),
        ]),
        "ffmpeg decode combined HEVC probe",
    )
}

#[derive(Debug)]
struct QualitySummary {
    mse_y: f64,
    mse_uv: f64,
    mse_all: f64,
    psnr_y: f64,
    psnr_uv: f64,
    psnr_all: f64,
}

fn compare_flat_nv12(args: &Args, decoded: &Path) -> Result<QualitySummary> {
    let bytes = fs::read(decoded).with_context(|| format!("read {}", decoded.display()))?;
    let y_size = usize::try_from(u64::from(args.width) * u64::from(args.height))?;
    let uv_size = y_size / 2;
    let expected_len = y_size + uv_size;
    if bytes.len() != expected_len {
        bail!(
            "decoded NV12 size mismatch: got {} bytes, expected {}",
            bytes.len(),
            expected_len
        );
    }

    let sum_y = bytes[..y_size]
        .iter()
        .map(|byte| square_diff(*byte, 16))
        .sum::<f64>();
    let sum_uv = bytes[y_size..]
        .iter()
        .map(|byte| square_diff(*byte, 128))
        .sum::<f64>();
    let mse_y = sum_y / y_size as f64;
    let mse_uv = sum_uv / uv_size as f64;
    let mse_all = (sum_y + sum_uv) / expected_len as f64;
    Ok(QualitySummary {
        mse_y,
        mse_uv,
        mse_all,
        psnr_y: psnr(mse_y),
        psnr_uv: psnr(mse_uv),
        psnr_all: psnr(mse_all),
    })
}

fn square_diff(actual: u8, expected: u8) -> f64 {
    let diff = f64::from(actual) - f64::from(expected);
    diff * diff
}

fn psnr(mse: f64) -> f64 {
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * ((255.0_f64 * 255.0) / mse).log10()
    }
}

fn format_psnr(value: f64) -> String {
    if value.is_infinite() {
        "inf".to_string()
    } else {
        format!("{value:.4}")
    }
}

fn run(command: &mut Command, label: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("spawn {label}"))?;
    if !output.status.success() {
        bail!(
            "{label} failed: status={}; stdout_tail={}; stderr_tail={}",
            output.status,
            tail(&String::from_utf8_lossy(&output.stdout)),
            tail(&String::from_utf8_lossy(&output.stderr))
        );
    }
    Ok(())
}

fn tail(text: &str) -> String {
    text.lines()
        .rev()
        .take(10)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" / ")
}
