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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Backend {
    Nv,
    Intel,
    Vulkan,
    Vt,
}

impl Backend {
    fn display(self) -> &'static str {
        match self {
            Self::Nv => "NVIDIA",
            Self::Intel => "Intel oneVPL",
            Self::Vulkan => "Vulkan",
            Self::Vt => "VideoToolbox",
        }
    }

    fn is_supported_on_host(self) -> bool {
        match self {
            Self::Nv | Self::Intel | Self::Vulkan => {
                cfg!(target_os = "windows") || cfg!(target_os = "linux")
            }
            Self::Vt => cfg!(target_os = "macos"),
        }
    }
}

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

    fn annexb_input(self) -> &'static str {
        match self {
            Self::H264 => "sample-videos/sample-10s.h264",
            Self::Hevc => "sample-videos/sample-10s.h265",
        }
    }

    fn ffmpeg_demuxer(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Run integrated video-hw backend benchmarks against ffmpeg")]
struct Args {
    /// Comma-separated backend list. Defaults to all backends available on the host target.
    #[arg(long, value_enum, value_delimiter = ',')]
    backends: Vec<Backend>,

    #[arg(long, value_enum, default_value_t = Codec::H264)]
    codec: Codec,

    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    release: bool,

    #[arg(long, default_value_t = 1)]
    warmup: usize,

    #[arg(long, default_value_t = 5)]
    repeat: usize,

    #[arg(long, default_value_t = 65_536)]
    chunk_bytes: usize,

    #[arg(long, default_value_t = 300)]
    frame_count: usize,

    #[arg(long, default_value_t = 640)]
    width: usize,

    #[arg(long, default_value_t = 360)]
    height: usize,

    #[arg(long, default_value_t = false)]
    verify: bool,

    #[arg(long, default_value_t = false)]
    equal_raw_input: bool,

    /// Continue the integrated run when a backend or case fails.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    allow_failures: bool,

    /// Pass backend-internal metric flags to scripts that support them.
    #[arg(long, default_value_t = false)]
    include_internal_metrics: bool,

    #[arg(long)]
    vt_enable_pipeline_scheduler: Option<bool>,

    #[arg(long)]
    vt_pipeline_queue_capacity: Option<usize>,

    #[arg(long, default_value = "output")]
    output_dir: PathBuf,
}

#[derive(Debug)]
struct BackendReport {
    backend: Backend,
    status: BackendStatus,
    report_path: Option<PathBuf>,
    cases: Vec<CaseSummary>,
}

#[derive(Debug)]
enum BackendStatus {
    Passed,
    Skipped(String),
    Failed(String),
}

impl BackendStatus {
    fn as_report_text(&self) -> String {
        match self {
            Self::Passed => "passed".to_string(),
            Self::Skipped(reason) => format!("skipped: {reason}"),
            Self::Failed(reason) => format!("failed: {reason}"),
        }
    }

    fn is_failure(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

#[derive(Debug, Clone)]
struct CaseSummary {
    case: String,
    mean_seconds: Option<f64>,
    p50_seconds: Option<f64>,
    throughput_fps: Option<f64>,
}

#[derive(Debug, Clone)]
struct CaseSamples {
    case: &'static str,
    frame_count: usize,
    seconds: Vec<f64>,
}

impl CaseSamples {
    fn summarize(&self) -> CaseSummary {
        let stats = Stats::from_samples(&self.seconds);
        CaseSummary {
            case: self.case.to_string(),
            mean_seconds: Some(stats.mean),
            p50_seconds: Some(stats.p50),
            throughput_fps: Some(self.frame_count as f64 / stats.mean.max(f64::EPSILON)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Stats {
    mean: f64,
    p50: f64,
}

impl Stats {
    fn from_samples(samples: &[f64]) -> Self {
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let count = sorted.len().max(1);
        let mean = sorted.iter().sum::<f64>() / count as f64;
        let p50 = percentile_nearest_rank(&sorted, 50.0);
        Self { mean, p50 }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.repeat == 0 {
        bail!("--repeat must be >= 1");
    }

    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create output directory: {}", args.output_dir.display()))?;

    let selected_backends = if args.backends.is_empty() {
        default_backends_for_host()
    } else {
        args.backends.clone()
    };

    let mut reports = Vec::new();
    for backend in selected_backends {
        let report = if backend.is_supported_on_host() {
            run_backend(backend, &args)
        } else {
            Ok(BackendReport {
                backend,
                status: BackendStatus::Skipped(format!(
                    "{} is not available on this target OS",
                    backend.display()
                )),
                report_path: None,
                cases: Vec::new(),
            })
        };

        match report {
            Ok(report) => {
                println!(
                    "{}: {}",
                    report.backend.display(),
                    report.status.as_report_text()
                );
                reports.push(report);
            }
            Err(err) => {
                let report = BackendReport {
                    backend,
                    status: BackendStatus::Failed(format!("{err:#}")),
                    report_path: None,
                    cases: Vec::new(),
                };
                println!(
                    "{}: {}",
                    report.backend.display(),
                    report.status.as_report_text()
                );
                if !args.allow_failures {
                    write_integrated_report(&args, &[report])?;
                    return Err(err);
                }
                reports.push(report);
            }
        }
    }

    let report_path = write_integrated_report(&args, &reports)?;
    println!("saved integrated report: {}", report_path.display());

    if !args.allow_failures
        && reports
            .iter()
            .any(|report| report.status.is_failure())
    {
        bail!("one or more backend benchmarks failed");
    }
    Ok(())
}

fn default_backends_for_host() -> Vec<Backend> {
    if cfg!(target_os = "macos") {
        return vec![Backend::Vt];
    }
    if cfg!(target_os = "windows") || cfg!(target_os = "linux") {
        return vec![Backend::Nv, Backend::Intel, Backend::Vulkan];
    }
    Vec::new()
}

fn run_backend(backend: Backend, args: &Args) -> Result<BackendReport> {
    match backend {
        Backend::Nv => run_child_precise_script(backend, "scripts/benchmark_ffmpeg_nv_precise.rs", args),
        Backend::Intel => {
            run_child_precise_script(backend, "scripts/benchmark_ffmpeg_intel_precise.rs", args)
        }
        Backend::Vt => run_child_precise_script(backend, "scripts/benchmark_ffmpeg_vt_precise.rs", args),
        Backend::Vulkan => run_vulkan_decode_benchmark(args),
    }
}

fn run_child_precise_script(
    backend: Backend,
    script_path: &str,
    args: &Args,
) -> Result<BackendReport> {
    let child_output_dir = Path::new("output");
    fs::create_dir_all(child_output_dir).context("create child script output directory")?;
    let before = existing_reports(child_output_dir)?;
    let mut command = Command::new("cargo");
    command.args([
        "+nightly",
        "-Zscript",
        script_path,
        "--codec",
        args.codec.as_cli(),
        "--warmup",
        &args.warmup.to_string(),
        "--repeat",
        &args.repeat.to_string(),
        "--frame-count",
        &args.frame_count.to_string(),
        "--width",
        &args.width.to_string(),
        "--height",
        &args.height.to_string(),
        "--chunk-bytes",
        &args.chunk_bytes.to_string(),
    ]);
    command.args(["--release", &args.release.to_string()]);
    if args.verify {
        command.arg("--verify");
    }
    if args.equal_raw_input && matches!(backend, Backend::Nv | Backend::Intel | Backend::Vt) {
        command.arg("--equal-raw-input");
    }
    if args.include_internal_metrics && matches!(backend, Backend::Nv | Backend::Vt) {
        command.arg("--include-internal-metrics");
    }
    if matches!(backend, Backend::Vt) {
        if let Some(enabled) = args.vt_enable_pipeline_scheduler {
            command
                .arg("--vt-enable-pipeline-scheduler")
                .arg(enabled.to_string());
        }
        if let Some(capacity) = args.vt_pipeline_queue_capacity {
            command
                .arg("--vt-pipeline-queue-capacity")
                .arg(capacity.to_string());
        }
    }
    if args.allow_failures && matches!(backend, Backend::Intel) {
        command.arg("--allow-case-failures");
    }

    run_inherited(&mut command).with_context(|| format!("run {}", backend.display()))?;
    let report_path = newest_new_report(child_output_dir, &before)
        .with_context(|| format!("locate {} report", backend.display()))?;
    let cases = parse_case_summaries(&report_path)?;
    Ok(BackendReport {
        backend,
        status: BackendStatus::Passed,
        report_path: Some(report_path),
        cases,
    })
}

fn run_vulkan_decode_benchmark(args: &Args) -> Result<BackendReport> {
    build_vulkan_decode_example(args.release)?;
    let profile = if args.release { "release" } else { "debug" };
    let decode_bin = example_bin_path(profile, "decode_to_yuv");
    let null_sink = null_sink();
    let total_rounds = args.warmup + args.repeat;
    let mut video_hw = CaseSamples {
        case: "video-hw decode",
        frame_count: 303,
        seconds: Vec::new(),
    };
    let mut ffmpeg = CaseSamples {
        case: "ffmpeg decode",
        frame_count: 303,
        seconds: Vec::new(),
    };

    for round in 0..total_rounds {
        let is_warmup = round < args.warmup;
        let phase = if is_warmup { "warmup" } else { "measure" };
        println!(
            "vulkan round {}/{total_rounds}, phase={phase}",
            round + 1
        );

        let video_hw_seconds = run_timed(vulkan_decode_command(&decode_bin, args, &null_sink)?)?;
        println!("  video-hw decode {:.3}s", video_hw_seconds);
        let ffmpeg_seconds = run_timed(ffmpeg_decode_command(args, &null_sink))?;
        println!("  ffmpeg decode    {:.3}s", ffmpeg_seconds);

        if !is_warmup {
            video_hw.seconds.push(video_hw_seconds);
            ffmpeg.seconds.push(ffmpeg_seconds);
        }
    }

    let now_secs = epoch_seconds()?;
    let report_path = args.output_dir.join(format!(
        "benchmark-vulkan-decode-{}-{now_secs}.md",
        args.codec.as_cli()
    ));
    let cases = vec![video_hw.summarize(), ffmpeg.summarize()];
    write_vulkan_report(&report_path, args, &cases)?;

    Ok(BackendReport {
        backend: Backend::Vulkan,
        status: BackendStatus::Passed,
        report_path: Some(report_path),
        cases,
    })
}

fn build_vulkan_decode_example(release: bool) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args(["build", "-p", "video-hw", "--features", "backend-vulkan", "--example", "decode_to_yuv"]);
    if release {
        command.arg("--release");
    }
    run_inherited(&mut command).context("build Vulkan decode_to_yuv example")
}

fn vulkan_decode_command(decode_bin: &Path, args: &Args, null_sink: &Path) -> Result<Command> {
    let mut command = Command::new(decode_bin);
    command.args([
        "--backend",
        "vulkan",
        "--codec",
        args.codec.as_cli(),
        "--input",
        args.codec.annexb_input(),
        "--input-format",
        "annexb",
        "--output-mode",
        "metadata",
        "--chunk-bytes",
        &args.chunk_bytes.to_string(),
        "--require-hardware",
    ]);
    command.stdout(Stdio::null()).stderr(Stdio::null());
    if cfg!(windows) {
        let _ = null_sink;
    }
    Ok(command)
}

fn ffmpeg_decode_command(args: &Args, null_sink: &Path) -> Command {
    let mut command = Command::new("ffmpeg");
    command.args([
        "-v",
        "error",
        "-f",
        args.codec.ffmpeg_demuxer(),
        "-i",
        args.codec.annexb_input(),
        "-f",
        "null",
        &null_sink.to_string_lossy(),
    ]);
    command
}

fn run_timed(mut command: Command) -> Result<f64> {
    let start = Instant::now();
    let status = command.status().context("spawn benchmark command")?;
    if !status.success() {
        bail!("benchmark command failed: status={status}");
    }
    Ok(start.elapsed().as_secs_f64())
}

fn run_inherited(command: &mut Command) -> Result<()> {
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("spawn command")?;
    if !status.success() {
        bail!("command failed: status={status}");
    }
    Ok(())
}

fn write_integrated_report(args: &Args, reports: &[BackendReport]) -> Result<PathBuf> {
    let now_secs = epoch_seconds()?;
    let path = args.output_dir.join(format!(
        "benchmark-backends-{}-{now_secs}.md",
        args.codec.as_cli()
    ));
    let mut report = String::new();
    writeln!(&mut report, "# Integrated Backend Benchmark Report")?;
    writeln!(&mut report, "epoch_seconds: {now_secs}")?;
    writeln!(&mut report, "codec: {}", args.codec.as_cli())?;
    writeln!(&mut report, "warmup: {}", args.warmup)?;
    writeln!(&mut report, "repeat: {}", args.repeat)?;
    writeln!(&mut report, "frame_count: {}", args.frame_count)?;
    writeln!(&mut report, "width: {}", args.width)?;
    writeln!(&mut report, "height: {}", args.height)?;
    writeln!(&mut report)?;
    writeln!(&mut report, "## Backends")?;
    writeln!(&mut report, "| Backend | Status | Report |")?;
    writeln!(&mut report, "|---|---|---|")?;
    for backend_report in reports {
        let report_ref = backend_report
            .report_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "-".to_string());
        writeln!(
            &mut report,
            "| {} | {} | {} |",
            backend_report.backend.display(),
            backend_report.status.as_report_text().replace('|', "\\|"),
            report_ref
        )?;
    }
    writeln!(&mut report)?;
    writeln!(&mut report, "## Summary")?;
    writeln!(
        &mut report,
        "| Backend | Case | mean(s) | p50(s) | throughput(fps) |"
    )?;
    writeln!(&mut report, "|---|---|---:|---:|---:|")?;
    for backend_report in reports {
        for case in &backend_report.cases {
            writeln!(
                &mut report,
                "| {} | {} | {} | {} | {} |",
                backend_report.backend.display(),
                case.case,
                fmt_optional(case.mean_seconds),
                fmt_optional(case.p50_seconds),
                fmt_optional(case.throughput_fps)
            )?;
        }
    }
    fs::write(&path, report).with_context(|| format!("write report: {}", path.display()))?;
    Ok(path)
}

fn write_vulkan_report(path: &Path, args: &Args, cases: &[CaseSummary]) -> Result<()> {
    let mut report = String::new();
    writeln!(&mut report, "# Vulkan Decode Benchmark Report")?;
    writeln!(&mut report, "codec: {}", args.codec.as_cli())?;
    writeln!(&mut report, "warmup: {}", args.warmup)?;
    writeln!(&mut report, "repeat: {}", args.repeat)?;
    writeln!(&mut report)?;
    writeln!(
        &mut report,
        "| Case | min(s) | mean(s) | p50(s) | p95(s) | p99(s) | max(s) | stddev(s) | CV(%) |"
    )?;
    writeln!(&mut report, "|---|---:|---:|---:|---:|---:|---:|---:|---:|")?;
    for case in cases {
        writeln!(
            &mut report,
            "| {} | n/a | {} | {} | n/a | n/a | n/a | n/a | n/a |",
            case.case,
            fmt_optional(case.mean_seconds),
            fmt_optional(case.p50_seconds)
        )?;
    }
    fs::write(path, report).with_context(|| format!("write report: {}", path.display()))
}

fn parse_case_summaries(report_path: &Path) -> Result<Vec<CaseSummary>> {
    let text = fs::read_to_string(report_path)
        .with_context(|| format!("read report: {}", report_path.display()))?;
    Ok(text
        .lines()
        .filter_map(parse_case_summary_line)
        .collect())
}

fn parse_case_summary_line(line: &str) -> Option<CaseSummary> {
    let cells = line
        .trim()
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect::<Vec<_>>();
    if cells.len() < 4 || cells.first().copied() == Some("Case") || cells[1] == "---:" {
        return None;
    }
    if !cells[0].contains("decode") && !cells[0].contains("encode") {
        return None;
    }
    let mean = parse_optional_f64(cells[2]);
    let p50 = parse_optional_f64(cells[3]);
    Some(CaseSummary {
        case: cells[0].to_string(),
        mean_seconds: mean,
        p50_seconds: p50,
        throughput_fps: None,
    })
}

fn existing_reports(output_dir: &Path) -> Result<Vec<PathBuf>> {
    if !output_dir.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_dir(output_dir)
        .with_context(|| format!("read output directory: {}", output_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect::<Vec<_>>())
}

fn newest_new_report(output_dir: &Path, before: &[PathBuf]) -> Result<PathBuf> {
    let before = before.iter().collect::<std::collections::HashSet<_>>();
    fs::read_dir(output_dir)
        .with_context(|| format!("read output directory: {}", output_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .filter(|path| !before.contains(path))
        .filter_map(|path| {
            let modified = path.metadata().and_then(|metadata| metadata.modified()).ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
        .context("no new markdown report produced")
}

fn example_bin_path(profile: &str, name: &str) -> PathBuf {
    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
    PathBuf::from("target")
        .join(profile)
        .join("examples")
        .join(format!("{name}{exe_suffix}"))
}

fn null_sink() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("NUL")
    } else {
        PathBuf::from("/dev/null")
    }
}

fn epoch_seconds() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before UNIX_EPOCH")?
        .as_secs())
}

fn parse_optional_f64(value: &str) -> Option<f64> {
    if value == "n/a" {
        return None;
    }
    value.parse::<f64>().ok()
}

fn fmt_optional(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn percentile_nearest_rank(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let n = sorted.len();
    let rank = ((percentile / 100.0) * n as f64).ceil().clamp(1.0, n as f64) as usize;
    sorted[rank - 1]
}
