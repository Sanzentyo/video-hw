#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
anyhow = "1"
---

//! Quality check: PSNR-based verification of video-hw encode and decode
//! against FFmpeg as reference.
//!
//! Usage:
//!   cargo +nightly -Zscript scripts/quality_check.rs [-- --help]
//!
//! Prerequisites:
//!   - FFmpeg in PATH (or FFMPEG_PATH env var)
//!   - Built examples: `cargo build --examples --all-features`
//!   - Sample videos in sample-videos/ (foreman_cif.*)

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

// ── Configuration ─────────────────────────────────────────────────────────────

const SAMPLE_MP4: &str = "sample-videos/foreman_cif.mp4";
const SAMPLE_H264: &str = "sample-videos/foreman_cif.h264";
const SAMPLE_H265: &str = "sample-videos/foreman_cif.h265";
const SAMPLE_FMP4: &str = "sample-videos/foreman_cif_fmp4.mp4";
const FRAME_COUNT: usize = 300;
const WIDTH: usize = 352;
const HEIGHT: usize = 288;

/// Minimum acceptable PSNR (dB) for encode quality tests.
/// Hardware encoders (NVENC, Quick Sync, Vulkan Video) typically achieve 25–27 dB on CIF
/// content at moderate bitrate, compared to FFmpeg's software CRF20 reference.
const PSNR_ENCODE_MIN: f64 = 25.0;
/// Minimum acceptable PSNR (dB) for decode pixel-output tests.
const PSNR_DECODE_MIN: f64 = 25.0;

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let run_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let output_dir = PathBuf::from(format!("output/quality-check-{run_id}"));
    fs::create_dir_all(&output_dir).context("failed to create output dir")?;

    let ffmpeg = find_ffmpeg()?;
    let cargo_bin = find_cargo_bin()?;

    eprintln!("FFmpeg: {}", ffmpeg.display());
    eprintln!("Output: {}", output_dir.display());
    eprintln!();

    let mut report = String::new();
    writeln!(report, "# Quality Check Report").unwrap();
    writeln!(report, "").unwrap();
    writeln!(report, "Run ID: {run_id}  ").unwrap();
    writeln!(report, "FFmpeg: {}  ", ffmpeg.display()).unwrap();
    writeln!(report).unwrap();
    writeln!(report, "## Environment").unwrap();
    writeln!(report, "").unwrap();
    let ver = run_output(&ffmpeg, &["-version"])
        .map(|o| o.lines().next().unwrap_or("").to_string())
        .unwrap_or_else(|_| "unknown".into());
    writeln!(report, "- FFmpeg: {ver}  ").unwrap();
    writeln!(report).unwrap();

    // Extract reference raw ARGB frames (for encode quality test).
    // RawFrameBuffer::Argb8888 is TRUE ARGB: byte[0]=A, byte[1]=R, byte[2]=G, byte[3]=B.
    let argb_path = output_dir.join("foreman_ref.argb");
    extract_argb_frames(&ffmpeg, SAMPLE_MP4, &argb_path)?;

    // Extract per-codec RGB24 reference frames (for decode quality test).
    let ref_rgb_h264 = output_dir.join("ref_h264.rgb");
    let ref_rgb_h265 = output_dir.join("ref_h265.rgb");
    extract_rgb24_frames(&ffmpeg, SAMPLE_H264, &ref_rgb_h264)?;
    extract_rgb24_frames(&ffmpeg, SAMPLE_H265, &ref_rgb_h265)?;

    let mut all_results: Vec<TestResult> = Vec::new();

    // ── Encode quality ─────────────────────────────────────────────────────────
    writeln!(report, "## Encode Quality (PSNR vs FFmpeg reference decode)").unwrap();
    writeln!(report, "").unwrap();
    writeln!(report, "| Backend | Codec | PSNR Y (dB) | PSNR Avg (dB) | Status |").unwrap();
    writeln!(report, "|---------|-------|-------------|---------------|--------|").unwrap();

    for (backend, codec, codec_cli) in encode_cases() {
        let result = run_encode_quality(
            &cargo_bin, &ffmpeg, &output_dir, &argb_path,
            backend, codec, codec_cli,
        );
        let row = format_result_row(&result, PSNR_ENCODE_MIN);
        writeln!(report, "{row}").unwrap();
        all_results.push(result);
    }
    writeln!(report).unwrap();

    // ── Decode quality (Intel/Vulkan only) ─────────────────────────────────────
    writeln!(report, "## Decode Pixel Quality (PSNR vs FFmpeg software decode)").unwrap();
    writeln!(report, "").unwrap();
    writeln!(report, "| Backend | Codec | PSNR Y (dB) | PSNR Avg (dB) | Status |").unwrap();
    writeln!(report, "|---------|-------|-------------|---------------|--------|").unwrap();

    for (backend, codec) in decode_quality_cases() {
        // Use the reference decoded from the same source bitstream being tested.
        let ref_rgb = if codec == "hevc" { &ref_rgb_h265 } else { &ref_rgb_h264 };
        let result = run_decode_quality(
            &cargo_bin, &ffmpeg, &output_dir, ref_rgb,
            backend, codec,
        );
        let row = format_result_row(&result, PSNR_DECODE_MIN);
        writeln!(report, "{row}").unwrap();
        all_results.push(result);
    }
    writeln!(report).unwrap();

    // ── fMP4 frame count ───────────────────────────────────────────────────────
    writeln!(report, "## fMP4 Decode Frame Count (LengthPrefixedSample path)").unwrap();
    writeln!(report, "").unwrap();
    writeln!(report, "| Backend | Codec | Frames | Expected | Status |").unwrap();
    writeln!(report, "|---------|-------|--------|----------|--------|").unwrap();

    for (backend, codec) in fmp4_frame_count_cases() {
        let result = run_fmp4_frame_count(&cargo_bin, &output_dir, backend, codec);
        let row = format_frame_count_row(&result, FRAME_COUNT);
        writeln!(report, "{row}").unwrap();
        all_results.push(result);
    }
    writeln!(report).unwrap();

    // ── Summary ────────────────────────────────────────────────────────────────
    let pass = all_results.iter().filter(|r| r.passed).count();
    let total = all_results.len();
    writeln!(report, "## Summary").unwrap();
    writeln!(report, "").unwrap();
    writeln!(report, "**{pass}/{total} tests passed**").unwrap();
    writeln!(report).unwrap();

    let report_path = output_dir.join("report.md");
    fs::write(&report_path, &report).context("failed to write report")?;
    eprintln!("\nReport: {}", report_path.display());
    print!("{report}");

    if pass < total {
        bail!("{} test(s) failed", total - pass);
    }
    Ok(())
}

// ── Test cases ────────────────────────────────────────────────────────────────

fn encode_cases() -> Vec<(&'static str, &'static str, &'static str)> {
    // (backend, human-label, codec-cli)
    let mut cases = vec![];
    for backend in ["nvidia", "intel", "vulkan"] {
        for (codec_label, codec_cli) in [("H264", "h264"), ("HEVC", "hevc")] {
            cases.push((backend, codec_label, codec_cli));
        }
    }
    cases
}

fn decode_quality_cases() -> Vec<(&'static str, &'static str)> {
    vec![
        ("nvidia", "h264"),
        ("nvidia", "hevc"),
        ("intel", "h264"),
        ("intel", "hevc"),
        ("vulkan", "h264"),
        ("vulkan", "hevc"),
    ]
}

fn fmp4_frame_count_cases() -> Vec<(&'static str, &'static str)> {
    vec![
        ("nvidia", "h264"),
        ("intel", "h264"),
        ("vulkan", "h264"),
    ]
}

// ── Test runner helpers ────────────────────────────────────────────────────────

#[derive(Debug)]
struct TestResult {
    label: String,
    passed: bool,
    psnr_y: Option<f64>,
    psnr_avg: Option<f64>,
    frame_count: Option<usize>,
    error: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
enum TestKind {
    EncodeQuality,
    DecodeQuality,
    FmpFrameCount,
}

fn run_encode_quality(
    cargo_bin: &Path,
    ffmpeg: &Path,
    output_dir: &Path,
    argb_path: &Path,
    backend: &str,
    codec_label: &str,
    codec_cli: &str,
) -> TestResult {
    let label = format!("{backend} / {codec_label} encode");

    let inner = || -> Result<(f64, f64)> {
        let encoded = output_dir.join(format!("encoded_{backend}_{codec_cli}.bin"));
        encode_with_backend(cargo_bin, backend, codec_cli, argb_path, &encoded)?;
        let (psnr_y, psnr_avg) = compute_psnr_encode(ffmpeg, &encoded, codec_cli, SAMPLE_MP4, output_dir,
            &format!("{backend}_{codec_cli}"))?;
        Ok((psnr_y, psnr_avg))
    };

    match inner() {
        Ok((py, pa)) => TestResult {
            label,
            passed: py >= PSNR_ENCODE_MIN,
            psnr_y: Some(py),
            psnr_avg: Some(pa),
            frame_count: None,
            error: None,
        },
        Err(e) => TestResult {
            label,
            passed: false,
            psnr_y: None,
            psnr_avg: None,
            frame_count: None,
            error: Some(format!("{e:#}")),
        },
    }
}

fn run_decode_quality(
    cargo_bin: &Path,
    ffmpeg: &Path,
    output_dir: &Path,
    ref_rgb_path: &Path,
    backend: &str,
    codec: &str,
) -> TestResult {
    let label = format!("{backend} / {codec} decode pixels");

    let inner = || -> Result<(f64, f64)> {
        let hw_rgb = output_dir.join(format!("decoded_{backend}_{codec}.rgb"));
        let input = if codec == "hevc" { SAMPLE_H265 } else { SAMPLE_H264 };
        decode_pixels_with_backend(cargo_bin, backend, codec, input, &hw_rgb)?;
        let (py, pa) = compute_psnr_decode(ffmpeg, &hw_rgb, ref_rgb_path, output_dir,
            &format!("{backend}_{codec}"))?;
        Ok((py, pa))
    };

    match inner() {
        Ok((py, pa)) => TestResult {
            label,
            passed: py >= PSNR_DECODE_MIN,
            psnr_y: Some(py),
            psnr_avg: Some(pa),
            frame_count: None,
            error: None,
        },
        Err(e) => TestResult {
            label,
            passed: false,
            psnr_y: None,
            psnr_avg: None,
            frame_count: None,
            error: Some(format!("{e:#}")),
        },
    }
}

fn run_fmp4_frame_count(
    cargo_bin: &Path,
    _output_dir: &Path,
    backend: &str,
    codec: &str,
) -> TestResult {
    let label = format!("{backend} / {codec} fMP4 frames");

    let inner = || -> Result<usize> {
        let stdout = run_output_checked(
            cargo_bin,
            &[
                "run", "--release",
                "--example", "decode_to_yuv",
                "--all-features",
                "--",
                "--backend", backend,
                "--codec", codec,
                "--input", SAMPLE_FMP4,
                "--input-format", "mp4",
                "--output-mode", "metadata",
            ],
        )?;
        parse_frames_from_stdout(&stdout)
    };

    match inner() {
        Ok(n) => TestResult {
            label,
            passed: n == FRAME_COUNT,
            psnr_y: None,
            psnr_avg: None,
            frame_count: Some(n),
            error: None,
        },
        Err(e) => TestResult {
            label,
            passed: false,
            psnr_y: None,
            psnr_avg: None,
            frame_count: None,
            error: Some(format!("{e:#}")),
        },
    }
}

// ── Sub-steps ─────────────────────────────────────────────────────────────────

fn extract_argb_frames(ffmpeg: &Path, input: &str, out: &Path) -> Result<()> {
    if out.exists() {
        eprintln!("  Skipping ARGB extraction (already exists): {}", out.display());
        return Ok(());
    }
    eprintln!("  Extracting ARGB reference frames from {input} ...");
    // RawFrameBuffer::Argb8888 is TRUE ARGB: byte[0]=A, byte[1]=R, byte[2]=G, byte[3]=B.
    // FFmpeg `argb` pixel format matches this byte order exactly.
    run_checked(ffmpeg, &[
        "-y", "-i", input,
        "-vsync", "0",
        "-pix_fmt", "argb",
        "-f", "rawvideo",
        out.to_str().unwrap_or(""),
    ])
    .context("extract ARGB frames")?;
    Ok(())
}

fn extract_rgb24_frames(ffmpeg: &Path, input: &str, out: &Path) -> Result<()> {
    if out.exists() {
        eprintln!("  Skipping RGB24 reference extraction (already exists): {}", out.display());
        return Ok(());
    }
    eprintln!("  Extracting RGB24 reference frames from {input} ...");
    run_checked(ffmpeg, &[
        "-y", "-i", input,
        "-vsync", "0",
        "-pix_fmt", "rgb24",
        "-f", "rawvideo",
        out.to_str().unwrap_or(""),
    ])
    .context("extract RGB24 reference")?;
    Ok(())
}

fn encode_with_backend(
    cargo_bin: &Path,
    backend: &str,
    codec: &str,
    argb_in: &Path,
    output: &Path,
) -> Result<()> {
    eprintln!("  Encoding {backend}/{codec} ...");
    run_checked(
        cargo_bin,
        &[
            "run", "--release",
            "--example", "encode_raw_argb",
            "--all-features",
            "--",
            "--backend", backend,
            "--codec", codec,
            "--width", &WIDTH.to_string(),
            "--height", &HEIGHT.to_string(),
            "--frame-count", &FRAME_COUNT.to_string(),
            "--fps", "30",
            "--input-raw", argb_in.to_str().unwrap_or(""),
            "--input-pix-fmt", "argb",
            "--output", output.to_str().unwrap_or(""),
        ],
    )
    .context("encode_raw_argb")?;
    Ok(())
}

fn decode_pixels_with_backend(
    cargo_bin: &Path,
    backend: &str,
    codec: &str,
    input: &str,
    output: &Path,
) -> Result<()> {
    eprintln!("  Decoding pixels {backend}/{codec} ...");
    run_checked(
        cargo_bin,
        &[
            "run", "--release",
            "--example", "decode_to_yuv",
            "--all-features",
            "--",
            "--backend", backend,
            "--codec", codec,
            "--input", input,
            "--output-mode", "rgb24",
            "--output", output.to_str().unwrap_or(""),
        ],
    )
    .context("decode_to_yuv")?;
    Ok(())
}

/// Compute PSNR between encoded output and original MP4 reference.
///
/// Adds explicit `-f h264`/`-f hevc` so FFmpeg can demux elementary streams,
/// and uses `trim=end_frame` to ensure both sides have exactly `FRAME_COUNT` frames
/// even when the encoded stream uses a slightly different clock (30 fps vs 29.97 fps).
fn compute_psnr_encode(
    ffmpeg: &Path,
    encoded: &Path,
    codec: &str,
    reference_mp4: &str,
    output_dir: &Path,
    tag: &str,
) -> Result<(f64, f64)> {
    let fmt = if codec == "hevc" { "hevc" } else { "h264" };
    let stats_file = output_dir.join(format!("psnr_enc_{tag}.txt"));
    // On Windows, FFmpeg lavfi treats backslashes as escape characters inside filter strings.
    // Use forward slashes in the path passed to the stats_file option.
    let stats_path = stats_file.to_str().unwrap_or("").replace('\\', "/");
    let _out = run_checked(
        ffmpeg,
        &[
            "-y",
            // Force 30 fps on the elementary stream: raw H.264/H.265 AnnexB has no
            // container-level timing; FFmpeg defaults to 25 fps, causing a 5-frame
            // drift at frame 300 vs the 30 fps reference.
            "-f", fmt, "-r", "30",
            "-i", encoded.to_str().unwrap_or(""),
            "-i", reference_mp4,
            "-lavfi",
            &format!(
                "[0:v]trim=end_frame={FRAME_COUNT},setpts=PTS-STARTPTS,format=yuv420p[a];\
                 [1:v]trim=end_frame={FRAME_COUNT},setpts=PTS-STARTPTS,format=yuv420p[b];\
                 [a][b]psnr=stats_file={stats_path}",
            ),
            "-f", "null", "-",
        ],
    )
    .context("ffmpeg PSNR (encode)")?;
    parse_psnr_stats(&stats_file)
}

/// Compute PSNR between hardware-decoded RGB24 and FFmpeg-decoded RGB24.
///
/// Both streams are converted to YUV420p before comparison so that the
/// PSNR stats file contains `psnr_y:` and `psnr_avg:` entries.
fn compute_psnr_decode(
    ffmpeg: &Path,
    hw_rgb: &Path,
    ref_rgb: &Path,
    output_dir: &Path,
    tag: &str,
) -> Result<(f64, f64)> {
    let stats_file = output_dir.join(format!("psnr_dec_{tag}.txt"));
    // Forward slashes required in lavfi filter strings on Windows.
    let stats_path = stats_file.to_str().unwrap_or("").replace('\\', "/");
    let size_str = format!("{WIDTH}x{HEIGHT}");
    let _out = run_checked(
        ffmpeg,
        &[
            "-y",
            "-f", "rawvideo", "-pix_fmt", "rgb24",
            "-s", &size_str, "-framerate", "30",
            "-i", hw_rgb.to_str().unwrap_or(""),
            "-f", "rawvideo", "-pix_fmt", "rgb24",
            "-s", &size_str, "-framerate", "30",
            "-i", ref_rgb.to_str().unwrap_or(""),
            "-lavfi",
            &format!(
                "[0:v]format=yuv420p[a];\
                 [1:v]format=yuv420p[b];\
                 [a][b]psnr=stats_file={stats_path}",
            ),
            "-f", "null", "-",
        ],
    )
    .context("ffmpeg PSNR (decode)")?;
    parse_psnr_stats(&stats_file)
}

// ── Parsing helpers ────────────────────────────────────────────────────────────

/// Parse the average PSNR (Y and average) from an FFmpeg psnr stats file.
fn parse_psnr_stats(path: &Path) -> Result<(f64, f64)> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("read psnr stats: {}", path.display()))?;
    let mut y_values: Vec<f64> = Vec::new();
    let mut avg_values: Vec<f64> = Vec::new();
    for line in content.lines() {
        // Format: "n:1 mse_avg:... mse_y:... psnr_y:40.12 psnr_u:... psnr_v:... psnr_avg:39.50"
        let mut py = None::<f64>;
        let mut pa = None::<f64>;
        for token in line.split_whitespace() {
            if let Some(v) = token.strip_prefix("psnr_y:") {
                py = v.parse().ok();
            } else if let Some(v) = token.strip_prefix("psnr_avg:") {
                pa = v.parse().ok();
            }
        }
        if let (Some(y), Some(a)) = (py, pa) {
            if y.is_finite() {
                y_values.push(y);
            }
            if a.is_finite() {
                avg_values.push(a);
            }
        }
    }
    if y_values.is_empty() {
        bail!("no finite PSNR values found in {}", path.display());
    }
    let mean_y = y_values.iter().copied().sum::<f64>() / y_values.len() as f64;
    let mean_avg = avg_values.iter().copied().sum::<f64>() / avg_values.len().max(1) as f64;
    Ok((mean_y, mean_avg))
}

fn parse_frames_from_stdout(stdout: &str) -> Result<usize> {
    // Looks for "frames=N" in stdout
    for line in stdout.lines() {
        for token in line.split_whitespace() {
            if let Some(v) = token.strip_prefix("frames=") {
                return v.parse().context("parse frame count");
            }
        }
    }
    bail!("no frames= token in stdout: {stdout}")
}

// ── Report formatting ─────────────────────────────────────────────────────────

fn format_result_row(r: &TestResult, _min_psnr: f64) -> String {
    let parts: Vec<&str> = r.label.splitn(3, " / ").collect();
    let backend = parts.first().copied().unwrap_or("?");
    let rest = parts.get(1).copied().unwrap_or("?");
    let status = if r.passed { "✅ PASS" } else { "❌ FAIL" };

    let psnr_y = r
        .psnr_y
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| r.error.clone().unwrap_or_else(|| "skipped".into()));
    let psnr_avg = r
        .psnr_avg
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "-".to_string());

    format!("| {backend} | {rest} | {psnr_y} | {psnr_avg} | {status} |")
}

fn format_frame_count_row(r: &TestResult, expected: usize) -> String {
    let parts: Vec<&str> = r.label.splitn(3, " / ").collect();
    let backend = parts.first().copied().unwrap_or("?");
    let codec = parts.get(1).copied().unwrap_or("?");
    let status = if r.passed { "✅ PASS" } else { "❌ FAIL" };
    let frames = r
        .frame_count
        .map(|n| n.to_string())
        .unwrap_or_else(|| r.error.clone().unwrap_or_else(|| "error".into()));
    format!("| {backend} | {codec} | {frames} | {expected} | {status} |")
}

// ── Process utilities ─────────────────────────────────────────────────────────

fn run_checked(exe: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new(exe)
        .args(args)
        .status()
        .with_context(|| format!("spawn {}", exe.display()))?;
    if !status.success() {
        bail!("{} exited with {status}", exe.display());
    }
    Ok(())
}

fn run_output(exe: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new(exe)
        .args(args)
        .output()
        .with_context(|| format!("spawn {}", exe.display()))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_output_checked(exe: &Path, args: &[&str]) -> Result<String> {
    let out: Output = Command::new(exe)
        .args(args)
        .output()
        .with_context(|| format!("spawn {}", exe.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("{} exited with {}:\n{stderr}", exe.display(), out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ── Discovery helpers ─────────────────────────────────────────────────────────

fn find_ffmpeg() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("FFMPEG_PATH") {
        return Ok(PathBuf::from(p));
    }
    // Try winget default install path
    let winget = PathBuf::from(r"C:\Users\sanze\AppData\Local\Microsoft\WinGet\Packages\Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe\ffmpeg-8.1-full_build\bin\ffmpeg.exe");
    if winget.exists() {
        return Ok(winget);
    }
    // Try PATH
    for dir in std::env::var("PATH").unwrap_or_default().split(';') {
        let candidate = PathBuf::from(dir).join("ffmpeg.exe");
        if candidate.exists() {
            return Ok(candidate);
        }
        let candidate = PathBuf::from(dir).join("ffmpeg");
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!("ffmpeg not found; set FFMPEG_PATH or add ffmpeg to PATH");
}

fn find_cargo_bin() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CARGO") {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let candidate = PathBuf::from(&home).join(".cargo").join("bin").join("cargo.exe");
    if candidate.exists() {
        return Ok(candidate);
    }
    Ok(PathBuf::from("cargo"))
}
