//! eframe camera recorder application state and UI panels.

use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use anyhow::Result;
use eframe::egui::{self, ColorImage};
use video_hw::{Backend, Codec};

use super::{
    capture::{
        CaptureCommand, CaptureConfigState, CaptureEvent, CapturedFrame, RecorderWorker,
        RecorderWorkerEvent,
    },
    cli::{RecordingSettings, next_recording_path},
    codec_samples::duration_90k_to_seconds,
    pixel::{UiFrame, frame_to_rgba},
    recorder::probe_recording_backend_path,
};

#[derive(Debug, Clone)]
pub(super) struct BackendProbeStatus {
    backend: Backend,
    available: bool,
    detail: String,
}

pub(super) struct CameraRecordApp {
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
    recording_first_frame_timestamp_us: Option<i64>,
    recording: Option<RecorderWorker>,
    recording_queue_depth: usize,
    controls_collapsed: bool,
    total_received: u64,
    total_rendered: u64,
    status_message: String,
}

pub(super) struct CameraAppInit {
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

impl CameraRecordApp {
    pub(super) fn new(
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
            recording_first_frame_timestamp_us: None,
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
        let mut statuses = Vec::new();
        for backend in Backend::supported() {
            let mut settings = self.current_recording_settings();
            settings.backend = backend;
            let status =
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
                };
            statuses.push(status);
        }
        self.backend_probe_statuses = statuses;
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

        let duration_limit_90k = self.duration_limit_90k();
        if let Some(recorder) = self.recording.as_mut() {
            let started_at = self.started_at.get_or_insert_with(Instant::now);
            // Derive PTS from capture timestamps so fMP4 duration matches observed capture timing
            // even when the UI update cadence or backend throughput fluctuates.
            let first_timestamp = *self
                .recording_first_frame_timestamp_us
                .get_or_insert(ui_frame.timestamp_us);
            let elapsed_us = ui_frame.timestamp_us.saturating_sub(first_timestamp).max(0);
            let pts_90k = i64::try_from(
                i128::from(elapsed_us)
                    .saturating_mul(90_000)
                    .saturating_div(1_000_000),
            )
            .unwrap_or(i64::MAX);
            if !(matches!(duration_limit_90k, Some(limit_90k) if pts_90k > limit_90k)) {
                recorder.submit_frame(ui_frame.rgba.clone(), pts_90k)?;
                self.recording_submitted_frames = self.recording_submitted_frames.saturating_add(1);
                self.recording_queue_depth = self.recording_queue_depth.saturating_add(1);
            }
            let _ = started_at;
        }

        self.latest = Some(ui_frame);
        Ok(())
    }

    fn drain_pending_capture_frames_for_recording(&mut self) -> Result<()> {
        const MAX_DRAIN_FRAMES_ON_STOP: usize = 512;
        for _ in 0..MAX_DRAIN_FRAMES_ON_STOP {
            let Ok(captured) = self.rx.try_recv() else {
                break;
            };
            self.handle_owned_frame(captured)?;
        }
        Ok(())
    }

    fn clear_capture_backlog_before_recording(&mut self) -> usize {
        const MAX_BACKLOG_CLEAR_FRAMES: usize = 60_000;
        let mut dropped = 0usize;
        while dropped < MAX_BACKLOG_CLEAR_FRAMES {
            match self.rx.try_recv() {
                Ok(_) => dropped = dropped.saturating_add(1),
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
            }
        }
        dropped
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

        let dropped_stale_frames = self.clear_capture_backlog_before_recording();

        self.recording_seq = self.recording_seq.saturating_add(1);
        let output_path = match next_recording_path(&self.output_dir, self.recording_seq) {
            Ok(path) => path,
            Err(err) => {
                self.status_message = format!("record start failed: {err:#}");
                return;
            }
        };

        let settings = self.current_recording_settings();

        match RecorderWorker::spawn(
            settings,
            self.width,
            self.height,
            self.fps,
            output_path.clone(),
        ) {
            Ok(worker) => {
                self.started_at = None;
                self.recording_submitted_frames = 0;
                self.recording_first_frame_timestamp_us = None;
                self.recording_queue_depth = 0;
                self.status_message = if dropped_stale_frames == 0 {
                    "recording ON: worker started".to_string()
                } else {
                    format!(
                        "recording ON: worker started (dropped_stale_frames={dropped_stale_frames})"
                    )
                };
                self.recording = Some(worker);
            }
            Err(err) => {
                self.status_message = format!("record start failed: {err:#}");
            }
        }
    }

    fn stop_recording(&mut self) {
        self.poll_recording_events();
        if self.recording.is_some()
            && let Err(err) = self.drain_pending_capture_frames_for_recording()
        {
            self.status_message = format!("record stop drain failed: {err:#}");
        }
        if let Some(mut recorder) = self.recording.take() {
            self.recording_queue_depth = 0;
            self.recording_submitted_frames = 0;
            self.recording_first_frame_timestamp_us = None;
            match recorder.finish() {
                Ok(summary) => {
                    self.status_message = format!(
                        "recording OFF: {} (segments={}, packets={}, flush_packets={}, bytes={}, duration_s={:.3})",
                        summary.output_path.display(),
                        summary.segments_written,
                        summary.packets_seen,
                        summary.flush_packets,
                        summary.bytes_written,
                        duration_90k_to_seconds(summary.duration_90k)
                    );
                    println!("{}", self.status_message);
                }
                Err(err) => {
                    self.status_message = format!("record stop failed: {err:#}");
                    eprintln!("{}", self.status_message);
                }
            }
            self.refresh_backend_probe_statuses();
        }
    }

    fn handle_recording_worker_error(&mut self, message: String) {
        self.recording_queue_depth = 0;
        self.recording_submitted_frames = 0;
        self.recording_first_frame_timestamp_us = None;
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
        eprintln!("{}", self.status_message);
    }

    fn poll_recording_events(&mut self) {
        let mut worker_error = None::<String>;
        if let Some(recorder) = self.recording.as_mut() {
            while let Some(event) = recorder.try_recv_event() {
                match event {
                    RecorderWorkerEvent::FrameConsumed => {
                        self.recording_queue_depth = self.recording_queue_depth.saturating_sub(1);
                    }
                    RecorderWorkerEvent::Status(message) => {
                        self.status_message = message;
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

    fn duration_limit_90k(&self) -> Option<i64> {
        let duration = self.duration?;
        let ticks = (duration.as_secs_f64() * 90_000.0).round();
        if !ticks.is_finite() || ticks <= 0.0 {
            return None;
        }
        Some(ticks.min(i64::MAX as f64) as i64)
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
                        Codec::Av1 => "av1 (video-hw-fmp4 only)",
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
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
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
        let frame_budget = MAX_FRAMES_PER_TICK;
        for _ in 0..frame_budget {
            let Ok(captured) = self.rx.try_recv() else {
                break;
            };
            if let Err(err) = self.handle_owned_frame(captured) {
                self.status_message = format!("recording error: {err:#}");
                self.stop_recording();
                break;
            }
        }

        if let Some(duration) = self.duration
            && let Some(started_at) = self.started_at
            && self.recording.is_some()
            && started_at.elapsed() >= duration
        {
            self.stop_recording();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::Panel::left("camera-controls")
            .resizable(true)
            .default_size(360.0)
            .show(root_ui, |ui| {
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

        egui::CentralPanel::default().show(root_ui, |ui| {
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
