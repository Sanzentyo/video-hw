use std::{
    borrow::Cow,
    fs,
    num::{NonZeroU32, NonZeroUsize},
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use clap::Parser;
use eframe::egui::{self, ColorImage};
use shiguredo_video_device::{
    PixelFormat, VideoCapture, VideoCaptureConfig, VideoDeviceList, VideoFrame, VideoFrameOwned,
};
use tokio::runtime::Builder;
use video_hw::{Backend, Codec};
use video_hw_fmp4::{
    AsyncRecording, AsyncWriterEvent, Finished, Fmp4Writer, Fmp4WriterConfig, Fmp4WriterStatus,
    Fmp4WriterSummary, FragmentFrames, FrameRate, FrameSize, Pts90k, Ready, RgbaFrame,
};

#[derive(Debug, Parser)]
#[command(about = "Preview camera and record fragmented MP4 (fMP4) with video-hw-fmp4")]
struct CliArgs {
    #[arg(long)]
    list_devices: bool,
    #[arg(long)]
    video_device_id: Option<String>,
    #[arg(long, default_value = "1280x720")]
    resolution: String,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long)]
    duration: Option<f64>,
    #[arg(long, default_value_t = false)]
    auto_start_recording: bool,
    #[arg(long, default_value = "auto")]
    backend: String,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
    #[arg(long, default_value_t = false)]
    intel_force_software: bool,
    #[arg(long, default_value_t = 30)]
    fragment_frames: usize,
    #[arg(long, default_value = "output/camera-fmp4-gui")]
    output_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct RecordingSettings {
    backend: Backend,
    codec: Codec,
    require_hardware: bool,
    intel_force_software: bool,
    fragment_frames: usize,
}

#[derive(Debug, Clone)]
struct CaptureConfigState {
    device_id: Option<String>,
    width: i32,
    height: i32,
    fps: i32,
}

enum CaptureCommand {
    Reconfigure(CaptureConfigState),
    Stop,
}

enum CaptureEvent {
    Reconfigured(CaptureConfigState),
    Error(String),
}

#[derive(Debug)]
struct Args {
    list_devices: bool,
    video_device_id: Option<String>,
    width: i32,
    height: i32,
    fps: i32,
    auto_start_recording: bool,
    fragment_frames: usize,
    duration: Option<Duration>,
    output_dir: PathBuf,
    recording_settings: RecordingSettings,
}

#[derive(Debug, Clone)]
struct UiFrame {
    width: usize,
    height: usize,
    timestamp_us: i64,
    rgba: Vec<u8>,
}

#[derive(Debug)]
struct CapturedFrame {
    frame: VideoFrameOwned,
}

enum RecorderWorkerCommand {
    SubmitFrame {
        frame_rgba: Vec<u8>,
        pts_90k: u64,
    },
    SetFragmentFrames {
        fragment_frames: usize,
    },
    Finish {
        reply_tx: mpsc::Sender<std::result::Result<Fmp4WriterSummary, String>>,
    },
}

enum RecorderWorkerEvent {
    FrameConsumed,
    Status(Fmp4WriterStatus),
    Error(String),
}

struct RecorderWorker {
    command_tx: mpsc::Sender<RecorderWorkerCommand>,
    event_rx: mpsc::Receiver<RecorderWorkerEvent>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl RecorderWorker {
    fn spawn(config: Fmp4WriterConfig) -> Result<Self> {
        let frame_size = config.frame_size;
        let (command_tx, command_rx) = mpsc::channel::<RecorderWorkerCommand>();
        let (event_tx, event_rx) = mpsc::channel::<RecorderWorkerEvent>();
        let (startup_tx, startup_rx) = mpsc::channel::<std::result::Result<(), String>>();
        let join_handle = thread::Builder::new()
            .name("camera-record-worker-thread".to_string())
            .spawn(move || {
                let runtime = match Builder::new_current_thread().build() {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ =
                            startup_tx.send(Err(format!("failed to build tokio runtime: {err:#}")));
                        return;
                    }
                };
                let writer = match Fmp4Writer::<Ready>::new(config).into_async_session() {
                    Ok(writer) => writer,
                    Err(err) => {
                        let _ =
                            startup_tx.send(Err(format!("failed to start async writer: {err:#}")));
                        return;
                    }
                };
                let _ = startup_tx.send(Ok(()));
                run_recorder_worker(runtime, writer, frame_size, command_rx, event_tx);
            })
            .context("failed to spawn recorder worker thread")?;
        match startup_rx
            .recv()
            .context("recorder worker exited before startup status")?
        {
            Ok(()) => {}
            Err(message) => {
                let _ = join_handle.join();
                anyhow::bail!(message);
            }
        }
        Ok(Self {
            command_tx,
            event_rx,
            join_handle: Some(join_handle),
        })
    }

    fn submit_frame(&mut self, frame_rgba: Vec<u8>, pts_90k: u64) -> Result<()> {
        self.command_tx
            .send(RecorderWorkerCommand::SubmitFrame {
                frame_rgba,
                pts_90k,
            })
            .context("failed to queue recorder frame")
    }

    fn set_fragment_frames(&mut self, fragment_frames: usize) -> Result<()> {
        self.command_tx
            .send(RecorderWorkerCommand::SetFragmentFrames { fragment_frames })
            .context("failed to queue fragment frame update")
    }

    fn try_recv_event(&mut self) -> Option<RecorderWorkerEvent> {
        self.event_rx.try_recv().ok()
    }

    fn finish(&mut self) -> Result<Fmp4WriterSummary> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.command_tx
            .send(RecorderWorkerCommand::Finish { reply_tx })
            .context("failed to send recorder worker finish command")?;
        let summary = reply_rx
            .recv()
            .context("failed to receive recorder worker finish result")?
            .map_err(anyhow::Error::msg)?;
        self.join()?;
        Ok(summary)
    }

    fn join_only(&mut self) -> Result<()> {
        self.join()
    }

    fn join(&mut self) -> Result<()> {
        if let Some(join_handle) = self.join_handle.take() {
            join_handle
                .join()
                .map_err(|_| anyhow::anyhow!("recorder worker thread panicked"))?;
        }
        Ok(())
    }
}

fn run_recorder_worker(
    runtime: tokio::runtime::Runtime,
    mut writer: Fmp4Writer<AsyncRecording>,
    frame_size: FrameSize,
    command_rx: mpsc::Receiver<RecorderWorkerCommand>,
    event_tx: mpsc::Sender<RecorderWorkerEvent>,
) {
    drain_writer_events(&mut writer, &event_tx);
    while let Ok(command) = command_rx.recv() {
        match command {
            RecorderWorkerCommand::SubmitFrame {
                frame_rgba,
                pts_90k,
            } => {
                let frame = match RgbaFrame::new(frame_rgba, frame_size) {
                    Ok(frame) => frame,
                    Err(err) => {
                        let _ = event_tx.send(RecorderWorkerEvent::FrameConsumed);
                        let _ = event_tx.send(RecorderWorkerEvent::Error(format!(
                            "invalid RGBA frame: {err:#}"
                        )));
                        break;
                    }
                };
                let result = runtime.block_on(writer.write_rgba(frame, Pts90k::new(pts_90k)));
                let _ = event_tx.send(RecorderWorkerEvent::FrameConsumed);
                if let Err(err) = result {
                    let _ = event_tx.send(RecorderWorkerEvent::Error(format!(
                        "failed to submit frame: {err:#}"
                    )));
                    break;
                }
                if let Some(message) = drain_writer_events(&mut writer, &event_tx) {
                    let _ = event_tx.send(RecorderWorkerEvent::Error(message));
                    break;
                }
            }
            RecorderWorkerCommand::SetFragmentFrames { fragment_frames } => {
                let fragment_frames = match NonZeroUsize::new(fragment_frames.max(1)) {
                    Some(value) => FragmentFrames::new(value),
                    None => unreachable!("fragment_frames.max(1) is non-zero"),
                };
                if let Err(err) = runtime.block_on(writer.set_fragment_frames(fragment_frames)) {
                    let _ = event_tx.send(RecorderWorkerEvent::Error(format!(
                        "failed to update fragment frequency: {err:#}"
                    )));
                    break;
                }
                if let Some(message) = drain_writer_events(&mut writer, &event_tx) {
                    let _ = event_tx.send(RecorderWorkerEvent::Error(message));
                    break;
                }
            }
            RecorderWorkerCommand::Finish { reply_tx } => {
                let result = finalize_async_writer(&runtime, writer);
                let _ = reply_tx.send(result.map_err(|err| format!("{err:#}")));
                return;
            }
        }
    }

    if let Err(err) = finalize_async_writer(&runtime, writer) {
        let _ = event_tx.send(RecorderWorkerEvent::Error(format!(
            "recorder worker shutdown failed: {err:#}"
        )));
    }
}

fn finalize_async_writer(
    runtime: &tokio::runtime::Runtime,
    writer: Fmp4Writer<AsyncRecording>,
) -> Result<Fmp4WriterSummary> {
    let finished: Fmp4Writer<Finished> = runtime.block_on(writer.finish())?;
    Ok(finished.into_summary())
}

fn drain_writer_events(
    writer: &mut Fmp4Writer<AsyncRecording>,
    event_tx: &mpsc::Sender<RecorderWorkerEvent>,
) -> Option<String> {
    while let Some(event) = writer.try_recv_event() {
        match event {
            AsyncWriterEvent::FrameConsumed => {
                let _ = event_tx.send(RecorderWorkerEvent::FrameConsumed);
            }
            AsyncWriterEvent::Status(status) => {
                let _ = event_tx.send(RecorderWorkerEvent::Status(status));
            }
            AsyncWriterEvent::Error(message) => return Some(message),
        }
    }
    None
}

#[derive(Debug, Clone)]
struct BackendProbeStatus {
    backend: Backend,
    available: bool,
    detail: String,
}

struct CameraRecordApp {
    rx: mpsc::Receiver<CapturedFrame>,
    capture_cmd_tx: mpsc::Sender<CaptureCommand>,
    capture_event_rx: mpsc::Receiver<CaptureEvent>,
    latest: Option<UiFrame>,
    texture: Option<egui::TextureHandle>,
    displayed_timestamp_us: Option<i64>,
    started_at: Option<Instant>,
    duration: Option<Duration>,
    video_device_id: Option<String>,
    width: i32,
    height: i32,
    fps: i32,
    output_dir: PathBuf,
    recording_settings: RecordingSettings,
    selected_backend: Backend,
    selected_codec: Codec,
    backend_probe_statuses: Vec<BackendProbeStatus>,
    pending_width: i32,
    pending_height: i32,
    pending_fps: i32,
    fragment_frames: usize,
    pending_fragment_frames: usize,
    recording_seq: u64,
    recording_submitted_frames: u64,
    recording: Option<RecorderWorker>,
    recording_queue_depth: usize,
    controls_collapsed: bool,
    total_received: u64,
    total_rendered: u64,
    status_message: String,
}

struct CameraAppInit {
    video_device_id: Option<String>,
    width: i32,
    height: i32,
    fps: i32,
    auto_start_recording: bool,
    fragment_frames: usize,
    duration: Option<Duration>,
    output_dir: PathBuf,
    recording_settings: RecordingSettings,
}

impl CameraRecordApp {
    fn new(
        rx: mpsc::Receiver<CapturedFrame>,
        capture_cmd_tx: mpsc::Sender<CaptureCommand>,
        capture_event_rx: mpsc::Receiver<CaptureEvent>,
        init: CameraAppInit,
    ) -> Self {
        let mut app = Self {
            rx,
            capture_cmd_tx,
            capture_event_rx,
            latest: None,
            texture: None,
            displayed_timestamp_us: None,
            started_at: None,
            duration: init.duration,
            video_device_id: init.video_device_id,
            width: init.width,
            height: init.height,
            fps: init.fps,
            output_dir: init.output_dir,
            selected_backend: init.recording_settings.backend,
            selected_codec: init.recording_settings.codec,
            backend_probe_statuses: Vec::new(),
            pending_width: init.width,
            pending_height: init.height,
            pending_fps: init.fps,
            fragment_frames: init.fragment_frames.max(1),
            pending_fragment_frames: init.fragment_frames.max(1),
            recording_settings: init.recording_settings,
            recording_seq: 0,
            recording_submitted_frames: 0,
            recording: None,
            recording_queue_depth: 0,
            controls_collapsed: false,
            total_received: 0,
            total_rendered: 0,
            status_message: "ready".to_string(),
        };
        app.refresh_backend_probe_statuses();
        if init.auto_start_recording {
            app.start_recording();
        }
        app
    }

    fn current_recording_settings(&self) -> RecordingSettings {
        let mut settings = self.recording_settings.clone();
        settings.backend = self.selected_backend;
        settings.codec = self.selected_codec;
        settings.fragment_frames = self.fragment_frames.max(1);
        settings
    }

    fn refresh_backend_probe_statuses(&mut self) {
        if self.recording.is_some() {
            return;
        }
        self.backend_probe_statuses = Backend::supported()
            .into_iter()
            .map(|backend| {
                let mut settings = self.current_recording_settings();
                settings.backend = backend;
                match probe_recording_backend_path(&settings, self.width, self.height, self.fps) {
                    Ok(resolved_backend) => BackendProbeStatus {
                        backend,
                        available: true,
                        detail: format!("available (resolved={resolved_backend})"),
                    },
                    Err(err) => BackendProbeStatus {
                        backend,
                        available: false,
                        detail: format!("{err:#}"),
                    },
                }
            })
            .collect();
    }

    fn selected_backend_probe(&self) -> Option<&BackendProbeStatus> {
        self.backend_probe_statuses
            .iter()
            .find(|status| status.backend == self.selected_backend)
    }

    fn handle_owned_frame(&mut self, captured: CapturedFrame) -> Result<()> {
        let Some(ui_frame) = frame_to_rgba(&captured.frame.as_frame()) else {
            return Ok(());
        };
        self.total_received = self.total_received.saturating_add(1);

        if let Some(recorder) = self.recording.as_mut() {
            let started_at = self.started_at.get_or_insert_with(Instant::now);
            // Use fixed-FPS PTS for recording. Capture wallclock looked attractive but produced
            // unstable container duration across devices and players.
            let pts_90k = self
                .recording_submitted_frames
                .saturating_mul(fps_frame_duration_90k(self.fps));
            recorder.submit_frame(ui_frame.rgba.clone(), pts_90k)?;
            self.recording_submitted_frames = self.recording_submitted_frames.saturating_add(1);
            self.recording_queue_depth = self.recording_queue_depth.saturating_add(1);
            let _ = started_at;
        }

        self.latest = Some(ui_frame);
        Ok(())
    }

    fn update_texture_if_needed(&mut self, ui: &egui::Ui) {
        let Some(frame) = &self.latest else {
            return;
        };
        if self.displayed_timestamp_us == Some(frame.timestamp_us) {
            return;
        }
        let expected_rgba_len = frame.width.saturating_mul(frame.height).saturating_mul(4);
        if frame.rgba.len() != expected_rgba_len {
            self.status_message = format!(
                "preview frame dropped: rgba size mismatch (expected {}, got {})",
                expected_rgba_len,
                frame.rgba.len()
            );
            self.displayed_timestamp_us = Some(frame.timestamp_us);
            return;
        }
        let image = ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba);
        match &mut self.texture {
            Some(texture) if texture.size() == [frame.width, frame.height] => {
                texture.set(image, egui::TextureOptions::LINEAR);
            }
            _ => {
                self.texture = Some(ui.ctx().load_texture(
                    "camera-preview-recorder",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        self.displayed_timestamp_us = Some(frame.timestamp_us);
        self.total_rendered = self.total_rendered.saturating_add(1);
    }

    fn poll_capture_events(&mut self) {
        while let Ok(event) = self.capture_event_rx.try_recv() {
            match event {
                CaptureEvent::Reconfigured(config) => {
                    self.width = config.width;
                    self.height = config.height;
                    self.fps = config.fps;
                    self.pending_width = config.width;
                    self.pending_height = config.height;
                    self.pending_fps = config.fps;
                    self.latest = None;
                    self.texture = None;
                    self.displayed_timestamp_us = None;
                    self.refresh_backend_probe_statuses();
                    self.status_message = format!(
                        "capture reconfigured: {}x{} @ {} fps",
                        config.width, config.height, config.fps
                    );
                }
                CaptureEvent::Error(message) => {
                    self.status_message = format!("capture error: {message}");
                }
            }
        }
    }

    fn apply_capture_settings(&mut self) {
        if self.pending_width <= 0 || self.pending_height <= 0 || self.pending_fps <= 0 {
            self.status_message = "capture settings must be positive".to_string();
            return;
        }
        if self.recording.is_some() {
            self.stop_recording();
        }

        let config = CaptureConfigState {
            device_id: self.video_device_id.clone(),
            width: self.pending_width,
            height: self.pending_height,
            fps: self.pending_fps,
        };
        if let Err(err) = self
            .capture_cmd_tx
            .send(CaptureCommand::Reconfigure(config.clone()))
        {
            self.status_message = format!("failed to send capture reconfigure command: {err}");
        } else {
            self.status_message = format!(
                "reconfiguring capture to {}x{} @ {} fps...",
                config.width, config.height, config.fps
            );
        }
    }

    fn apply_fragment_settings(&mut self) {
        self.fragment_frames = self.pending_fragment_frames.max(1);
        self.recording_settings.fragment_frames = self.fragment_frames;
        if let Some(recorder) = self.recording.as_mut() {
            if let Err(err) = recorder.set_fragment_frames(self.fragment_frames) {
                self.status_message = format!("failed to apply fragment settings: {err:#}");
                return;
            }
            self.status_message = format!(
                "fragment settings queued: {} frame(s)/fragment",
                self.fragment_frames
            );
        } else {
            self.status_message = format!(
                "fragment settings applied: {} frame(s)/fragment",
                self.fragment_frames
            );
        }
    }

    fn start_recording(&mut self) {
        if self.recording.is_some() {
            return;
        }
        if let Some(status) = self.selected_backend_probe()
            && !status.available
        {
            self.status_message = format!("record start blocked: {}", status.detail);
            return;
        }

        self.recording_seq = self.recording_seq.saturating_add(1);
        let output_path = match next_recording_path(&self.output_dir, self.recording_seq) {
            Ok(path) => path,
            Err(err) => {
                self.status_message = format!("record start failed: {err:#}");
                return;
            }
        };

        let config = match make_writer_config(
            &self.current_recording_settings(),
            self.width,
            self.height,
            self.fps,
            output_path,
        ) {
            Ok(config) => config,
            Err(err) => {
                self.status_message = format!("record start failed: {err:#}");
                return;
            }
        };

        match RecorderWorker::spawn(config) {
            Ok(worker) => {
                self.started_at = None;
                self.recording_submitted_frames = 0;
                self.recording_queue_depth = 0;
                self.status_message = "recording ON: worker started".to_string();
                self.recording = Some(worker);
            }
            Err(err) => {
                self.status_message = format!("record start failed: {err:#}");
            }
        }
    }

    fn stop_recording(&mut self) {
        if let Some(mut recorder) = self.recording.take() {
            self.recording_queue_depth = 0;
            self.recording_submitted_frames = 0;
            match recorder.finish() {
                Ok(summary) => {
                    self.status_message = format_summary(&summary);
                }
                Err(err) => {
                    self.status_message = format!("record stop failed: {err:#}");
                }
            }
            self.refresh_backend_probe_statuses();
        }
    }

    fn handle_recording_worker_error(&mut self, message: String) {
        self.recording_queue_depth = 0;
        self.recording_submitted_frames = 0;
        let join_result = if let Some(mut recorder) = self.recording.take() {
            let result = recorder.join_only();
            self.refresh_backend_probe_statuses();
            result
        } else {
            Ok(())
        };
        self.status_message = match join_result {
            Ok(()) => format!("recording worker error: {message}"),
            Err(err) => format!("recording worker error: {message}; join failed: {err:#}"),
        };
    }

    fn poll_recording_events(&mut self) {
        let mut worker_error = None::<String>;
        if let Some(recorder) = self.recording.as_mut() {
            while let Some(event) = recorder.try_recv_event() {
                match event {
                    RecorderWorkerEvent::FrameConsumed => {
                        self.recording_queue_depth = self.recording_queue_depth.saturating_sub(1);
                    }
                    RecorderWorkerEvent::Status(status) => {
                        self.status_message = format_status(&status);
                    }
                    RecorderWorkerEvent::Error(message) => {
                        worker_error = Some(message);
                        break;
                    }
                }
            }
        }

        if let Some(message) = worker_error {
            self.handle_recording_worker_error(message);
        }
    }

    fn show_recording_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(format!(
                "recording: {}",
                if self.recording.is_some() {
                    "ON"
                } else {
                    "OFF"
                }
            ));
            if self.recording.is_some() {
                if ui.button("Stop Recording").clicked() {
                    self.stop_recording();
                }
            } else if ui.button("Start Recording").clicked() {
                self.start_recording();
            }
        });
        if let Some(status) = self.selected_backend_probe()
            && !status.available
            && self.recording.is_none()
        {
            ui.label(format!("start preflight warning: {}", status.detail));
        }
        ui.label(format!("status: {}", self.status_message));
    }

    fn show_controls_scroll_contents(&mut self, ui: &mut egui::Ui) {
        ui.heading("Camera Controls");
        ui.label(format!(
            "capture: {}x{} @ {} fps",
            self.width, self.height, self.fps
        ));
        ui.label(format!(
            "frames: received={} rendered={}",
            self.total_received, self.total_rendered
        ));
        if self.recording.is_some() {
            ui.label(format!(
                "record queue: pending={}",
                self.recording_queue_depth
            ));
        }

        if let Some(latest) = &self.latest {
            ui.label(format!("timestamp_us: {}", latest.timestamp_us));
        } else {
            ui.label("waiting for frames...");
        }

        let previous_backend = self.selected_backend;
        let previous_codec = self.selected_codec;
        ui.add_enabled_ui(self.recording.is_none(), |ui| {
            ui.horizontal(|ui| {
                ui.label("backend:");
                egui::ComboBox::from_id_salt("record-backend")
                    .selected_text(self.selected_backend.to_string())
                    .show_ui(ui, |ui| {
                        for backend in Backend::supported() {
                            ui.selectable_value(
                                &mut self.selected_backend,
                                backend,
                                backend.to_string(),
                            );
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("codec:");
                egui::ComboBox::from_id_salt("record-codec")
                    .selected_text(match self.selected_codec {
                        Codec::H264 => "h264",
                        Codec::Hevc => "hevc",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.selected_codec, Codec::H264, "h264");
                        ui.selectable_value(&mut self.selected_codec, Codec::Hevc, "hevc");
                    });
            });
        });
        if previous_backend != self.selected_backend || previous_codec != self.selected_codec {
            self.refresh_backend_probe_statuses();
        }

        egui::CollapsingHeader::new("backend availability")
            .default_open(true)
            .show(ui, |ui| {
                for status in &self.backend_probe_statuses {
                    let availability = if status.available {
                        "available"
                    } else {
                        "unavailable"
                    };
                    let selected_mark = if status.backend == self.selected_backend {
                        "*"
                    } else {
                        " "
                    };
                    ui.label(format!(
                        "{selected_mark} {}: {} ({})",
                        status.backend, availability, status.detail
                    ));
                }
            });

        ui.horizontal(|ui| {
            ui.label("capture settings:");
            ui.add(egui::DragValue::new(&mut self.pending_width).range(160..=3840));
            ui.label("x");
            ui.add(egui::DragValue::new(&mut self.pending_height).range(120..=2160));
            ui.label("@");
            ui.add(egui::DragValue::new(&mut self.pending_fps).range(1..=120));
            ui.label("fps");
            if ui.button("Apply Capture").clicked() {
                self.apply_capture_settings();
            }
        });

        ui.horizontal(|ui| {
            ui.label("fragment frequency:");
            ui.add(egui::DragValue::new(&mut self.pending_fragment_frames).range(1..=300));
            ui.label("frame(s)/fragment");
            if ui.button("Apply Fragment").clicked() {
                self.apply_fragment_settings();
            }
        });
    }
}

impl Drop for CameraRecordApp {
    fn drop(&mut self) {
        if let Some(mut recorder) = self.recording.take()
            && let Err(err) = recorder.finish()
        {
            eprintln!("failed to finalize recording on drop: {err:#}");
        }
    }
}

impl eframe::App for CameraRecordApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(16));
        self.poll_capture_events();
        self.poll_recording_events();

        if let Some(duration) = self.duration
            && let Some(started_at) = self.started_at
            && self.recording.is_some()
            && started_at.elapsed() >= duration
        {
            self.stop_recording();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        const MAX_FRAMES_PER_TICK: usize = 8;
        for _ in 0..MAX_FRAMES_PER_TICK {
            let Ok(captured) = self.rx.try_recv() else {
                break;
            };
            if let Err(err) = self.handle_owned_frame(captured) {
                self.status_message = format!("recording error: {err:#}");
                self.stop_recording();
                break;
            }
        }

        egui::SidePanel::left("camera-controls")
            .resizable(true)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let toggle_label = if self.controls_collapsed {
                        ">> expand controls"
                    } else {
                        "<< collapse controls"
                    };
                    if ui.button(toggle_label).clicked() {
                        self.controls_collapsed = !self.controls_collapsed;
                    }
                });
                self.show_recording_toolbar(ui);
                if !self.controls_collapsed {
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("camera-controls-scroll")
                        .show(ui, |ui| {
                            self.show_controls_scroll_contents(ui);
                        });
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Camera Preview");
            self.update_texture_if_needed(ui);
            if let Some(texture) = &self.texture {
                ui.add(
                    egui::Image::from_texture(egui::load::SizedTexture::from_handle(texture))
                        .shrink_to_fit(),
                );
            } else {
                ui.label("waiting for frames...");
            }
        });
    }
}

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

    let (tx, rx) = mpsc::channel::<CapturedFrame>();
    let (capture_cmd_tx, capture_cmd_rx) = mpsc::channel::<CaptureCommand>();
    let (capture_event_tx, capture_event_rx) = mpsc::channel::<CaptureEvent>();
    let (capture_ready_tx, capture_ready_rx) = mpsc::channel::<Result<()>>();
    let initial_capture_config = CaptureConfigState {
        device_id: args.video_device_id.clone(),
        width: args.width,
        height: args.height,
        fps: args.fps,
    };
    let capture_thread = thread::Builder::new()
        .name("camera-capture-thread".to_string())
        .spawn(move || {
            let mut current_config = initial_capture_config;
            let start_capture = |config: &CaptureConfigState| -> Result<VideoCapture> {
                let frame_tx = tx.clone();
                let mut capture = VideoCapture::new(
                    VideoCaptureConfig {
                        device_id: config.device_id.clone(),
                        width: config.width,
                        height: config.height,
                        fps: config.fps,
                        pixel_format: None,
                    },
                    move |frame: VideoFrame<'_>| {
                        let _ = frame_tx.send(CapturedFrame {
                            frame: frame.to_owned(),
                        });
                    },
                )
                .map_err(|err| anyhow::anyhow!("failed to create video capture: {err}"))?;
                capture
                    .start()
                    .map_err(|err| anyhow::anyhow!("failed to start video capture: {err}"))?;
                Ok(capture)
            };

            let mut capture = match start_capture(&current_config) {
                Ok(capture) => capture,
                Err(err) => {
                    let _ = capture_ready_tx.send(Err(err));
                    return;
                }
            };
            let _ = capture_ready_tx.send(Ok(()));
            let _ = capture_event_tx.send(CaptureEvent::Reconfigured(current_config.clone()));

            while let Ok(command) = capture_cmd_rx.recv() {
                match command {
                    CaptureCommand::Reconfigure(new_config) => {
                        capture.stop();
                        match start_capture(&new_config) {
                            Ok(new_capture) => {
                                capture = new_capture;
                                current_config = new_config.clone();
                                let _ =
                                    capture_event_tx.send(CaptureEvent::Reconfigured(new_config));
                            }
                            Err(err) => {
                                let _ = capture_event_tx.send(CaptureEvent::Error(format!(
                                    "capture reconfigure failed: {err:#}"
                                )));
                                match start_capture(&current_config) {
                                    Ok(restore_capture) => {
                                        capture = restore_capture;
                                        let _ = capture_event_tx.send(CaptureEvent::Reconfigured(
                                            current_config.clone(),
                                        ));
                                    }
                                    Err(restore_err) => {
                                        let _ = capture_event_tx.send(CaptureEvent::Error(
                                            format!("capture restore failed: {restore_err:#}"),
                                        ));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    CaptureCommand::Stop => {
                        capture.stop();
                        break;
                    }
                }
            }
        })
        .context("failed to spawn camera capture thread")?;

    capture_ready_rx
        .recv()
        .context("camera capture thread exited before startup status")??;

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

    if let Err(err) = eframe::run_native(
        "Camera fMP4 Recorder (video-hw-fmp4)",
        options,
        Box::new(app),
    ) {
        eprintln!("eframe failed: {err}");
    }

    let _ = capture_cmd_tx.send(CaptureCommand::Stop);
    if let Err(err) = capture_thread.join() {
        eprintln!("capture thread join failed: {err:?}");
    }
    println!("camera preview stopped");
    Ok(())
}

fn parse_args() -> Result<Args> {
    let cli = CliArgs::parse();
    let fragment_frames = cli.fragment_frames.max(1);
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

fn make_writer_config(
    settings: &RecordingSettings,
    width: i32,
    height: i32,
    fps: i32,
    output_path: PathBuf,
) -> Result<Fmp4WriterConfig> {
    let frame_size = FrameSize::new(
        NonZeroU32::new(u32::try_from(width).context("width must be >= 0")?)
            .context("width must be > 0")?,
        NonZeroU32::new(u32::try_from(height).context("height must be >= 0")?)
            .context("height must be > 0")?,
    );
    Ok(Fmp4WriterConfig {
        output_path,
        frame_size,
        frame_rate: FrameRate::new(
            NonZeroU32::new(u32::try_from(fps).context("fps must be >= 0")?)
                .context("fps must be > 0")?,
        ),
        backend: settings.backend,
        codec: settings.codec,
        require_hardware: settings.require_hardware,
        intel_force_software: settings.intel_force_software,
        fragment_frames: FragmentFrames::new(
            NonZeroUsize::new(settings.fragment_frames.max(1))
                .expect("fragment_frames.max(1) is non-zero"),
        ),
    })
}

fn probe_recording_backend_path(
    settings: &RecordingSettings,
    width: i32,
    height: i32,
    fps: i32,
) -> Result<video_hw::BackendKind> {
    let config = make_writer_config(
        settings,
        width,
        height,
        fps,
        PathBuf::from("/tmp/probe.mp4"),
    )?;
    settings
        .backend
        .resolve_encoder(&video_hw::EncoderConfig::new(
            config.codec,
            i32::try_from(config.frame_rate.get().get()).context("fps must fit in i32")?,
            config.require_hardware,
        ))
        .context("failed to resolve encoder backend")
}

fn parse_resolution(s: &str) -> Option<(i32, i32)> {
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

fn parse_codec(raw: &str) -> Result<Codec> {
    match raw.to_ascii_lowercase().as_str() {
        "h264" => Ok(Codec::H264),
        "hevc" | "h265" => Ok(Codec::Hevc),
        other => anyhow::bail!("unsupported codec: {other}"),
    }
}

fn list_devices() {
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

fn next_recording_path(output_dir: &PathBuf, seq: u64) -> Result<PathBuf> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Ok(output_dir.join(format!("camera-recording-{epoch_ms}-{seq:03}.mp4")))
}

fn fps_frame_duration_90k(fps: i32) -> u64 {
    90_000 / u64::try_from(fps.max(1)).unwrap_or(1)
}

fn duration_90k_to_seconds(duration_90k: u64) -> f64 {
    duration_90k as f64 / 90_000.0
}

fn format_status(status: &Fmp4WriterStatus) -> String {
    format!(
        "recording ON: {} (segments={}, packets={}, bytes={}, fragment_frames={})",
        status.output_path.display(),
        status.segments_written,
        status.packets_seen,
        status.bytes_written,
        status.fragment_frames.get().get(),
    )
}

fn format_summary(summary: &Fmp4WriterSummary) -> String {
    format!(
        "recording OFF: {} (segments={}, packets={}, flush_packets={}, bytes={}, duration_s={:.3})",
        summary.output_path.display(),
        summary.segments_written,
        summary.packets_seen,
        summary.flush_packets,
        summary.bytes_written,
        duration_90k_to_seconds(summary.duration_90k)
    )
}

fn strip_stride<'a>(
    data: &'a [u8],
    row_bytes: usize,
    height: usize,
    stride: usize,
) -> Cow<'a, [u8]> {
    if stride == row_bytes {
        Cow::Borrowed(&data[..row_bytes * height])
    } else {
        let mut result = Vec::with_capacity(row_bytes * height);
        for row in 0..height {
            let start = row * stride;
            result.extend_from_slice(&data[start..start + row_bytes]);
        }
        Cow::Owned(result)
    }
}

fn frame_to_rgba(frame: &VideoFrame<'_>) -> Option<UiFrame> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let stride = frame.stride as usize;
    let stride_uv = frame.stride_uv as usize;

    let rgba = match frame.pixel_format {
        PixelFormat::Nv12 => {
            let y = strip_stride(frame.data, w, h, stride);
            let uv_data = frame.uv_data?;
            let uv = strip_stride(uv_data, w, h / 2, stride_uv);
            nv12_to_rgba(&y, &uv, w, h)
        }
        PixelFormat::I420 => {
            let y = strip_stride(frame.data, w, h, stride);
            let uv_data = frame.uv_data?;
            let uv_w = w / 2;
            let uv_h = h / 2;
            let u_plane_size = stride_uv * uv_h;
            if uv_data.len() < u_plane_size * 2 {
                return None;
            }
            let u = strip_stride(&uv_data[..u_plane_size], uv_w, uv_h, stride_uv);
            let v = strip_stride(
                &uv_data[u_plane_size..u_plane_size * 2],
                uv_w,
                uv_h,
                stride_uv,
            );
            i420_to_rgba(&y, &u, &v, w, h)
        }
        PixelFormat::Yuy2 => {
            let data = strip_stride(frame.data, w * 2, h, stride);
            yuy2_to_rgba(&data, w, h)
        }
        PixelFormat::Unknown(_) => {
            use std::sync::Once;
            static WARN: Once = Once::new();
            WARN.call_once(|| {
                eprintln!(
                    "warning: unsupported pixel format for preview/recording: {}",
                    frame.pixel_format.name()
                );
            });
            return None;
        }
    };

    Some(UiFrame {
        width: w,
        height: h,
        timestamp_us: frame.timestamp_us,
        rgba,
    })
}

fn nv12_to_rgba(y_plane: &[u8], uv_plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let y_row = row * w;
        let uv_row = (row / 2) * w;
        for col in 0..w {
            let y = y_plane[y_row + col];
            let uv_index = uv_row + (col / 2) * 2;
            let u = uv_plane[uv_index];
            let v = uv_plane[uv_index + 1];
            let (r, g, b) = yuv_to_rgb(y, u, v);
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

fn i420_to_rgba(y_plane: &[u8], u_plane: &[u8], v_plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    let uv_w = w / 2;
    for row in 0..h {
        let y_row = row * w;
        let uv_row = (row / 2) * uv_w;
        for col in 0..w {
            let y = y_plane[y_row + col];
            let uv_index = uv_row + (col / 2);
            let u = u_plane[uv_index];
            let v = v_plane[uv_index];
            let (r, g, b) = yuv_to_rgb(y, u, v);
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

fn yuy2_to_rgba(data: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let row_start = row * w * 2;
        let row_data = &data[row_start..row_start + w * 2];
        for chunk in row_data.chunks_exact(4) {
            let y0 = chunk[0];
            let u = chunk[1];
            let y1 = chunk[2];
            let v = chunk[3];
            let (r0, g0, b0) = yuv_to_rgb(y0, u, v);
            let (r1, g1, b1) = yuv_to_rgb(y1, u, v);
            out.extend_from_slice(&[r0, g0, b0, 255]);
            out.extend_from_slice(&[r1, g1, b1, 255]);
        }
    }
    out
}

fn yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let c = (y as i32 - 16).max(0);
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let r = (298 * c + 409 * e + 128) >> 8;
    let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
    let b = (298 * c + 516 * d + 128) >> 8;
    (clamp_u8(r), clamp_u8(g), clamp_u8(b))
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}
