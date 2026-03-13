#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"
---

use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const LIBVPL_REPO_URL: &str = "https://github.com/intel/libvpl";

#[derive(Debug)]
struct Config {
    apply: bool,
    force: bool,
    prefix: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    let config = parse_args(args)?;

    let source_dir = config.prefix;
    let build_dir = source_dir.join("build");
    let install_dir = source_dir.join("install");
    let include_dir = install_dir.join("include").join("vpl");
    let lib_dir = install_dir.join("lib");
    let bin_dir = install_dir.join("bin");
    let mfx_header = include_dir.join("mfx.h");
    let vpl_lib = lib_dir.join("vpl.lib");

    println!("== oneVPL setup ==");
    println!("prefix: {}", source_dir.display());
    println!("mode: {}", if config.apply { "apply" } else { "dry-run" });
    println!();

    if !config.apply {
        println!("Dry-run mode. The following commands would be executed:");
        println!(
            "  git clone --depth 1 {} {}",
            LIBVPL_REPO_URL,
            source_dir.display()
        );
        println!(
            "  cmake -S {} -B {} -DCMAKE_INSTALL_PREFIX={}",
            source_dir.display(),
            build_dir.display(),
            install_dir.display()
        );
        println!(
            "  cmake --build {} --config Release --target install",
            build_dir.display()
        );
        println!();
        print_env_export(&include_dir, &lib_dir, &bin_dir);
        println!();
        println!("Run with `--apply` to perform the setup.");
        return Ok(());
    }

    if config.force && source_dir.exists() {
        println!("Removing existing prefix due to --force: {}", source_dir.display());
        fs::remove_dir_all(&source_dir)?;
    }

    let already_installed = mfx_header.exists() && vpl_lib.exists() && !config.force;
    if already_installed {
        println!(
            "Detected existing oneVPL artifacts, skipping build: {} and {}",
            mfx_header.display(),
            vpl_lib.display()
        );
    } else {
        if !source_dir.exists() {
            run(
                "git",
                vec![
                    OsString::from("clone"),
                    OsString::from("--depth"),
                    OsString::from("1"),
                    OsString::from(LIBVPL_REPO_URL),
                    source_dir.as_os_str().to_os_string(),
                ],
            )?;
        } else if !source_dir.join(".git").exists() {
            return Err(format!(
                "Prefix exists but is not a git repository: {} (use --force to recreate)",
                source_dir.display()
            )
            .into());
        } else {
            println!("Using existing repository: {}", source_dir.display());
        }

        run(
            "cmake",
            vec![
                OsString::from("-S"),
                source_dir.as_os_str().to_os_string(),
                OsString::from("-B"),
                build_dir.as_os_str().to_os_string(),
                OsString::from(format!(
                    "-DCMAKE_INSTALL_PREFIX={}",
                    install_dir.display()
                )),
            ],
        )?;
        run(
            "cmake",
            vec![
                OsString::from("--build"),
                build_dir.as_os_str().to_os_string(),
                OsString::from("--config"),
                OsString::from("Release"),
                OsString::from("--target"),
                OsString::from("install"),
            ],
        )?;
    }

    ensure_exists(&mfx_header)?;
    ensure_exists(&vpl_lib)?;
    println!();
    println!("oneVPL artifacts are ready.");
    println!("  {}", mfx_header.display());
    println!("  {}", vpl_lib.display());
    println!();
    print_env_export(&include_dir, &lib_dir, &bin_dir);
    println!();
    println!("Recommended validation:");
    println!("  cargo clippy --workspace --all-targets --features backend-intel");
    println!("  cargo test --workspace --features backend-intel -- --nocapture");

    Ok(())
}

fn parse_args(args: Vec<OsString>) -> Result<Config, Box<dyn Error>> {
    let mut apply = false;
    let mut force = false;
    let mut prefix: Option<PathBuf> = None;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.to_string_lossy().as_ref() {
            "--apply" => apply = true,
            "--force" => force = true,
            "--prefix" => {
                let value = iter
                    .next()
                    .ok_or("--prefix requires a value (path)")?;
                prefix = Some(PathBuf::from(value));
            }
            unknown => {
                return Err(format!("Unknown argument: {unknown}").into());
            }
        }
    }

    let default_prefix = env::temp_dir().join("libvpl-runtime");
    Ok(Config {
        apply,
        force,
        prefix: prefix.unwrap_or(default_prefix),
    })
}

fn run(program: &str, args: Vec<OsString>) -> Result<(), Box<dyn Error>> {
    let rendered_args = args
        .iter()
        .map(|arg| quote_if_needed(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    println!("> {program} {rendered_args}");

    let status = Command::new(program).args(args).status()?;
    if !status.success() {
        return Err(format!("{program} failed with exit status {status}").into());
    }
    Ok(())
}

fn ensure_exists(path: &Path) -> Result<(), Box<dyn Error>> {
    if !path.exists() {
        return Err(format!("Required file not found: {}", path.display()).into());
    }
    Ok(())
}

fn print_env_export(include_dir: &Path, lib_dir: &Path, bin_dir: &Path) {
    println!("Set these in PowerShell:");
    println!("  $env:LIBVPL_INCLUDE_PATH = \"{}\"", include_dir.display());
    println!("  $env:LIBVPL_LIBRARY_PATH = \"{}\"", lib_dir.display());
    println!("  $env:Path = \"{};$env:Path\"", bin_dir.display());
}

fn quote_if_needed(arg: &str) -> String {
    if arg.contains(' ') {
        format!("\"{arg}\"")
    } else {
        arg.to_owned()
    }
}

fn print_help() {
    println!("Usage:");
    println!("  cargo +nightly -Zscript scripts/setup_onevpl.rs [--apply] [--force] [--prefix <path>]");
    println!();
    println!("Options:");
    println!("  --apply          Execute clone/build/install. Without this flag, runs in dry-run mode.");
    println!("  --force          Remove existing prefix before setup.");
    println!("  --prefix <path>  Install/work prefix (default: %TEMP%\\libvpl-runtime).");
    println!("  -h, --help       Show this help.");
}
