# video-hw Upstream Features Needed For Signage Analysis

## Context

This repository needs frame-accurate access to large saved HEVC MP4/fMP4
sessions, then selected-person crop and behavior tracking based on
`hisdf_output.msgpack.zlib`.

`Sanzentyo/video-hw` branch `codex/fmp4-indexed-reader` already covers the main
reader responsibilities:

- metadata-first MP4/fMP4 indexing
- file-backed sample reads with range cache
- `SampleId`/`TrackId`/`MediaTime`
- PTS-to-sample lookup
- previous-keyframe/GOP lookup
- `EncodedSample::to_annexb()`
- `FrameDecoder::decode_sample`
- `FrameDecoder::decode_range`

The items below are the remaining upstream work that would make long-running
signage analysis safer and simpler.

## Priority 1

### Streaming Decode Cursor

`FrameDecoder::decode_range` currently returns a `Vec`. That is acceptable for
small windows such as `-3..=3`, but it is not ideal for long behavior tracking.

Add a streaming API such as:

```rust
pub struct DecodedFrameIter<'a> { /* ... */ }

impl FrameDecoder<'_> {
    pub fn decode_range_iter(
        &mut self,
        request: FrameDecodeRangeRequest,
    ) -> Result<DecodedFrameIter<'_>>;
}
```

or make `GopCursor` own enough decoder state to expose:

```rust
impl GopCursor {
    pub fn next_decoded(
        &mut self,
        decoder: &mut FrameDecoder<'_>,
    ) -> Result<Option<DecodedSampleFrame>>;
}
```

Requirements:

- Decode from the previous keyframe only when entering a new GOP.
- Yield frames as soon as the backend produces them.
- Preserve `SampleId` association for each yielded frame.
- Avoid accumulating all decoded frames in memory.
- Expose backend errors with sample id and track id context.

### PTS Lookup With Delta

`sample_at_pts(track, pts)` returns a `SampleId`, using the previous sample for
inexact PTS. The analysis report needs to know whether the match was exact.

Add a detailed lookup API:

```rust
pub struct SampleLookup {
    pub requested_pts: MediaTime,
    pub matched_sample: SampleId,
    pub matched_pts: MediaTime,
    pub delta_ticks: i128,
    pub delta_seconds: f64,
    pub exact: bool,
}

impl Fmp4Reader<SyncReading> {
    pub fn sample_at_pts_with_delta(
        &mut self,
        track: TrackId,
        pts: MediaTime,
    ) -> Option<SampleLookup>;
}
```

Requirements:

- Make off-by-one or timescale conversion mistakes visible.
- Define tie-breaking behavior explicitly.
- Keep the existing simple `sample_at_pts` as a convenience wrapper if desired.

### NAL Length Size Handling

`EncodedSample::to_annexb()` appears to parse length-prefixed samples as
4-byte NAL lengths. That is common, but MP4 sample entries can declare other
length sizes.

Required behavior:

- Use `avcC.length_size_minus_one` and `hvcC.length_size_minus_one` when
  converting to Annex-B.
- Support 1, 2, and 4 byte length fields where valid.
- Return a clear error if the sample entry has an unsupported or invalid length
  size.
- Add tests for at least 1-byte, 2-byte, and 4-byte H.264/HEVC samples.

## Priority 2

### Decode Window Helper

The signage validator repeatedly needs a small window around a center sample.

Add a helper API:

```rust
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

impl FrameDecoder<'_> {
    pub fn decode_window(
        &mut self,
        request: FrameDecodeWindowRequest,
    ) -> Result<FrameDecodeRangeResult>;
}
```

Requirements:

- Build the window from the track sample slice, not by raw `SampleId`
  arithmetic unless the ids are proven contiguous within the same track.
- Decode from the previous keyframe of the first requested sample.
- Return every frame with its `SampleId`.

### Structured Backend Diagnostics

Analysis reports should record exactly what decoder path was used.

Add structured diagnostics to decode results:

```rust
pub struct DecodeDiagnostics {
    pub requested_backend: Backend,
    pub resolved_backend: BackendKind,
    pub require_hardware: bool,
    pub output_mode: DecodeOutputMode,
    pub fps: i32,
    pub fallback_used: bool,
    pub fallback_reason: Option<String>,
}
```

This is important because hardware decode availability differs by machine.

### Large File Smoke Checks

Add or keep a script/example that can validate the reader against large real
files without decoding the whole video.

Suggested checks:

- Open with `IndexMode::Eager` and report indexed sample count.
- Open with `IndexMode::Lazy` and verify open does not index samples.
- Resolve first, middle, and last sample by PTS.
- Read one GOP around each checkpoint.
- Decode one frame at each checkpoint with the requested backend.
- Print cache stats and decoded backend diagnostics.

## Priority 3

### Cache Tuning Hooks

The range cache defaults are reasonable, but analysis should be able to tune
them per workload.

Useful additions:

- expose cache config in `Fmp4ReaderStatus`
- expose a `clear_cache()` method for controlled memory release
- optionally expose per-track/sample read statistics for diagnostics

### Metadata Export

For reproducible analysis reports, it may be useful to serialize the MP4 sample
index.

Potential API:

```rust
pub struct Mp4IndexSnapshot {
    pub tracks: Vec<Fmp4Track>,
    pub samples: Vec<SampleMeta>,
}

impl Fmp4Reader<SyncReading> {
    pub fn index_snapshot(&mut self) -> Result<Mp4IndexSnapshot>;
}
```

This should be optional. It is not required for initial frame validation.

## What Is Not Needed Upstream

Keep these in `signage-backend` or analysis-specific crates:

- HISDF decoding
- selected-person bbox lookup
- person crop extraction
- behavior tracking logic
- PSNR/MAE report formatting
- derived artifact persistence
- decoded/cropped frame cache policy for analysis jobs

Those are application semantics, not generic video reader responsibilities.
