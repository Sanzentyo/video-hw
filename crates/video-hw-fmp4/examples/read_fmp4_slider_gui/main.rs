mod app;
mod decode;
mod media;
mod smoke;

use std::path::PathBuf;

use anyhow::{Context, Result};
use app::ReaderApp;
use clap::Parser;
use media::load_video_samples;
use smoke::run_smoke_test;
use tracing::debug;
use tracing_subscriber::{EnvFilter, util::SubscriberInitExt};
use video_hw::Backend;

#[derive(Debug, Parser)]
#[command(about = "Read fMP4/MP4 with seek slider")]
struct CliArgs {
    input: PathBuf,
    #[arg(long, default_value = "auto")]
    backend: String,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
    #[arg(long, default_value_t = false)]
    strict_backend: bool,
    #[arg(long, default_value_t = false)]
    auto_play: bool,
    #[arg(long, default_value_t = false)]
    smoke_test: bool,
}

fn main() -> Result<()> {
    init_tracing();
    let cli = CliArgs::parse();
    let backend: Backend = cli
        .backend
        .parse()
        .with_context(|| format!("unsupported backend: {}", cli.backend))?;
    let video = load_video_samples(cli.input)?;

    if cli.smoke_test {
        run_smoke_test(&video, backend, cli.require_hardware, cli.strict_backend)?;
        return Ok(());
    }

    let app = ReaderApp::new(
        video,
        cli.auto_play,
        backend,
        cli.require_hardware,
        cli.strict_backend,
    );
    debug!(
        backend = %backend,
        require_hardware = cli.require_hardware,
        strict_backend = cli.strict_backend,
        auto_play = cli.auto_play,
        "starting reader GUI"
    );
    let mut options = eframe::NativeOptions::default();
    if cfg!(target_os = "macos") {
        options.run_and_return = false;
    }
    let app_creator =
        move |_cc: &eframe::CreationContext<'_>| Ok(Box::new(app) as Box<dyn eframe::App>);
    if let Err(err) = eframe::run_native("fMP4 Reader Slider", options, Box::new(app_creator)) {
        anyhow::bail!("eframe failed: {err}");
    }
    Ok(())
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .finish()
        .try_init()
        .ok();
}
