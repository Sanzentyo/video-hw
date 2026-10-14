use std::{
    collections::{HashMap, VecDeque},
    hash::{Hash, Hasher},
    sync::mpsc,
    time::{Duration, Instant},
};

use eframe::egui;
use tracing::{debug, error, info, warn};
use video_hw::Backend;

use super::{
    decode::{DecodeWorker, DecodeWorkerRequest, DecodedPreview, spawn_decode_worker},
    media::LoadedVideo,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreviewCacheKey {
    sample_index: usize,
    backend: Backend,
    require_hardware: bool,
    strict_backend: bool,
}

impl Hash for PreviewCacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.sample_index.hash(state);
        self.require_hardware.hash(state);
        self.strict_backend.hash(state);
        self.backend.to_string().hash(state);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecodeStatusLevel {
    Success,
    Warning,
    Error,
}

#[derive(Clone)]
struct CachedDecodeOutcome {
    preview: Option<DecodedPreview>,
    status: String,
    level: DecodeStatusLevel,
}

#[derive(Debug, Default)]
struct DecodeScheduler {
    active: Option<DecodeWorkerRequest>,
}

impl DecodeScheduler {
    fn is_busy(&self) -> bool {
        self.active.is_some()
    }

    #[cfg(test)]
    fn active_key(&self) -> Option<PreviewCacheKey> {
        self.active.map(|request| PreviewCacheKey {
            sample_index: request.sample_index,
            backend: request.backend,
            require_hardware: request.require_hardware,
            strict_backend: request.strict_backend,
        })
    }

    fn try_activate(&mut self, request: DecodeWorkerRequest) -> bool {
        if self.active.is_some() {
            return false;
        }
        self.active = Some(request);
        true
    }

    fn complete(&mut self, request_id: u64) -> Option<DecodeWorkerRequest> {
        if self
            .active
            .is_some_and(|request| request.request_id == request_id)
        {
            self.active.take()
        } else {
            None
        }
    }

    fn clear(&mut self) {
        self.active = None;
    }
}

pub(super) struct ReaderApp {
    video: LoadedVideo,
    current_index: usize,
    slider_seconds: f64,
    at_end_of_stream: bool,
    playing: bool,
    last_advance_at: Instant,
    status_message: String,
    selected_backend: Backend,
    require_hardware: bool,
    strict_backend: bool,
    decode_worker: DecodeWorker,
    next_decode_request_id: u64,
    decode_generation: u64,
    decode_scheduler: DecodeScheduler,
    decode_cache: HashMap<PreviewCacheKey, CachedDecodeOutcome>,
    decode_cache_lru: VecDeque<PreviewCacheKey>,
    decode_cache_resident_bytes: usize,
    texture: Option<egui::TextureHandle>,
    displayed_cache_key: Option<PreviewCacheKey>,
    decode_status: String,
}

impl ReaderApp {
    const DECODE_CACHE_CAPACITY: usize = 64;
    #[cfg(not(test))]
    const DECODE_CACHE_MAX_BYTES: usize = 256 * 1024 * 1024;
    #[cfg(test)]
    const DECODE_CACHE_MAX_BYTES: usize = 1024;
    pub(super) fn new(
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
            video.sample_count()
        );
        Self {
            video,
            current_index: 0,
            slider_seconds: 0.0,
            at_end_of_stream: false,
            playing: auto_play,
            last_advance_at: Instant::now(),
            status_message,
            selected_backend,
            require_hardware,
            strict_backend,
            decode_worker,
            next_decode_request_id: 1,
            decode_generation: 0,
            decode_scheduler: DecodeScheduler::default(),
            decode_cache: HashMap::new(),
            decode_cache_lru: VecDeque::new(),
            decode_cache_resident_bytes: 0,
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
        let clamped = sample_index.min(self.video.sample_count().saturating_sub(1));
        self.current_index = clamped;
        self.slider_seconds = self.video.sample_timestamp_seconds(clamped);
        self.at_end_of_stream = false;
        let keyframe_start = self.video.keyframe_start_for(clamped);
        self.apply_status_message(format!(
            "seeked to sample {} (previous keyframe={})",
            clamped, keyframe_start
        ));
        self.last_advance_at = Instant::now();
        self.displayed_cache_key = None;
    }

    fn seek_to_seconds(&mut self, seconds: f64) {
        let clamped = seconds.clamp(0.0, self.video.total_duration_seconds());
        if clamped >= self.video.total_duration_seconds() {
            self.seek_to_end();
        } else {
            let target = self.video.sample_index_for_seconds(clamped);
            self.seek_to_index(target);
        }
    }

    fn seek_to_end(&mut self) {
        self.current_index = self.video.sample_count().saturating_sub(1);
        self.slider_seconds = self.video.total_duration_seconds();
        self.at_end_of_stream = true;
        self.playing = false;
        self.last_advance_at = Instant::now();
        self.displayed_cache_key = None;
        self.apply_status_message("reached end of stream".to_string());
    }

    fn advance_playback(&mut self) {
        if self.at_end_of_stream || self.current_index + 1 >= self.video.sample_count() {
            self.seek_to_end();
            return;
        }
        self.current_index += 1;
        self.slider_seconds = self.video.sample_timestamp_seconds(self.current_index);
        self.apply_status_message(format!("play sample {}", self.current_index));
        self.displayed_cache_key = None;
    }

    fn reset_decoder_state(&mut self, reason: &str) {
        self.decode_generation = self.decode_generation.saturating_add(1);
        self.texture = None;
        self.displayed_cache_key = None;
        self.decode_cache.clear();
        self.decode_cache_lru.clear();
        self.decode_cache_resident_bytes = 0;
        self.apply_decode_status(format!("decoder reset: {reason}"));
    }

    fn restart_decode_worker(&mut self, reason: &str) {
        self.decode_generation = self.decode_generation.saturating_add(1);
        self.decode_scheduler.clear();
        self.decode_worker = spawn_decode_worker(self.video.clone());
        self.apply_decode_warning(format!("decode worker restarted: {reason}"));
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
            strict_backend: self.strict_backend,
        }
    }

    fn apply_cached_decode_status(&mut self, cached: &CachedDecodeOutcome) {
        match cached.level {
            DecodeStatusLevel::Success => self.apply_decode_status(cached.status.clone()),
            DecodeStatusLevel::Warning => self.apply_decode_warning(cached.status.clone()),
            DecodeStatusLevel::Error => self.apply_decode_error(cached.status.clone()),
        }
    }

    fn cached_decode_bytes(entry: &CachedDecodeOutcome) -> usize {
        entry
            .preview
            .as_ref()
            .map_or(0, DecodedPreview::resident_bytes)
    }

    fn touch_decode_cache_key(&mut self, key: PreviewCacheKey) {
        self.decode_cache_lru.retain(|existing| *existing != key);
        self.decode_cache_lru.push_back(key);
        while self.decode_cache_lru.len() > Self::DECODE_CACHE_CAPACITY
            || self.decode_cache_resident_bytes > Self::DECODE_CACHE_MAX_BYTES
        {
            if let Some(oldest) = self.decode_cache_lru.pop_front() {
                if let Some(removed) = self.decode_cache.remove(&oldest) {
                    self.decode_cache_resident_bytes = self
                        .decode_cache_resident_bytes
                        .saturating_sub(Self::cached_decode_bytes(&removed));
                }
                if self.displayed_cache_key == Some(oldest) {
                    self.displayed_cache_key = None;
                }
            }
        }
    }

    fn store_decode_cache_entry(&mut self, key: PreviewCacheKey, entry: CachedDecodeOutcome) {
        let entry_bytes = Self::cached_decode_bytes(&entry);
        if let Some(replaced) = self.decode_cache.insert(key, entry) {
            self.decode_cache_resident_bytes = self
                .decode_cache_resident_bytes
                .saturating_sub(Self::cached_decode_bytes(&replaced));
        }
        self.decode_cache_resident_bytes =
            self.decode_cache_resident_bytes.saturating_add(entry_bytes);
        self.touch_decode_cache_key(key);
    }

    fn poll_decode_events(&mut self, ctx: &egui::Context) {
        loop {
            let event = match self.decode_worker.try_recv() {
                Ok(event) => event,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.restart_decode_worker("previous worker disconnected unexpectedly");
                    break;
                }
            };
            if self
                .decode_scheduler
                .complete(event.request.request_id)
                .is_none()
            {
                warn!(
                    request_id = event.request.request_id,
                    sample_index = event.request.sample_index,
                    "discarding decode result that does not match the active request"
                );
                continue;
            }
            if event.request.generation != self.decode_generation {
                debug!(
                    request_id = event.request.request_id,
                    request_generation = event.request.generation,
                    current_generation = self.decode_generation,
                    "discarding stale decode result after decoder reset"
                );
                continue;
            }
            let key = PreviewCacheKey {
                sample_index: event.request.sample_index,
                backend: event.request.backend,
                require_hardware: event.request.require_hardware,
                strict_backend: event.request.strict_backend,
            };
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
                    let current_outcome = if let Some(preview) = &attempt.preview {
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
                            level: DecodeStatusLevel::Success,
                        }
                    } else {
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
                            level: DecodeStatusLevel::Warning,
                        }
                    };
                    self.store_decode_cache_entry(key, current_outcome);
                    if self.current_cache_key() == key
                        && let Some(cached) = self.decode_cache.get(&key).cloned()
                    {
                        if let Some(preview) = &cached.preview {
                            self.update_preview_texture(ctx, preview);
                        } else {
                            self.texture = None;
                        }
                        self.apply_cached_decode_status(&cached);
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
                            level: DecodeStatusLevel::Error,
                        },
                    );
                    if self.current_cache_key() == key {
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
            self.apply_cached_decode_status(&cached);
            self.touch_decode_cache_key(key);
            return;
        }
        if self.decode_scheduler.is_busy() {
            return;
        }
        self.try_start_decode(key);
    }

    fn try_start_decode(&mut self, key: PreviewCacheKey) {
        if self.decode_scheduler.is_busy() {
            return;
        }
        let request = DecodeWorkerRequest {
            request_id: self.next_decode_request_id,
            generation: self.decode_generation,
            sample_index: key.sample_index,
            backend: key.backend,
            require_hardware: key.require_hardware,
            strict_backend: self.strict_backend,
        };
        self.next_decode_request_id = self.next_decode_request_id.saturating_add(1);
        if !self.decode_scheduler.try_activate(request) {
            return;
        }
        match self.decode_worker.try_send_decode(request) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                let _ = self.decode_scheduler.complete(request.request_id);
                self.apply_decode_warning(
                    "decode worker command slot was busy; retrying latest position".to_string(),
                );
                return;
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                let _ = self.decode_scheduler.complete(request.request_id);
                self.restart_decode_worker("command channel disconnected");
                return;
            }
        }
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
            .min(self.video.sample_count().saturating_sub(1));
        if next_sample_index == self.current_index {
            return;
        }
        let key = PreviewCacheKey {
            sample_index: next_sample_index,
            backend: self.selected_backend,
            require_hardware: self.require_hardware,
            strict_backend: self.strict_backend,
        };
        if self.decode_cache.contains_key(&key) || self.decode_scheduler.is_busy() {
            return;
        }
        self.try_start_decode(key);
    }
}

impl eframe::App for ReaderApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        ctx.request_repaint_after(Duration::from_millis(16));

        let now = Instant::now();
        if self.playing {
            while self.playing {
                let frame_interval = Duration::from_secs_f64(
                    self.video
                        .sample_duration_seconds(self.current_index)
                        .max(1.0 / 120.0),
                );
                if now.saturating_duration_since(self.last_advance_at) < frame_interval {
                    break;
                }
                self.last_advance_at += frame_interval;
                self.advance_playback();
            }
        }

        let mut decoder_settings_changed = false;
        egui::Panel::top("reader-controls").show(root_ui, |ui| {
            ui.horizontal(|ui| {
                let play_label = if self.playing { "Pause" } else { "Play" };
                if ui.button(play_label).clicked() {
                    if self.playing {
                        self.playing = false;
                    } else {
                        if self.at_end_of_stream {
                            self.seek_to_index(0);
                        }
                        self.playing = true;
                    }
                    self.last_advance_at = Instant::now();
                }
                if ui.button("Prev").clicked() {
                    self.playing = false;
                    self.seek_to_index(self.current_index.saturating_sub(1));
                }
                if ui.button("Next").clicked() {
                    self.playing = false;
                    if self.current_index + 1 >= self.video.sample_count() {
                        self.seek_to_end();
                    } else {
                        self.seek_to_index(self.current_index + 1);
                    }
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
            ui.horizontal(|ui| {
                if self.decode_scheduler.is_busy() {
                    ui.spinner();
                }
                ui.label(format!("decode: {}", self.decode_status));
            });
        });

        if decoder_settings_changed {
            self.reset_decoder_state("decoder settings changed");
        }

        self.decode_current_preview(&ctx);
        self.maybe_prefetch_next_sample();

        egui::CentralPanel::default().show(root_ui, |ui| {
            if let Some(texture) = &self.texture {
                ui.add(
                    egui::Image::from_texture(egui::load::SizedTexture::from_handle(texture))
                        .shrink_to_fit(),
                );
            } else {
                ui.label("decoded preview unavailable for current sample/backend");
            }
            ui.separator();

            let sample = self
                .video
                .sample(self.current_index)
                .expect("loaded video timeline is non-empty");
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
                self.video.sample_count()
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

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
