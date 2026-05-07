# NVIDIA Benchmark Comparison Guide - 2026-05-07

## Scope

This note defines how to compare `video-hw` NVIDIA decode/encode performance
against FFmpeg without mixing incompatible workloads. AV1 is intentionally out
of scope for this pass; use H.264 and HEVC only.

## Existing Benchmarks

| Tool | Purpose | Compare with FFmpeg? | Notes |
|---|---|---|---|
| `crates/video-hw/benches/decode_bench.rs` | Criterion benchmark for internal Annex-B decoder throughput | No | Useful for regression tracking inside `video-hw`, not an apples-to-apples FFmpeg report. |
| `scripts/benchmark_ffmpeg_nv_precise.rs` | NVIDIA-specific repeated FFmpeg parity benchmark | Yes | Primary tool for H.264/HEVC decode and encode comparison. |
| `scripts/benchmark_ffmpeg_backends.rs` | Aggregates backend-specific scripts | Yes | Useful for broad backend parity, less convenient when tuning NVIDIA details. |
| fMP4 application roundtrip checks | Application pipeline timing and frame-order validation | Not directly | Includes fMP4 parsing, windowing, host frames, NVENC output, and PSNR/order checks. |

## Decode Modes

The comparison must use the same output contract on both sides:

| Comparison target | video-hw command mode | FFmpeg equivalent | What it measures |
|---|---|---|---|
| Pure decoder throughput | `--decode-output-mode metadata` | CUVID decode to null sink | Parser/decode overhead with no host pixel output. |
| Host NV12 output | `--decode-output-mode nv12` | `hwdownload,format=nv12` to rawvideo null sink | NVDEC plus GPU-to-host NV12 transfer. |
| Host RGB output | `--decode-output-mode rgb24` | `hwdownload,format=nv12,format=rgb24` to rawvideo null sink | NVDEC plus host-visible RGB conversion contract. |

The source/SAM/fMP4 pipeline should be compared against `nv12` or `rgb24`
depending on the consumer. Comparing it with FFmpeg null-sink decode overstates
the FFmpeg advantage because null-sink decode does not produce host pixels.

## Encode Modes

Use `--equal-raw-input` for normal encode comparison. It generates one ARGB raw
frame sequence and feeds that same byte stream to both `video-hw` and FFmpeg.

Without `--equal-raw-input`, video-hw uses its synthetic frame generator while
FFmpeg uses `testsrc2`; that is acceptable for smoke testing but not for a
performance parity claim.

## Recommended Commands

Metadata decode plus equal-input encode:

```sh
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --decode-output-mode metadata --warmup 1 --repeat 5 --verify --equal-raw-input --include-internal-metrics
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --decode-output-mode metadata --warmup 1 --repeat 5 --verify --equal-raw-input --include-internal-metrics
```

Host RGB decode comparison:

```sh
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --decode-output-mode rgb24 --warmup 1 --repeat 5 --verify --equal-raw-input --include-internal-metrics
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --decode-output-mode rgb24 --warmup 1 --repeat 5 --verify --equal-raw-input --include-internal-metrics
```

Local NVIDIA-enabled FFmpeg build:

```sh
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs \
  --codec h264 \
  --decode-output-mode rgb24 \
  --warmup 1 \
  --repeat 5 \
  --verify \
  --equal-raw-input \
  --include-internal-metrics \
  --ffmpeg-path /home/sanzentyo/git/ffmpeg-nvidia-build/bin/ffmpeg \
  --ffprobe-path /home/sanzentyo/git/ffmpeg-nvidia-build/bin/ffprobe
```

## Acceptance Criteria

- H.264 and HEVC are measured separately.
- Each result uses `warmup >= 1` and `repeat >= 3`; prefer `repeat >= 5`.
- Encode parity claims use `--equal-raw-input`.
- Decode parity claims state the decode output mode and use the matching FFmpeg
  path.
- Reports include CV so unstable measurements can be rejected or rerun.
- `--verify` passes for both video-hw and FFmpeg encoded outputs.
- fMP4 application measurements remain separate and include PSNR/order checks
  when B-frames are involved.
