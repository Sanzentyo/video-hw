//! CLI parsing, recording settings, device listing, and output naming.

use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use clap::Parser;
use shiguredo_video_device::VideoDeviceList;
use video_hw::{Backend, Codec};

#[derive(Debug, Parser)]
#[command(about = "Preview camera and record fragmented MP4 (fMP4) with recording ON/OFF")]
pub(super) struct CliArgs {
    #[arg(long)]
    pub(super) list_devices: bool,
    #[arg(long)]
    pub(super) video_device_id: Option<String>,
    #[arg(long, default_value = "1280x720")]
    pub(super) resolution: String,
    #[arg(long, default_value_t = 30)]
    pub(super) fps: i32,
    #[arg(long)]
    pub(super) duration: Option<f64>,
    #[arg(long, default_value_t = false)]
    pub(super) auto_start_recording: bool,
    #[arg(long, default_value = "auto")]
    pub(super) backend: String,
    #[arg(long, default_value = "h264")]
    pub(super) codec: String,
    #[arg(long, default_value_t = false)]
    pub(super) require_hardware: bool,
    #[arg(long, default_value_t = false)]
    pub(super) intel_force_software: bool,
    #[arg(long, default_value_t = 30)]
    pub(super) fragment_frames: u32,
    #[arg(long, default_value = "output/camera-fmp4")]
    pub(super) output_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct RecordingSettings {
    pub(super) backend: Backend,
    pub(super) codec: Codec,
    pub(super) require_hardware: bool,
    pub(super) intel_force_software: bool,
    pub(super) fragment_frames: usize,
}

#[derive(Debug)]
pub(super) struct Args {
    pub(super) list_devices: bool,
    pub(super) video_device_id: Option<String>,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) fps: i32,
    pub(super) auto_start_recording: bool,
    pub(super) fragment_frames: usize,
    pub(super) duration: Option<Duration>,
    pub(super) output_dir: PathBuf,
    pub(super) recording_settings: RecordingSettings,
}

pub(super) fn parse_args() -> Result<Args> {
    let cli = CliArgs::parse();
    let fragment_frames = usize::try_from(cli.fragment_frames).unwrap_or(1).max(1);
    let (width, height) = parse_resolution(&cli.resolution)
        .with_context(|| format!("invalid resolution: {}", cli.resolution))?;
    let recording_settings = if cli.list_devices {
        RecordingSettings {
            backend: Backend::Auto,
            codec: Codec::H264,
            require_hardware: cli.require_hardware,
            intel_force_software: cli.intel_force_software,
            fragment_frames,
        }
    } else {
        RecordingSettings {
            backend: cli.backend.parse()?,
            codec: parse_codec(&cli.codec)?,
            require_hardware: cli.require_hardware,
            intel_force_software: cli.intel_force_software,
            fragment_frames,
        }
    };

    Ok(Args {
        list_devices: cli.list_devices,
        video_device_id: cli.video_device_id,
        width,
        height,
        fps: cli.fps,
        auto_start_recording: cli.auto_start_recording,
        fragment_frames,
        duration: cli.duration.map(Duration::from_secs_f64),
        output_dir: cli.output_dir,
        recording_settings,
    })
}

pub(super) fn parse_resolution(s: &str) -> Option<(i32, i32)> {
    match s.to_lowercase().as_str() {
        "4k" | "2160p" => Some((3840, 2160)),
        "1080p" => Some((1920, 1080)),
        "720p" => Some((1280, 720)),
        "540p" => Some((960, 540)),
        _ => {
            let parts: Vec<&str> = s.split('x').collect();
            if parts.len() == 2 {
                let w = parts[0].parse().ok()?;
                let h = parts[1].parse().ok()?;
                Some((w, h))
            } else {
                None
            }
        }
    }
}

pub(super) fn parse_codec(raw: &str) -> Result<Codec> {
    match raw.to_ascii_lowercase().as_str() {
        "h264" => Ok(Codec::H264),
        "hevc" | "h265" => Ok(Codec::Hevc),
        "av1" => anyhow::bail!(
            "AV1 fMP4 is supported by the video-hw-fmp4 examples; \
             this legacy example only records H.264/HEVC"
        ),
        other => anyhow::bail!("unsupported codec: {other}"),
    }
}

pub(super) fn list_devices() {
    println!("=== video devices ===");
    match VideoDeviceList::enumerate() {
        Ok(devices) => {
            if devices.is_empty() {
                println!("no video devices found");
            } else {
                for device in devices.devices() {
                    let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
                    let id = device.unique_id().unwrap_or_else(|_| "Unknown".to_string());
                    println!("{name}");
                    println!("  id: {id}");
                    for fmt in device.formats() {
                        println!(
                            "  {}x{} @ {:.0}-{:.0} fps ({})",
                            fmt.width,
                            fmt.height,
                            fmt.min_fps,
                            fmt.max_fps,
                            fmt.pixel_format.name()
                        );
                    }
                }
            }
        }
        Err(err) => eprintln!("failed to enumerate video devices: {err:?}"),
    }
}

pub(super) fn next_recording_path(output_dir: &PathBuf, seq: u64) -> Result<PathBuf> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Ok(output_dir.join(format!("camera-recording-{epoch_ms}-{seq:03}.mp4")))
}
