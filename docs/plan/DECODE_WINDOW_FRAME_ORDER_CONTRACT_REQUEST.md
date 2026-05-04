# video-hw Decode Window Frame Order Contract Request

## Summary

`video-hw-fmp4` should make the frame ordering contract of `FrameDecoder::decode_window()` explicit and safe for callers.

For windowed fMP4 decoding, the caller usually needs frames in presentation order. With B-frames, decode order, sample id order, packet DTS order, and presentation PTS order can differ. The current API makes it too easy for callers to accidentally consume frames in the wrong order or to use unreliable backend-derived timestamps as a workaround.

This request is independent of any application-specific tracking, segmentation, or overlay logic. It concerns the public behavior and metadata contract of `video-hw` / `video-hw-fmp4` decode APIs.

## Problem

For H.264 MP4 files containing B-frames, a caller can observe frame identifiers such as:

```text
sample_id: 0, 3, 2, 4, 1, 7, 6, 8, 5, ...
```

This is not a safe presentation-order sequence for downstream frame-by-frame processing.

The current practical workaround is to:

1. Read the target sample metadata from `Fmp4Reader::samples(track_id)`.
2. Build the expected window in presentation order from sample metadata.
3. Decode the window.
4. Build a `sample_id -> DecodedFrame` map.
5. Reconstruct the output frames by iterating the expected sample metadata order.

That workaround should not be necessary for the normal case. If it is necessary, the API should expose it directly and document it clearly.

## Why Backend PTS Is Not Enough

Sorting by `DecodedFrame::pts_90k` is not a reliable substitute for sample metadata ordering.

In at least one NVIDIA hardware decode path, decoded frame `pts_90k` values were observed to produce a worse ordering than leaving frames in the original API-returned order. For example, sorting decoded frames by backend-provided `pts_90k` moved late samples to the beginning of the sequence.

Therefore, the ordering source should be fMP4 sample metadata owned by `video-hw-fmp4`, not timestamps recovered from backend decoded frames after decode.

## Requested Behavior

Please implement one of the following API-level fixes.

### Preferred Option: Presentation-Ordered Decode Window

`FrameDecoder::decode_window()` should return frames in presentation order for the requested window.

The returned `DecodedSampleFrame` values should be ordered according to the fMP4 sample metadata presentation order for the requested sample range. B-frames and decoder output order should not leak into the returned vector order.

### Alternative Option: Explicit Ordered API

If changing `decode_window()` would be a breaking behavior change, add a new API such as:

```rust
decode_window_presentation_order(...)
decode_window_decode_order(...)
```

or add an explicit order option:

```rust
FrameOrder::Presentation
FrameOrder::Decode
FrameOrder::SampleMetadata
```

The default should be safe for normal frame-by-frame consumers, ideally presentation order.

### Metadata Requirement

Each returned frame should carry reliable fMP4-derived metadata, not only backend-derived frame metadata.

Suggested fields:

```rust
pub struct DecodedSampleFrame {
    pub sample_id: Option<SampleId>,
    pub sample_meta: Option<SampleMeta>,
    pub presentation_index: Option<usize>,
    pub frame: DecodedFrame,
}
```

At minimum, expose:

- `sample_id`
- sample `pts`
- sample `dts`
- sample duration
- presentation index within the returned decode window

The important point is that callers should not have to separately query `reader.samples()` and reconstruct the mapping themselves.

## Diagnostics Requirement

Decode diagnostics should report the ordering contract used for the returned frames.

Example:

```rust
pub enum ReturnedFrameOrder {
    Presentation,
    Decode,
    Unknown,
}
```

Diagnostics should also report mismatches or missing sample associations, for example:

- number of requested samples
- number of decoded frames
- number of frames with sample metadata attached
- number of dropped/unmatched frames
- returned frame order

## Reproduction Scenario

Use any H.264 MP4/fMP4 input with B-frames.

One easy way to create a test input is to encode numbered or high-motion frames with libx264 using B-frames enabled:

```bash
ffmpeg -f lavfi -i testsrc2=size=1920x1080:rate=30 -frames:v 120 \
  -c:v libx264 -bf 2 -g 30 -pix_fmt yuv420p bframes_input.mp4
```

Then decode a 120-frame window through `video-hw-fmp4` with a hardware backend such as NVIDIA:

```rust
let mut request = FrameDecodeWindowRequest::new(video_track_id, first_sample_id);
request.before = 0;
request.after = 119;
request.backend = Backend::Nvidia;
request.require_hardware = true;
request.output_mode = DecodeOutputMode::Nv12;

let mut decoder = FrameDecoder::new(&mut reader);
let result = decoder.decode_window(request)?;
```

The returned frame sequence should match the fMP4 sample metadata presentation order for the requested window.

## Verification Method

A robust verification should not rely only on visual inspection.

Recommended checks:

1. Generate or use a B-frame input with visually distinguishable frames.
2. Decode the same requested window through `video-hw-fmp4`.
3. Encode the decoded frames back to MP4.
4. Decode the round-tripped MP4.
5. Compare source presentation frame `i` with round-trip frame `i` using PSNR or exact synthetic frame markers.

For a lossy H.264 round trip, exact pixel equality is not expected. The frame order check should compare each source frame against all round-trip frames and verify that the diagonal match is the best match, or within a small tolerance of the best match when adjacent frames are visually similar.

For a synthetic numbered-frame input, the preferred test is to validate the visible frame number or embedded marker, which avoids false positives from visually similar adjacent frames.

## Acceptance Criteria

- `decode_window()` or the new ordered decode API returns frames in documented presentation order for H.264 inputs with B-frames.
- Returned frame ordering is derived from fMP4 sample metadata, not backend decoded-frame PTS alone.
- Each returned frame exposes enough sample metadata for callers to audit ordering without separately reconstructing a sample map.
- The API documentation clearly states the order of returned frames.
- Decode diagnostics expose the returned frame order and sample association counts.
- Unit or integration tests cover at least one B-frame H.264 MP4/fMP4 input.
- A test verifies that the returned sequence matches sample metadata presentation order.
- A round-trip decode -> encode -> decode test verifies that frame `i` remains aligned with frame `i` for a B-frame input.
- The implementation works with the NVIDIA backend and does not rely on NVIDIA-specific timestamp behavior.
- Existing callers that require decode-order frames have a documented way to request decode order or recover it from metadata.

## Non-Goals

- This request does not ask for application-specific tracking, segmentation, or overlay fixes.
- This request does not require changing encoder behavior.
- This request does not require changing model inference code.
- This request does not require changing pixel format conversion behavior except where metadata must be preserved across conversion.

## Rationale

Frame-by-frame consumers naturally assume that a decoded window is safe to iterate in display order. If that assumption is false, failures appear downstream as jitter, temporal discontinuity, or mismatched annotations. Those failures are difficult to debug because every individual decoded frame may look valid.

The fMP4 layer already has the authoritative sample metadata needed to define presentation order. The decode API should preserve and expose that ordering directly so ordinary callers do not need to reimplement ordering recovery.
