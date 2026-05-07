# NVIDIA Performance Comparison - 2026-05-07

## Environment

- Backend: NVIDIA
- Codecs measured in this pass: H.264 and HEVC
- AV1: not measured in this pass
- FFmpeg: `/home/sanzentyo/git/ffmpeg-nvidia-build/bin/ffmpeg`
- FFprobe: `/home/sanzentyo/git/ffmpeg-nvidia-build/bin/ffprobe`
- CUDA override required for this host: `CUDARC_CUDA_VERSION=12090`

Without the CUDA override, `cudarc` was built as CUDA 13.1 on this CUDA 12.9
host and tried to load `cuDevSmResourceSplit`, which is not available from the
installed driver library.

## H.264 Results

Input: `sample-videos/sample-10s.h264`, 303 decode frames. Encode uses
`--equal-raw-input true`, 640x360, 300 frames.

| Mode | video-hw | FFmpeg | Result |
|---|---:|---:|---|
| metadata decode | `553.10 fps` | `480.05 fps` | video-hw `+15.22%`, pass |
| host NV12 decode | `277.96 fps` | `462.08 fps` | video-hw `-39.85%`, fail |
| host RGB24 decode | `118.40 fps` | `262.38 fps` | video-hw `-54.88%`, fail |
| equal-input encode | `496.35 fps` | `555.06 fps` | video-hw `-10.58%`, borderline fail |

`--nv-max-in-flight 4` and `8` did not materially improve encode throughput in
the repeat=3 measurements; both remained around `476-477 fps`.

## HEVC Results

Input: `sample-videos/sample-10s.h265`, 303 decode frames. Encode uses
`--equal-raw-input true`, 640x360, 300 frames.

| Mode | video-hw | FFmpeg | Result |
|---|---:|---:|---|
| metadata decode | `539.05 fps` | `524.05 fps` | video-hw `+2.86%`, pass |
| equal-input encode | `499.97 fps` | `590.95 fps` | video-hw `-15.40%`, fail |

## Interpretation

- Metadata decode is not the current NV bottleneck. H.264 is faster than FFmpeg,
  and HEVC is effectively at parity.
- Host-output decode is the largest gap. NV12 readback is already slower than
  FFmpeg, and RGB24 adds substantial CPU conversion/allocation cost.
- Equal-input encode remains slower than FFmpeg by roughly `10-15%` in these
  runs. Internal metrics show the NVENC submit/reap time is small; most of the
  remaining wall time is outside the SDK encode calls.

## Rejected Local Experiments

- A 2x2 NV12-to-RGB coefficient reuse change was tested against H.264 RGB24
  decode and did not produce a measurable improvement.
- Increasing raw-input reader capacity in `encode_raw_argb` did not improve the
  equal-input encode wall time.

Both changes were reverted instead of being kept as unproven optimization.

## Next Improvements

- For host-output decode, separate NVDEC map/copy timing from top-level RGB
  conversion/allocation timing. Current `[nv.decode]` internal metrics do not
  include the top-level `DecodedFrame::Rgb24` conversion cost.
- Add a native output mode or benchmark path that writes decoded NV12/RGB into a
  reusable caller-owned buffer. The current API returns a fresh `Vec` per frame,
  which is expensive for 1920x1080 application paths.
- For encode, measure input-file read time and `RawFrameBuffer` construction
  separately from encoder submit/reap. The SDK time is already small compared
  with process-level wall time.
