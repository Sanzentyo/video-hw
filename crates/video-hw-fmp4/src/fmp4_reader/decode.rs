use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result, anyhow};
use video_hw::{
    AnyDecodeSession, Backend, BackendError, BackendKind, BitstreamInput, DecodeOutputMode,
    DecodedFrame, DecoderConfig, Timestamp90k,
};

use super::{
    EncodedSample, EncodedSampleIter, Fmp4Reader, GopSegment, MediaTime, SampleId, SampleMeta,
    SampleRange, SyncReading, TrackId, config::Fmp4Track,
};

#[cfg(feature = "serde")]
mod serde_backend {
    use std::str::FromStr;

    use serde::{Deserialize, Deserializer, Serializer, de};
    use video_hw::Backend;

    pub fn serialize<S>(backend: &Backend, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&backend.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Backend, D::Error>
    where
        D: Deserializer<'de>,
    {
        Backend::from_str(&String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[cfg(feature = "serde")]
mod serde_backend_kind {
    use serde::{Deserialize, Deserializer, Serializer, de};
    use video_hw::BackendKind;

    pub fn serialize<S>(backend: &BackendKind, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&backend.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BackendKind, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            "videotoolbox" | "vt" => Ok(BackendKind::VideoToolbox),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            "nvidia" | "nv" => Ok(BackendKind::Nvidia),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            "intel" | "qsv" => Ok(BackendKind::Intel),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            "vulkan" => Ok(BackendKind::Vulkan),
            other => Err(de::Error::custom(format!(
                "backend kind {other:?} is not available with the enabled features"
            ))),
        }
    }
}

#[cfg(feature = "serde")]
mod serde_decode_output_mode {
    use serde::{Deserialize, Deserializer, Serializer, de};
    use video_hw::DecodeOutputMode;

    pub fn serialize<S>(mode: &DecodeOutputMode, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&mode.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<DecodeOutputMode, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "metadata" => Ok(DecodeOutputMode::Metadata),
            "nv12" => Ok(DecodeOutputMode::Nv12),
            "rgb24" => Ok(DecodeOutputMode::Rgb24),
            other => Err(de::Error::unknown_variant(
                other,
                &["metadata", "nv12", "rgb24"],
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FrameDecodeRequest {
    pub track_id: TrackId,
    pub target_sample: SampleId,
    pub backend: Backend,
    pub require_hardware: bool,
    pub output_mode: DecodeOutputMode,
    pub fps: Option<i32>,
}

impl FrameDecodeRequest {
    pub fn new(track_id: TrackId, target_sample: SampleId) -> Self {
        Self {
            track_id,
            target_sample,
            backend: Backend::Auto,
            require_hardware: false,
            output_mode: DecodeOutputMode::Rgb24,
            fps: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecodedSampleFrame {
    pub sample_id: Option<SampleId>,
    pub sample_meta: Option<SampleMeta>,
    pub presentation_index: Option<usize>,
    pub frame: DecodedFrame,
}

#[derive(Debug, Clone)]
pub struct FrameDecodeRangeRequest {
    pub range: SampleRange,
    pub backend: Backend,
    pub require_hardware: bool,
    pub output_mode: DecodeOutputMode,
    pub fps: Option<i32>,
}

impl FrameDecodeRangeRequest {
    pub fn new(range: SampleRange) -> Self {
        Self {
            range,
            backend: Backend::Auto,
            require_hardware: false,
            output_mode: DecodeOutputMode::Rgb24,
            fps: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FrameDecodeWindowRequest {
    pub track_id: TrackId,
    pub center_sample: SampleId,
    pub before: u32,
    pub after: u32,
    pub backend: Backend,
    pub require_hardware: bool,
    pub output_mode: DecodeOutputMode,
    pub fps: Option<i32>,
}

impl FrameDecodeWindowRequest {
    pub fn new(track_id: TrackId, center_sample: SampleId) -> Self {
        Self {
            track_id,
            center_sample,
            before: 0,
            after: 0,
            backend: Backend::Auto,
            require_hardware: false,
            output_mode: DecodeOutputMode::Rgb24,
            fps: None,
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ReturnedFrameOrder {
    Presentation,
    Decode,
    Unknown,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DecodeDiagnostics {
    #[cfg_attr(feature = "serde", serde(with = "serde_backend"))]
    pub requested_backend: Backend,
    #[cfg_attr(feature = "serde", serde(with = "serde_backend_kind"))]
    pub resolved_backend: BackendKind,
    pub require_hardware: bool,
    #[cfg_attr(feature = "serde", serde(with = "serde_decode_output_mode"))]
    pub output_mode: DecodeOutputMode,
    pub fps: i32,
    pub fallback_used: bool,
    pub fallback_reason: Option<String>,
    pub requested_sample_count: usize,
    pub decoded_frame_count: usize,
    pub returned_sample_count: usize,
    pub frames_with_sample_metadata_count: usize,
    pub dropped_or_unmatched_frame_count: usize,
    pub ambiguous_sample_association_count: usize,
    pub returned_frame_order: ReturnedFrameOrder,
    pub missing_sample_ids: Vec<SampleId>,
}

#[derive(Debug, Clone)]
pub struct FrameDecodeResult {
    pub target_sample: SampleId,
    pub diagnostics: DecodeDiagnostics,
    pub resolved_backend: BackendKind,
    pub output_mode: DecodeOutputMode,
    pub frames: Vec<DecodedSampleFrame>,
    pub target_frame_index: Option<usize>,
}

impl FrameDecodeResult {
    pub fn target_frame(&self) -> Option<&DecodedSampleFrame> {
        self.target_frame_index
            .and_then(|index| self.frames.get(index))
    }
}

#[derive(Debug, Clone)]
pub struct FrameDecodeRangeResult {
    pub range: SampleRange,
    pub diagnostics: DecodeDiagnostics,
    pub resolved_backend: BackendKind,
    pub output_mode: DecodeOutputMode,
    pub frames: Vec<DecodedSampleFrame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DecodedFrameCacheConfig {
    pub max_frames: usize,
    pub max_bytes: usize,
}

impl Default for DecodedFrameCacheConfig {
    fn default() -> Self {
        Self {
            max_frames: 64,
            max_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DecodedFrameCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    pub evictions: u64,
    pub resident_frames: usize,
    pub resident_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct CachedFrameDecodeResult {
    pub range: SampleRange,
    pub diagnostics: DecodeDiagnostics,
    pub resolved_backend: BackendKind,
    pub output_mode: DecodeOutputMode,
    pub frames: Vec<DecodedSampleFrame>,
    pub cache_stats_delta: DecodedFrameCacheStats,
}

#[derive(Debug, Clone)]
pub struct GopCursor {
    pub range: SampleRange,
    pub decode_start_sample: SampleId,
    pub next_sample: SampleId,
}

impl GopCursor {
    pub fn new(reader: &mut Fmp4Reader<SyncReading>, range: SampleRange) -> Result<Self> {
        let first_sample = {
            let samples = samples_in_range(reader, range)?;
            let Some(first) = samples.first() else {
                anyhow::bail!("sample range is empty");
            };
            first.sample_id
        };
        let decode_start_sample = reader
            .keyframe_before(first_sample)
            .with_context(|| format!("no keyframe before sample {}", first_sample))?;
        Ok(Self {
            range,
            decode_start_sample,
            next_sample: decode_start_sample,
        })
    }
}

pub struct FrameDecoder<'a> {
    reader: &'a mut Fmp4Reader<SyncReading>,
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
pub struct CachedFrameDecoder<'a> {
    reader: &'a mut Fmp4Reader<SyncReading>,
    cache: DecodedFrameCache,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DecodedFrameCacheKey {
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
struct DecodedFrameCache {
    config: DecodedFrameCacheConfig,
    entries: HashMap<DecodedFrameCacheKey, DecodedFrameCacheEntry>,
    lru: VecDeque<DecodedFrameCacheKey>,
    stats: DecodedFrameCacheStats,
}

pub struct DecodedFrameIter<'a> {
    range: SampleRange,
    diagnostics: DecodeDiagnostics,
    track: Fmp4Track,
    session: AnyDecodeSession,
    decode_samples: EncodedSampleIter<'a>,
    wanted_sample_ids: HashSet<SampleId>,
    sample_meta_by_id: HashMap<SampleId, SampleMeta>,
    presentation_index_by_id: HashMap<SampleId, usize>,
    pts_to_sample: HashMap<i64, SampleId>,
    buffered_frames: VecDeque<DecodedSampleFrame>,
    target_frame_index: Option<usize>,
    finished: bool,
}

impl DecodedFrameIter<'_> {
    pub fn diagnostics(&self) -> &DecodeDiagnostics {
        &self.diagnostics
    }

    pub fn range(&self) -> SampleRange {
        self.range
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
impl DecodedFrameCache {
    fn new(config: DecodedFrameCacheConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
            lru: VecDeque::new(),
            stats: DecodedFrameCacheStats::default(),
        }
    }

    fn stats(&self) -> DecodedFrameCacheStats {
        let mut stats = self.stats.clone();
        stats.resident_frames = self.entries.len();
        stats
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.lru.clear();
        self.stats.resident_frames = 0;
        self.stats.resident_bytes = 0;
    }

    fn get(
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

    fn insert(
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

impl<'a> FrameDecoder<'a> {
    pub fn new(reader: &'a mut Fmp4Reader<SyncReading>) -> Self {
        Self { reader }
    }

    pub fn decode_sample(&mut self, request: FrameDecodeRequest) -> Result<FrameDecodeResult> {
        let target_meta = self
            .reader
            .sample_meta(request.target_sample)
            .cloned()
            .with_context(|| format!("unknown target sample {}", request.target_sample))?;
        anyhow::ensure!(
            target_meta.track_id == request.track_id,
            "target sample {} belongs to track {}, not requested track {}",
            request.target_sample,
            target_meta.track_id,
            request.track_id
        );
        let track = self
            .reader
            .tracks()
            .iter()
            .find(|track| track.track_id == request.track_id)
            .cloned()
            .with_context(|| format!("unknown track {}", request.track_id))?;
        let codec = track
            .codec()
            .with_context(|| format!("track {} has no supported video codec", track.track_id))?;
        let fps = {
            let samples = self.reader.samples(request.track_id)?;
            request.fps.unwrap_or_else(|| estimate_fps(samples)).max(1)
        };
        let mut config = DecoderConfig::new(codec, fps, request.require_hardware);
        config.output_mode = request.output_mode;
        let (mut session, mut diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let resolved_backend = diagnostics.resolved_backend;
        let mut frames = VecDeque::new();
        let mut target_frame_index = None;
        let mut pts_to_sample = HashMap::<i64, SampleId>::new();
        let gop_samples = self
            .reader
            .iter_gop_for_sample(request.target_sample)?
            .collect::<Result<Vec<_>>>()?;
        let gop_metas = gop_samples
            .iter()
            .map(|sample| sample.meta.clone())
            .collect::<Vec<_>>();
        let gop_sample_order = ordered_sample_ids_by_pts(&gop_metas);
        let gop_meta_by_id = sample_meta_by_id(&gop_metas);
        let gop_presentation_index_by_id = presentation_index_by_id(&gop_sample_order);
        for sample in gop_samples {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.target_sample,
                target_frame_index: &mut target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &gop_meta_by_id,
                presentation_index_by_id: &gop_presentation_index_by_id,
            };
            submit_sample(&mut session, &track, sample, &mut collect)?;
        }

        for frame in session
            .flush()
            .map_err(anyhow::Error::new)
            .context("decoder flush failed")?
        {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.target_sample,
                target_frame_index: &mut target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &gop_meta_by_id,
                presentation_index_by_id: &gop_presentation_index_by_id,
            };
            push_decoded_frame(frame, &mut collect);
        }
        let ambiguous_sample_association_count =
            align_frames_to_presentation_order(&mut frames, &gop_sample_order);
        sort_frames_by_sample_order(&mut frames, &gop_sample_order);
        let decoded_frame_count = frames.len();
        attach_sample_metadata(&mut frames, &gop_meta_by_id, &gop_sample_order);
        target_frame_index = find_frame_index(&frames, request.target_sample);
        finalize_decode_diagnostics(
            &mut diagnostics,
            &gop_sample_order,
            decoded_frame_count,
            ambiguous_sample_association_count,
            ReturnedFrameOrder::Presentation,
            &frames,
        );

        Ok(FrameDecodeResult {
            target_sample: request.target_sample,
            diagnostics,
            resolved_backend,
            output_mode: request.output_mode,
            frames: frames.into_iter().collect(),
            target_frame_index,
        })
    }

    pub fn decode_range(
        &mut self,
        request: FrameDecodeRangeRequest,
    ) -> Result<FrameDecodeRangeResult> {
        let cursor = GopCursor::new(self.reader, request.range)?;
        let track = self
            .reader
            .tracks()
            .iter()
            .find(|track| track.track_id == request.range.track_id)
            .cloned()
            .with_context(|| format!("unknown track {}", request.range.track_id))?;
        let codec = track
            .codec()
            .with_context(|| format!("track {} has no supported video codec", track.track_id))?;
        let fps = {
            let samples = self.reader.samples(request.range.track_id)?;
            request.fps.unwrap_or_else(|| estimate_fps(samples)).max(1)
        };
        let mut config = DecoderConfig::new(codec, fps, request.require_hardware);
        config.output_mode = request.output_mode;
        let (mut session, mut diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let resolved_backend = diagnostics.resolved_backend;
        let requested_samples = samples_in_range(self.reader, request.range)?.to_vec();
        let wanted_sample_order = ordered_sample_ids_by_pts(&requested_samples);
        let wanted_sample_ids = wanted_sample_order.iter().copied().collect::<HashSet<_>>();
        let requested_meta_by_id = sample_meta_by_id(&requested_samples);
        let requested_presentation_index_by_id = presentation_index_by_id(&wanted_sample_order);
        let end_pts = requested_samples
            .last()
            .map(sample_end_pts)
            .context("sample range is empty")?;
        let mut frames = VecDeque::new();
        let mut ignored_target_frame_index = None;
        let mut pts_to_sample = HashMap::<i64, SampleId>::new();
        let decode_segment = GopSegment {
            track_id: request.range.track_id,
            keyframe_sample: cursor.decode_start_sample,
            end_sample_exclusive: request.range.end_sample_exclusive,
            start_pts: self
                .reader
                .sample_meta(cursor.decode_start_sample)
                .context("decode start sample disappeared")?
                .pts,
            end_pts,
        };

        let decode_samples = self
            .reader
            .iter_encoded(decode_segment)?
            .collect::<Result<Vec<_>>>()?;
        let decode_sample_order = ordered_sample_ids_by_pts(
            &decode_samples
                .iter()
                .map(|sample| sample.meta.clone())
                .collect::<Vec<_>>(),
        );
        for sample in decode_samples {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.range.start_sample,
                target_frame_index: &mut ignored_target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &requested_meta_by_id,
                presentation_index_by_id: &requested_presentation_index_by_id,
            };
            submit_sample(&mut session, &track, sample, &mut collect)?;
        }
        for frame in session
            .flush()
            .map_err(anyhow::Error::new)
            .context("decoder flush failed")?
        {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.range.start_sample,
                target_frame_index: &mut ignored_target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &requested_meta_by_id,
                presentation_index_by_id: &requested_presentation_index_by_id,
            };
            push_decoded_frame(frame, &mut collect);
        }
        let decoded_frame_count = frames.len();
        let ambiguous_sample_association_count =
            align_frames_to_presentation_order(&mut frames, &decode_sample_order);
        retain_requested_frames(&mut frames, &wanted_sample_ids);
        sort_frames_by_sample_order(&mut frames, &wanted_sample_order);
        attach_sample_metadata(&mut frames, &requested_meta_by_id, &wanted_sample_order);
        finalize_decode_diagnostics(
            &mut diagnostics,
            &wanted_sample_order,
            decoded_frame_count,
            ambiguous_sample_association_count,
            ReturnedFrameOrder::Presentation,
            &frames,
        );
        Ok(FrameDecodeRangeResult {
            range: request.range,
            diagnostics,
            resolved_backend,
            output_mode: request.output_mode,
            frames: frames.into_iter().collect(),
        })
    }

    pub fn decode_range_iter(
        &mut self,
        request: FrameDecodeRangeRequest,
    ) -> Result<DecodedFrameIter<'_>> {
        let cursor = GopCursor::new(self.reader, request.range)?;
        let track = self
            .reader
            .tracks()
            .iter()
            .find(|track| track.track_id == request.range.track_id)
            .cloned()
            .with_context(|| format!("unknown track {}", request.range.track_id))?;
        let codec = track
            .codec()
            .with_context(|| format!("track {} has no supported video codec", track.track_id))?;
        let fps = {
            let samples = self.reader.samples(request.range.track_id)?;
            request.fps.unwrap_or_else(|| estimate_fps(samples)).max(1)
        };
        let mut config = DecoderConfig::new(codec, fps, request.require_hardware);
        config.output_mode = request.output_mode;
        let (session, diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let (wanted_sample_ids, sample_meta_by_id, presentation_index_by_id, end_pts) = {
            let samples = samples_in_range(self.reader, request.range)?;
            let wanted_sample_ids = samples
                .iter()
                .map(|sample| sample.sample_id)
                .collect::<HashSet<_>>();
            let sample_order = ordered_sample_ids_by_pts(samples);
            let sample_meta_by_id = sample_meta_by_id(samples);
            let presentation_index_by_id = sample_order
                .iter()
                .enumerate()
                .map(|(index, sample_id)| (*sample_id, index))
                .collect::<HashMap<_, _>>();
            let end_pts = samples
                .last()
                .map(|sample| {
                    MediaTime::new(
                        sample.pts.ticks.saturating_add(u64::from(sample.duration)),
                        sample.pts.timescale,
                    )
                })
                .context("sample range is empty")?;
            (
                wanted_sample_ids,
                sample_meta_by_id,
                presentation_index_by_id,
                end_pts,
            )
        };
        let decode_segment = GopSegment {
            track_id: request.range.track_id,
            keyframe_sample: cursor.decode_start_sample,
            end_sample_exclusive: request.range.end_sample_exclusive,
            start_pts: self
                .reader
                .sample_meta(cursor.decode_start_sample)
                .context("decode start sample disappeared")?
                .pts,
            end_pts,
        };

        let decode_samples = self.reader.iter_encoded(decode_segment)?;
        Ok(DecodedFrameIter {
            range: request.range,
            diagnostics,
            track,
            session,
            decode_samples,
            wanted_sample_ids,
            sample_meta_by_id,
            presentation_index_by_id,
            pts_to_sample: HashMap::new(),
            buffered_frames: VecDeque::new(),
            target_frame_index: None,
            finished: false,
        })
    }

    pub fn decode_window(
        &mut self,
        request: FrameDecodeWindowRequest,
    ) -> Result<FrameDecodeRangeResult> {
        let samples = self.reader.samples(request.track_id)?.to_vec();
        let center_decode_index = samples
            .iter()
            .position(|sample| sample.sample_id == request.center_sample)
            .with_context(|| {
                format!(
                    "center sample {} does not belong to track {}",
                    request.center_sample, request.track_id
                )
            })?;
        let wanted_sample_order = presentation_window_sample_ids(
            &samples,
            request.center_sample,
            request.before,
            request.after,
        )?;
        let wanted_sample_ids = wanted_sample_order.iter().copied().collect::<HashSet<_>>();
        let sample_meta_by_id = sample_meta_by_id(&samples);
        let presentation_index_by_id = presentation_index_by_id(&wanted_sample_order);
        let (decode_first_index, decode_end_index) =
            decode_span_for_sample_ids(&samples, &wanted_sample_ids)?;
        let decode_anchor_index = decode_first_index.min(center_decode_index);
        let decode_start_sample = self
            .reader
            .keyframe_before(samples[decode_anchor_index].sample_id)
            .with_context(|| {
                format!(
                    "no keyframe before sample {}",
                    samples[decode_anchor_index].sample_id
                )
            })?;
        let decode_end_exclusive = samples
            .get(decode_end_index)
            .map_or(SampleId(u64::MAX), |sample| sample.sample_id);
        let decode_end_pts = samples
            .get(decode_end_index.saturating_sub(1))
            .map(sample_end_pts)
            .context("decode window is empty")?;
        let range = SampleRange {
            track_id: request.track_id,
            start_sample: *wanted_sample_order
                .first()
                .context("presentation window is empty")?,
            end_sample_exclusive: wanted_sample_order
                .last()
                .and_then(|last| sample_after_in_pts_order(&samples, *last))
                .unwrap_or(SampleId(u64::MAX)),
        };

        let track = self
            .reader
            .tracks()
            .iter()
            .find(|track| track.track_id == request.track_id)
            .cloned()
            .with_context(|| format!("unknown track {}", request.track_id))?;
        let codec = track
            .codec()
            .with_context(|| format!("track {} has no supported video codec", track.track_id))?;
        let fps = request.fps.unwrap_or_else(|| estimate_fps(&samples)).max(1);
        let mut config = DecoderConfig::new(codec, fps, request.require_hardware);
        config.output_mode = request.output_mode;
        let (mut session, mut diagnostics) = create_decode_session(request.backend, config)
            .with_context(|| format!("failed to create decoder session for {}", request.backend))?;
        let resolved_backend = diagnostics.resolved_backend;
        let decode_start_pts = self
            .reader
            .sample_meta(decode_start_sample)
            .context("decode start sample disappeared")?
            .pts;
        let decode_segment = GopSegment {
            track_id: request.track_id,
            keyframe_sample: decode_start_sample,
            end_sample_exclusive: decode_end_exclusive,
            start_pts: decode_start_pts,
            end_pts: decode_end_pts,
        };
        let decode_samples = self
            .reader
            .iter_encoded(decode_segment)?
            .collect::<Result<Vec<_>>>()?;
        let decode_sample_order = ordered_sample_ids_by_pts(
            &decode_samples
                .iter()
                .map(|sample| sample.meta.clone())
                .collect::<Vec<_>>(),
        );
        let mut frames = VecDeque::new();
        let mut target_frame_index = None;
        let mut pts_to_sample = HashMap::<i64, SampleId>::new();
        for sample in decode_samples {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.center_sample,
                target_frame_index: &mut target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &sample_meta_by_id,
                presentation_index_by_id: &presentation_index_by_id,
            };
            submit_sample(&mut session, &track, sample, &mut collect)?;
        }
        for frame in session
            .flush()
            .map_err(anyhow::Error::new)
            .context("decoder flush failed")?
        {
            let mut collect = DecodeCollectState {
                pts_to_sample: &mut pts_to_sample,
                frames: &mut frames,
                target_sample: request.center_sample,
                target_frame_index: &mut target_frame_index,
                filter_sample_ids: None,
                sample_meta_by_id: &sample_meta_by_id,
                presentation_index_by_id: &presentation_index_by_id,
            };
            push_decoded_frame(frame, &mut collect);
        }
        let decoded_frame_count = frames.len();
        let ambiguous_sample_association_count =
            align_frames_to_presentation_order(&mut frames, &decode_sample_order);
        retain_requested_frames(&mut frames, &wanted_sample_ids);
        sort_frames_by_sample_order(&mut frames, &wanted_sample_order);
        attach_sample_metadata(&mut frames, &sample_meta_by_id, &wanted_sample_order);
        finalize_decode_diagnostics(
            &mut diagnostics,
            &wanted_sample_order,
            decoded_frame_count,
            ambiguous_sample_association_count,
            ReturnedFrameOrder::Presentation,
            &frames,
        );
        Ok(FrameDecodeRangeResult {
            range,
            diagnostics,
            resolved_backend,
            output_mode: request.output_mode,
            frames: frames.into_iter().collect(),
        })
    }
}

impl Iterator for DecodedFrameIter<'_> {
    type Item = Result<DecodedSampleFrame>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(frame) = self.buffered_frames.pop_front() {
                return Some(Ok(frame));
            }
            if self.finished {
                return None;
            }
            match self.decode_samples.next() {
                Some(Ok(sample)) => {
                    let sample_id = sample.meta.sample_id;
                    let mut collect = DecodeCollectState {
                        pts_to_sample: &mut self.pts_to_sample,
                        frames: &mut self.buffered_frames,
                        target_sample: self.range.start_sample,
                        target_frame_index: &mut self.target_frame_index,
                        filter_sample_ids: Some(&self.wanted_sample_ids),
                        sample_meta_by_id: &self.sample_meta_by_id,
                        presentation_index_by_id: &self.presentation_index_by_id,
                    };
                    if let Err(err) =
                        submit_sample(&mut self.session, &self.track, sample, &mut collect)
                    {
                        return Some(Err(err.context(format!(
                            "streaming decode failed at sample {} on track {}",
                            sample_id, self.track.track_id
                        ))));
                    }
                }
                Some(Err(err)) => return Some(Err(err)),
                None => {
                    self.finished = true;
                    match self.session.flush().map_err(anyhow::Error::new) {
                        Ok(frames) => {
                            for frame in frames {
                                let mut collect = DecodeCollectState {
                                    pts_to_sample: &mut self.pts_to_sample,
                                    frames: &mut self.buffered_frames,
                                    target_sample: self.range.start_sample,
                                    target_frame_index: &mut self.target_frame_index,
                                    filter_sample_ids: Some(&self.wanted_sample_ids),
                                    sample_meta_by_id: &self.sample_meta_by_id,
                                    presentation_index_by_id: &self.presentation_index_by_id,
                                };
                                push_decoded_frame(frame, &mut collect);
                            }
                        }
                        Err(err) => return Some(Err(err.context("decoder flush failed"))),
                    }
                }
            }
        }
    }
}

fn create_decode_session(
    requested_backend: Backend,
    config: DecoderConfig,
) -> Result<(AnyDecodeSession, DecodeDiagnostics)> {
    let resolved_backend = requested_backend.resolve_decoder(&config).with_context(|| {
        format!(
            "failed to resolve decoder backend: requested={}, codec={}, fps={}, require_hardware={}",
            requested_backend, config.codec, config.fps, config.require_hardware
        )
    })?;
    match AnyDecodeSession::with_backend_kind(resolved_backend, config.clone()) {
        Ok(session) => Ok((
            session,
            DecodeDiagnostics {
                requested_backend,
                resolved_backend,
                require_hardware: config.require_hardware,
                output_mode: config.output_mode,
                fps: config.fps,
                fallback_used: false,
                fallback_reason: None,
                requested_sample_count: 0,
                decoded_frame_count: 0,
                returned_sample_count: 0,
                frames_with_sample_metadata_count: 0,
                dropped_or_unmatched_frame_count: 0,
                ambiguous_sample_association_count: 0,
                returned_frame_order: ReturnedFrameOrder::Unknown,
                missing_sample_ids: Vec::new(),
            },
        )),
        Err(primary_err) if requested_backend != Backend::Auto && !config.require_hardware => {
            let fallback_backend = Backend::Auto
                .resolve_decoder(&config)
                .context("failed to resolve fallback auto decoder backend")?;
            let fallback_reason =
                format!("requested backend {resolved_backend} failed: {primary_err}");
            let session = AnyDecodeSession::with_backend_kind(fallback_backend, config.clone())
                .with_context(|| {
                    format!(
                        "fallback backend {fallback_backend} also failed after {resolved_backend}"
                    )
                })?;
            Ok((
                session,
                DecodeDiagnostics {
                    requested_backend,
                    resolved_backend: fallback_backend,
                    require_hardware: config.require_hardware,
                    output_mode: config.output_mode,
                    fps: config.fps,
                    fallback_used: fallback_backend != resolved_backend,
                    fallback_reason: Some(fallback_reason),
                    requested_sample_count: 0,
                    decoded_frame_count: 0,
                    returned_sample_count: 0,
                    frames_with_sample_metadata_count: 0,
                    dropped_or_unmatched_frame_count: 0,
                    ambiguous_sample_association_count: 0,
                    returned_frame_order: ReturnedFrameOrder::Unknown,
                    missing_sample_ids: Vec::new(),
                },
            ))
        }
        Err(err) => Err(anyhow!(err).context(format!(
            "failed to create decoder session with {resolved_backend}"
        ))),
    }
}

fn submit_sample(
    session: &mut AnyDecodeSession,
    track: &Fmp4Track,
    sample: EncodedSample,
    collect: &mut DecodeCollectState<'_>,
) -> Result<()> {
    let sample_id = sample.meta.sample_id;
    let pts_90k = sample_timestamp_to_90k(&sample.meta);
    if let Some(pts) = pts_90k {
        collect.pts_to_sample.insert(pts.0, sample_id);
    }
    let annexb = sample
        .to_annexb()
        .with_context(|| format!("failed to convert sample {sample_id} to Annex-B"))?;
    loop {
        match session.submit(BitstreamInput::AnnexBChunk {
            chunk: annexb.clone(),
            pts_90k,
        }) {
            Ok(()) => break,
            Err(BackendError::TemporaryBackpressure(_)) => drain_ready_frames(session, collect)?,
            Err(err) => {
                return Err(anyhow!(err).context(format!(
                    "decoder submit failed at sample {sample_id} on track {}",
                    track.track_id
                )));
            }
        }
    }
    drain_ready_frames(session, collect)
        .with_context(|| format!("failed to reap decoded frames after sample {sample_id}"))
}

fn drain_ready_frames(
    session: &mut AnyDecodeSession,
    collect: &mut DecodeCollectState<'_>,
) -> Result<()> {
    loop {
        match session.try_reap() {
            Ok(Some(frame)) => {
                push_decoded_frame(frame, collect);
            }
            Ok(None) => return Ok(()),
            Err(err) => return Err(anyhow!(err)),
        }
    }
}

fn push_decoded_frame(frame: DecodedFrame, collect: &mut DecodeCollectState<'_>) {
    let sample_id =
        frame_pts_90k(&frame).and_then(|pts| collect.pts_to_sample.get(&pts.0).copied());
    if let Some(filter) = collect.filter_sample_ids
        && !sample_id.is_some_and(|sample_id| filter.contains(&sample_id))
    {
        return;
    }
    if sample_id == Some(collect.target_sample) && collect.target_frame_index.is_none() {
        *collect.target_frame_index = Some(collect.frames.len());
    }
    collect.frames.push_back(DecodedSampleFrame {
        sample_id,
        sample_meta: sample_id
            .and_then(|sample_id| collect.sample_meta_by_id.get(&sample_id).cloned()),
        presentation_index: sample_id
            .and_then(|sample_id| collect.presentation_index_by_id.get(&sample_id).copied()),
        frame,
    });
}

struct DecodeCollectState<'a> {
    pts_to_sample: &'a mut HashMap<i64, SampleId>,
    frames: &'a mut VecDeque<DecodedSampleFrame>,
    target_sample: SampleId,
    target_frame_index: &'a mut Option<usize>,
    filter_sample_ids: Option<&'a HashSet<SampleId>>,
    sample_meta_by_id: &'a HashMap<SampleId, SampleMeta>,
    presentation_index_by_id: &'a HashMap<SampleId, usize>,
}

fn ordered_sample_ids_by_pts(samples: &[SampleMeta]) -> Vec<SampleId> {
    let mut ordered = samples.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|sample| (sample.pts.ticks, sample.sample_id));
    ordered.into_iter().map(|sample| sample.sample_id).collect()
}

fn sample_meta_by_id(samples: &[SampleMeta]) -> HashMap<SampleId, SampleMeta> {
    samples
        .iter()
        .map(|sample| (sample.sample_id, sample.clone()))
        .collect()
}

fn presentation_index_by_id(sample_order: &[SampleId]) -> HashMap<SampleId, usize> {
    sample_order
        .iter()
        .enumerate()
        .map(|(index, sample_id)| (*sample_id, index))
        .collect()
}

fn presentation_window_sample_ids(
    samples: &[SampleMeta],
    center_sample: SampleId,
    before: u32,
    after: u32,
) -> Result<Vec<SampleId>> {
    let ordered = ordered_sample_ids_by_pts(samples);
    let center_index = ordered
        .iter()
        .position(|sample| *sample == center_sample)
        .with_context(|| {
            format!(
                "center sample {} is not present in PTS order",
                center_sample
            )
        })?;
    let before = usize::try_from(before).context("window before overflows usize")?;
    let after = usize::try_from(after).context("window after overflows usize")?;
    let start = center_index.saturating_sub(before);
    let end = center_index
        .saturating_add(after)
        .saturating_add(1)
        .min(ordered.len());
    Ok(ordered[start..end].to_vec())
}

fn decode_span_for_sample_ids(
    samples: &[SampleMeta],
    sample_ids: &HashSet<SampleId>,
) -> Result<(usize, usize)> {
    let mut positions = samples
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| sample_ids.contains(&sample.sample_id).then_some(index));
    let Some(first) = positions.next() else {
        anyhow::bail!("decode sample set is empty");
    };
    let (min_index, max_index) = positions.fold((first, first), |(min_index, max_index), index| {
        (min_index.min(index), max_index.max(index))
    });
    Ok((min_index, max_index.saturating_add(1)))
}

fn sample_after_in_pts_order(samples: &[SampleMeta], sample_id: SampleId) -> Option<SampleId> {
    let ordered = ordered_sample_ids_by_pts(samples);
    let index = ordered.iter().position(|sample| *sample == sample_id)?;
    ordered.get(index.saturating_add(1)).copied()
}

fn sample_end_pts(sample: &SampleMeta) -> MediaTime {
    MediaTime::new(
        sample.pts.ticks.saturating_add(u64::from(sample.duration)),
        sample.pts.timescale,
    )
}

fn sort_frames_by_sample_order(
    frames: &mut VecDeque<DecodedSampleFrame>,
    sample_order: &[SampleId],
) {
    let order = sample_order
        .iter()
        .enumerate()
        .map(|(index, sample)| (*sample, index))
        .collect::<HashMap<_, _>>();
    let mut sorted = frames.drain(..).collect::<Vec<_>>();
    sorted.sort_by_key(|frame| {
        frame
            .sample_id
            .and_then(|sample| order.get(&sample).copied())
            .unwrap_or(usize::MAX)
    });
    *frames = sorted.into();
}

fn align_frames_to_presentation_order(
    frames: &mut VecDeque<DecodedSampleFrame>,
    sample_order: &[SampleId],
) -> usize {
    let order = sample_order
        .iter()
        .enumerate()
        .map(|(index, sample_id)| (*sample_id, index))
        .collect::<HashMap<_, _>>();
    let mapped_indices = frames
        .iter()
        .map(|frame| {
            frame
                .sample_id
                .and_then(|sample_id| order.get(&sample_id).copied())
        })
        .collect::<Vec<_>>();
    let all_frames_mapped = mapped_indices.iter().all(Option::is_some);
    let mapped_sequence_is_presentation_order = mapped_indices
        .iter()
        .flatten()
        .try_fold(0_usize, |previous, index| {
            (*index >= previous).then_some(*index)
        })
        .is_some();

    if all_frames_mapped && mapped_sequence_is_presentation_order {
        return 0;
    }

    let mut ambiguous = 0;
    for (frame, sample_id) in frames.iter_mut().zip(sample_order.iter().copied()) {
        if frame.sample_id != Some(sample_id) {
            ambiguous += 1;
        }
        frame.sample_id = Some(sample_id);
    }
    ambiguous
}

fn retain_requested_frames(
    frames: &mut VecDeque<DecodedSampleFrame>,
    wanted_sample_ids: &HashSet<SampleId>,
) {
    frames.retain(|frame| {
        frame
            .sample_id
            .is_some_and(|sample_id| wanted_sample_ids.contains(&sample_id))
    });
}

fn attach_sample_metadata(
    frames: &mut VecDeque<DecodedSampleFrame>,
    sample_meta_by_id: &HashMap<SampleId, SampleMeta>,
    sample_order: &[SampleId],
) {
    let presentation_index_by_id = presentation_index_by_id(sample_order);
    for frame in frames {
        frame.sample_meta = frame
            .sample_id
            .and_then(|sample_id| sample_meta_by_id.get(&sample_id).cloned());
        frame.presentation_index = frame
            .sample_id
            .and_then(|sample_id| presentation_index_by_id.get(&sample_id).copied());
    }
}

fn find_frame_index(
    frames: &VecDeque<DecodedSampleFrame>,
    target_sample: SampleId,
) -> Option<usize> {
    frames
        .iter()
        .position(|frame| frame.sample_id == Some(target_sample))
}

fn finalize_decode_diagnostics(
    diagnostics: &mut DecodeDiagnostics,
    requested_sample_order: &[SampleId],
    decoded_frame_count: usize,
    ambiguous_sample_association_count: usize,
    returned_frame_order: ReturnedFrameOrder,
    frames: &VecDeque<DecodedSampleFrame>,
) {
    let returned = frames
        .iter()
        .filter_map(|frame| frame.sample_id)
        .collect::<HashSet<_>>();
    diagnostics.requested_sample_count = requested_sample_order.len();
    diagnostics.decoded_frame_count = decoded_frame_count;
    diagnostics.returned_sample_count = frames.len();
    diagnostics.frames_with_sample_metadata_count = frames
        .iter()
        .filter(|frame| frame.sample_meta.is_some())
        .count();
    diagnostics.dropped_or_unmatched_frame_count = decoded_frame_count.saturating_sub(frames.len());
    diagnostics.ambiguous_sample_association_count = ambiguous_sample_association_count;
    diagnostics.returned_frame_order = returned_frame_order;
    diagnostics.missing_sample_ids = requested_sample_order
        .iter()
        .copied()
        .filter(|sample| !returned.contains(sample))
        .collect();
}

fn samples_in_range(
    reader: &mut Fmp4Reader<SyncReading>,
    range: SampleRange,
) -> Result<&[SampleMeta]> {
    let samples = reader.samples(range.track_id)?;
    let start = samples
        .iter()
        .position(|sample| sample.sample_id == range.start_sample)
        .with_context(|| {
            format!(
                "range start sample {} does not belong to track {}",
                range.start_sample, range.track_id
            )
        })?;
    let end = samples
        .iter()
        .position(|sample| sample.sample_id == range.end_sample_exclusive)
        .unwrap_or(samples.len());
    anyhow::ensure!(start <= end, "invalid sample range");
    Ok(&samples[start..end])
}

fn frame_pts_90k(frame: &DecodedFrame) -> Option<Timestamp90k> {
    match frame {
        DecodedFrame::Metadata { pts_90k, .. }
        | DecodedFrame::Nv12 { pts_90k, .. }
        | DecodedFrame::Rgb24 { pts_90k, .. } => *pts_90k,
    }
}

fn sample_timestamp_to_90k(sample: &SampleMeta) -> Option<Timestamp90k> {
    let scaled_90k = u128::from(sample.pts.ticks).saturating_mul(90_000)
        / u128::from(sample.pts.timescale.get());
    i64::try_from(scaled_90k).ok().map(Timestamp90k)
}

fn estimate_fps(samples: &[SampleMeta]) -> i32 {
    samples
        .iter()
        .find_map(|sample| {
            if sample.duration == 0 {
                return None;
            }
            let timescale = u64::from(sample.dts.timescale.get());
            let duration = u64::from(sample.duration);
            let fps = (timescale + (duration / 2)) / duration;
            i32::try_from(fps.max(1)).ok()
        })
        .unwrap_or(30)
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
fn cache_key_for_request(
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
fn cache_stats_delta(
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        num::NonZeroU32,
        process::{Command, Stdio},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use crate::fmp4_reader::{Fmp4ReaderConfig, MediaTime, TrackKind};
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
    use video_hw::{Codec, DecoderConfig};
    use video_hw::{DecodedFrame, Dimensions};

    #[test]
    fn frame_decode_request_defaults_to_auto_rgb() {
        let request = FrameDecodeRequest::new(TrackId(1), SampleId(2));
        assert_eq!(request.track_id, TrackId(1));
        assert_eq!(request.target_sample, SampleId(2));
        assert_eq!(request.backend, Backend::Auto);
        assert_eq!(request.output_mode, DecodeOutputMode::Rgb24);
        assert!(!request.require_hardware);
    }

    #[test]
    fn frame_decode_range_request_defaults_to_auto_rgb() {
        let range = SampleRange {
            track_id: TrackId(1),
            start_sample: SampleId(2),
            end_sample_exclusive: SampleId(4),
        };
        let request = FrameDecodeRangeRequest::new(range);
        assert_eq!(request.range, range);
        assert_eq!(request.backend, Backend::Auto);
        assert_eq!(request.output_mode, DecodeOutputMode::Rgb24);
        assert!(!request.require_hardware);
    }

    #[test]
    fn frame_decode_window_request_defaults_to_auto_rgb() {
        let request = FrameDecodeWindowRequest::new(TrackId(1), SampleId(2));
        assert_eq!(request.track_id, TrackId(1));
        assert_eq!(request.center_sample, SampleId(2));
        assert_eq!(request.before, 0);
        assert_eq!(request.after, 0);
        assert_eq!(request.backend, Backend::Auto);
        assert_eq!(request.output_mode, DecodeOutputMode::Rgb24);
        assert!(!request.require_hardware);
    }

    #[test]
    fn decoded_frame_cache_config_has_bounded_default() {
        let config = DecodedFrameCacheConfig::default();
        assert_eq!(config.max_frames, 64);
        assert_eq!(config.max_bytes, 256 * 1024 * 1024);
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
    #[test]
    fn decoded_frame_cache_tracks_hits_lru_and_frame_evictions() {
        let backend = test_resolved_backend();
        let mut cache = DecodedFrameCache::new(DecodedFrameCacheConfig {
            max_frames: 2,
            max_bytes: usize::MAX,
        });
        let key1 = test_cache_key(SampleId(1));
        let key2 = test_cache_key(SampleId(2));
        let key3 = test_cache_key(SampleId(3));
        cache.insert(
            key1.clone(),
            test_decoded_frame(SampleId(1), 4),
            backend,
            false,
            None,
        );
        cache.insert(
            key2.clone(),
            test_decoded_frame(SampleId(2), 4),
            backend,
            false,
            None,
        );
        assert!(cache.get(&key1).is_some());
        cache.insert(
            key3.clone(),
            test_decoded_frame(SampleId(3), 4),
            backend,
            false,
            None,
        );

        assert!(cache.get(&key1).is_some(), "recent hit should protect key1");
        assert!(cache.get(&key2).is_none(), "oldest key should be evicted");
        assert!(cache.get(&key3).is_some());
        let stats = cache.stats();
        assert_eq!(stats.inserts, 3);
        assert_eq!(stats.evictions, 1);
        assert_eq!(stats.resident_frames, 2);
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
    #[test]
    fn decoded_frame_cache_enforces_byte_budget_and_clear() {
        let backend = test_resolved_backend();
        let mut cache = DecodedFrameCache::new(DecodedFrameCacheConfig {
            max_frames: 10,
            max_bytes: 12,
        });
        cache.insert(
            test_cache_key(SampleId(1)),
            test_decoded_frame(SampleId(1), 12),
            backend,
            false,
            None,
        );
        cache.insert(
            test_cache_key(SampleId(2)),
            test_decoded_frame(SampleId(2), 12),
            backend,
            false,
            None,
        );
        let stats = cache.stats();
        assert_eq!(stats.evictions, 1);
        assert_eq!(stats.resident_frames, 1);
        assert_eq!(stats.resident_bytes, 12);

        cache.clear();
        let stats = cache.stats();
        assert_eq!(stats.resident_frames, 0);
        assert_eq!(stats.resident_bytes, 0);
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
    #[test]
    fn decoded_frame_cache_key_separates_backend_mode_and_fps() {
        let auto_rgb = cache_key_for_request(
            TrackId(1),
            SampleId(1),
            Backend::Auto,
            false,
            DecodeOutputMode::Rgb24,
            30,
        );
        let auto_nv12 = cache_key_for_request(
            TrackId(1),
            SampleId(1),
            Backend::Auto,
            false,
            DecodeOutputMode::Nv12,
            30,
        );
        let auto_rgb_60 = cache_key_for_request(
            TrackId(1),
            SampleId(1),
            Backend::Auto,
            false,
            DecodeOutputMode::Rgb24,
            60,
        );
        assert_ne!(auto_rgb, auto_nv12);
        assert_ne!(auto_rgb, auto_rgb_60);

        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        assert_ne!(
            auto_rgb,
            cache_key_for_request(
                TrackId(1),
                SampleId(1),
                Backend::Nvidia,
                false,
                DecodeOutputMode::Rgb24,
                30,
            )
        );
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        assert_ne!(
            auto_rgb,
            cache_key_for_request(
                TrackId(1),
                SampleId(1),
                Backend::VideoToolbox,
                false,
                DecodeOutputMode::Rgb24,
                30,
            )
        );
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
    fn test_resolved_backend() -> BackendKind {
        Backend::Auto
            .resolve_decoder(&DecoderConfig::new(Codec::H264, 30, false))
            .expect("a decoder backend should be available with enabled test features")
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
    fn test_cache_key(sample_id: SampleId) -> DecodedFrameCacheKey {
        cache_key_for_request(
            TrackId(1),
            sample_id,
            Backend::Auto,
            false,
            DecodeOutputMode::Rgb24,
            30,
        )
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
    fn test_decoded_frame(sample_id: SampleId, byte_len: usize) -> DecodedSampleFrame {
        DecodedSampleFrame {
            sample_id: Some(sample_id),
            sample_meta: None,
            presentation_index: None,
            frame: DecodedFrame::Rgb24 {
                dims: Dimensions {
                    width: NonZeroU32::new(2).expect("non-zero width"),
                    height: NonZeroU32::new(2).expect("non-zero height"),
                },
                pts_90k: None,
                data: vec![sample_id.0 as u8; byte_len],
            },
        }
    }

    #[test]
    fn gop_cursor_starts_from_previous_keyframe() {
        let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.mp4");
        let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path))
            .into_sync_session()
            .expect("sample should open");
        let video_track = reader
            .tracks()
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .expect("video track should exist")
            .track_id;
        let samples = reader.samples(video_track).expect("video samples");
        let start = samples
            .iter()
            .find(|sample| !sample.keyframe)
            .expect("sample should contain inter frames")
            .sample_id;
        let range = SampleRange {
            track_id: video_track,
            start_sample: start,
            end_sample_exclusive: SampleId(u64::MAX),
        };
        let cursor = GopCursor::new(&mut reader, range).expect("cursor should resolve");

        assert_eq!(cursor.range, range);
        assert_eq!(
            cursor.decode_start_sample,
            reader
                .keyframe_before(start)
                .expect("previous keyframe should exist")
        );
        assert_eq!(cursor.next_sample, cursor.decode_start_sample);
    }

    #[test]
    fn sample_timestamp_uses_sample_timescale() {
        let meta = SampleMeta {
            sample_id: SampleId(0),
            track_id: TrackId(1),
            offset: 0,
            size: 0,
            dts: MediaTime::new(0, NonZeroU32::new(30_000).expect("timescale")),
            pts: MediaTime::new(15_000, NonZeroU32::new(30_000).expect("timescale")),
            duration: 1_001,
            composition_time_offset: None,
            keyframe: true,
        };
        assert_eq!(sample_timestamp_to_90k(&meta), Some(Timestamp90k(45_000)));
    }

    #[test]
    fn presentation_window_uses_pts_order_not_decode_order() {
        let timescale = NonZeroU32::new(30_000).expect("timescale");
        let samples = [
            (0, 0, 2_002),
            (1, 1_001, 6_006),
            (2, 2_002, 4_004),
            (3, 3_003, 3_003),
            (4, 4_004, 5_005),
        ]
        .into_iter()
        .map(|(sample_id, dts, pts)| SampleMeta {
            sample_id: SampleId(sample_id),
            track_id: TrackId(1),
            offset: 0,
            size: 0,
            dts: MediaTime::new(dts, timescale),
            pts: MediaTime::new(pts, timescale),
            duration: 1_001,
            composition_time_offset: None,
            keyframe: sample_id == 0,
        })
        .collect::<Vec<_>>();

        assert_eq!(
            presentation_window_sample_ids(&samples, SampleId(2), 2, 1)
                .expect("window should resolve"),
            vec![SampleId(0), SampleId(3), SampleId(2), SampleId(4)]
        );
        let wanted = [SampleId(0), SampleId(3), SampleId(2), SampleId(4)]
            .into_iter()
            .collect::<HashSet<_>>();
        assert_eq!(
            decode_span_for_sample_ids(&samples, &wanted).expect("span should resolve"),
            (0, 5)
        );
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
    #[test]
    fn decode_window_returns_presentation_order_for_reordered_mp4() {
        let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("foreman_cif.mp4");
        let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path))
            .into_sync_session()
            .expect("sample should open");
        let video_track = reader
            .tracks()
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .expect("video track should exist")
            .track_id;
        let samples = reader.samples(video_track).expect("video samples");
        assert_eq!(samples[0].sample_id, SampleId(0));
        assert_eq!(samples[0].pts.ticks, 2_002);
        assert_eq!(samples[1].sample_id, SampleId(1));
        assert_eq!(samples[1].pts.ticks, 6_006);
        assert_eq!(samples[2].sample_id, SampleId(2));
        assert_eq!(samples[2].pts.ticks, 4_004);
        assert_eq!(samples[3].sample_id, SampleId(3));
        assert_eq!(samples[3].pts.ticks, 3_003);
        assert_eq!(samples[4].sample_id, SampleId(4));
        assert_eq!(samples[4].pts.ticks, 5_005);

        let mut request = FrameDecodeWindowRequest::new(video_track, SampleId(2));
        request.before = 2;
        request.after = 1;
        request.output_mode = DecodeOutputMode::Metadata;
        let mut decoder = FrameDecoder::new(&mut reader);
        let result = match decoder.decode_window(request) {
            Ok(result) => result,
            Err(err) => {
                let error = format!("{err:#}");
                if error.contains("failed to create decoder session")
                    || error.contains("auto backend selection failed")
                    || error.contains("unsupported config")
                {
                    eprintln!("skipping decode-window reorder test: {error}");
                    return;
                }
                panic!("decode_window failed unexpectedly: {error}");
            }
        };
        let returned = result
            .frames
            .iter()
            .map(|frame| frame.sample_id.expect("decoded frame should map to sample"))
            .collect::<Vec<_>>();
        assert_eq!(
            returned,
            vec![SampleId(0), SampleId(3), SampleId(2), SampleId(4)]
        );
        for (index, frame) in result.frames.iter().enumerate() {
            assert_eq!(frame.presentation_index, Some(index));
            assert_eq!(
                frame.sample_meta.as_ref().map(|sample| sample.sample_id),
                frame.sample_id
            );
        }
        assert_eq!(result.diagnostics.requested_sample_count, 4);
        assert_eq!(result.diagnostics.returned_sample_count, 4);
        assert_eq!(result.diagnostics.frames_with_sample_metadata_count, 4);
        assert!(result.diagnostics.ambiguous_sample_association_count <= 4);
        assert!(matches!(
            result.diagnostics.returned_frame_order,
            ReturnedFrameOrder::Presentation
        ));
        assert!(result.diagnostics.missing_sample_ids.is_empty());
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
    #[test]
    fn decode_window_matches_synthetic_markers_for_bframe_and_no_bframe_inputs() {
        if Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping synthetic marker alignment test: ffmpeg is not available");
            return;
        }

        const WIDTH: u32 = 128;
        const HEIGHT: u32 = 72;
        const FRAMES: usize = 18;
        const WINDOW_START: usize = 4;
        const WINDOW_FRAMES: usize = 10;
        let temp_dir = std::env::temp_dir().join(format!(
            "video-hw-fmp4-marker-alignment-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let source_rgb = make_marker_rgb(WIDTH as usize, HEIGHT as usize, FRAMES);
        let source_raw = temp_dir.join("source.rgb");
        let bframe_mp4 = temp_dir.join("bframes.mp4");
        let no_bframe_mp4 = temp_dir.join("no-bframes.mp4");
        fs::write(&source_raw, &source_rgb).expect("source raw should be written");

        encode_marker_mp4(&source_raw, &bframe_mp4, WIDTH, HEIGHT, 2);
        encode_marker_mp4(&source_raw, &no_bframe_mp4, WIDTH, HEIGHT, 0);

        assert_decode_window_matches_markers(
            &bframe_mp4,
            WIDTH,
            HEIGHT,
            FRAMES,
            WINDOW_START,
            WINDOW_FRAMES,
            true,
        );
        assert_decode_window_matches_markers(
            &no_bframe_mp4,
            WIDTH,
            HEIGHT,
            FRAMES,
            WINDOW_START,
            WINDOW_FRAMES,
            false,
        );

        let _ = fs::remove_dir_all(temp_dir);
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
    #[test]
    fn decode_window_roundtrip_preserves_presentation_frame_alignment() {
        if Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping decode-window roundtrip test: ffmpeg is not available");
            return;
        }

        let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("foreman_cif.mp4");
        let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path.clone()))
            .into_sync_session()
            .expect("sample should open");
        let video_track = reader
            .tracks()
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .expect("video track should exist")
            .track_id;
        let mut request = FrameDecodeWindowRequest::new(video_track, SampleId(2));
        request.before = 2;
        request.after = 1;
        request.output_mode = DecodeOutputMode::Rgb24;
        let mut decoder = FrameDecoder::new(&mut reader);
        let result = match decoder.decode_window(request) {
            Ok(result) => result,
            Err(err) => {
                let error = format!("{err:#}");
                if error.contains("failed to create decoder session")
                    || error.contains("auto backend selection failed")
                    || error.contains("unsupported config")
                {
                    eprintln!("skipping decode-window roundtrip test: {error}");
                    return;
                }
                panic!("decode_window failed unexpectedly: {error}");
            }
        };
        assert_eq!(result.frames.len(), 4);

        let (width, height, decoded_rgb) = rgb24_window_payload(&result.frames);
        let temp_dir = std::env::temp_dir().join(format!(
            "video-hw-fmp4-frame-order-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let decoded_raw = temp_dir.join("decoded.rgb");
        let source_raw = temp_dir.join("source.rgb");
        let roundtrip_mp4 = temp_dir.join("roundtrip.mp4");
        let roundtrip_raw = temp_dir.join("roundtrip.rgb");
        fs::write(&decoded_raw, &decoded_rgb).expect("decoded raw should be written");

        run_ffmpeg(&[
            "-y",
            "-i",
            input_path.to_str().expect("utf-8 input path"),
            "-frames:v",
            "4",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            source_raw.to_str().expect("utf-8 source path"),
        ]);
        run_ffmpeg(&[
            "-y",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgb24",
            "-video_size",
            &format!("{width}x{height}"),
            "-framerate",
            "30",
            "-i",
            decoded_raw.to_str().expect("utf-8 decoded path"),
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            roundtrip_mp4.to_str().expect("utf-8 roundtrip path"),
        ]);
        run_ffmpeg(&[
            "-y",
            "-i",
            roundtrip_mp4.to_str().expect("utf-8 roundtrip path"),
            "-frames:v",
            "4",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            roundtrip_raw.to_str().expect("utf-8 roundtrip raw path"),
        ]);

        let source = fs::read(&source_raw).expect("source raw should exist");
        let roundtrip = fs::read(&roundtrip_raw).expect("roundtrip raw should exist");
        assert_diagonal_best_match(&source, &roundtrip, width as usize * height as usize * 3, 4);

        let _ = fs::remove_dir_all(temp_dir);
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
    #[test]
    fn cached_frame_decoder_reuses_decoded_target_frame() {
        let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("foreman_cif.mp4");
        let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path))
            .into_sync_session()
            .expect("sample should open");
        let video_track = reader
            .tracks()
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .expect("video track should exist")
            .track_id;
        let target_sample = reader
            .samples(video_track)
            .expect("video samples")
            .get(2)
            .expect("sample 2 should exist")
            .sample_id;
        let mut request = FrameDecodeWindowRequest::new(video_track, target_sample);
        request.after = 2;
        request.output_mode = DecodeOutputMode::Rgb24;
        let mut cached = CachedFrameDecoder::new(
            &mut reader,
            DecodedFrameCacheConfig {
                max_frames: 8,
                max_bytes: 32 * 1024 * 1024,
            },
        );
        let first = match cached.decode_sample_cached(request.clone()) {
            Ok(result) => result,
            Err(err) => {
                let error = format!("{err:#}");
                if error.contains("failed to create decoder session")
                    || error.contains("auto backend selection failed")
                    || error.contains("unsupported config")
                {
                    eprintln!("skipping cached decode test: {error}");
                    return;
                }
                panic!("cached decode failed unexpectedly: {error}");
            }
        };
        let second = cached
            .decode_sample_cached(request)
            .expect("second decode should be served from cache");

        assert_eq!(first.frames.len(), 1);
        assert_eq!(second.frames.len(), 1);
        assert_eq!(second.cache_stats_delta.hits, 1);
        assert_eq!(second.cache_stats_delta.misses, 0);
        assert_eq!(second.diagnostics.decoded_frame_count, 0);
        assert_eq!(second.frames[0].sample_id, Some(target_sample));
        assert_eq!(
            rgb24_bytes(&first.frames[0]),
            rgb24_bytes(&second.frames[0]),
            "cached target frame should match the original decoded target"
        );
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
    fn rgb24_window_payload(frames: &[DecodedSampleFrame]) -> (u32, u32, Vec<u8>) {
        let mut dims = None;
        let mut raw = Vec::new();
        for frame in frames {
            let DecodedFrame::Rgb24 {
                dims: frame_dims,
                data,
                ..
            } = &frame.frame
            else {
                panic!("decode window should return RGB24 frames");
            };
            let frame_dims = *frame_dims;
            if let Some(expected) = dims {
                assert_eq!(frame_dims, expected);
            } else {
                dims = Some(frame_dims);
            }
            raw.extend_from_slice(data);
        }
        let dims = dims.expect("at least one frame");
        (dims.width.get(), dims.height.get(), raw)
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
    fn rgb24_bytes(frame: &DecodedSampleFrame) -> &[u8] {
        let DecodedFrame::Rgb24 { data, .. } = &frame.frame else {
            panic!("expected RGB24 frame");
        };
        data
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
    fn encode_marker_mp4(
        source_raw: &std::path::Path,
        output_mp4: &std::path::Path,
        width: u32,
        height: u32,
        bframes: u32,
    ) {
        run_ffmpeg(&[
            "-y",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgb24",
            "-video_size",
            &format!("{width}x{height}"),
            "-framerate",
            "30",
            "-i",
            source_raw.to_str().expect("utf-8 source path"),
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "18",
            "-g",
            "12",
            "-bf",
            &bframes.to_string(),
            "-pix_fmt",
            "yuv420p",
            output_mp4.to_str().expect("utf-8 output path"),
        ]);
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
    fn assert_decode_window_matches_markers(
        input_path: &std::path::Path,
        width: u32,
        height: u32,
        total_frames: usize,
        window_start: usize,
        window_frames: usize,
        expect_reordered_samples: bool,
    ) {
        let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path.to_path_buf()))
            .into_sync_session()
            .expect("synthetic sample should open");
        let video_track = reader
            .tracks()
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .expect("video track should exist")
            .track_id;
        let samples = reader.samples(video_track).expect("video samples").to_vec();
        let sample_id_order = samples
            .iter()
            .map(|sample| sample.sample_id)
            .collect::<Vec<_>>();
        let pts_order = ordered_sample_ids_by_pts(&samples);
        assert_eq!(samples.len(), total_frames);
        if expect_reordered_samples {
            assert_ne!(
                sample_id_order, pts_order,
                "synthetic B-frame input should have reordered PTS/sample order"
            );
        } else {
            assert_eq!(
                sample_id_order, pts_order,
                "no-B-frame control should keep sample order in presentation order"
            );
        }

        let mut request = FrameDecodeWindowRequest::new(video_track, pts_order[window_start]);
        request.after = u32::try_from(window_frames - 1).expect("frame count fits u32");
        request.output_mode = DecodeOutputMode::Rgb24;
        let mut decoder = FrameDecoder::new(&mut reader);
        let result = match decoder.decode_window(request) {
            Ok(result) => result,
            Err(err) => {
                let error = format!("{err:#}");
                if error.contains("failed to create decoder session")
                    || error.contains("auto backend selection failed")
                    || error.contains("unsupported config")
                {
                    eprintln!("skipping synthetic marker alignment test: {error}");
                    return;
                }
                panic!("decode_window failed unexpectedly: {error}");
            }
        };
        assert_eq!(result.frames.len(), window_frames);
        for (index, frame) in result.frames.iter().enumerate() {
            assert_eq!(frame.presentation_index, Some(index));
            assert_eq!(
                frame.sample_meta.as_ref().map(|sample| sample.sample_id),
                frame.sample_id
            );
            assert_eq!(
                frame.sample_meta.as_ref().map(|sample| sample.pts),
                samples
                    .iter()
                    .find(|sample| sample.sample_id == pts_order[window_start + index])
                    .map(|sample| sample.pts)
            );
        }

        let (decoded_width, decoded_height, decoded_rgb) = rgb24_window_payload(&result.frames);
        assert_eq!((decoded_width, decoded_height), (width, height));
        let reference_raw = input_path.with_extension("reference.rgb");
        run_ffmpeg(&[
            "-y",
            "-i",
            input_path.to_str().expect("utf-8 input path"),
            "-frames:v",
            &total_frames.to_string(),
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            reference_raw.to_str().expect("utf-8 reference path"),
        ]);
        let reference_rgb = fs::read(&reference_raw).expect("reference raw should exist");
        let frame_len = width as usize * height as usize * 3;
        let reference_window =
            &reference_rgb[window_start * frame_len..(window_start + window_frames) * frame_len];
        assert_diagonal_best_match(reference_window, &decoded_rgb, frame_len, window_frames);
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
    fn make_marker_rgb(width: usize, height: usize, frames: usize) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(width * height * 3 * frames);
        for frame in 0..frames {
            let stripe_start = frame * width / frames;
            let stripe_end = (frame + 1) * width / frames;
            let base = [
                (frame * 19 + 20) as u8,
                235_u8.saturating_sub((frame * 17) as u8),
                (frame * 11 + 40) as u8,
            ];
            for y in 0..height {
                for x in 0..width {
                    let in_stripe = x >= stripe_start && x < stripe_end;
                    let bit_cell = (x / 16).saturating_add((y / 12) * 8);
                    let bit = ((frame >> (bit_cell % 4)) & 1) != 0;
                    let checker = ((x / 8) + (y / 8) + frame) % 2 == 0;
                    let pixel = if in_stripe {
                        [255, 255, 255]
                    } else if bit {
                        [base[0], 20, base[2]]
                    } else if checker {
                        base
                    } else {
                        [base[0] / 2, base[1] / 2, base[2] / 2]
                    };
                    rgb.extend_from_slice(&pixel);
                }
            }
        }
        rgb
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
    fn run_ffmpeg(args: &[&str]) {
        let output = Command::new("ffmpeg")
            .args(args)
            .output()
            .expect("ffmpeg should launch");
        assert!(
            output.status.success(),
            "ffmpeg failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
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
    fn assert_diagonal_best_match(
        source: &[u8],
        roundtrip: &[u8],
        frame_len: usize,
        frames: usize,
    ) {
        assert_eq!(source.len(), frame_len * frames);
        assert_eq!(roundtrip.len(), frame_len * frames);
        for source_index in 0..frames {
            let source_frame =
                &source[source_index * frame_len..source_index.saturating_add(1) * frame_len];
            let mut best_index = 0;
            let mut best_mse = f64::INFINITY;
            let mut diagonal_mse = f64::INFINITY;
            for roundtrip_index in 0..frames {
                let roundtrip_frame = &roundtrip
                    [roundtrip_index * frame_len..roundtrip_index.saturating_add(1) * frame_len];
                let mse = frame_mse(source_frame, roundtrip_frame);
                if roundtrip_index == source_index {
                    diagonal_mse = mse;
                }
                if mse < best_mse {
                    best_mse = mse;
                    best_index = roundtrip_index;
                }
            }
            assert!(
                best_index == source_index || diagonal_mse <= best_mse * 1.02 + 1.0,
                "source frame {source_index} matched roundtrip frame {best_index}; diagonal mse={diagonal_mse:.4}, best mse={best_mse:.4}"
            );
        }
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
    fn frame_mse(left: &[u8], right: &[u8]) -> f64 {
        assert_eq!(left.len(), right.len());
        let sum = left
            .iter()
            .zip(right)
            .map(|(left, right)| {
                let delta = f64::from(*left) - f64::from(*right);
                delta * delta
            })
            .sum::<f64>();
        sum / left.len() as f64
    }

    #[cfg(all(
        feature = "serde",
        any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                any(
                    feature = "backend-nvidia",
                    feature = "backend-intel",
                    feature = "backend-vulkan"
                ),
                any(target_os = "linux", target_os = "windows")
            )
        )
    ))]
    #[test]
    fn serde_roundtrips_decode_diagnostics() {
        let resolved_backend = Backend::Auto
            .resolve_decoder(&DecoderConfig::new(Codec::H264, 30, false))
            .expect("a backend feature is enabled");
        let diagnostics = DecodeDiagnostics {
            requested_backend: Backend::Auto,
            resolved_backend,
            require_hardware: false,
            output_mode: DecodeOutputMode::Rgb24,
            fps: 30,
            fallback_used: true,
            fallback_reason: Some("primary backend failed".to_string()),
            requested_sample_count: 3,
            decoded_frame_count: 4,
            returned_sample_count: 2,
            frames_with_sample_metadata_count: 2,
            dropped_or_unmatched_frame_count: 2,
            ambiguous_sample_association_count: 1,
            returned_frame_order: ReturnedFrameOrder::Presentation,
            missing_sample_ids: vec![SampleId(42)],
        };
        let json = serde_json::to_string(&diagnostics).expect("serialize diagnostics");
        assert!(json.contains("\"requested_backend\":\"auto\""));
        assert!(json.contains("\"output_mode\":\"rgb24\""));
        let roundtrip: DecodeDiagnostics =
            serde_json::from_str(&json).expect("deserialize diagnostics");
        assert_eq!(roundtrip.requested_backend, diagnostics.requested_backend);
        assert_eq!(roundtrip.resolved_backend, diagnostics.resolved_backend);
        assert_eq!(roundtrip.output_mode, diagnostics.output_mode);
        assert_eq!(roundtrip.fallback_used, diagnostics.fallback_used);
        assert_eq!(roundtrip.fallback_reason, diagnostics.fallback_reason);
        assert!(matches!(
            roundtrip.returned_frame_order,
            ReturnedFrameOrder::Presentation
        ));
        assert_eq!(
            roundtrip.ambiguous_sample_association_count,
            diagnostics.ambiguous_sample_association_count
        );
        assert_eq!(roundtrip.missing_sample_ids, diagnostics.missing_sample_ids);
    }
}
