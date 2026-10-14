//! Camera preview + fragmented MP4 recorder example.
//!
//! This example captures camera frames via `shiguredo_video_device`,
//! previews them in an `eframe` window, and can toggle recording ON/OFF.
//! Recorded chunks are encoded with `video-hw` (H.264/HEVC) and muxed into
//! fragmented MP4 (`ftyp+moov` + repeated `moof+mdat`) via `shiguredo_mp4`.
//! Use the `video-hw-fmp4` crate examples for AV1 fMP4 recording.

use anyhow::{Context, Result};

#[path = "camera_record_fmp4/app.rs"]
mod app;
#[path = "camera_record_fmp4/capture.rs"]
mod capture;
#[path = "camera_record_fmp4/cli.rs"]
mod cli;
#[path = "camera_record_fmp4/codec_samples.rs"]
mod codec_samples;
#[path = "camera_record_fmp4/pixel.rs"]
mod pixel;
#[path = "camera_record_fmp4/recorder.rs"]
mod recorder;
#[cfg(test)]
#[path = "camera_record_fmp4/tests.rs"]
mod tests;

use app::{CameraAppInit, CameraRecordApp};
use capture::{CaptureConfigState, CaptureRuntime};
use cli::{list_devices, parse_args};
use recorder::probe_recording_backend_path;

fn main() -> Result<()> {
    let args = parse_args()?;

    if args.list_devices {
        list_devices();
        return Ok(());
    }

    if args.auto_start_recording {
        let mut preflight_settings = args.recording_settings.clone();
        preflight_settings.fragment_frames = args.fragment_frames;
        probe_recording_backend_path(&preflight_settings, args.width, args.height, args.fps)
            .context("auto-start preflight failed")?;
    }

    let initial_capture_config = CaptureConfigState {
        device_id: args.video_device_id.clone(),
        width: args.width,
        height: args.height,
        fps: args.fps,
    };
    let (rx, capture_event_rx, capture) =
        CaptureRuntime::start(initial_capture_config)?.into_parts();
    let capture_cmd_tx = capture.command_sender();

    println!(
        "camera preview started: {}x{} @ {}",
        args.width, args.height, args.fps
    );
    println!("recording output dir: {}", args.output_dir.display());
    println!(
        "initial recording backend: {}",
        args.recording_settings.backend
    );
    println!("initial recording codec: {}", args.recording_settings.codec);
    println!(
        "initial fragment frequency: {} frame(s)/fragment",
        args.fragment_frames
    );
    if args.auto_start_recording {
        println!("auto-start recording: enabled");
    }
    println!("backend/codec/resolution/fragment frequency can be changed in GUI");
    println!("toggle recording with Start/Stop buttons");

    let mut options = eframe::NativeOptions::default();
    if cfg!(target_os = "macos") {
        options.run_and_return = false;
    }
    let app_capture_cmd_tx = capture_cmd_tx.clone();
    let app_video_device_id = args.video_device_id.clone();
    let app = move |_cc: &eframe::CreationContext<'_>| {
        Ok(Box::new(CameraRecordApp::new(
            rx,
            app_capture_cmd_tx,
            capture_event_rx,
            CameraAppInit {
                video_device_id: app_video_device_id,
                width: args.width,
                height: args.height,
                fps: args.fps,
                auto_start_recording: args.auto_start_recording,
                fragment_frames: args.fragment_frames,
                duration: args.duration,
                output_dir: args.output_dir.clone(),
                recording_settings: args.recording_settings.clone(),
            },
        )) as Box<dyn eframe::App>)
    };

    if let Err(err) = eframe::run_native("Camera fMP4 Recorder", options, Box::new(app)) {
        eprintln!("eframe failed: {err}");
    }

    capture.stop();
    println!("camera preview stopped");
    Ok(())
}
