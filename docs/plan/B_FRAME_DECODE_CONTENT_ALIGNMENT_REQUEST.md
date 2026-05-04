# video-hw B-frame Decode Content Alignment Request

## Summary

`video-hw-fmp4` should guarantee that decoded frame content is aligned with the presentation-order sample metadata returned by the decode API.

Recent changes expose `sample_meta`, `presentation_index`, and `returned_frame_order: Presentation`, which are useful and necessary. However, for H.264 MP4 inputs with B-frames, a caller can still observe visual jitter even when the returned metadata appears presentation-ordered. Re-encoding the same input without B-frames removes the jitter.

This suggests that the metadata order is now correct, but the decoded image content can still be associated with the wrong presentation sample for B-frame streams.

## Problem

For B-frame H.264 MP4 inputs, the decode result can report:

- `returned_frame_order = Presentation`
- `frames_with_sample_metadata_count == requested_sample_count`
- `presentation_index == frame index`
- monotonically increasing sample PTS
- no missing sample ids

but downstream visual output still jitters, as if adjacent or nearby frame contents are being emitted under the wrong presentation sample.

When the same source interval is first normalized to a no-B-frame H.264 stream (`-bf 0`) and decoded through the same pipeline, the jitter disappears. The no-B-frame stream has linear PTS/DTS/sample order, and decoded frame content appears stable.

Therefore, the remaining issue is likely not the public metadata order itself, but the association between backend-decoded frame content and fMP4 sample metadata for reordered streams.

## Expected Contract

For every returned `DecodedSampleFrame`:

```rust
DecodedSampleFrame {
    sample_id,
    sample_meta,
    presentation_index,
    frame,
}
```

the `frame` pixels must correspond to the presentation frame described by `sample_meta` and `presentation_index`.

It is not sufficient for the returned vector and metadata fields to be sorted in presentation order if the decoded image payload belongs to a different sample.

## Why This Matters

Frame-by-frame consumers often feed decoded frames into temporal algorithms such as:

- object tracking
- segmentation propagation
- optical flow
- video overlays
- frame differencing
- quality checks

These consumers may not be able to detect that only the image payload is mis-associated with metadata. The failure appears as temporal jitter, unstable tracking, or mismatched annotations even though the API metadata looks valid.

## Reproduction Shape

Use an H.264 MP4 with B-frames enabled.

Example input characteristics:

- `has_b_frames > 0`
- packet PTS order differs from DTS/decode order
- presentation order begins at PTS 0

Then decode a window with hardware decode, for example NVIDIA, and encode the returned frames in the order provided by `decode_window()`.

If the resulting video jitters but the same pipeline using a no-B-frame version of the input does not jitter, the decoded image content is likely not aligned with the returned presentation metadata.

## Suggested Verification

Please add tests that validate content alignment, not only metadata ordering.

### Synthetic Marker Test

Create a short B-frame H.264 input where each presentation frame contains a unique visible marker, such as a frame number or deterministic color pattern.

Then decode with `video-hw-fmp4` and verify:

- returned frame `i` has `presentation_index == i`
- returned frame `i` has sample PTS matching presentation frame `i`
- returned frame `i` visibly/pixel-wise contains marker `i`

This avoids relying on PSNR with natural video, where adjacent frames can be too similar.

### Round-trip Content Test

For a B-frame input:

1. Decode a presentation window with `video-hw-fmp4`.
2. Encode the returned frames to a no-B-frame or simple GOP output.
3. Decode the output.
4. Verify that source presentation frame `i` matches output frame `i`.

For synthetic marker input, this can be exact marker validation. For natural input, use PSNR/SSIM with caution and compare against neighboring frames to detect frame swaps.

### Comparison Control

Run the same test on a no-B-frame version of the input. Both B-frame and no-B-frame inputs should produce stable, content-aligned presentation sequences.

## Acceptance Criteria

- For B-frame H.264 MP4/fMP4 inputs, each returned decoded image payload corresponds to its attached `sample_meta` and `presentation_index`.
- `returned_frame_order: Presentation` means both metadata and pixel content are in presentation order.
- Tests cover a reordered/B-frame stream where PTS order differs from decode/DTS order.
- Tests validate decoded image content using frame markers or another deterministic per-frame signal.
- Hardware decode paths, especially NVIDIA, pass the content-alignment test.
- Backend-provided decoded-frame timestamps are not the sole source of truth unless they are proven to map correctly to fMP4 sample metadata.
- Diagnostics can identify unmatched or ambiguously matched decoded frames when content/sample association cannot be guaranteed.
- A no-B-frame control input and a B-frame input both produce non-jittery presentation-order output when decoded and re-encoded in returned order.

## Non-Goals

- This request does not require changing encoder quality settings.
- This request does not require changing application-level tracking, segmentation, or overlay logic.
- This request does not require supporting arbitrary visual comparison for natural videos as the primary test method.
- This request does not require changing the existing public metadata fields, except if additional fields are needed to audit association.

## Implementation Notes

The fMP4 layer owns the authoritative sample metadata. Hardware decoder output order and backend timestamps may be in decode order or may have backend-specific quirks.

The implementation should ensure that the decoded frame payload is matched to the correct presentation sample before returning `DecodedSampleFrame`. If this cannot be done reliably for a backend, the API should report that uncertainty instead of returning apparently valid presentation metadata with mis-associated pixels.

The key distinction is:

- Metadata sorted correctly: necessary but not sufficient.
- Pixel content matched to that metadata: required.
