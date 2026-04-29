# video-hw-fmp4 Large MP4 Analysis Plan

## Goal

Saved signage sessions contain large HEVC MP4 files, `video_log.jsonl`, and
pipeline outputs such as `metadata.json`, `frame.png`, and
`hisdf_output.msgpack.zlib`. For behavior analysis, we need to recover the video
frame corresponding to a pipeline output, crop the selected person, and process
neighboring frames without mutating the saved data.

The current short-term validation uses `ffmpeg` to extract frames and compares
them with saved `frame.png` using PSNR/MAE. This is suitable for verification.
For a robust Rust-native path, `video-hw-fmp4` should provide a large-file
reader and `video-hw` should handle decode.

## Current Constraints

`video-hw-fmp4` already has useful concepts:

- `Fmp4Reader` supports regular MP4 and fragmented MP4.
- The reader API is a breaking-change `SampleMeta` + `EncodedSample` API.
  `SampleMeta` carries timestamps, duration, keyframe status, composition time
  offset, and file ranges without loading payload bytes.
- `EncodedSample::to_annexb()` converts MP4 length-prefixed H.264/HEVC samples
  to Annex-B for `video-hw` after `read_sample` or a GOP iterator loads payload
  bytes on demand.
- The slider GUI example demonstrates the correct decode strategy: seek to a
  sample, find the previous keyframe, submit samples from keyframe to target,
  and display the decoded frame.

The blocking issue is memory behavior. The current reader reads the entire MP4
with `fs::read`, and the GUI example stores all sample payloads. This is not
acceptable for 18GB+ session videos.

## Architecture

Separate responsibilities into five layers.

```text
Mp4Index
  Lightweight table of tracks, samples, timestamps, offsets, sizes, keyframes.

SampleStore
  File-backed range reads with bounded byte cache and optional read-ahead.

GopReader
  Resolves target sample -> previous keyframe..target sample.
  Converts samples to Annex-B and prepends parameter sets as needed.

FrameDecoder
  Feeds GOP samples into video-hw and returns decoded RGB/NV12 frames.

Analysis Layer
  Compares against saved frame.png, crops selected person using HISDF bbox,
  tracks behavior, and owns decoded-frame cache policy.
```

`video-hw-fmp4` should own `Mp4Index`, `SampleStore`, and `GopReader`.
Decoded-frame caches should live above it because preview, verification, and
behavior tracking need different cache policies.

## Core Types

Use small newtypes so APIs cannot mix track IDs, sample IDs, and time units by
accident.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SampleId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MediaTime {
    pub ticks: u64,
    pub timescale: NonZeroU32,
}

#[derive(Debug, Clone)]
pub struct SampleMeta {
    pub sample_id: SampleId,
    pub track_id: TrackId,
    pub offset: u64,
    pub size: u32,
    pub dts: MediaTime,
    pub pts: MediaTime,
    pub duration: u32,
    pub composition_time_offset: Option<i64>,
    pub keyframe: bool,
}
```

The sample table should store metadata only. Encoded payloads must be loaded on
demand.

## Reader API

`Mp4Reader` should be file-backed by default.

```rust
pub struct Mp4ReaderConfig {
    pub input_path: PathBuf,
    pub index_mode: IndexMode,
    pub range_cache: RangeCacheConfig,
}

pub enum IndexMode {
    Eager,
    Lazy,
}

pub struct RangeCacheConfig {
    pub chunk_size: usize,
    pub max_bytes: usize,
    pub read_ahead_chunks: usize,
}

impl Mp4Reader {
    pub fn open(config: Mp4ReaderConfig) -> Result<Self>;
    pub fn tracks(&self) -> &[Mp4Track];
    pub fn samples(&self, track: TrackId) -> Result<&[SampleMeta]>;
    pub fn sample_at_pts(&self, track: TrackId, pts: MediaTime) -> Option<SampleId>;
    pub fn sample_meta(&self, sample: SampleId) -> Option<&SampleMeta>;
    pub fn keyframe_before(&self, sample: SampleId) -> Option<SampleId>;
    pub fn gop_for_sample(&self, sample: SampleId) -> Option<GopSegment>;
    pub fn read_sample(&mut self, sample: SampleId) -> Result<EncodedSample>;
    pub fn iter_gop_for_sample(&mut self, sample: SampleId) -> Result<EncodedSampleIter<'_>>;
    pub fn read_gop(&mut self, sample: SampleId) -> Result<Vec<EncodedSample>>;
}
```

`IndexMode::Eager` should be the default. It scans metadata at open time and
keeps random access and seek operations cheap. `IndexMode::Lazy` is reserved
for extending the metadata index on moof boundaries in very large fragmented
files where open latency matters more than immediate random access.

## Range Cache

The byte cache should cache file chunks, not individual samples. Sample offsets
often cluster, and GOP decode reads sequential samples.

Recommended default:

```text
chunk_size: 8 MiB
max_bytes: 512 MiB
read_ahead_chunks: 1
```

The cache key is `chunk_index = offset / chunk_size`. Reads that cross chunk
boundaries concatenate slices from multiple chunks. Eviction is LRU by byte
budget.

Expose cache stats for diagnostics:

```rust
pub struct RangeCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub resident_bytes: usize,
}
```

## GOP Handling

Frame-accurate random access requires decoding from the previous keyframe.

```rust
pub struct GopSegment {
    pub track_id: TrackId,
    pub keyframe_sample: SampleId,
    pub end_sample_exclusive: SampleId,
    pub start_pts: MediaTime,
    pub end_pts: MediaTime,
}

pub struct EncodedSample {
    pub sample_id: SampleId,
    pub meta: SampleMeta,
    pub codec: Codec,
    pub layout: EncodedLayout,
    pub parameter_sets: Vec<Vec<u8>>,
    pub data: Vec<u8>,
}

impl EncodedSample {
    pub fn to_annexb(&self) -> Result<Vec<u8>>;
}
```

Parameter sets should be attached to keyframes when converting `hvc1`/`hev1` or
`avc1` samples to Annex-B. This preserves the previous Annex-B conversion
semantics without keeping the removed `Fmp4ReadSample` compatibility type.

## Decode API

`FrameDecoder` should hide GOP replay from callers.

```rust
pub struct FrameDecodeRequest {
    pub track_id: TrackId,
    pub target_sample: SampleId,
    pub backend: Backend,
    pub require_hardware: bool,
    pub output_mode: DecodeOutputMode,
}

pub struct DecodedVideoFrame {
    pub sample_id: SampleId,
    pub pts: MediaTime,
    pub width: u32,
    pub height: u32,
    pub pixels: DecodedPixels,
}

pub enum DecodedPixels {
    Rgb24(Vec<u8>),
    Nv12 { pitch: usize, data: Vec<u8> },
}

impl FrameDecoder {
    pub fn decode_sample(
        &mut self,
        reader: &mut Mp4Reader,
        request: FrameDecodeRequest,
    ) -> Result<DecodedVideoFrame>;

    pub fn decode_window(
        &mut self,
        reader: &mut Mp4Reader,
        center: SampleId,
        before: u32,
        after: u32,
    ) -> Result<Vec<DecodedVideoFrame>>;

    pub fn decode_range(
        &mut self,
        reader: &mut Mp4Reader,
        range: SampleRange,
    ) -> Result<DecodedFrameIter>;
}
```

For random single-frame access, `decode_sample` decodes from
`keyframe_before(target)` to `target`. For sequential analysis,
`decode_range` should keep a cursor and avoid restarting at every frame.

## Sequential Cursor

Behavior tracking is naturally sequential. Add an explicit cursor API.

```rust
pub struct GopCursor {
    pub segment: GopSegment,
    pub next_sample: SampleId,
}

impl GopCursor {
    pub fn new(reader: &Mp4Reader, start: SampleId) -> Result<Self>;
    pub fn next_decoded(
        &mut self,
        reader: &mut Mp4Reader,
        decoder: &mut FrameDecoder,
    ) -> Result<Option<DecodedVideoFrame>>;
}
```

When the cursor crosses a GOP boundary, it flushes the decoder and starts from
the next keyframe. For windowed analysis, keep the current GOP and optionally
preload the next GOP.

## Cache Ownership

Use three cache levels.

```text
MP4 index cache
  Always resident. Stores metadata only.

Byte range cache
  Owned by SampleStore. Bounded LRU by bytes.

Decoded frame cache
  Owned by analysis/preview layer. Bounded LRU by frames or bytes.
```

Do not put decoded-frame cache inside `video-hw-fmp4` initially. Decoded RGB
frames are large:

```text
1920x1080 RGB24: about 6 MiB/frame
1920x1080 RGBA:  about 8 MiB/frame
```

Suggested decoded cache defaults for analysis:

```text
verification: 7 to 31 frames
single-person tracking: 256 frames or 1 GiB
long sequential tracking: current GOP + next GOP prefetch
```

## Analysis Integration

The analysis layer should map pipeline outputs to video samples.

Short term:

```text
metadata.json.frame_index
  -> video_log.jsonl frame_index
  -> video_log.jsonl pts
  -> MP4 sample at pts
```

Long term:

```text
metadata.json timestamp/frame_index
  -> MP4 sample index via Mp4Index
  -> decode sample/window via FrameDecoder
```

For validation, always compare extracted frame with saved `frame.png` and report
all offsets rather than silently selecting the best one.

```rust
pub struct FrameWindowRequest {
    pub center: SampleId,
    pub before: u32,
    pub after: u32,
}

pub struct FrameComparison {
    pub offset: i32,
    pub sample_id: SampleId,
    pub psnr_average_db: f64,
    pub mae_average: f64,
}
```

For selected-person extraction:

```text
pipeline metadata -> selected_index
hisdf_output.msgpack.zlib -> HisdfOutput
selected_index -> person_bboxes[selected_index].body.bbox
decoded frame -> crop by bbox
```

## Implementation Phases

### Immediate execution order

Before starting the large-file reader, finish the Vulkan HEVC decode cleanup so
GOP replay has a reliable HEVC backend:

1. Add a regression test for display-order POC sorting and PTS assignment in the
   Vulkan backend.
2. Remove or mark internal the temporary HEVC decode diagnostics that were only
   used for the NVIDIA P/B-frame investigation.
3. Document the remaining decode-side diagnostics and the expected raw-vs-raw
   validation workflow.

Then start the fMP4 work with a spike that answers the first open question:
whether `shiguredo_mp4` can expose all needed sample metadata without keeping
payload bytes resident. This is a breaking API change: do not preserve
`Fmp4ReadSample`, do not add deprecated aliases, and update examples/tests to
the metadata-first reader contract in the same series:

1. Add `TrackId`, `SampleId`, `MediaTime`, `SampleMeta`, and `GopSegment`.
2. Add an index-building path that stores sample metadata only.
3. Replace `Fmp4ReadSample` with `EncodedSample` and make `read_sample(SampleId)`
   load bytes on demand.
4. Replace the backing `Vec<u8>` with `SampleStore` / file-range cache once
   metadata parity is proven.

### Phase 1: Large-file reader

- Replace `fs::read` with file-backed range reading.
- Build a metadata-only sample index.
- Add `read_sample(SampleId)`.
- Add cache stats and tests against normal MP4 and fMP4.

### Phase 2: GOP utilities

- Add `keyframe_before`, `gop_for_sample`, and `read_gop`.
- Preserve `to_annexb` behavior.
- Add tests for HEVC parameter sets on keyframes.

### Phase 3: Decoder integration

- Add `FrameDecoder::decode_sample`.
- Support `Rgb24` first, `Nv12` as an option.
- Keep backend fallback explicit and observable in result metadata.

### Phase 4: Sequential analysis

- Add `GopCursor` and `decode_range`.
- Add decoded-frame cache in `analysis-tools`, not in `video-hw-fmp4`.
- Add frame-window verification reports with PSNR/MAE.

### Phase 5: Behavior tracking

- Decode frames in timestamp order.
- Crop selected person using HISDF bbox.
- Persist derived analysis artifacts outside `mock/`.
- Never mutate the copied session data.

## Open Questions

- Whether `shiguredo_mp4` exposes enough sample table metadata without reading
  payloads. If not, wrap or extend it with an index-building path.
- Whether `Mmap` is worth supporting on Linux/Windows, or whether explicit
  range reads are sufficient.
- How `video-hw` backends behave for HEVC RGB/NV12 output on the target machine.
- Whether long-running tracking should cache decoded RGB or only cropped person
  images to reduce memory.
