#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
anyhow = "1"
clap = { version = "4.5", features = ["derive"] }
---

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::Parser;

const REQUIRED_ENCODE_SYMBOLS: &[&str] = &[
    "VK_KHR_video_encode_av1",
    "video_encode_av1",
    "VideoEncodeAV1",
    "ENCODE_AV1",
];

const EXPECTED_DECODE_SYMBOLS: &[&str] = &[
    "VK_KHR_video_decode_av1",
    "video_decode_av1",
    "VideoDecodeAV1",
    "DECODE_AV1",
];

#[derive(Debug, Parser)]
#[command(about = "Check whether the local ash binding exposes Vulkan AV1 encode symbols")]
struct Args {
    #[arg(long, default_value = "ash")]
    crate_name: String,

    #[arg(long, default_value = "output/vulkan-av1-encode-bindings")]
    output_dir: PathBuf,

    #[arg(long, default_value_t = false)]
    fail_on_missing: bool,
}

#[derive(Debug)]
struct CargoInfo {
    version: Option<String>,
    registry_source: Option<PathBuf>,
}

#[derive(Debug)]
struct SymbolScan {
    symbol: &'static str,
    present: bool,
    matches: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create output dir: {}", args.output_dir.display()))?;

    let info = cargo_info(&args.crate_name)?;
    let source = info.registry_source.as_ref().with_context(|| {
        format!(
            "could not find local registry source for crate {}",
            args.crate_name
        )
    })?;
    let encode_scans = scan_symbols(source, REQUIRED_ENCODE_SYMBOLS)?;
    let decode_scans = scan_symbols(source, EXPECTED_DECODE_SYMBOLS)?;
    let has_encode_bindings = encode_scans.iter().any(|scan| scan.present);
    let has_decode_bindings = decode_scans.iter().any(|scan| scan.present);

    let report_path = write_report(
        &args,
        &info,
        source,
        &decode_scans,
        &encode_scans,
        has_decode_bindings,
        has_encode_bindings,
    )?;
    println!("saved report: {}", report_path.display());

    if args.fail_on_missing && !has_encode_bindings {
        bail!("{} does not expose Vulkan AV1 encode bindings", args.crate_name);
    }
    Ok(())
}

fn cargo_info(crate_name: &str) -> Result<CargoInfo> {
    let output = Command::new("cargo")
        .args(["info", crate_name])
        .output()
        .with_context(|| format!("spawn cargo info {crate_name}"))?;
    if !output.status.success() {
        bail!(
            "cargo info {crate_name} failed: status={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout
        .lines()
        .find_map(|line| line.strip_prefix("version: "))
        .map(str::trim)
        .map(ToOwned::to_owned);
    let registry_source = version
        .as_ref()
        .and_then(|version| find_registry_source(crate_name, version).ok().flatten());
    Ok(CargoInfo {
        version,
        registry_source,
    })
}

fn find_registry_source(crate_name: &str, version: &str) -> Result<Option<PathBuf>> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".cargo")))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .context("could not determine cargo home")?;
    let src_root = cargo_home.join("registry").join("src");
    if !src_root.exists() {
        return Ok(None);
    }
    let target_name = format!("{crate_name}-{version}");
    for registry in fs::read_dir(&src_root)
        .with_context(|| format!("read cargo registry src root: {}", src_root.display()))?
    {
        let registry = registry?.path();
        let candidate = registry.join(&target_name);
        if candidate.exists() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn scan_symbols(source: &Path, symbols: &[&'static str]) -> Result<Vec<SymbolScan>> {
    symbols
        .iter()
        .copied()
        .map(|symbol| {
            let matches = count_symbol_matches(source, symbol)
                .with_context(|| format!("scan symbol {symbol} in {}", source.display()))?;
            Ok(SymbolScan {
                symbol,
                present: matches > 0,
                matches,
            })
        })
        .collect()
}

fn count_symbol_matches(root: &Path, symbol: &str) -> Result<usize> {
    let mut count = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(&path).with_context(|| format!("read dir: {}", path.display()))? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_some_and(|extension| extension == "rs") {
                let text = fs::read_to_string(&path)
                    .with_context(|| format!("read Rust source: {}", path.display()))?;
                count = count.saturating_add(text.matches(symbol).count());
            }
        }
    }
    Ok(count)
}

fn write_report(
    args: &Args,
    info: &CargoInfo,
    source: &Path,
    decode_scans: &[SymbolScan],
    encode_scans: &[SymbolScan],
    has_decode_bindings: bool,
    has_encode_bindings: bool,
) -> Result<PathBuf> {
    let epoch = epoch_seconds()?;
    let path = args
        .output_dir
        .join(format!("vulkan-av1-encode-bindings-{epoch}.md"));
    let mut text = String::new();
    writeln!(&mut text, "# Vulkan AV1 Encode Binding Check")?;
    writeln!(&mut text, "epoch_seconds: {epoch}")?;
    writeln!(&mut text, "crate: {}", args.crate_name)?;
    writeln!(
        &mut text,
        "version: {}",
        info.version.as_deref().unwrap_or("unknown")
    )?;
    writeln!(&mut text, "registry_source: {}", source.display())?;
    writeln!(&mut text, "decode_bindings_present: {has_decode_bindings}")?;
    writeln!(&mut text, "encode_bindings_present: {has_encode_bindings}")?;
    writeln!(&mut text)?;
    writeln!(&mut text, "## Decode Symbols")?;
    write_symbol_table(&mut text, decode_scans)?;
    writeln!(&mut text)?;
    writeln!(&mut text, "## Encode Symbols")?;
    write_symbol_table(&mut text, encode_scans)?;
    writeln!(&mut text)?;
    if has_encode_bindings {
        writeln!(
            &mut text,
            "Interpretation: Vulkan AV1 encode bindings are present; implementation can move to driver capability probing."
        )?;
    } else {
        writeln!(
            &mut text,
            "Interpretation: Vulkan AV1 encode remains blocked at the Rust Vulkan binding layer."
        )?;
    }
    fs::write(&path, text).with_context(|| format!("write report: {}", path.display()))?;
    Ok(path)
}

fn write_symbol_table(text: &mut String, scans: &[SymbolScan]) -> Result<()> {
    writeln!(text, "| Symbol | Present | Matches |")?;
    writeln!(text, "|---|---:|---:|")?;
    for scan in scans {
        writeln!(
            text,
            "| `{}` | {} | {} |",
            scan.symbol, scan.present, scan.matches
        )?;
    }
    Ok(())
}

fn epoch_seconds() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?
        .as_secs())
}
