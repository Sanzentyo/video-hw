use std::{
    num::{NonZeroU32, NonZeroUsize},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Parser;
use video_hw::{Backend, Codec};
use video_hw_fmp4::{
    Fmp4Writer, Fmp4WriterConfig, FragmentFrames, FrameRate, FrameSize, Pts90k, Ready, RgbaFrame,
};

#[derive(Debug, Parser)]
struct Cli {
    #[arg(long, default_value = "output/synthetic-fmp4.mp4")]
    output: PathBuf,
    #[arg(long, default_value_t = 640)]
    width: u32,
    #[arg(long, default_value_t = 360)]
    height: u32,
    #[arg(long, default_value_t = 30)]
    fps: u32,
    #[arg(long, default_value_t = 120)]
    frames: u32,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long, default_value_t = 30)]
    fragment_frames: usize,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let codec = match cli.codec.as_str() {
        "h264" => Codec::H264,
        "hevc" | "h265" => Codec::Hevc,
        other => anyhow::bail!("unsupported codec: {other}"),
    };
    let frame_size = FrameSize::new(
        NonZeroU32::new(cli.width).context("width must be > 0")?,
        NonZeroU32::new(cli.height).context("height must be > 0")?,
    );
    let config = Fmp4WriterConfig {
        output_path: cli.output,
        frame_size,
        frame_rate: FrameRate::new(NonZeroU32::new(cli.fps).context("fps must be > 0")?),
        backend: Backend::Auto,
        codec,
        require_hardware: cli.require_hardware,
        intel_force_software: false,
        fragment_frames: FragmentFrames::new(
            NonZeroUsize::new(cli.fragment_frames).context("fragment_frames must be > 0")?,
        ),
    };
    let mut writer = Fmp4Writer::<Ready>::new(config).into_sync_session()?;
    let frame_duration = 90_000_u64 / u64::from(cli.fps.max(1));
    for index in 0..cli.frames {
        let rgba = make_rgba_pattern(frame_size, index);
        let frame = RgbaFrame::new(rgba, frame_size)?;
        writer.write_rgba(frame, Pts90k::new(u64::from(index) * frame_duration))?;
    }
    let finished = writer.finish()?;
    let summary = finished.into_summary();
    println!(
        "wrote {} (segments={}, packets={}, bytes={}, duration_90k={})",
        summary.output_path.display(),
        summary.segments_written,
        summary.packets_seen,
        summary.bytes_written,
        summary.duration_90k,
    );
    Ok(())
}

fn make_rgba_pattern(size: FrameSize, index: u32) -> Vec<u8> {
    let width = size.width().get() as usize;
    let height = size.height().get() as usize;
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            rgba.extend_from_slice(&[
                ((x + index as usize) % 256) as u8,
                ((y * 2 + index as usize) % 256) as u8,
                (((x + y) / 2 + index as usize * 3) % 256) as u8,
                255,
            ]);
        }
    }
    rgba
}
