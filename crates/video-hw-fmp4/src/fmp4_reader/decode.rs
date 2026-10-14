use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result, anyhow};
use shiguredo_mp4::boxes::SampleEntry;
use video_hw::{
    AnyDecodeSession, Backend, BackendDecoderOptions, BackendError, BackendKind, BitstreamInput,
    Codec, DecodeOutputMode, DecodedFrame, DecoderConfig, Timestamp90k, VtDecoderOptions,
};

use super::{
    EncodedSample, EncodedSampleIter, Fmp4Reader, GopSegment, MediaTime, SampleId, SampleMeta,
    SampleRange, SyncReading, TrackId, config::Fmp4Track,
};

mod alignment;
mod cache;
mod decoder;
mod iterator;
mod model;
mod session;

pub use cache::CachedFrameDecoder;
pub use decoder::{FrameDecoder, GopCursor};
pub use iterator::DecodedFrameIter;
pub use model::*;

use alignment::*;
#[cfg(all(
    test,
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
use cache::{DecodedFrameCache, DecodedFrameCacheKey, cache_key_for_request};
use session::*;

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
