//! Decoded-frame cache storage, eviction, and cached-window orchestration.

use super::*;

#[cfg_attr(
    not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )),
    allow(dead_code)
)]
pub struct CachedFrameDecoder<'a> {
    reader: &'a mut Fmp4Reader<SyncReading>,
    cache: DecodedFrameCache,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct DecodedFrameCacheKey {
    track_id: TrackId,
    sample_id: SampleId,
    backend: String,
    require_hardware: bool,
    output_mode: String,
    fps: i32,
}

#[derive(Debug, Clone)]
#[cfg_attr(
    not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )),
    allow(dead_code)
)]
struct DecodedFrameCacheEntry {
    frame: DecodedSampleFrame,
    resolved_backend: BackendKind,
    fallback_used: bool,
    fallback_reason: Option<String>,
    size_bytes: usize,
}

#[derive(Debug, Clone)]
#[cfg_attr(
    not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )),
    allow(dead_code)
)]
pub(super) struct DecodedFrameCache {
    config: DecodedFrameCacheConfig,
    entries: HashMap<DecodedFrameCacheKey, DecodedFrameCacheEntry>,
    lru: VecDeque<DecodedFrameCacheKey>,
    stats: DecodedFrameCacheStats,
}

#[cfg_attr(
    not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )),
    allow(dead_code)
)]
impl DecodedFrameCache {
    pub(super) fn new(config: DecodedFrameCacheConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
            lru: VecDeque::new(),
            stats: DecodedFrameCacheStats::default(),
        }
    }

    pub(super) fn stats(&self) -> DecodedFrameCacheStats {
        let mut stats = self.stats.clone();
        stats.resident_frames = self.entries.len();
        stats
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.lru.clear();
        self.stats.resident_frames = 0;
        self.stats.resident_bytes = 0;
    }

    pub(super) fn get(
        &mut self,
        key: &DecodedFrameCacheKey,
    ) -> Option<(DecodedSampleFrame, BackendKind, bool, Option<String>)> {
        let entry = self.entries.get(key).cloned();
        match entry {
            Some(entry) => {
                self.stats.hits = self.stats.hits.saturating_add(1);
                self.touch(key.clone());
                Some((
                    entry.frame,
                    entry.resolved_backend,
                    entry.fallback_used,
                    entry.fallback_reason,
                ))
            }
            None => {
                self.stats.misses = self.stats.misses.saturating_add(1);
                None
            }
        }
    }

    pub(super) fn insert(
        &mut self,
        key: DecodedFrameCacheKey,
        frame: DecodedSampleFrame,
        resolved_backend: BackendKind,
        fallback_used: bool,
        fallback_reason: Option<String>,
    ) {
        if self.config.max_frames == 0 || self.config.max_bytes == 0 {
            return;
        }

        let size_bytes = decoded_frame_size_bytes(&frame.frame);
        if let Some(existing) = self.entries.remove(&key) {
            self.stats.resident_bytes = self
                .stats
                .resident_bytes
                .saturating_sub(existing.size_bytes);
            self.lru.retain(|existing_key| existing_key != &key);
        }

        self.stats.inserts = self.stats.inserts.saturating_add(1);
        self.stats.resident_bytes = self.stats.resident_bytes.saturating_add(size_bytes);
        self.entries.insert(
            key.clone(),
            DecodedFrameCacheEntry {
                frame,
                resolved_backend,
                fallback_used,
                fallback_reason,
                size_bytes,
            },
        );
        self.touch(key);
        self.evict_to_budget();
        self.stats.resident_frames = self.entries.len();
    }

    fn touch(&mut self, key: DecodedFrameCacheKey) {
        self.lru.retain(|existing| existing != &key);
        self.lru.push_back(key);
    }

    fn evict_to_budget(&mut self) {
        while self.entries.len() > self.config.max_frames
            || self.stats.resident_bytes > self.config.max_bytes
        {
            let Some(oldest) = self.lru.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.stats.resident_bytes =
                    self.stats.resident_bytes.saturating_sub(entry.size_bytes);
                self.stats.evictions = self.stats.evictions.saturating_add(1);
            }
        }
    }
}

impl<'a> CachedFrameDecoder<'a> {
    pub fn new(reader: &'a mut Fmp4Reader<SyncReading>, config: DecodedFrameCacheConfig) -> Self {
        Self {
            reader,
            cache: DecodedFrameCache::new(config),
        }
    }

    pub fn cache_stats(&self) -> DecodedFrameCacheStats {
        self.cache.stats()
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    pub fn decode_sample_cached(
        &mut self,
        request: FrameDecodeWindowRequest,
    ) -> Result<CachedFrameDecodeResult> {
        let samples = self.reader.samples(request.track_id)?.to_vec();
        let fps = request.fps.unwrap_or_else(|| estimate_fps(&samples)).max(1);
        let key = cache_key_for_request(
            request.track_id,
            request.center_sample,
            request.backend,
            request.require_hardware,
            request.output_mode,
            fps,
        );
        let stats_before = self.cache.stats();
        if let Some((mut frame, resolved_backend, fallback_used, fallback_reason)) =
            self.cache.get(&key)
        {
            frame.presentation_index = Some(0);
            let range = single_sample_range(&samples, request.track_id, request.center_sample)?;
            let frames = vec![frame];
            let diagnostics = cached_decode_diagnostics(
                &request,
                resolved_backend,
                fps,
                fallback_used,
                fallback_reason,
                1,
                &frames,
            );
            return Ok(CachedFrameDecodeResult {
                range,
                diagnostics,
                resolved_backend,
                output_mode: request.output_mode,
                frames,
                cache_stats_delta: cache_stats_delta(&stats_before, &self.cache.stats()),
            });
        }

        let result = {
            let mut decoder = FrameDecoder::new(self.reader);
            decoder.decode_window(request.clone())?
        };
        let target_frame = result
            .frames
            .iter()
            .find(|frame| frame.sample_id == Some(request.center_sample))
            .cloned()
            .with_context(|| {
                format!(
                    "decode window did not return center sample {}",
                    request.center_sample
                )
            })?;
        self.cache_decoded_frames(&request, fps, &result);
        let range = single_sample_range(&samples, request.track_id, request.center_sample)?;
        let mut frames = vec![target_frame];
        frames[0].presentation_index = Some(0);
        Ok(CachedFrameDecodeResult {
            range,
            diagnostics: result.diagnostics,
            resolved_backend: result.resolved_backend,
            output_mode: result.output_mode,
            frames,
            cache_stats_delta: cache_stats_delta(&stats_before, &self.cache.stats()),
        })
    }

    #[cfg(not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )))]
    pub fn decode_sample_cached(
        &mut self,
        _request: FrameDecodeWindowRequest,
    ) -> Result<CachedFrameDecodeResult> {
        anyhow::bail!("no decoder backend feature is enabled")
    }

    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    pub fn decode_window_cached(
        &mut self,
        request: FrameDecodeWindowRequest,
    ) -> Result<CachedFrameDecodeResult> {
        let samples = self.reader.samples(request.track_id)?.to_vec();
        let wanted_sample_order = presentation_window_sample_ids(
            &samples,
            request.center_sample,
            request.before,
            request.after,
        )?;
        let fps = request.fps.unwrap_or_else(|| estimate_fps(&samples)).max(1);
        let stats_before = self.cache.stats();
        let mut cached_frames = Vec::with_capacity(wanted_sample_order.len());
        let mut cached_backend = None;
        let mut cached_fallback_used = false;
        let mut cached_fallback_reason = None;
        let mut all_cached = true;
        for (index, sample_id) in wanted_sample_order.iter().copied().enumerate() {
            let key = cache_key_for_request(
                request.track_id,
                sample_id,
                request.backend,
                request.require_hardware,
                request.output_mode,
                fps,
            );
            if let Some((mut frame, resolved_backend, fallback_used, fallback_reason)) =
                self.cache.get(&key)
            {
                frame.presentation_index = Some(index);
                cached_backend.get_or_insert(resolved_backend);
                cached_fallback_used |= fallback_used;
                if cached_fallback_reason.is_none() {
                    cached_fallback_reason = fallback_reason;
                }
                cached_frames.push(frame);
            } else {
                all_cached = false;
                break;
            }
        }

        if all_cached {
            let resolved_backend = cached_backend.context("cached window is empty")?;
            let range = window_range_from_order(&samples, request.track_id, &wanted_sample_order)?;
            let diagnostics = cached_decode_diagnostics(
                &request,
                resolved_backend,
                fps,
                cached_fallback_used,
                cached_fallback_reason,
                wanted_sample_order.len(),
                &cached_frames,
            );
            return Ok(CachedFrameDecodeResult {
                range,
                diagnostics,
                resolved_backend,
                output_mode: request.output_mode,
                frames: cached_frames,
                cache_stats_delta: cache_stats_delta(&stats_before, &self.cache.stats()),
            });
        }

        let result = {
            let mut decoder = FrameDecoder::new(self.reader);
            decoder.decode_window(request.clone())?
        };
        self.cache_decoded_frames(&request, fps, &result);
        Ok(CachedFrameDecodeResult {
            range: result.range,
            diagnostics: result.diagnostics,
            resolved_backend: result.resolved_backend,
            output_mode: result.output_mode,
            frames: result.frames,
            cache_stats_delta: cache_stats_delta(&stats_before, &self.cache.stats()),
        })
    }

    #[cfg(not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )))]
    pub fn decode_window_cached(
        &mut self,
        _request: FrameDecodeWindowRequest,
    ) -> Result<CachedFrameDecodeResult> {
        anyhow::bail!("no decoder backend feature is enabled")
    }

    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    fn cache_decoded_frames(
        &mut self,
        request: &FrameDecodeWindowRequest,
        fps: i32,
        result: &FrameDecodeRangeResult,
    ) {
        for frame in &result.frames {
            let Some(sample_id) = frame.sample_id else {
                continue;
            };
            let key = cache_key_for_request(
                request.track_id,
                sample_id,
                request.backend,
                request.require_hardware,
                request.output_mode,
                fps,
            );
            self.cache.insert(
                key,
                frame.clone(),
                result.resolved_backend,
                result.diagnostics.fallback_used,
                result.diagnostics.fallback_reason.clone(),
            );
        }
    }
}

#[cfg_attr(
    not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )),
    allow(dead_code)
)]
pub(super) fn cache_key_for_request(
    track_id: TrackId,
    sample_id: SampleId,
    backend: Backend,
    require_hardware: bool,
    output_mode: DecodeOutputMode,
    fps: i32,
) -> DecodedFrameCacheKey {
    DecodedFrameCacheKey {
        track_id,
        sample_id,
        backend: backend.to_string(),
        require_hardware,
        output_mode: output_mode.to_string(),
        fps,
    }
}

#[cfg_attr(
    not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )),
    allow(dead_code)
)]
fn decoded_frame_size_bytes(frame: &DecodedFrame) -> usize {
    match frame {
        DecodedFrame::Metadata { .. } => 0,
        DecodedFrame::Nv12 { data, .. } | DecodedFrame::Rgb24 { data, .. } => data.len(),
    }
}

#[cfg_attr(
    not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )),
    allow(dead_code)
)]
pub(super) fn cache_stats_delta(
    before: &DecodedFrameCacheStats,
    after: &DecodedFrameCacheStats,
) -> DecodedFrameCacheStats {
    DecodedFrameCacheStats {
        hits: after.hits.saturating_sub(before.hits),
        misses: after.misses.saturating_sub(before.misses),
        inserts: after.inserts.saturating_sub(before.inserts),
        evictions: after.evictions.saturating_sub(before.evictions),
        resident_frames: after.resident_frames,
        resident_bytes: after.resident_bytes,
    }
}

#[cfg_attr(
    not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )),
    allow(dead_code)
)]
fn cached_decode_diagnostics(
    request: &FrameDecodeWindowRequest,
    resolved_backend: BackendKind,
    fps: i32,
    fallback_used: bool,
    fallback_reason: Option<String>,
    requested_sample_count: usize,
    frames: &[DecodedSampleFrame],
) -> DecodeDiagnostics {
    DecodeDiagnostics {
        requested_backend: request.backend,
        resolved_backend,
        require_hardware: request.require_hardware,
        output_mode: request.output_mode,
        fps,
        fallback_used,
        fallback_reason,
        requested_sample_count,
        decoded_frame_count: 0,
        returned_sample_count: frames.len(),
        frames_with_sample_metadata_count: frames
            .iter()
            .filter(|frame| frame.sample_meta.is_some())
            .count(),
        dropped_or_unmatched_frame_count: 0,
        ambiguous_sample_association_count: 0,
        returned_frame_order: ReturnedFrameOrder::Presentation,
        missing_sample_ids: Vec::new(),
    }
}

#[cfg_attr(
    not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )),
    allow(dead_code)
)]
fn single_sample_range(
    samples: &[SampleMeta],
    track_id: TrackId,
    sample_id: SampleId,
) -> Result<SampleRange> {
    let start = samples
        .iter()
        .position(|sample| sample.sample_id == sample_id)
        .with_context(|| format!("sample {sample_id} does not belong to track {track_id}"))?;
    Ok(SampleRange {
        track_id,
        start_sample: sample_id,
        end_sample_exclusive: samples
            .get(start.saturating_add(1))
            .map_or(SampleId(u64::MAX), |sample| sample.sample_id),
    })
}

#[cfg_attr(
    not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    )),
    allow(dead_code)
)]
fn window_range_from_order(
    samples: &[SampleMeta],
    track_id: TrackId,
    sample_order: &[SampleId],
) -> Result<SampleRange> {
    let start_sample = *sample_order.first().context("cached window is empty")?;
    let end_sample_exclusive = sample_order
        .last()
        .and_then(|last| sample_after_in_pts_order(samples, *last))
        .unwrap_or(SampleId(u64::MAX));
    Ok(SampleRange {
        track_id,
        start_sample,
        end_sample_exclusive,
    })
}
