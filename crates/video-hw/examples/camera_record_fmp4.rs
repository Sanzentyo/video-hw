//! Camera preview + fragmented MP4 recorder example.
//!
//! This example captures camera frames via `shiguredo_video_device`,
//! previews them in an `eframe` window, and can toggle recording ON/OFF.
//! Recorded chunks are encoded with `video-hw` (H.264) and muxed into
//! fragmented MP4 (`ftyp+moov` + repeated `moof+mdat`) via `shiguredo_mp4`.

use std::{
    borrow::Cow,
    collections::VecDeque,
    fs::{self, File},
    io::{BufWriter, Write},
    num::NonZeroU32,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use clap::Parser;
use eframe::egui::{self, ColorImage};
use shiguredo_mp4::{
    TrackKind, Uint,
    boxes::{
        Avc1Box, AvccBox, Hev1Box, HvccBox, HvccNalUintArray, SampleEntry, VisualSampleEntryFields,
    },
    mux::{Fmp4SegmentMuxer, Sample},
};
use shiguredo_video_device::{
    PixelFormat, VideoCapture, VideoCaptureConfig, VideoDeviceList, VideoFrame, VideoFrameOwned,
};
use video_hw::BackendError;
#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::IntelEncoderAdapter;
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::NvEncoderAdapter;
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use video_hw::VtEncoderAdapter;
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::VulkanEncoderAdapter;
use video_hw::{
    Backend, BackendEncoderOptions, BackendKind, Codec, Dimensions, EncodeFrame, EncodedChunk,
    EncodedLayout, EncoderConfig, IntelEncoderOptions, RawFrameBuffer, Timestamp90k,
};

#[derive(Debug, Parser)]
#[command(about = "Preview camera and record fragmented MP4 (fMP4) with recording ON/OFF")]
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
    #[arg(long, default_value = "auto")]
    backend: String,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
    #[arg(long, default_value_t = false)]
    intel_force_software: bool,
    #[arg(long, default_value_t = 1)]
    fragment_frames: u32,
    #[arg(long, default_value = "output\\camera-fmp4")]
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
struct PendingSample {
    keyframe: bool,
    pts_90k: i64,
    data: Vec<u8>,
}

#[derive(Debug)]
struct RecorderSummary {
    output_path: PathBuf,
    segments_written: u64,
    packets_seen: u64,
    flush_packets: u64,
    bytes_written: u64,
}

struct RecorderState {
    encoder: BackendEncoderSession,
    muxer: Fmp4SegmentMuxer,
    writer: BufWriter<File>,
    output_path: PathBuf,
    dims: Dimensions,
    width_u16: u16,
    height_u16: u16,
    codec: Codec,
    timescale: NonZeroU32,
    default_duration: u32,
    next_fallback_pts_90k: i64,
    last_written_pts_90k: Option<i64>,
    pending_force_keyframe: bool,
    submitted_frames: u64,
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    sample_entry: Option<SampleEntry>,
    sample_entry_emitted: bool,
    init_written: bool,
    pending_samples: VecDeque<PendingSample>,
    fragment_frames: usize,
    segments_written: u64,
    packets_seen: u64,
    bytes_written: u64,
}

impl RecorderState {
    fn new(
        settings: &RecordingSettings,
        width: i32,
        height: i32,
        fps: i32,
        output_path: PathBuf,
    ) -> Result<Self> {
        let width_u32 = u32::try_from(width).context("width must be >= 0")?;
        let height_u32 = u32::try_from(height).context("height must be >= 0")?;
        let dims = dims(width_u32, height_u32)?;
        let width_u16 =
            u16::try_from(width).context("width must fit in u16 for mp4 sample entry")?;
        let height_u16 =
            u16::try_from(height).context("height must fit in u16 for mp4 sample entry")?;

        let mut config = EncoderConfig::new(settings.codec, fps, settings.require_hardware);
        let resolved_backend = settings
            .backend
            .resolve_encoder(&config)
            .context("failed to resolve encoder backend")?;
        if backend_is_intel(resolved_backend) {
            config.backend_options = BackendEncoderOptions::Intel(IntelEncoderOptions {
                force_software: settings.intel_force_software,
                ..Default::default()
            });
        }

        if let Some(parent) = output_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create output dir: {}", parent.display()))?;
        }
        let file = File::create(&output_path)
            .with_context(|| format!("failed to create {}", output_path.display()))?;
        let writer = BufWriter::new(file);

        let default_duration = u32::try_from((90_000 / fps.max(1)).max(1))
            .context("failed to compute default frame duration")?;

        Ok(Self {
            encoder: BackendEncoderSession::new(resolved_backend, config)?,
            muxer: Fmp4SegmentMuxer::new().context("failed to create fMP4 muxer")?,
            writer,
            output_path,
            dims,
            width_u16,
            height_u16,
            codec: settings.codec,
            timescale: NonZeroU32::new(90_000).expect("90_000 is non-zero"),
            default_duration,
            next_fallback_pts_90k: 0,
            last_written_pts_90k: None,
            pending_force_keyframe: true,
            submitted_frames: 0,
            vps: None,
            sps: None,
            pps: None,
            sample_entry: None,
            sample_entry_emitted: false,
            init_written: false,
            pending_samples: VecDeque::new(),
            fragment_frames: settings.fragment_frames.max(1),
            segments_written: 0,
            packets_seen: 0,
            bytes_written: 0,
        })
    }

    fn submit_argb_frame(
        &mut self,
        frame_argb: Vec<u8>,
        dims: Dimensions,
        pts_90k: i64,
    ) -> Result<()> {
        if dims != self.dims {
            anyhow::bail!(
                "captured frame dimensions changed during recording: expected {}, got {}",
                self.dims,
                dims
            );
        }

        let fragment_interval = self.fragment_frames.max(1) as u64;
        let force_keyframe =
            self.pending_force_keyframe || self.submitted_frames.is_multiple_of(fragment_interval);
        self.encoder
            .submit(EncodeFrame {
                dims,
                pts_90k: Some(Timestamp90k(pts_90k)),
                buffer: RawFrameBuffer::Argb8888(frame_argb),
                force_keyframe,
            })
            .context("encoder submit failed")?;
        self.pending_force_keyframe = false;
        self.submitted_frames = self.submitted_frames.saturating_add(1);

        while let Some(chunk) = self.encoder.try_reap().context("encoder try_reap failed")? {
            self.handle_chunk(chunk)?;
        }

        if self.encoder.requires_periodic_fragment_flush()
            && self.submitted_frames.is_multiple_of(fragment_interval)
        {
            for chunk in self
                .encoder
                .flush()
                .context("encoder periodic fragment flush failed")?
            {
                self.handle_chunk(chunk)?;
            }
        }

        Ok(())
    }

    fn finish(mut self) -> Result<RecorderSummary> {
        let mut flush_packets = 0_u64;
        for chunk in self.encoder.flush().context("encoder flush failed")? {
            flush_packets = flush_packets.saturating_add(1);
            self.handle_chunk(chunk)?;
        }

        if self.sample_entry.is_none() && !self.pending_samples.is_empty() {
            anyhow::bail!("recording ended before SPS/PPS could be observed");
        }

        self.flush_pending_if_ready(true)?;

        if self.segments_written > 0 {
            let mfra = self
                .muxer
                .mfra_bytes()
                .context("failed to build mfra box")?;
            self.writer
                .write_all(&mfra)
                .context("failed to write mfra box")?;
            self.bytes_written = self.bytes_written.saturating_add(mfra.len() as u64);
        }

        self.writer.flush().context("failed to flush output file")?;
        Ok(RecorderSummary {
            output_path: self.output_path,
            segments_written: self.segments_written,
            packets_seen: self.packets_seen,
            flush_packets,
            bytes_written: self.bytes_written,
        })
    }

    fn progress_status(&self) -> String {
        format!(
            "recording ON: {} (segments={}, packets={}, bytes={}, fragment_frames={})",
            self.output_path.display(),
            self.segments_written,
            self.packets_seen,
            self.bytes_written,
            self.fragment_frames
        )
    }

    fn set_fragment_frames(&mut self, fragment_frames: usize) -> Result<()> {
        self.fragment_frames = fragment_frames.max(1);
        self.pending_force_keyframe = true;
        self.submitted_frames = 0;
        self.flush_pending_if_ready(false)
    }

    fn handle_chunk(&mut self, chunk: EncodedChunk) -> Result<()> {
        self.packets_seen = self.packets_seen.saturating_add(1);
        if chunk.codec != self.codec {
            anyhow::bail!(
                "encoded chunk codec mismatch: expected {}, got {}",
                self.codec,
                chunk.codec
            );
        }
        if chunk.layout != EncodedLayout::AnnexB {
            anyhow::bail!(
                "unsupported encoded layout for fMP4 recorder: {} (expected annexb)",
                chunk.layout
            );
        }

        let pts_90k = chunk.pts_90k.map(|pts| pts.0).unwrap_or_else(|| {
            let fallback = self.next_fallback_pts_90k;
            self.next_fallback_pts_90k = self
                .next_fallback_pts_90k
                .saturating_add(i64::from(self.default_duration));
            fallback
        });

        let sample_data = match self.codec {
            Codec::H264 => annexb_chunk_to_avcc_sample(&chunk.data, &mut self.sps, &mut self.pps)?,
            Codec::Hevc => annexb_chunk_to_hvcc_sample(
                &chunk.data,
                &mut self.vps,
                &mut self.sps,
                &mut self.pps,
            )?,
        };
        if let Some(sample_data) = sample_data {
            self.pending_samples.push_back(PendingSample {
                keyframe: chunk.is_keyframe,
                pts_90k,
                data: sample_data,
            });
        }

        self.ensure_sample_entry();
        self.flush_pending_if_ready(false)?;
        Ok(())
    }

    fn ensure_sample_entry(&mut self) {
        if self.sample_entry.is_some() {
            return;
        }
        self.sample_entry = match self.codec {
            Codec::H264 => {
                let (Some(sps), Some(pps)) = (&self.sps, &self.pps) else {
                    return;
                };
                Some(create_h264_sample_entry(
                    self.width_u16,
                    self.height_u16,
                    sps,
                    pps,
                ))
            }
            Codec::Hevc => {
                let (Some(vps), Some(sps), Some(pps)) = (&self.vps, &self.sps, &self.pps) else {
                    return;
                };
                Some(create_hevc_sample_entry(
                    self.width_u16,
                    self.height_u16,
                    vps,
                    sps,
                    pps,
                ))
            }
        };
    }

    fn flush_pending_if_ready(&mut self, flush_partial_fragment: bool) -> Result<()> {
        if self.sample_entry.is_none() {
            return Ok(());
        }
        let fragment_frames = self.fragment_frames.max(1);
        while self.pending_samples.len() >= fragment_frames
            || (flush_partial_fragment && !self.pending_samples.is_empty())
        {
            let take_count = if flush_partial_fragment {
                self.pending_samples.len().min(fragment_frames)
            } else {
                fragment_frames
            };
            let mut fragment_samples = Vec::with_capacity(take_count);
            for _ in 0..take_count {
                if let Some(sample) = self.pending_samples.pop_front() {
                    fragment_samples.push(sample);
                }
            }
            if fragment_samples.is_empty() {
                break;
            }
            self.write_media_fragment(fragment_samples)?;
        }
        Ok(())
    }

    fn write_media_fragment(&mut self, fragment_samples: Vec<PendingSample>) -> Result<()> {
        let include_sample_entry = !self.sample_entry_emitted;
        let mut payload = Vec::new();
        let mut samples = Vec::with_capacity(fragment_samples.len());
        for (index, pending) in fragment_samples.into_iter().enumerate() {
            let sample_entry = if include_sample_entry && index == 0 {
                self.sample_entry.clone()
            } else {
                None
            };
            let duration = self.sample_duration_for_pts(pending.pts_90k);
            let data_offset =
                u64::try_from(payload.len()).context("segment payload offset overflow")?;
            samples.push(Sample {
                track_kind: TrackKind::Video,
                sample_entry,
                keyframe: pending.keyframe,
                timescale: self.timescale,
                duration,
                composition_time_offset: None,
                data_offset,
                data_size: pending.data.len(),
            });
            payload.extend_from_slice(&pending.data);
        }

        let metadata = self
            .muxer
            .create_media_segment_metadata(&samples)
            .context("failed to create media segment metadata")?;

        if !self.init_written {
            let init = self
                .muxer
                .init_segment_bytes()
                .context("failed to build init segment")?;
            self.writer
                .write_all(&init)
                .context("failed to write init segment")?;
            self.bytes_written = self.bytes_written.saturating_add(init.len() as u64);
            self.init_written = true;
        }

        self.writer
            .write_all(&metadata)
            .context("failed to write media segment metadata")?;
        self.writer
            .write_all(&payload)
            .context("failed to write media segment payload")?;
        self.writer
            .flush()
            .context("failed to flush media fragment")?;
        self.writer
            .get_ref()
            .sync_data()
            .context("failed to sync media fragment to disk")?;

        self.bytes_written = self
            .bytes_written
            .saturating_add((metadata.len() + payload.len()) as u64);
        self.segments_written = self.segments_written.saturating_add(1);
        self.sample_entry_emitted = true;
        Ok(())
    }

    fn sample_duration_for_pts(&mut self, pts_90k: i64) -> u32 {
        let duration = match self.last_written_pts_90k {
            Some(prev) => {
                let diff = pts_90k.saturating_sub(prev);
                if diff > 0 {
                    u32::try_from(diff).unwrap_or(self.default_duration)
                } else {
                    self.default_duration
                }
            }
            None => self.default_duration,
        };
        self.last_written_pts_90k = Some(pts_90k);
        duration
    }
}

struct CameraRecordApp {
    rx: mpsc::Receiver<VideoFrameOwned>,
    capture_cmd_tx: mpsc::Sender<CaptureCommand>,
    capture_event_rx: mpsc::Receiver<CaptureEvent>,
    latest: Option<UiFrame>,
    texture: Option<egui::TextureHandle>,
    displayed_timestamp_us: Option<i64>,
    started_at: Instant,
    duration: Option<Duration>,
    video_device_id: Option<String>,
    width: i32,
    height: i32,
    fps: i32,
    output_dir: PathBuf,
    recording_settings: RecordingSettings,
    selected_codec: Codec,
    pending_width: i32,
    pending_height: i32,
    pending_fps: i32,
    fragment_frames: usize,
    pending_fragment_frames: usize,
    recording_seq: u64,
    recording: Option<RecorderState>,
    total_received: u64,
    total_rendered: u64,
    status_message: String,
}

struct CameraAppInit {
    video_device_id: Option<String>,
    width: i32,
    height: i32,
    fps: i32,
    fragment_frames: usize,
    duration: Option<Duration>,
    output_dir: PathBuf,
    recording_settings: RecordingSettings,
}

impl CameraRecordApp {
    fn new(
        rx: mpsc::Receiver<VideoFrameOwned>,
        capture_cmd_tx: mpsc::Sender<CaptureCommand>,
        capture_event_rx: mpsc::Receiver<CaptureEvent>,
        init: CameraAppInit,
    ) -> Self {
        Self {
            rx,
            capture_cmd_tx,
            capture_event_rx,
            latest: None,
            texture: None,
            displayed_timestamp_us: None,
            started_at: Instant::now(),
            duration: init.duration,
            video_device_id: init.video_device_id,
            width: init.width,
            height: init.height,
            fps: init.fps,
            output_dir: init.output_dir,
            selected_codec: init.recording_settings.codec,
            pending_width: init.width,
            pending_height: init.height,
            pending_fps: init.fps,
            fragment_frames: init.fragment_frames.max(1),
            pending_fragment_frames: init.fragment_frames.max(1),
            recording_settings: init.recording_settings,
            recording_seq: 0,
            recording: None,
            total_received: 0,
            total_rendered: 0,
            status_message: "ready".to_string(),
        }
    }

    fn handle_owned_frame(&mut self, owned: VideoFrameOwned) -> Result<()> {
        let Some(ui_frame) = frame_to_rgba(&owned.as_frame()) else {
            return Ok(());
        };
        self.total_received = self.total_received.saturating_add(1);

        if let Some(recorder) = self.recording.as_mut() {
            let argb = rgba_to_argb_bytes(&ui_frame.rgba);
            let dims = dims(
                u32::try_from(ui_frame.width).context("frame width overflow")?,
                u32::try_from(ui_frame.height).context("frame height overflow")?,
            )?;
            let pts_90k = timestamp_us_to_90k(ui_frame.timestamp_us);
            recorder.submit_argb_frame(argb, dims, pts_90k)?;
            self.status_message = recorder.progress_status();
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
            self.status_message = recorder.progress_status();
        } else {
            self.status_message = format!(
                "fragment settings applied: {} frame(s)/fragment",
                self.fragment_frames
            );
        }
    }

    fn recording_preflight(&self) -> Result<()> {
        let config = EncoderConfig::new(
            self.selected_codec,
            self.fps,
            self.recording_settings.require_hardware,
        );
        self.recording_settings
            .backend
            .resolve_encoder(&config)
            .with_context(|| {
                format!(
                    "encoder backend resolution failed (backend={}, codec={}, require_hardware={})",
                    self.recording_settings.backend,
                    self.selected_codec,
                    self.recording_settings.require_hardware
                )
            })?;
        Ok(())
    }

    fn start_recording(&mut self) {
        if self.recording.is_some() {
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

        let mut settings = self.recording_settings.clone();
        settings.codec = self.selected_codec;
        settings.fragment_frames = self.fragment_frames.max(1);

        match RecorderState::new(
            &settings,
            self.width,
            self.height,
            self.fps,
            output_path.clone(),
        ) {
            Ok(state) => {
                self.status_message = state.progress_status();
                self.recording = Some(state);
            }
            Err(err) => {
                self.status_message = format!("record start failed: {err:#}");
            }
        }
    }

    fn stop_recording(&mut self) {
        if let Some(recorder) = self.recording.take() {
            match recorder.finish() {
                Ok(summary) => {
                    self.status_message = format!(
                        "recording OFF: {} (segments={}, packets={}, flush_packets={}, bytes={})",
                        summary.output_path.display(),
                        summary.segments_written,
                        summary.packets_seen,
                        summary.flush_packets,
                        summary.bytes_written
                    );
                }
                Err(err) => {
                    self.status_message = format!("record stop failed: {err:#}");
                }
            }
        }
    }
}

impl Drop for CameraRecordApp {
    fn drop(&mut self) {
        if let Some(recorder) = self.recording.take()
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

        while let Ok(owned) = self.rx.try_recv() {
            if let Err(err) = self.handle_owned_frame(owned) {
                self.status_message = format!("recording error: {err:#}");
                self.stop_recording();
                break;
            }
        }

        if let Some(duration) = self.duration
            && self.started_at.elapsed() >= duration
        {
            self.stop_recording();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Camera Preview + fMP4 Recorder");
            ui.label(format!(
                "capture: {}x{} @ {} fps",
                self.width, self.height, self.fps
            ));
            ui.label(format!(
                "frames: received={} rendered={}",
                self.total_received, self.total_rendered
            ));
            ui.label(format!(
                "recording: {}",
                if self.recording.is_some() {
                    "ON"
                } else {
                    "OFF"
                }
            ));
            ui.label(format!("status: {}", self.status_message));

            if let Some(latest) = &self.latest {
                ui.label(format!("timestamp_us: {}", latest.timestamp_us));
            } else {
                ui.label("waiting for frames...");
            }

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

            ui.horizontal(|ui| {
                if self.recording.is_some() {
                    if ui.button("Stop Recording").clicked() {
                        self.stop_recording();
                    }
                } else {
                    let preflight = self.recording_preflight();
                    if ui.button("Start Recording").clicked() {
                        self.start_recording();
                    }
                    if let Err(err) = preflight {
                        ui.label(format!("start preflight warning: {err:#}"));
                    }
                }
            });

            self.update_texture_if_needed(ui);
            if let Some(texture) = &self.texture {
                ui.add(
                    egui::Image::from_texture(egui::load::SizedTexture::from_handle(texture))
                        .shrink_to_fit(),
                );
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

    let (tx, rx) = mpsc::channel::<VideoFrameOwned>();
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
                        let _ = frame_tx.send(frame.to_owned());
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
    println!("initial recording codec: {}", args.recording_settings.codec);
    println!(
        "initial fragment frequency: {} frame(s)/fragment",
        args.fragment_frames
    );
    println!("codec/resolution/fragment frequency can be changed in GUI");
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

    let _ = capture_cmd_tx.send(CaptureCommand::Stop);
    if let Err(err) = capture_thread.join() {
        eprintln!("capture thread join failed: {err:?}");
    }
    println!("camera preview stopped");
    Ok(())
}

fn parse_args() -> Result<Args> {
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
            backend: parse_backend(&cli.backend)?,
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
        fragment_frames,
        duration: cli.duration.map(Duration::from_secs_f64),
        output_dir: cli.output_dir,
        recording_settings,
    })
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

fn parse_backend(raw: &str) -> Result<Backend> {
    match raw.to_ascii_lowercase().as_str() {
        "auto" => Ok(Backend::Auto),
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        "vt" | "videotoolbox" => Ok(Backend::VideoToolbox),
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        "nvidia" | "nv" => Ok(Backend::Nvidia),
        #[cfg(all(
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        "intel" | "qsv" => Ok(Backend::Intel),
        #[cfg(all(
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        "vulkan" | "vk" => Ok(Backend::Vulkan),
        other => anyhow::bail!("unsupported backend: {other}"),
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

fn create_h264_sample_entry(width: u16, height: u16, sps: &[u8], pps: &[u8]) -> SampleEntry {
    let avc_profile_indication = sps.get(1).copied().unwrap_or(66);
    let profile_compatibility = sps.get(2).copied().unwrap_or(0);
    let avc_level_indication = sps.get(3).copied().unwrap_or(30);

    SampleEntry::Avc1(Avc1Box {
        visual: VisualSampleEntryFields {
            data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            width,
            height,
            horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
            vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
            frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
            compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
            depth: VisualSampleEntryFields::DEFAULT_DEPTH,
        },
        avcc_box: AvccBox {
            avc_profile_indication,
            profile_compatibility,
            avc_level_indication,
            length_size_minus_one: Uint::new(3),
            sps_list: vec![sps.to_vec()],
            pps_list: vec![pps.to_vec()],
            chroma_format: Some(Uint::new(1)),
            bit_depth_luma_minus8: Some(Uint::new(0)),
            bit_depth_chroma_minus8: Some(Uint::new(0)),
            sps_ext_list: vec![],
        },
        unknown_boxes: vec![],
    })
}

fn create_hevc_sample_entry(
    width: u16,
    height: u16,
    vps: &[u8],
    sps: &[u8],
    pps: &[u8],
) -> SampleEntry {
    // Conservative baseline values. The VPS/SPS/PPS arrays carry decoder-critical details.
    let nalu_arrays = vec![
        HvccNalUintArray {
            array_completeness: Uint::new(1),
            nal_unit_type: Uint::new(32),
            nalus: vec![vps.to_vec()],
        },
        HvccNalUintArray {
            array_completeness: Uint::new(1),
            nal_unit_type: Uint::new(33),
            nalus: vec![sps.to_vec()],
        },
        HvccNalUintArray {
            array_completeness: Uint::new(1),
            nal_unit_type: Uint::new(34),
            nalus: vec![pps.to_vec()],
        },
    ];

    SampleEntry::Hev1(Hev1Box {
        visual: VisualSampleEntryFields {
            data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            width,
            height,
            horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
            vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
            frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
            compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
            depth: VisualSampleEntryFields::DEFAULT_DEPTH,
        },
        hvcc_box: HvccBox {
            general_profile_space: Uint::new(0),
            general_tier_flag: Uint::new(0),
            general_profile_idc: Uint::new(1),
            general_profile_compatibility_flags: 0,
            general_constraint_indicator_flags: Uint::new(0),
            general_level_idc: 120,
            min_spatial_segmentation_idc: Uint::new(0),
            parallelism_type: Uint::new(0),
            chroma_format_idc: Uint::new(1),
            bit_depth_luma_minus8: Uint::new(0),
            bit_depth_chroma_minus8: Uint::new(0),
            avg_frame_rate: 0,
            constant_frame_rate: Uint::new(0),
            num_temporal_layers: Uint::new(0),
            temporal_id_nested: Uint::new(1),
            length_size_minus_one: Uint::new(3),
            nalu_arrays,
        },
        unknown_boxes: vec![],
    })
}

fn annexb_chunk_to_avcc_sample(
    annexb: &[u8],
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus: Vec<&[u8]> = Vec::new();
    for nalu in split_annexb_nalus(annexb) {
        if nalu.is_empty() {
            continue;
        }
        match h264_nal_type(nalu) {
            7 => *sps_out = Some(nalu.to_vec()),
            8 => *pps_out = Some(nalu.to_vec()),
            _ => sample_nalus.push(nalu),
        }
    }

    if sample_nalus.is_empty() {
        return Ok(None);
    }

    let mut avcc = Vec::new();
    for nalu in sample_nalus {
        let len = u32::try_from(nalu.len()).context("NAL too large for AVCC")?;
        avcc.extend_from_slice(&len.to_be_bytes());
        avcc.extend_from_slice(nalu);
    }
    Ok(Some(avcc))
}

fn annexb_chunk_to_hvcc_sample(
    annexb: &[u8],
    vps_out: &mut Option<Vec<u8>>,
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus: Vec<&[u8]> = Vec::new();
    for nalu in split_annexb_nalus(annexb) {
        if nalu.is_empty() {
            continue;
        }
        match hevc_nal_type(nalu) {
            32 => *vps_out = Some(nalu.to_vec()),
            33 => *sps_out = Some(nalu.to_vec()),
            34 => *pps_out = Some(nalu.to_vec()),
            _ => sample_nalus.push(nalu),
        }
    }

    if sample_nalus.is_empty() {
        return Ok(None);
    }

    let mut hvcc = Vec::new();
    for nalu in sample_nalus {
        let len = u32::try_from(nalu.len()).context("NAL too large for hvcC sample")?;
        hvcc.extend_from_slice(&len.to_be_bytes());
        hvcc.extend_from_slice(nalu);
    }
    Ok(Some(hvcc))
}

fn split_annexb_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let mut cursor = 0usize;

    while let Some((start, start_code_len)) = find_start_code(data, cursor) {
        let nalu_start = start + start_code_len;
        let nalu_end = find_start_code(data, nalu_start)
            .map(|(pos, _)| pos)
            .unwrap_or(data.len());
        if nalu_start < nalu_end {
            nalus.push(&data[nalu_start..nalu_end]);
        }
        cursor = nalu_end;
    }

    if nalus.is_empty() && !data.is_empty() {
        nalus.push(data);
    }
    nalus
}

fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    if data.len() < 3 || from >= data.len() {
        return None;
    }
    let mut i = from;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if data.get(i + 2) == Some(&1) {
                return Some((i, 3));
            }
            if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                return Some((i, 4));
            }
        }
        i += 1;
    }
    None
}

fn h264_nal_type(nalu: &[u8]) -> u8 {
    nalu[0] & 0x1f
}

fn hevc_nal_type(nalu: &[u8]) -> u8 {
    (nalu[0] >> 1) & 0x3f
}

fn timestamp_us_to_90k(timestamp_us: i64) -> i64 {
    timestamp_us.saturating_mul(90).div_euclid(1000)
}

fn rgba_to_argb_bytes(rgba: &[u8]) -> Vec<u8> {
    let mut argb = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        argb.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    argb
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

enum BackendEncoderSession {
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    Nvidia(Box<video_hw::EncodeSession<NvEncoderAdapter>>),
    #[cfg(all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    Intel(Box<video_hw::EncodeSession<IntelEncoderAdapter>>),
    #[cfg(all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    ))]
    Vulkan(Box<video_hw::EncodeSession<VulkanEncoderAdapter>>),
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    VideoToolbox(Box<video_hw::EncodeSession<VtEncoderAdapter>>),
}

#[cfg(any(
    all(feature = "backend-vt", target_os = "macos"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    )
))]
impl BackendEncoderSession {
    fn new(backend: BackendKind, config: EncoderConfig) -> Result<Self> {
        let session = match backend {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Nvidia => Self::Nvidia(Box::new(video_hw::EncodeSession::<
                NvEncoderAdapter,
            >::new(config))),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Intel => Self::Intel(Box::new(video_hw::EncodeSession::<
                IntelEncoderAdapter,
            >::new(config))),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Vulkan => Self::Vulkan(Box::new(video_hw::EncodeSession::<
                VulkanEncoderAdapter,
            >::new(config))),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            BackendKind::VideoToolbox => Self::VideoToolbox(Box::new(video_hw::EncodeSession::<
                VtEncoderAdapter,
            >::new(config))),
        };
        Ok(session)
    }

    fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError> {
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.submit(frame),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.submit(frame),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.submit(frame),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.submit(frame),
        }
    }

    fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.try_reap(),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.try_reap(),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.try_reap(),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.try_reap(),
        }
    }

    fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.flush(),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.flush(),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.flush(),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.flush(),
        }
    }

    fn requires_periodic_fragment_flush(&self) -> bool {
        #[cfg(all(
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        if matches!(self, Self::Intel(_)) {
            return true;
        }
        false
    }
}

#[cfg(not(any(
    all(feature = "backend-vt", target_os = "macos"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    )
)))]
impl BackendEncoderSession {
    fn new(_backend: BackendKind, _config: EncoderConfig) -> Result<Self> {
        anyhow::bail!(
            "no encoder backend compiled; enable one of backend-nvidia/backend-intel/backend-vulkan/backend-vt"
        )
    }

    fn submit(&mut self, _frame: EncodeFrame) -> Result<(), BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no encoder backend compiled for this target".to_string(),
        ))
    }

    fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no encoder backend compiled for this target".to_string(),
        ))
    }

    fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no encoder backend compiled for this target".to_string(),
        ))
    }

    fn requires_periodic_fragment_flush(&self) -> bool {
        false
    }
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
fn backend_is_intel(backend: BackendKind) -> bool {
    matches!(backend, BackendKind::Intel)
}

#[cfg(not(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
)))]
fn backend_is_intel(_backend: BackendKind) -> bool {
    false
}

fn dims(width: u32, height: u32) -> Result<Dimensions> {
    let width = NonZeroU32::new(width).context("width must be > 0")?;
    let height = NonZeroU32::new(height).context("height must be > 0")?;
    Ok(Dimensions { width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h264_sample_entry_sets_avcc_extended_fields() {
        let sample_entry = create_h264_sample_entry(
            1280,
            720,
            &[0x67, 0x64, 0x00, 0x1f],
            &[0x68, 0xee, 0x3c, 0x80],
        );

        let SampleEntry::Avc1(avc1) = sample_entry else {
            panic!("expected avc1 sample entry");
        };

        assert!(avc1.avcc_box.chroma_format.is_some());
        assert!(avc1.avcc_box.bit_depth_luma_minus8.is_some());
        assert!(avc1.avcc_box.bit_depth_chroma_minus8.is_some());
    }

    #[test]
    fn h264_sample_entry_allows_init_segment_build() {
        let sample_entry = create_h264_sample_entry(
            1280,
            720,
            &[0x67, 0x64, 0x00, 0x1f],
            &[0x68, 0xee, 0x3c, 0x80],
        );
        let mut muxer = Fmp4SegmentMuxer::new().expect("muxer should initialize");
        let sample = Sample {
            track_kind: TrackKind::Video,
            sample_entry: Some(sample_entry),
            keyframe: true,
            timescale: NonZeroU32::new(90_000).expect("constant timescale is non-zero"),
            duration: 3_000,
            composition_time_offset: None,
            data_offset: 0,
            data_size: 4,
        };
        muxer
            .create_media_segment_metadata(&[sample])
            .expect("sample metadata should initialize track");

        let init = muxer
            .init_segment_bytes()
            .expect("init segment should be buildable");

        assert!(!init.is_empty());
    }
}
