# NVIDIA fMP4 Pipeline Performance Notes - 2026-05-07

## Scope

This note records the safe NVIDIA/fMP4 pipeline optimizations validated on a
1920x1080 H.264 fMP4 sample with B-frames. The workload is an application-style
path that decodes frames to host memory, optionally processes them, and encodes
them again with NVENC.

## Implemented Changes

- NVIDIA encode no longer clones queued ARGB frames in the main flush path.
  Pending frames are consumed by value.
- ARGB-to-NVENC byte order conversion now runs in place before upload instead
  of allocating a second converted buffer.
- `AnyDecodeSession` has a borrowed Annex-B submit path for callers that already
  prepared an Annex-B payload.
- fMP4 range/window decode uses that borrowed submit path and avoids an extra
  payload clone.
- Non-streaming fMP4 range/window decode does not force a `try_reap` after every
  submitted sample. Streaming iteration keeps the per-sample reap behavior so it
  can yield incrementally.
- NV12-to-RGB24 host conversion now uses precomputed YUV coefficient tables and
  processes two pixels per chroma pair, reducing per-frame CPU conversion work
  while keeping the same integer conversion formula.

## Validation

Command class:

```sh
cargo clippy -p video-hw --features backend-nvidia --no-deps -- -D warnings
cargo clippy -p video-hw-fmp4 --features backend-nvidia --no-deps -- -D warnings
```

Application-side roundtrip order check:

- Input: `tracking_overlay_10s_byte_track.mp4`
- Frames: 120
- Decode/encode path: fMP4 decode -> NVENC encode -> fMP4 decode -> NVENC encode
- Result:
  - diagonal best frames: `120/120`
  - diagonal best rate: `1.0`
  - minimum diagonal PSNR: `32.7989 dB`
  - mean diagonal PSNR: `40.8500 dB`
  - minimum margin to second best: `0.3345 dB`

Warm performance observed from the application-side release build:

| Stage | Time / 120 frames | Approx FPS |
|---|---:|---:|
| source fMP4 decode | `1193 ms` | `101 fps` |
| roundtrip fMP4 decode | `749 ms` | `160 fps` |
| first NVENC encode | `767 ms` | `157 fps` |
| second NVENC encode | `696 ms` | `172 fps` |

The NV12-to-RGB24 table change mainly improves the re-encoded roundtrip decode
case (`951 ms` -> `749 ms` for 120 frames in this sample). It does not materially
improve the original B-frame source decode path, which remains around
`1190 ms` for 120 frames.

SAM overlay workload after these changes remained SAM-bound:

| Stage | Time / 120 frames | Approx FPS |
|---|---:|---:|
| fMP4 decode | `1173 ms` | `102 fps` |
| SAM/EdgeTAM tracking | `8770 ms` | `13.7 fps` |
| NVENC encode | `266 ms` | `450 fps` |
| total | `10562 ms` | `11.4 fps` |

## Rejected Optimization

A larger experiment split NVDEC parser submission from display-surface mapping
and delayed host readback until later. It improved the shape of the pipeline but
was rejected because B-frame roundtrip validation failed:

- diagonal best frames: `2/120`
- diagonal best rate: `0.0167`

The likely cause is NVDEC display surface reuse before host copy. A future
version may revisit this only with explicit surface lifetime ownership or a
bounded copy queue that maps/copies each display frame before the surface can be
reused.

A separate parser configuration experiment raised `ulMaxNumDecodeSurfaces` and
`ulMaxDisplayDelay`. It was also rejected: validation returned only `90/120`
diagonal-best frames (`0.75` rate), despite presentation-order metadata. Parser
reorder/surface settings should not be changed without the same PSNR diagonal
acceptance check.

## Follow-up

- The remaining decode gap to FFmpeg should be measured against an equivalent
  host-output workload. Decode-to-null numbers are not comparable to the
  application path because this pipeline downloads NV12/RGB frames to host
  memory.
- Further decode optimization should preserve the B-frame PSNR diagonal check as
  an acceptance criterion.
