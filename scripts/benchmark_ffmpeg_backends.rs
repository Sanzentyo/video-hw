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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    fn as_cli(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
        }
    }

    fn ffmpeg_demuxer(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "obu",
        }
    }

    fn software_encoder(self) -> &'static str {
        match self {
            Self::H264 => "libx264",
            Self::Hevc => "libx265",
            Self::Av1 => "libaom-av1",
        }
    }

    fn annexb_extension(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "h265",
            Self::Av1 => "av1",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum VulkanDecodeInputFormat {
    Annexb,
    Fmp4,
}

impl VulkanDecodeInputFormat {
    fn as_cli(self) -> &'static str {
        match self {
            Self::Annexb => "annexb",
            Self::Fmp4 => "fmp4",
        }
    }

    fn decode_to_yuv_input_format(self) -> &'static str {
        match self {
            Self::Annexb => "annexb",
            Self::Fmp4 => "mp4",
        }
    }

    fn output_extension(self, codec: Codec) -> &'static str {
        match self {
            Self::Annexb => codec.annexb_extension(),
            Self::Fmp4 => "mp4",
        }
    }
}

#[derive(Debug, Clone, Parser)]
#[command(about = "Run integrated video-hw backend benchmarks against ffmpeg")]
struct Args {
    /// Comma-separated backend list. Defaults to all backends available on the host target.
    #[arg(long, value_enum, value_delimiter = ',')]
    backends: Vec<Backend>,

    #[arg(long, value_enum, default_value_t = Codec::H264)]
    codec: Codec,

    /// Comma-separated codec list. When set, runs one integrated report per codec.
    #[arg(long, value_enum, value_delimiter = ',')]
    codecs: Vec<Codec>,

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

    /// Continue the integrated run when a backend or case fails.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    allow_failures: bool,

    /// Pass backend-internal metric flags to scripts that support them.
    #[arg(long, default_value_t = false)]
    include_internal_metrics: bool,

    /// Vulkan adapter indexes to test. Defaults to all adapters reported by vulkaninfo.
    #[arg(long, value_delimiter = ',')]
    vulkan_adapter_indexes: Vec<usize>,

    /// Vulkan decode input container. fmp4 is currently supported for AV1 only.
    #[arg(long, value_enum, default_value_t = VulkanDecodeInputFormat::Annexb)]
    vulkan_decode_input_format: VulkanDecodeInputFormat,

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
    status: String,
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

#[derive(Debug, Clone)]
struct VulkanAdapterInfo {
    index: usize,
    name: String,
    vendor_id: Option<u32>,
    device_id: Option<u32>,
    supports_decoding: Option<bool>,
    supports_encoding: Option<bool>,
}

impl CaseSamples {
    fn summarize(&self) -> CaseSummary {
        let stats = Stats::from_samples(&self.seconds);
        CaseSummary {
            case: self.case.to_string(),
            status: "passed".to_string(),
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
    if args.frame_count == 0 {
        bail!("--frame-count must be >= 1");
    }

    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create output directory: {}", args.output_dir.display()))?;

    let selected_backends = if args.backends.is_empty() {
        default_backends_for_host()
    } else {
        args.backends.clone()
    };
    let selected_codecs = if args.codecs.is_empty() {
        vec![args.codec]
    } else {
        args.codecs.clone()
    };

    let mut any_failure = false;
    for codec in selected_codecs {
        let mut codec_args = args.clone();
        codec_args.codec = codec;
        let mut reports = Vec::new();
        println!("codec: {}", codec.as_cli());
        for backend in &selected_backends {
            let backend = *backend;
            let report = if backend.is_supported_on_host() {
                run_backend(backend, &codec_args)
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
                    any_failure |= report.status.is_failure();
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
                    any_failure = true;
                    if !codec_args.allow_failures {
                        write_integrated_report(&codec_args, &[report])?;
                        return Err(err);
                    }
                    reports.push(report);
                }
            }
        }

        let report_path = write_integrated_report(&codec_args, &reports)?;
        println!("saved integrated report: {}", report_path.display());
    }

    if !args.allow_failures && any_failure {
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

fn discover_vulkaninfo_adapters() -> Result<Vec<VulkanAdapterInfo>> {
    let output = Command::new("vulkaninfo")
        .arg("--summary")
        .output()
        .context("run vulkaninfo --summary")?;
    if !output.status.success() {
        bail!(
            "vulkaninfo --summary failed: status={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut adapters = Vec::new();
    let mut current: Option<VulkanAdapterInfo> = None;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("GPU") {
            if let Some(adapter) = current.take() {
                adapters.push(adapter);
            }
            if let Some((index, suffix)) = rest.split_once(':')
                && suffix.is_empty()
                && let Ok(index) = index.parse::<usize>()
            {
                current = Some(VulkanAdapterInfo {
                    index,
                    name: String::new(),
                    vendor_id: None,
                    device_id: None,
                    supports_decoding: None,
                    supports_encoding: None,
                });
            }
            continue;
        }
        let Some(adapter) = current.as_mut() else {
            continue;
        };
        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "deviceName" => adapter.name = value.to_string(),
                "vendorID" => adapter.vendor_id = parse_u32_auto(value),
                "deviceID" => adapter.device_id = parse_u32_auto(value),
                _ => {}
            }
        }
    }
    if let Some(adapter) = current.take() {
        adapters.push(adapter);
    }
    if adapters.is_empty() {
        bail!("vulkaninfo reported no GPU entries");
    }
    Ok(adapters)
}

fn parse_u32_auto(value: &str) -> Option<u32> {
    value
        .strip_prefix("0x")
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .or_else(|| value.parse::<u32>().ok())
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
    if args.include_internal_metrics && matches!(backend, Backend::Nv | Backend::Vt) {
        command.arg("--include-internal-metrics");
    }
    if args.allow_failures && matches!(backend, Backend::Intel) {
        command.arg("--allow-case-failures");
    }

    run_inherited(&mut command).with_context(|| format!("run {}", backend.display()))?;
    let report_path = newest_new_report(child_output_dir, &before)
        .with_context(|| format!("locate {} report", backend.display()))?;
    let cases = parse_case_summaries(&report_path)?;
    let status = parse_child_report_status(&report_path, args.verify)?;
    Ok(BackendReport {
        backend,
        status,
        report_path: Some(report_path),
        cases,
    })
}

fn run_vulkan_decode_benchmark(args: &Args) -> Result<BackendReport> {
    if args.vulkan_decode_input_format == VulkanDecodeInputFormat::Fmp4
        && args.codec != Codec::Av1
    {
        bail!("--vulkan-decode-input-format fmp4 is currently supported only with --codec av1");
    }
    build_vulkan_examples(args.release)?;
    let profile = if args.release { "release" } else { "debug" };
    let decode_bin = example_bin_path(profile, "decode_to_yuv");
    let encode_bin = example_bin_path(profile, "encode_synthetic");
    let list_adapters_bin = example_bin_path(profile, "list_vulkan_adapters");
    let null_sink = null_sink();
    let total_rounds = args.warmup + args.repeat;
    let decode_input = ensure_vulkan_decode_input(args)?;
    let ffmpeg_adapters = discover_vulkaninfo_adapters().unwrap_or_else(|err| {
        eprintln!("failed to discover Vulkan adapters via vulkaninfo: {err:#}");
        Vec::new()
    });
    let video_hw_adapters = discover_video_hw_vulkan_adapters(&list_adapters_bin)
        .unwrap_or_else(|err| {
            eprintln!("failed to discover video-hw Vulkan adapters: {err:#}");
            Vec::new()
        });

    let mut cases = Vec::new();
    for adapter in &video_hw_adapters {
        if !args.vulkan_adapter_indexes.is_empty()
            && !args.vulkan_adapter_indexes.contains(&adapter.index)
        {
            continue;
        }
        println!(
            "video-hw vulkan adapter {} ({})",
            adapter.index, adapter.name
        );
        let video_hw_adapter_label = adapter_label(adapter);
        let ffmpeg_match = find_matching_vulkaninfo_adapter(adapter, &ffmpeg_adapters);
        cases.push(run_vulkan_case(
            adapter.index,
            &video_hw_adapter_label,
            "video-hw decode",
            args.frame_count,
            total_rounds,
            args.warmup,
            || vulkan_decode_command(&decode_bin, args, &decode_input, adapter.index),
        ));
        if let Some(ffmpeg_adapter) = ffmpeg_match {
            cases.push(run_vulkan_case(
                ffmpeg_adapter.index,
                &adapter_label(ffmpeg_adapter),
                "ffmpeg vulkan decode",
                args.frame_count,
                total_rounds,
                args.warmup,
                || Ok(ffmpeg_vulkan_decode_command(
                    args,
                    &decode_input,
                    ffmpeg_adapter.index,
                    &null_sink,
                )),
            ));
        } else {
            cases.push(unavailable_case(
                "ffmpeg vulkan decode",
                &video_hw_adapter_label,
                "no vulkaninfo adapter with matching name/device id",
            ));
        }
        if args.codec == Codec::Av1 {
            cases.push(unavailable_case(
                "video-hw encode",
                &video_hw_adapter_label,
                "Vulkan AV1 encode is blocked by current ash bindings",
            ));
        } else {
            cases.push(run_vulkan_case(
                adapter.index,
                &video_hw_adapter_label,
                "video-hw encode",
                args.frame_count,
                total_rounds,
                args.warmup,
                || {
                    vulkan_encode_command(
                        &encode_bin,
                        args,
                        adapter.index,
                        ffmpeg_match.map(|adapter| adapter.index),
                        &null_sink,
                    )
                },
            ));
        }
        if let Some(ffmpeg_adapter) = ffmpeg_match {
            cases.push(run_vulkan_case(
                ffmpeg_adapter.index,
                &adapter_label(ffmpeg_adapter),
                "ffmpeg vulkan encode",
                args.frame_count,
                total_rounds,
                args.warmup,
                || Ok(ffmpeg_vulkan_encode_command(
                    args,
                    ffmpeg_adapter.index,
                    &null_sink,
                )),
            ));
        } else {
            cases.push(unavailable_case(
                "ffmpeg vulkan encode",
                &video_hw_adapter_label,
                "no vulkaninfo adapter with matching name/device id",
            ));
        }
    }

    for ffmpeg_adapter in &ffmpeg_adapters {
        if !args.vulkan_adapter_indexes.is_empty()
            && !args.vulkan_adapter_indexes.contains(&ffmpeg_adapter.index)
        {
            continue;
        }
        if find_matching_video_hw_adapter(ffmpeg_adapter, &video_hw_adapters).is_some() {
            continue;
        }
        println!(
            "ffmpeg-only vulkan adapter {} ({})",
            ffmpeg_adapter.index, ffmpeg_adapter.name
        );
        let adapter_label = adapter_label(ffmpeg_adapter);
        if args.codec == Codec::Hevc {
            cases.push(run_vulkan_case(
                ffmpeg_adapter.index,
                &adapter_label,
                "video-hw decode",
                args.frame_count,
                total_rounds,
                args.warmup,
                || {
                    vulkan_hevc_physical_decode_command(
                        &decode_bin,
                        args,
                        &decode_input,
                        ffmpeg_adapter.index,
                    )
                },
            ));
        } else {
            cases.push(unavailable_case(
                "video-hw decode",
                &adapter_label,
                "adapter is not exposed by vk-video/video-hw",
            ));
        }
        cases.push(run_vulkan_case(
            ffmpeg_adapter.index,
            &adapter_label,
            "ffmpeg vulkan decode",
            args.frame_count,
            total_rounds,
            args.warmup,
            || Ok(ffmpeg_vulkan_decode_command(
                args,
                &decode_input,
                ffmpeg_adapter.index,
                &null_sink,
            )),
        ));
        cases.push(unavailable_case(
            "video-hw encode",
            &adapter_label,
            "adapter is not exposed by vk-video/video-hw",
        ));
        cases.push(run_vulkan_case(
            ffmpeg_adapter.index,
            &adapter_label,
            "ffmpeg vulkan encode",
            args.frame_count,
            total_rounds,
            args.warmup,
            || Ok(ffmpeg_vulkan_encode_command(
                args,
                ffmpeg_adapter.index,
                &null_sink,
            )),
        ));
    }

    let now_secs = epoch_seconds()?;
    let report_path = args.output_dir.join(format!(
        "benchmark-vulkan-{}-{now_secs}.md",
        args.codec.as_cli()
    ));
    write_vulkan_report(&report_path, args, &decode_input, &cases)?;
    let status = if cases.iter().any(|case| case.status.starts_with("failed:")) {
        BackendStatus::Failed("one or more Vulkan adapter cases failed".to_string())
    } else {
        BackendStatus::Passed
    };

    Ok(BackendReport {
        backend: Backend::Vulkan,
        status,
        report_path: Some(report_path),
        cases,
    })
}

fn build_vulkan_examples(release: bool) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args([
        "build",
        "-p",
        "video-hw",
        "--features",
        "backend-vulkan",
        "--example",
        "decode_to_yuv",
        "--example",
        "encode_synthetic",
        "--example",
        "list_vulkan_adapters",
    ]);
    if release {
        command.arg("--release");
    }
    run_inherited(&mut command).context("build Vulkan examples")
}

fn discover_video_hw_vulkan_adapters(list_adapters_bin: &Path) -> Result<Vec<VulkanAdapterInfo>> {
    let output = Command::new(list_adapters_bin)
        .output()
        .context("run list_vulkan_adapters")?;
    if !output.status.success() {
        bail!(
            "list_vulkan_adapters failed: status={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter_map(parse_video_hw_vulkan_adapter_line)
        .collect())
}

fn parse_video_hw_vulkan_adapter_line(line: &str) -> Option<VulkanAdapterInfo> {
    let cells = line.split('\t').collect::<Vec<_>>();
    if cells.len() != 6 {
        return None;
    }
    Some(VulkanAdapterInfo {
        index: cells[0].parse().ok()?,
        name: cells[1].to_string(),
        vendor_id: cells[2].parse().ok(),
        device_id: cells[3].parse().ok(),
        supports_decoding: cells[4].parse().ok(),
        supports_encoding: cells[5].parse().ok(),
    })
}

fn find_matching_vulkaninfo_adapter<'a>(
    video_hw: &VulkanAdapterInfo,
    vulkaninfo: &'a [VulkanAdapterInfo],
) -> Option<&'a VulkanAdapterInfo> {
    vulkaninfo
        .iter()
        .find(|adapter| same_vulkan_adapter(video_hw, adapter))
}

fn find_matching_video_hw_adapter<'a>(
    vulkaninfo: &VulkanAdapterInfo,
    video_hw: &'a [VulkanAdapterInfo],
) -> Option<&'a VulkanAdapterInfo> {
    video_hw
        .iter()
        .find(|adapter| same_vulkan_adapter(adapter, vulkaninfo))
}

fn same_vulkan_adapter(left: &VulkanAdapterInfo, right: &VulkanAdapterInfo) -> bool {
    if left.name != right.name {
        return false;
    }
    match (
        left.vendor_id,
        left.device_id,
        right.vendor_id,
        right.device_id,
    ) {
        (Some(left_vendor), Some(left_device), Some(right_vendor), Some(right_device)) => {
            left_vendor == right_vendor && left_device == right_device
        }
        _ => true,
    }
}

fn adapter_label(adapter: &VulkanAdapterInfo) -> String {
    let decode = adapter
        .supports_decoding
        .map(|value| if value { "decode" } else { "no-decode" })
        .unwrap_or("decode?");
    let encode = adapter
        .supports_encoding
        .map(|value| if value { "encode" } else { "no-encode" })
        .unwrap_or("encode?");
    format!("{} {decode}/{encode}", adapter.name)
}

fn run_vulkan_case<F>(
    adapter_index: usize,
    adapter_name: &str,
    label: &'static str,
    frame_count: usize,
    total_rounds: usize,
    warmup: usize,
    mut command_factory: F,
) -> CaseSummary
where
    F: FnMut() -> Result<Command>,
{
    let mut samples = CaseSamples {
        case: label,
        frame_count,
        seconds: Vec::new(),
    };
    for round in 0..total_rounds {
        let is_warmup = round < warmup;
        let phase = if is_warmup { "warmup" } else { "measure" };
        println!(
            "  {label} round {}/{total_rounds}, phase={phase}",
            round + 1
        );
        let command = match command_factory() {
            Ok(command) => command,
            Err(err) => return failed_case(adapter_index, adapter_name, label, err),
        };
        match run_timed(command) {
            Ok(seconds) => {
                println!("    {seconds:.3}s");
                if !is_warmup {
                    samples.seconds.push(seconds);
                }
            }
            Err(err) => return failed_case(adapter_index, adapter_name, label, err),
        }
    }
    let mut summary = samples.summarize();
    summary.case = format!("{label} [{adapter_name} vk{adapter_index}]");
    summary
}

fn failed_case(
    adapter_index: usize,
    adapter_name: &str,
    label: &str,
    err: anyhow::Error,
) -> CaseSummary {
    let status = format!("failed: {err:#}").replace('|', "\\|");
    eprintln!("  {label} [{adapter_name} vk{adapter_index}] {status}");
    CaseSummary {
        case: format!("{label} [{adapter_name} vk{adapter_index}]"),
        status,
        mean_seconds: None,
        p50_seconds: None,
        throughput_fps: None,
    }
}

fn unavailable_case(label: &str, adapter_name: &str, reason: &str) -> CaseSummary {
    CaseSummary {
        case: format!("{label} [{adapter_name}]"),
        status: format!("unavailable: {reason}").replace('|', "\\|"),
        mean_seconds: None,
        p50_seconds: None,
        throughput_fps: None,
    }
}

fn ensure_vulkan_decode_input(args: &Args) -> Result<PathBuf> {
    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create output directory: {}", args.output_dir.display()))?;
    let path = args.output_dir.join(format!(
        "benchmark-vulkan-decode-input-{}-{}-{}x{}-{}f.{}",
        args.codec.as_cli(),
        args.vulkan_decode_input_format.as_cli(),
        args.width,
        args.height,
        args.frame_count,
        args.vulkan_decode_input_format.output_extension(args.codec)
    ));
    let mut command = Command::new("ffmpeg");
    command.args([
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        &format!(
            "testsrc2=size={}x{}:rate=30:duration={}",
            args.width,
            args.height,
            (args.frame_count as f64 / 30.0).max(0.001)
        ),
        "-frames:v",
        &args.frame_count.to_string(),
        "-pix_fmt",
        "yuv420p",
        "-an",
        "-c:v",
        args.codec.software_encoder(),
    ]);
    match args.codec {
        Codec::H264 => {
            command.args([
                "-preset",
                "veryfast",
                "-tune",
                "zerolatency",
                "-x264-params",
                "keyint=30:min-keyint=30:scenecut=0",
            ]);
        }
        Codec::Hevc => {
            command.args([
                "-preset",
                "ultrafast",
                "-x265-params",
                "log-level=error:keyint=30:min-keyint=30:bframes=0",
            ]);
        }
        Codec::Av1 => {
            command.args(["-cpu-used", "8", "-row-mt", "1", "-g", "1", "-lag-in-frames", "0"]);
        }
    }
    match args.vulkan_decode_input_format {
        VulkanDecodeInputFormat::Annexb => {
            command.args(["-f", args.codec.ffmpeg_demuxer(), &path.to_string_lossy()]);
        }
        VulkanDecodeInputFormat::Fmp4 => {
            command.args([
                "-movflags",
                "+frag_keyframe+empty_moov+delay_moov+default_base_moof",
                "-f",
                "mp4",
                &path.to_string_lossy(),
            ]);
        }
    }
    run_inherited(&mut command)
        .with_context(|| format!("generate Vulkan decode input: {}", path.display()))?;
    Ok(path)
}

fn vulkan_decode_command(
    decode_bin: &Path,
    args: &Args,
    decode_input: &Path,
    adapter_index: usize,
) -> Result<Command> {
    let mut command = Command::new(decode_bin);
    let decode_input = decode_input.to_string_lossy().to_string();
    command.args([
        "--backend",
        "vulkan",
        "--codec",
        args.codec.as_cli(),
        "--input",
        &decode_input,
        "--input-format",
        args.vulkan_decode_input_format
            .decode_to_yuv_input_format(),
        "--output-mode",
        "metadata",
        "--chunk-bytes",
        &args.chunk_bytes.to_string(),
        "--require-hardware",
        "--vulkan-adapter-index",
        &adapter_index.to_string(),
    ]);
    Ok(command)
}

fn vulkan_hevc_physical_decode_command(
    decode_bin: &Path,
    args: &Args,
    decode_input: &Path,
    physical_device_index: usize,
) -> Result<Command> {
    let mut command = vulkan_decode_command(decode_bin, args, decode_input, physical_device_index)?;
    command.env(
        "VIDEO_HW_VULKAN_HEVC_DECODE_PHYSICAL_DEVICE_INDEX",
        physical_device_index.to_string(),
    );
    command.env("VIDEO_HW_VULKAN_HEVC_EXPERIMENTAL_DPB", "1");
    Ok(command)
}

fn vulkan_encode_command(
    encode_bin: &Path,
    args: &Args,
    adapter_index: usize,
    ffmpeg_adapter_index: Option<usize>,
    null_sink: &Path,
) -> Result<Command> {
    let mut command = Command::new(encode_bin);
    if args.codec == Codec::Hevc {
        let ffmpeg_adapter_index = ffmpeg_adapter_index.with_context(|| {
            format!("no matching FFmpeg Vulkan adapter for video-hw adapter {adapter_index}")
        })?;
        let parameter_sample =
            ensure_ffmpeg_vulkan_hevc_parameter_sample(args, ffmpeg_adapter_index)?;
        command.env(
            "VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SAMPLE_PATH",
            parameter_sample,
        );
        command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_MODE", "sample");
        command.env("VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_VUI_SAFETY", "preserve");
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
    }
    command.args([
        "--backend",
        "vulkan",
        "--codec",
        args.codec.as_cli(),
        "--frame-count",
        &args.frame_count.to_string(),
        "--width",
        &args.width.to_string(),
        "--height",
        &args.height.to_string(),
        "--discard-output",
        "--require-hardware",
        "--vulkan-adapter-index",
        &adapter_index.to_string(),
    ]);
    if !cfg!(windows) {
        command.args(["--output", &null_sink.to_string_lossy()]);
    }
    Ok(command)
}

fn ensure_ffmpeg_vulkan_hevc_parameter_sample(
    args: &Args,
    adapter_index: usize,
) -> Result<PathBuf> {
    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create output directory: {}", args.output_dir.display()))?;
    let path = args.output_dir.join(format!(
        "ffmpeg-vulkan-hevc-parameter-{}x{}-adapter-{}.h265",
        args.width, args.height, adapter_index
    ));
    let mut command = Command::new("ffmpeg");
    command.args([
        "-v",
        "error",
        "-y",
        "-init_hw_device",
        &format!("vulkan:{adapter_index}"),
        "-f",
        "lavfi",
        "-i",
        &format!(
            "testsrc2=size={}x{}:rate=30:duration={}",
            args.width,
            args.height,
            (1.0_f64 / 30.0).max(0.001)
        ),
        "-frames:v",
        "1",
        "-vf",
        "format=nv12,hwupload",
        "-c:v",
        "hevc_vulkan",
        "-f",
        "hevc",
        &path.to_string_lossy(),
    ]);
    run_inherited(&mut command)
        .with_context(|| format!("generate FFmpeg Vulkan HEVC parameter sample: {}", path.display()))?;
    Ok(path)
}

fn ffmpeg_vulkan_decode_command(
    args: &Args,
    decode_input: &Path,
    adapter_index: usize,
    null_sink: &Path,
) -> Command {
    let mut command = Command::new("ffmpeg");
    let decode_input = decode_input.to_string_lossy().to_string();
    command.args([
        "-v",
        "error",
        "-init_hw_device",
        &format!("vulkan:{adapter_index}"),
        "-hwaccel",
        "vulkan",
        "-hwaccel_output_format",
        "vulkan",
    ]);
    if args.vulkan_decode_input_format == VulkanDecodeInputFormat::Annexb {
        command.args(["-f", args.codec.ffmpeg_demuxer()]);
    }
    command.args(["-i", &decode_input, "-f", "null", &null_sink.to_string_lossy()]);
    command
}

fn ffmpeg_vulkan_encode_command(args: &Args, adapter_index: usize, null_sink: &Path) -> Command {
    let mut command = Command::new("ffmpeg");
    command.args([
        "-v",
        "error",
        "-y",
        "-init_hw_device",
        &format!("vulkan:{adapter_index}"),
        "-f",
        "lavfi",
        "-i",
        &format!(
            "testsrc2=size={}x{}:rate=30:duration={}",
            args.width,
            args.height,
            (args.frame_count as f64 / 30.0).max(0.001)
        ),
        "-frames:v",
        &args.frame_count.to_string(),
        "-vf",
        "format=nv12,hwupload",
        "-c:v",
        match args.codec {
            Codec::H264 => "h264_vulkan",
            Codec::Hevc => "hevc_vulkan",
            Codec::Av1 => "av1_vulkan",
        },
        "-f",
        "null",
        &null_sink.to_string_lossy(),
    ]);
    command
}

fn run_timed(mut command: Command) -> Result<f64> {
    let start = Instant::now();
    let output = command.output().context("spawn benchmark command")?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "benchmark command failed: status={}; stdout_tail={}; stderr_tail={}",
            output.status,
            tail_for_report(&stdout),
            tail_for_report(&stderr)
        );
    }
    Ok(start.elapsed().as_secs_f64())
}

fn tail_for_report(text: &str) -> String {
    let lines = text
        .lines()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return "-".to_string();
    }
    lines.join(" / ").replace('|', "\\|")
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
        "| Backend | Case | Status | mean(s) | p50(s) | throughput(fps) |"
    )?;
    writeln!(&mut report, "|---|---|---|---:|---:|---:|")?;
    for backend_report in reports {
        for case in &backend_report.cases {
            writeln!(
                &mut report,
                "| {} | {} | {} | {} | {} | {} |",
                backend_report.backend.display(),
                case.case,
                case.status.replace('|', "\\|"),
                fmt_optional(case.mean_seconds),
                fmt_optional(case.p50_seconds),
                fmt_optional(case.throughput_fps)
            )?;
        }
    }
    fs::write(&path, report).with_context(|| format!("write report: {}", path.display()))?;
    Ok(path)
}

fn write_vulkan_report(
    path: &Path,
    args: &Args,
    decode_input: &Path,
    cases: &[CaseSummary],
) -> Result<()> {
    let mut report = String::new();
    writeln!(&mut report, "# Vulkan Backend Benchmark Report")?;
    writeln!(&mut report, "codec: {}", args.codec.as_cli())?;
    writeln!(&mut report, "warmup: {}", args.warmup)?;
    writeln!(&mut report, "repeat: {}", args.repeat)?;
    writeln!(&mut report, "frame_count: {}", args.frame_count)?;
    writeln!(&mut report, "width: {}", args.width)?;
    writeln!(&mut report, "height: {}", args.height)?;
    writeln!(
        &mut report,
        "decode_input_format: {}",
        args.vulkan_decode_input_format.as_cli()
    )?;
    writeln!(&mut report, "decode_input: {}", decode_input.display())?;
    writeln!(&mut report)?;
    writeln!(
        &mut report,
        "| Case | Status | min(s) | mean(s) | p50(s) | p95(s) | p99(s) | max(s) | stddev(s) | CV(%) |"
    )?;
    writeln!(
        &mut report,
        "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|"
    )?;
    for case in cases {
        writeln!(
            &mut report,
            "| {} | {} | n/a | {} | {} | n/a | n/a | n/a | n/a | n/a |",
            case.case,
            case.status.replace('|', "\\|"),
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

fn parse_child_report_status(report_path: &Path, verify_requested: bool) -> Result<BackendStatus> {
    let text = fs::read_to_string(report_path)
        .with_context(|| format!("read report: {}", report_path.display()))?;

    let parity_failed = text.lines().any(|line| {
        line.trim_start().starts_with("- Overall:")
            && (line.contains("=> FAIL") || line.contains("Overall: FAIL"))
    });
    if parity_failed {
        return Ok(BackendStatus::Failed(
            "child report parity check failed".to_string(),
        ));
    }

    if verify_requested {
        let verification_failed = text
            .lines()
            .skip_while(|line| line.trim() != "## Verification")
            .skip(1)
            .take_while(|line| !line.starts_with("## "))
            .any(|line| {
                let line = line.to_ascii_lowercase();
                line.contains("failed") || line.contains("skipped")
            });
        if verification_failed {
            return Ok(BackendStatus::Failed(
                "child report verification did not pass".to_string(),
            ));
        }
    }

    Ok(BackendStatus::Passed)
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
        status: "passed".to_string(),
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
