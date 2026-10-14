//! Public decode requests, results, diagnostics, and cache contracts.

use super::*;

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
            "android" => Ok(BackendKind::Android),
            "videotoolbox" | "vt" => Ok(BackendKind::VideoToolbox),
            "nvidia" | "nv" => Ok(BackendKind::Nvidia),
            "intel" | "qsv" => Ok(BackendKind::Intel),
            "vulkan" => Ok(BackendKind::Vulkan),
            other => Err(de::Error::unknown_variant(
                other,
                &["android", "videotoolbox", "nvidia", "intel", "vulkan"],
            )),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
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
