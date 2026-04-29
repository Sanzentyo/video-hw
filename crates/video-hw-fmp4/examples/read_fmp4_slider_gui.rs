use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::{Hash, Hasher},
    num::NonZeroU32,
    path::PathBuf,
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::Parser;
use eframe::egui;
use shiguredo_mp4::TrackKind;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{EnvFilter, util::SubscriberInitExt};
use video_hw::{Backend, BackendKind, DecodeOutputMode, DecodedFrame, Nv12Frame, nv12_to_rgb24};
use video_hw_fmp4::{
    Fmp4Reader, Fmp4ReaderConfig, Fmp4Track, FrameDecodeRequest, FrameDecoder, SampleMeta,
};

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

#[derive(Debug, Clone)]
struct LoadedVideo {
    input_path: PathBuf,
    track: Fmp4Track,
    samples: Vec<SampleMeta>,
    keyframe_indices: Vec<usize>,
    total_duration_ticks: u64,
    default_sample_duration_ticks: u64,
    estimated_fps: i32,
}

impl LoadedVideo {
    fn total_duration_seconds(&self) -> f64 {
        ticks_to_seconds(self.total_duration_ticks, self.track.timescale)
    }

    fn sample_timestamp_seconds(&self, sample_index: usize) -> f64 {
        let clamped = sample_index.min(self.samples.len().saturating_sub(1));
        ticks_to_seconds(self.samples[clamped].pts.ticks, self.track.timescale)
    }

    fn sample_duration_ticks(&self, sample_index: usize) -> u64 {
        let clamped = sample_index.min(self.samples.len().saturating_sub(1));
        u64::from(self.samples[clamped].duration).max(self.default_sample_duration_ticks)
    }

    fn sample_duration_seconds(&self, sample_index: usize) -> f64 {
        ticks_to_seconds(
            self.sample_duration_ticks(sample_index),
            self.track.timescale,
        )
    }

    fn sample_index_for_seconds(&self, seconds: f64) -> usize {
        if self.samples.is_empty() {
            return 0;
        }
        let target_ticks = seconds_to_ticks(seconds.max(0.0), self.track.timescale);
        match self
            .samples
            .binary_search_by_key(&target_ticks, |sample| sample.pts.ticks)
        {
            Ok(index) => index,
            Err(0) => 0,
            Err(pos) => pos
                .saturating_sub(1)
                .min(self.samples.len().saturating_sub(1)),
        }
    }

    fn keyframe_start_for(&self, sample_index: usize) -> usize {
        if self.keyframe_indices.is_empty() {
            return 0;
        }
        let clamped = sample_index.min(self.samples.len().saturating_sub(1));
        match self.keyframe_indices.binary_search(&clamped) {
            Ok(index) => self.keyframe_indices[index],
            Err(0) => 0,
            Err(pos) => self.keyframe_indices[pos.saturating_sub(1)],
        }
    }
}

#[derive(Debug, Clone)]
struct DecodedPreview {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

struct DecodeAttempt {
    preview: Option<DecodedPreview>,
    frame_count: usize,
    decode_backend: Backend,
    resolved_backend: BackendKind,
    output_mode: DecodeOutputMode,
    fallback_used: bool,
    fallback_reason: Option<String>,
    buffered_previews: Vec<(usize, DecodedPreview)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreviewCacheKey {
    sample_index: usize,
    backend: Backend,
    require_hardware: bool,
}

impl Hash for PreviewCacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.sample_index.hash(state);
        self.require_hardware.hash(state);
        self.backend.to_string().hash(state);
    }
}

#[derive(Clone)]
struct CachedDecodeOutcome {
    preview: Option<DecodedPreview>,
    status: String,
}

#[derive(Debug, Clone, Copy)]
struct DecodeWorkerRequest {
    request_id: u64,
    sample_index: usize,
    backend: Backend,
    require_hardware: bool,
    strict_backend: bool,
}

enum DecodeWorkerCommand {
    Decode(DecodeWorkerRequest),
    Shutdown,
}

struct DecodeWorkerEvent {
    request: DecodeWorkerRequest,
    result: Result<DecodeAttempt, String>,
}

struct DecodeWorker {
    command_tx: mpsc::Sender<DecodeWorkerCommand>,
    event_rx: mpsc::Receiver<DecodeWorkerEvent>,
    join_handle: Option<JoinHandle<()>>,
}

struct ReaderApp {
    video: LoadedVideo,
    current_index: usize,
    slider_seconds: f64,
    playing: bool,
    last_advance_at: Instant,
    status_message: String,
    selected_backend: Backend,
    require_hardware: bool,
    strict_backend: bool,
    decode_worker: DecodeWorker,
    next_decode_request_id: u64,
    inflight_decode_key: Option<PreviewCacheKey>,
    inflight_decode_request_id: Option<u64>,
    pending_decode_keys: HashSet<PreviewCacheKey>,
    decode_cache: HashMap<PreviewCacheKey, CachedDecodeOutcome>,
    decode_cache_lru: VecDeque<PreviewCacheKey>,
    texture: Option<egui::TextureHandle>,
    displayed_cache_key: Option<PreviewCacheKey>,
    decode_status: String,
}

impl ReaderApp {
    const DECODE_CACHE_CAPACITY: usize = 256;
    fn new(
        video: LoadedVideo,
        auto_play: bool,
        selected_backend: Backend,
        require_hardware: bool,
        strict_backend: bool,
    ) -> Self {
        let decode_worker = spawn_decode_worker(video.clone());
        let status_message = format!(
            "loaded {} (track={}, samples={})",
            video.input_path.display(),
            video.track.track_id,
            video.samples.len()
        );
        Self {
            video,
            current_index: 0,
            slider_seconds: 0.0,
            playing: auto_play,
            last_advance_at: Instant::now(),
            status_message,
            selected_backend,
            require_hardware,
            strict_backend,
            decode_worker,
            next_decode_request_id: 1,
            inflight_decode_key: None,
            inflight_decode_request_id: None,
            pending_decode_keys: HashSet::new(),
            decode_cache: HashMap::new(),
            decode_cache_lru: VecDeque::new(),
            texture: None,
            displayed_cache_key: None,
            decode_status: "decoder idle".to_string(),
        }
    }

    fn apply_status_message(&mut self, status: String) {
        self.status_message = status.clone();
        info!("{status}");
    }

    fn apply_decode_status(&mut self, status: String) {
        self.decode_status = status.clone();
        info!("{status}");
    }

    fn apply_decode_warning(&mut self, status: String) {
        self.decode_status = status.clone();
        warn!("{status}");
    }

    fn apply_decode_error(&mut self, status: String) {
        self.decode_status = status.clone();
        error!("{status}");
    }

    fn seek_to_index(&mut self, sample_index: usize) {
        let clamped = sample_index.min(self.video.samples.len().saturating_sub(1));
        self.current_index = clamped;
        self.slider_seconds = self.video.sample_timestamp_seconds(clamped);
        let keyframe_start = self.video.keyframe_start_for(clamped);
        self.apply_status_message(format!(
            "seeked to sample {} (previous keyframe={})",
            clamped, keyframe_start
        ));
        self.last_advance_at = Instant::now();
        self.displayed_cache_key = None;
    }

    fn seek_to_seconds(&mut self, seconds: f64) {
        let target = self.video.sample_index_for_seconds(seconds);
        self.seek_to_index(target);
    }

    fn advance_playback(&mut self) {
        if self.current_index + 1 >= self.video.samples.len() {
            self.playing = false;
            self.apply_status_message("reached end of stream".to_string());
            return;
        }
        self.current_index += 1;
        self.slider_seconds = self.video.sample_timestamp_seconds(self.current_index);
        self.apply_status_message(format!("play sample {}", self.current_index));
        self.displayed_cache_key = None;
    }

    fn reset_decoder_state(&mut self, reason: &str) {
        self.texture = None;
        self.displayed_cache_key = None;
        self.inflight_decode_key = None;
        self.inflight_decode_request_id = None;
        self.pending_decode_keys.clear();
        self.decode_cache.clear();
        self.decode_cache_lru.clear();
        self.apply_decode_status(format!("decoder reset: {reason}"));
    }

    fn update_preview_texture(&mut self, ctx: &egui::Context, preview: &DecodedPreview) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [preview.width, preview.height],
            &preview.rgba,
        );
        match &mut self.texture {
            Some(texture) if texture.size() == [preview.width, preview.height] => {
                texture.set(image, egui::TextureOptions::LINEAR);
            }
            _ => {
                self.texture = Some(ctx.load_texture(
                    "fmp4-reader-preview",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
    }

    fn current_cache_key(&self) -> PreviewCacheKey {
        PreviewCacheKey {
            sample_index: self.current_index,
            backend: self.selected_backend,
            require_hardware: self.require_hardware,
        }
    }

    fn touch_decode_cache_key(&mut self, key: PreviewCacheKey) {
        self.decode_cache_lru.retain(|existing| *existing != key);
        self.decode_cache_lru.push_back(key);
        while self.decode_cache_lru.len() > Self::DECODE_CACHE_CAPACITY {
            if let Some(oldest) = self.decode_cache_lru.pop_front() {
                self.decode_cache.remove(&oldest);
                if self.displayed_cache_key == Some(oldest) {
                    self.displayed_cache_key = None;
                }
            }
        }
    }

    fn store_decode_cache_entry(&mut self, key: PreviewCacheKey, entry: CachedDecodeOutcome) {
        self.decode_cache.insert(key, entry);
        self.touch_decode_cache_key(key);
    }

    fn poll_decode_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.decode_worker.event_rx.try_recv() {
            let key = PreviewCacheKey {
                sample_index: event.request.sample_index,
                backend: event.request.backend,
                require_hardware: event.request.require_hardware,
            };
            self.pending_decode_keys.remove(&key);
            if self.inflight_decode_request_id == Some(event.request.request_id) {
                self.inflight_decode_key = None;
                self.inflight_decode_request_id = None;
            }
            match event.result {
                Ok(attempt) => {
                    info!(
                        request_id = event.request.request_id,
                        sample_index = key.sample_index,
                        requested_backend = %key.backend,
                        decode_backend = %attempt.decode_backend,
                        resolved_backend = %attempt.resolved_backend,
                        output_mode = %attempt.output_mode,
                        frame_count = attempt.frame_count,
                        preview_available = attempt.preview.is_some(),
                        "decode worker completed request"
                    );
                    let decode_backend = attempt.decode_backend.to_string();
                    let resolved_backend = attempt.resolved_backend.to_string();
                    let output_mode = attempt.output_mode.to_string();
                    if let Some(preview) = &attempt.preview {
                        self.store_decode_cache_entry(
                            key,
                            CachedDecodeOutcome {
                                preview: Some(preview.clone()),
                                status: format!(
                                    "decoded sample {} (requested={}, decode_backend={}, resolved={}, mode={}, frames={})",
                                    key.sample_index,
                                    key.backend,
                                    decode_backend,
                                    resolved_backend,
                                    output_mode,
                                    attempt.frame_count
                                ),
                            },
                        );
                    } else {
                        self.store_decode_cache_entry(
                            key,
                            CachedDecodeOutcome {
                                preview: None,
                                status: format!(
                                    "decoded metadata-only sample {} (requested={}, decode_backend={}, resolved={}, mode={}, frames={})",
                                    key.sample_index,
                                    key.backend,
                                    decode_backend,
                                    resolved_backend,
                                    output_mode,
                                    attempt.frame_count
                                ),
                            },
                        );
                    }
                    for (sample_index, preview) in attempt.buffered_previews {
                        let buffered_key = PreviewCacheKey {
                            sample_index,
                            backend: key.backend,
                            require_hardware: key.require_hardware,
                        };
                        if !self.decode_cache.contains_key(&buffered_key) {
                            self.store_decode_cache_entry(
                                buffered_key,
                                CachedDecodeOutcome {
                                    preview: Some(preview),
                                    status: format!(
                                        "prefetched sample {} (requested={}, decode_backend={}, resolved={}, mode={})",
                                        sample_index,
                                        key.backend,
                                        decode_backend,
                                        resolved_backend,
                                        output_mode
                                    ),
                                },
                            );
                        }
                    }
                    if self.current_cache_key() == key
                        && let Some(cached) = self.decode_cache.get(&key).cloned()
                    {
                        if let Some(preview) = &cached.preview {
                            self.update_preview_texture(ctx, preview);
                            self.apply_decode_status(cached.status.clone());
                        } else {
                            self.texture = None;
                            self.apply_decode_warning(cached.status.clone());
                        }
                        self.displayed_cache_key = Some(key);
                    }
                }
                Err(error) => {
                    warn!(
                        request_id = event.request.request_id,
                        sample_index = key.sample_index,
                        requested_backend = %key.backend,
                        "decode worker request failed: {error}"
                    );
                    let status = format!("decode failed at sample {}: {error}", key.sample_index);
                    self.store_decode_cache_entry(
                        key,
                        CachedDecodeOutcome {
                            preview: None,
                            status,
                        },
                    );
                    if self.current_cache_key() == key {
                        self.playing = false;
                        self.texture = None;
                        self.displayed_cache_key = Some(key);
                        self.apply_decode_error(format!(
                            "decode failed at sample {}: {error}",
                            key.sample_index
                        ));
                    }
                }
            }
        }
    }

    fn ensure_decode_requested(&mut self, ctx: &egui::Context) {
        let key = self.current_cache_key();
        if self.displayed_cache_key == Some(key) {
            return;
        }
        if let Some(cached) = self.decode_cache.get(&key).cloned() {
            if let Some(preview) = &cached.preview {
                self.update_preview_texture(ctx, preview);
            } else {
                self.texture = None;
            }
            self.displayed_cache_key = Some(key);
            self.apply_decode_status(cached.status);
            self.touch_decode_cache_key(key);
            return;
        }
        if self.pending_decode_keys.contains(&key) {
            return;
        }
        let request = DecodeWorkerRequest {
            request_id: self.next_decode_request_id,
            sample_index: key.sample_index,
            backend: key.backend,
            require_hardware: key.require_hardware,
            strict_backend: self.strict_backend,
        };
        self.next_decode_request_id = self.next_decode_request_id.saturating_add(1);
        if let Err(err) = self
            .decode_worker
            .command_tx
            .send(DecodeWorkerCommand::Decode(request))
        {
            self.playing = false;
            self.apply_decode_error(format!("failed to queue decode request: {err}"));
            return;
        }
        self.pending_decode_keys.insert(key);
        self.inflight_decode_key = Some(key);
        self.inflight_decode_request_id = Some(request.request_id);
        self.apply_decode_status(format!(
            "decoding sample {} (backend={}, require_hardware={}, fallback={})",
            key.sample_index, key.backend, key.require_hardware, !self.strict_backend
        ));
    }

    fn decode_current_preview(&mut self, ctx: &egui::Context) {
        self.poll_decode_events(ctx);
        self.ensure_decode_requested(ctx);
    }

    fn maybe_prefetch_next_sample(&mut self) {
        let next_sample_index = self
            .current_index
            .saturating_add(1)
            .min(self.video.samples.len().saturating_sub(1));
        if next_sample_index == self.current_index {
            return;
        }
        let key = PreviewCacheKey {
            sample_index: next_sample_index,
            backend: self.selected_backend,
            require_hardware: self.require_hardware,
        };
        if self.decode_cache.contains_key(&key)
            || self.pending_decode_keys.contains(&key)
            || self.inflight_decode_request_id.is_some()
        {
            return;
        }
        let request = DecodeWorkerRequest {
            request_id: self.next_decode_request_id,
            sample_index: key.sample_index,
            backend: key.backend,
            require_hardware: key.require_hardware,
            strict_backend: self.strict_backend,
        };
        self.next_decode_request_id = self.next_decode_request_id.saturating_add(1);
        if self
            .decode_worker
            .command_tx
            .send(DecodeWorkerCommand::Decode(request))
            .is_ok()
        {
            self.pending_decode_keys.insert(key);
        }
    }
}

impl eframe::App for ReaderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(16));

        let now = Instant::now();
        if self.playing {
            let frame_interval = Duration::from_secs_f64(
                self.video
                    .sample_duration_seconds(self.current_index)
                    .max(1.0 / 120.0),
            );
            while now.saturating_duration_since(self.last_advance_at) >= frame_interval {
                self.advance_playback();
                if !self.playing {
                    break;
                }
                self.last_advance_at += frame_interval;
            }
        }

        let mut decoder_settings_changed = false;
        egui::TopBottomPanel::top("reader-controls").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let play_label = if self.playing { "Pause" } else { "Play" };
                if ui.button(play_label).clicked() {
                    self.playing = !self.playing;
                    self.last_advance_at = Instant::now();
                }
                if ui.button("Prev").clicked() {
                    self.playing = false;
                    self.seek_to_index(self.current_index.saturating_sub(1));
                }
                if ui.button("Next").clicked() {
                    self.playing = false;
                    self.seek_to_index(
                        (self.current_index + 1).min(self.video.samples.len().saturating_sub(1)),
                    );
                }
                if ui.button("Restart").clicked() {
                    self.playing = false;
                    self.seek_to_index(0);
                }
            });

            ui.horizontal(|ui| {
                ui.label("backend:");
                let previous_backend = self.selected_backend;
                egui::ComboBox::from_id_salt("read-fmp4-backend")
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
                if previous_backend != self.selected_backend {
                    decoder_settings_changed = true;
                }

                let previous_require_hardware = self.require_hardware;
                ui.checkbox(&mut self.require_hardware, "require hardware");
                if previous_require_hardware != self.require_hardware {
                    decoder_settings_changed = true;
                }

                let previous_strict_backend = self.strict_backend;
                ui.checkbox(&mut self.strict_backend, "strict backend");
                if previous_strict_backend != self.strict_backend {
                    decoder_settings_changed = true;
                }

                if ui.button("Reload decoder").clicked() {
                    decoder_settings_changed = true;
                }
            });

            let max_seconds = self.video.total_duration_seconds().max(0.001);
            let mut slider_seconds = self.slider_seconds.clamp(0.0, max_seconds);
            if ui
                .add(egui::Slider::new(&mut slider_seconds, 0.0..=max_seconds).text("position (s)"))
                .changed()
            {
                self.playing = false;
                self.seek_to_seconds(slider_seconds);
            }
            ui.label(format!("status: {}", self.status_message));
            ui.label(format!("decode: {}", self.decode_status));
        });

        if decoder_settings_changed {
            self.playing = false;
            self.reset_decoder_state("decoder settings changed");
        }

        self.decode_current_preview(ctx);
        self.maybe_prefetch_next_sample();

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(texture) = &self.texture {
                ui.add(
                    egui::Image::from_texture(egui::load::SizedTexture::from_handle(texture))
                        .shrink_to_fit(),
                );
            } else {
                ui.label("decoded preview unavailable for current sample/backend");
            }
            ui.separator();

            let sample = &self.video.samples[self.current_index];
            let codec = self
                .video
                .track
                .codec()
                .map(|codec| codec.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let layout = self
                .video
                .track
                .encoded_layout()
                .map(|layout| layout.to_string())
                .unwrap_or_else(|| "unknown".to_string());

            ui.heading("fMP4 reader (slider seek + decode preview)");
            ui.separator();
            ui.label(format!("input: {}", self.video.input_path.display()));
            ui.label(format!("track_id: {}", self.video.track.track_id));
            ui.label(format!("codec/layout: {codec} / {layout}"));
            ui.label(format!(
                "sample: {}/{}",
                self.current_index + 1,
                self.video.samples.len()
            ));
            ui.label(format!(
                "time: {:.3}/{:.3}s  (timescale={})",
                self.slider_seconds,
                self.video.total_duration_seconds(),
                self.video.track.timescale
            ));
            ui.label(format!(
                "duration: {:.3}s ({} ticks)  keyframe={}",
                self.video.sample_duration_seconds(self.current_index),
                self.video.sample_duration_ticks(self.current_index),
                sample.keyframe
            ));
            ui.label(format!("sample_id: {}", sample.sample_id));
            ui.label(format!("offset: {} bytes: {}", sample.offset, sample.size));
            ui.label(format!("estimated_fps: {}", self.video.estimated_fps));
            ui.small("Fmp4Reader metadata index + on-demand backend decoder preview.");
        });
    }
}

impl Drop for ReaderApp {
    fn drop(&mut self) {
        let _ = self
            .decode_worker
            .command_tx
            .send(DecodeWorkerCommand::Shutdown);
        if let Some(handle) = self.decode_worker.join_handle.take() {
            let _ = handle.join();
        }
    }
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

fn run_smoke_test(
    video: &LoadedVideo,
    backend: Backend,
    require_hardware: bool,
    strict_backend: bool,
) -> Result<()> {
    println!(
        "smoke_test input={} track={} samples={} duration_s={:.3}",
        video.input_path.display(),
        video.track.track_id,
        video.samples.len(),
        video.total_duration_seconds()
    );
    let checkpoints = [
        0usize,
        video.samples.len() / 2,
        video.samples.len().saturating_sub(1),
    ];
    for &target in &checkpoints {
        let sample = &video.samples[target];
        println!(
            "smoke_seek sample={} pts={} dur={} keyframe={} keyframe_start={}",
            target,
            sample.pts.ticks,
            sample.duration,
            sample.keyframe,
            video.keyframe_start_for(target)
        );
    }
    info!(
        backend = %backend,
        require_hardware,
        strict_backend,
        samples = video.samples.len(),
        "starting smoke test"
    );

    anyhow::ensure!(
        video
            .samples
            .windows(2)
            .all(|window| window[0].dts.ticks <= window[1].dts.ticks),
        "sample DTS values are not monotonic"
    );

    for &target in &checkpoints {
        let attempt =
            decode_sample_index_sync(video, backend, require_hardware, target, strict_backend)
                .with_context(|| format!("failed to decode checkpoint sample {target}"))?;
        let resolved_backend = attempt.resolved_backend.to_string();
        let output_mode = attempt.output_mode.to_string();
        let fallback_reason = attempt.fallback_reason.as_deref().unwrap_or("none");
        match attempt.preview {
            Some(preview) => println!(
                "smoke_decode sample={} requested_backend={} decode_backend={} resolved_backend={} mode={} fallback_used={} fallback_reason={} frame={}x{} frames={}",
                target,
                backend,
                attempt.decode_backend,
                resolved_backend,
                output_mode,
                attempt.fallback_used,
                fallback_reason,
                preview.width,
                preview.height,
                attempt.frame_count
            ),
            None => println!(
                "smoke_decode sample={} requested_backend={} decode_backend={} resolved_backend={} mode={} fallback_used={} fallback_reason={} preview=none frames={}",
                target,
                backend,
                attempt.decode_backend,
                resolved_backend,
                output_mode,
                attempt.fallback_used,
                fallback_reason,
                attempt.frame_count
            ),
        }
        anyhow::ensure!(
            attempt.frame_count > 0,
            "decoder produced no frames at sample {} for backend {}",
            target,
            resolved_backend
        );
    }

    println!("smoke_result=ok");
    info!("smoke test completed");
    Ok(())
}

fn load_video_samples(input_path: PathBuf) -> Result<LoadedVideo> {
    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path.clone()))
        .into_sync_session()
        .with_context(|| format!("failed to open {}", input_path.display()))?;

    let track_id = reader
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .context("failed to find video track in input")?
        .track_id;
    let samples = reader
        .samples(track_id)
        .with_context(|| format!("failed to get samples for track {}", track_id))?
        .to_vec();
    let track = reader
        .tracks()
        .iter()
        .find(|track| track.track_id == track_id)
        .cloned()
        .context("video track disappeared after indexing samples")?;
    let _finished = reader.finish();

    anyhow::ensure!(
        !samples.is_empty(),
        "input has no samples for video track {}",
        track.track_id
    );

    let default_sample_duration_ticks =
        average_non_zero_duration_ticks(&samples).unwrap_or_else(|| {
            let fps = 30_u64;
            u64::from(track.timescale.get()).saturating_div(fps.max(1))
        });
    let total_duration_ticks = if track.duration > 0 {
        track.duration
    } else {
        let last = samples.last().expect("samples is non-empty");
        last.pts
            .ticks
            .saturating_add(u64::from(last.duration).max(default_sample_duration_ticks))
    };
    let mut keyframe_indices: Vec<usize> = samples
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| sample.keyframe.then_some(index))
        .collect();
    if keyframe_indices.first().copied() != Some(0) {
        keyframe_indices.insert(0, 0);
    }
    let estimated_fps = estimate_fps(track.timescale, &samples);
    track
        .codec()
        .context("failed to determine video codec from sample entry")?;

    Ok(LoadedVideo {
        input_path,
        track,
        samples,
        keyframe_indices,
        total_duration_ticks,
        default_sample_duration_ticks,
        estimated_fps,
    })
}

fn estimate_fps(timescale: NonZeroU32, samples: &[SampleMeta]) -> i32 {
    let non_zero_durations: Vec<u32> = samples
        .iter()
        .map(|sample| sample.duration)
        .filter(|duration| *duration > 0)
        .collect();
    if non_zero_durations.is_empty() {
        return 30;
    }
    let average_duration = non_zero_durations
        .iter()
        .map(|duration| f64::from(*duration))
        .sum::<f64>()
        / non_zero_durations.len() as f64;
    let fps = (f64::from(timescale.get()) / average_duration).round();
    fps.clamp(1.0, 120.0) as i32
}

fn average_non_zero_duration_ticks(samples: &[SampleMeta]) -> Option<u64> {
    let non_zero_durations: Vec<u64> = samples
        .iter()
        .map(|sample| u64::from(sample.duration))
        .filter(|duration| *duration > 0)
        .collect();
    if non_zero_durations.is_empty() {
        return None;
    }
    let count = u64::try_from(non_zero_durations.len()).ok()?;
    Some(non_zero_durations.iter().sum::<u64>() / count.max(1))
}

fn ticks_to_seconds(ticks: u64, timescale: NonZeroU32) -> f64 {
    ticks as f64 / f64::from(timescale.get())
}

fn seconds_to_ticks(seconds: f64, timescale: NonZeroU32) -> u64 {
    if seconds <= 0.0 {
        return 0;
    }
    let scaled = seconds * f64::from(timescale.get());
    if !scaled.is_finite() || scaled <= 0.0 {
        return 0;
    }
    scaled.round() as u64
}

fn decoded_frame_to_preview(frame: DecodedFrame) -> Result<Option<DecodedPreview>> {
    match frame {
        DecodedFrame::Rgb24 { dims, data, .. } => {
            let width = usize::try_from(dims.width.get()).context("rgb24 width overflows usize")?;
            let height =
                usize::try_from(dims.height.get()).context("rgb24 height overflows usize")?;
            let rgba = rgb24_to_rgba(width, height, &data)?;
            Ok(Some(DecodedPreview {
                width,
                height,
                rgba,
            }))
        }
        DecodedFrame::Nv12 {
            dims, pitch, data, ..
        } => {
            let width = usize::try_from(dims.width.get()).context("nv12 width overflows usize")?;
            let height =
                usize::try_from(dims.height.get()).context("nv12 height overflows usize")?;
            let rgb = nv12_to_rgb24(&Nv12Frame {
                width,
                height,
                pitch,
                pts_90k: None,
                data,
            })
            .map_err(|err| anyhow::anyhow!("nv12_to_rgb24 failed: {err}"))?;
            let rgba = rgb24_to_rgba(rgb.width, rgb.height, &rgb.data)?;
            Ok(Some(DecodedPreview {
                width: rgb.width,
                height: rgb.height,
                rgba,
            }))
        }
        DecodedFrame::Metadata { .. } => Ok(None),
    }
}

fn rgb24_to_rgba(width: usize, height: usize, rgb24: &[u8]) -> Result<Vec<u8>> {
    let pixel_count = width
        .checked_mul(height)
        .context("image dimensions overflow while converting rgb24 to rgba")?;
    let expected_len = pixel_count
        .checked_mul(3)
        .context("rgb24 byte size overflow")?;
    anyhow::ensure!(
        rgb24.len() == expected_len,
        "rgb24 payload size mismatch: expected {}, got {}",
        expected_len,
        rgb24.len()
    );
    let rgba_capacity = pixel_count
        .checked_mul(4)
        .context("rgba byte size overflow")?;
    let mut rgba = Vec::with_capacity(rgba_capacity);
    for rgb in rgb24.chunks_exact(3) {
        rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    Ok(rgba)
}

fn is_preview_mode_fallback_error(err: &anyhow::Error) -> bool {
    const PREVIEW_FALLBACK_PATTERNS: &[&str] = &[
        "requires backend ARGB payload",
        "NV12 payload size mismatch",
        "failed to read decoded frame payload",
        "unsupported decoded pixel format",
    ];
    err.chain().any(|cause| {
        let message = cause.to_string();
        PREVIEW_FALLBACK_PATTERNS
            .iter()
            .any(|pattern| message.contains(pattern))
    })
}

fn decode_attempt_for_mode(
    video: &LoadedVideo,
    decode_backend: Backend,
    require_hardware: bool,
    sample_index: usize,
    output_mode: DecodeOutputMode,
) -> Result<DecodeAttempt> {
    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(video.input_path.clone()))
        .into_sync_session()
        .with_context(|| format!("failed to open {}", video.input_path.display()))?;
    let mut latest_preview = None;
    let mut fallback_preview = None;
    let mut buffered_previews = Vec::new();
    let target_sample = video
        .samples
        .get(sample_index)
        .with_context(|| format!("sample index {sample_index} is out of range"))?;
    let mut request = FrameDecodeRequest::new(video.track.track_id, target_sample.sample_id);
    request.backend = decode_backend;
    request.require_hardware = require_hardware;
    request.output_mode = output_mode;
    request.fps = Some(video.estimated_fps.max(1));

    let mut decoder = FrameDecoder::new(&mut reader);
    let decoded = decoder.decode_sample(request).with_context(|| {
        format!(
            "failed to decode GOP for sample {} (backend={decode_backend}, output_mode={output_mode})",
            target_sample.sample_id
        )
    })?;
    #[cfg(any(
        feature = "backend-vt",
        feature = "backend-nvidia",
        feature = "backend-intel",
        feature = "backend-vulkan"
    ))]
    let fallback_used = decoded.diagnostics.fallback_used;
    #[cfg(not(any(
        feature = "backend-vt",
        feature = "backend-nvidia",
        feature = "backend-intel",
        feature = "backend-vulkan"
    )))]
    let fallback_used = false;
    #[cfg(any(
        feature = "backend-vt",
        feature = "backend-nvidia",
        feature = "backend-intel",
        feature = "backend-vulkan"
    ))]
    let fallback_reason = decoded.diagnostics.fallback_reason.clone();
    #[cfg(not(any(
        feature = "backend-vt",
        feature = "backend-nvidia",
        feature = "backend-intel",
        feature = "backend-vulkan"
    )))]
    let fallback_reason = None;
    let resolved_backend = decoded.resolved_backend;
    let frame_count = decoded.frames.len();
    let target_frame_index = decoded.target_frame_index;
    let sample_index_by_id = video
        .samples
        .iter()
        .enumerate()
        .map(|(index, meta)| (meta.sample_id, index))
        .collect::<HashMap<_, _>>();

    for (frame_index, decoded_frame) in decoded.frames.into_iter().enumerate() {
        let index = decoded_frame
            .sample_id
            .and_then(|sample_id| sample_index_by_id.get(&sample_id).copied())
            .unwrap_or(sample_index);
        if let Some(preview) = decoded_frame_to_preview(decoded_frame.frame)? {
            buffered_previews.push((index, preview.clone()));
            fallback_preview = Some(preview.clone());
            if target_frame_index == Some(frame_index) {
                latest_preview = Some(preview);
            }
        }
    }
    if latest_preview.is_none() {
        latest_preview = fallback_preview;
    }

    Ok(DecodeAttempt {
        preview: latest_preview,
        frame_count,
        decode_backend,
        resolved_backend,
        output_mode,
        fallback_used,
        fallback_reason,
        buffered_previews,
    })
}

fn decode_sample_index_sync(
    video: &LoadedVideo,
    requested_backend: Backend,
    require_hardware: bool,
    sample_index: usize,
    strict_backend: bool,
) -> Result<DecodeAttempt> {
    let mut fallbacks = Vec::<Backend>::new();
    let mut fallback_seen = HashSet::<String>::new();
    let mut push_fallback = |backend: Backend| {
        let label = backend.to_string();
        if fallback_seen.insert(label) {
            fallbacks.push(backend);
        }
    };
    if requested_backend == Backend::Auto {
        for backend in Backend::supported() {
            if backend != Backend::Auto {
                push_fallback(backend);
            }
        }
    } else if !strict_backend {
        push_fallback(requested_backend);
        for backend in Backend::supported() {
            if backend != Backend::Auto && backend != requested_backend {
                push_fallback(backend);
            }
        }
    } else {
        push_fallback(requested_backend);
    }
    anyhow::ensure!(
        !fallbacks.is_empty(),
        "no decode backend available for requested backend {requested_backend}"
    );
    debug!(
        requested_backend = %requested_backend,
        require_hardware,
        strict_backend,
        fallback_chain = ?fallbacks,
        sample_index,
        "built decode fallback chain"
    );
    let mut last_mode_error = None::<anyhow::Error>;
    for &decode_backend in &fallbacks {
        for output_mode in [DecodeOutputMode::Rgb24, DecodeOutputMode::Nv12] {
            match decode_attempt_for_mode(
                video,
                decode_backend,
                require_hardware,
                sample_index,
                output_mode,
            ) {
                Ok(attempt) if attempt.preview.is_some() => return Ok(attempt),
                Ok(_) => {}
                Err(err) if !is_preview_mode_fallback_error(&err) => {
                    warn!(
                        backend = %decode_backend,
                        mode = %output_mode,
                        sample_index,
                        "decoder mode attempt failed: {err:#}"
                    );
                    last_mode_error = Some(err);
                    break;
                }
                Err(err) => {
                    debug!(
                        backend = %decode_backend,
                        mode = %output_mode,
                        sample_index,
                        "preview mode fallback triggered: {err:#}"
                    );
                    last_mode_error = Some(err);
                }
            }
        }
    }
    for &decode_backend in &fallbacks {
        match decode_attempt_for_mode(
            video,
            decode_backend,
            require_hardware,
            sample_index,
            DecodeOutputMode::Metadata,
        ) {
            Ok(attempt) => return Ok(attempt),
            Err(err) => {
                last_mode_error = Some(err);
            }
        }
    }
    if let Some(err) = last_mode_error {
        Err(err)
    } else {
        anyhow::bail!(
            "no decode backend could decode sample {} for requested backend {}",
            sample_index,
            requested_backend
        );
    }
}

fn spawn_decode_worker(video: LoadedVideo) -> DecodeWorker {
    let (command_tx, command_rx) = mpsc::channel::<DecodeWorkerCommand>();
    let (event_tx, event_rx) = mpsc::channel::<DecodeWorkerEvent>();
    let join_handle = std::thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            match command {
                DecodeWorkerCommand::Decode(request) => {
                    debug!(
                        request_id = request.request_id,
                        sample_index = request.sample_index,
                        backend = %request.backend,
                        require_hardware = request.require_hardware,
                        strict_backend = request.strict_backend,
                        "decode worker received request"
                    );
                    let result = decode_sample_index_sync(
                        &video,
                        request.backend,
                        request.require_hardware,
                        request.sample_index,
                        request.strict_backend,
                    )
                    .map_err(|err| format!("{err:#}"));
                    if let Err(err) = &result {
                        warn!(
                            request_id = request.request_id,
                            sample_index = request.sample_index,
                            "decode worker request failed: {err}"
                        );
                    }
                    let _ = event_tx.send(DecodeWorkerEvent { request, result });
                }
                DecodeWorkerCommand::Shutdown => break,
            }
        }
    });
    DecodeWorker {
        command_tx,
        event_rx,
        join_handle: Some(join_handle),
    }
}
