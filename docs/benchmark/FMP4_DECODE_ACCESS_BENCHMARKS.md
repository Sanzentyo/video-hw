# fMP4 Decode Access Benchmarks

`video-hw-fmp4` has a byte-range cache, GOP replay helpers, and an explicit
decoded-frame LRU through `CachedFrameDecoder`. Long contiguous reads should use
`decode_range_iter`, while sparse preview or revisit-heavy workloads can use
`CachedFrameDecoder::decode_sample_cached` with a small prefetch window.

Run the benchmark with:

```sh
cargo +nightly -Zscript scripts/benchmark_fmp4_decode_access.rs -- --input sample-videos/foreman_cif.mp4 --backend auto --frame-count 90
```

On Windows/Linux, the wrapper builds with `backend-nvidia backend-intel
backend-vulkan` by default. On macOS, it builds with `backend-vt`. You can
override this:

```sh
cargo +nightly -Zscript scripts/benchmark_fmp4_decode_access.rs --features "backend-nvidia backend-intel backend-vulkan" -- --backend nvidia --require-hardware
```

For generated AV1 fMP4 input, the wrapper can first run
`write_synthetic_fmp4` and then pass the generated file to the benchmark:

```sh
cargo +nightly -Zscript scripts/benchmark_fmp4_decode_access.rs --features "backend-nvidia backend-intel backend-vulkan" --generate-codec av1 --generate-backend nvidia --generate-width 320 --generate-height 180 --generate-frames 90 --generate-fragment-frames 30 --generate-require-hardware -- --backend nvidia --require-hardware --frame-count 90
```

The report is written to:

```text
output/benchmark-fmp4-decode-access-<epoch>.md
```

## Cases

- `decode_range_iter_contiguous`
  - One decoder session over a contiguous sample range.
  - This is the expected efficient path for tracking or analysis that consumes
    frames in order.
- `decode_sample_sequential_no_cache`
  - One `decode_sample` call per contiguous sample.
  - This shows the cost when each target can replay from a GOP keyframe.
- `cached_decode_sample_sequential`
  - Library decoded-frame LRU through `CachedFrameDecoder`.
  - Misses decode a small presentation window and retain nearby frames.
- `decode_sample_random_no_cache`
  - Sparse deterministic sample order without decoded-frame cache.
- `cached_decode_sample_random`
  - Sparse deterministic sample order with library decoded-frame cache.
- `decode_sample_reverse_no_cache`
  - Cold reverse sample order without decoded-frame cache.
- `cached_decode_sample_reverse_before`
  - Cold reverse access with reverse-direction prefetch (`before > 0`).
- `cached_decode_sample_reverse_after`
  - Cold reverse access with forward prefetch as a direction-mismatch control.
- `cached_decode_sample_ping_pong`
  - Forward then reverse through the span to expose warm-cache revisit reuse.

## Correctness Columns

By default the benchmark decodes an FFmpeg RGB24 reference and reports max MSE
and min PSNR for frames returned by each access pattern. Use
`--reference sequential-baseline` to compare against the crate's own contiguous
decode baseline, or `--reference none` for timing-only runs.

## Reading The Result

- `sample reads` and `bytes read` count encoded sample payload reads requested
  by the reader.
- `range hit/miss/evict` is the byte-range cache behavior. Repeated GOP replay
  can still show range hits while spending decode time again.
- `decoded hit/miss/insert/evict` is populated by `CachedFrameDecoder` cases.
  Hits avoid a decoder call entirely.
- If contiguous `decode_range_iter` is much faster than repeated
  `decode_sample`, the implementation is behaving as designed: efficient for
  long sequential ranges, while `CachedFrameDecoder` covers sparse/revisit
  access.
- `cached_decode_sample_ping_pong` is a warm reverse case. The cold reverse
  cases are `decode_sample_reverse_no_cache`,
  `cached_decode_sample_reverse_before`, and
  `cached_decode_sample_reverse_after`.

## Current Observation

On this Windows machine, a 30-frame H.264 run against
`sample-videos/foreman_cif.mp4` with `--backend auto` produced
`output/benchmark-fmp4-decode-access-1777904682.md`.

Key numbers:

- `decode_range_iter_contiguous`: 0.959 s, 30 encoded sample reads, 88 KB
  payload read, 29/1 byte-cache hit/miss, min PSNR 45.512 dB.
- `decode_sample_sequential_no_cache`: 465 encoded sample reads, 1.63 MB
  payload read, same min PSNR. This confirms repeated single-sample decode
  replays GOP payloads even though the byte range cache avoids disk misses.
- `cached_decode_sample_sequential`: 22 decoded cache hits and 8 misses for 30
  requests, 190 encoded sample reads, 56 inserts, 6 evictions, same min PSNR.
- `cached_decode_sample_ping_pong`: 48 decoded cache hits for 60 requests,
  showing expected reuse when recently decoded frames are revisited.
- Reverse cold-access reports should compare `cached_decode_sample_reverse_before`
  against `cached_decode_sample_reverse_after`; reverse-before is the intended
  prefetch direction for backwards reads.

The result matches the current implementation model: contiguous decode is
efficient when callers use the streaming range API; exact-frame sparse access is
minimal at the encoded byte-cache layer only unless callers opt into
`CachedFrameDecoder`.

## macOS / VideoToolbox Smoke

On this macOS machine, a 30-frame H.264 run against
`sample-videos/foreman_cif.mp4` with `--backend auto --reference sequential-baseline`
produced `output/benchmark-fmp4-decode-access-1777946150.md`.

Key numbers:

- `decode_range_iter_contiguous`: 0.016 s, 30 encoded sample reads, 88 KB
  payload read, 29/1 byte-cache hit/miss.
- `decode_sample_sequential_no_cache`: 0.314 s, 465 encoded sample reads,
  1.63 MB payload read.
- `cached_decode_sample_sequential`: 23 decoded cache hits and 7 misses for
  30 requests, 168 encoded sample reads, 63 inserts, 0 evictions.
- `cached_decode_sample_ping_pong`: 53 decoded cache hits for 60 requests,
  showing expected reuse when recently decoded frames are revisited.

## AV1 fMP4 Hardware Runs

Generated AV1 fMP4 inputs were created with `write_synthetic_fmp4` and measured
with the same access-pattern benchmark. Both runs compare RGB24 output against
FFmpeg software decode and therefore report frame correctness through max MSE /
min PSNR in addition to cache behavior.

- NVIDIA AV1 fMP4, 320x180, 90 frames:
  `output/benchmark-fmp4-decode-access-1778069696.md`.
  `decode_range_iter_contiguous` returned all 90 frames in 1.049 s with min
  PSNR 45.950 dB. Per-sample sequential and reverse no-cache calls replayed GOP
  data and took 7.430 s / 7.149 s with 1395 sample reads. The decoded-frame LRU
  reduced sequential and reverse-before access to 0.847 s / 0.846 s with 80 hits
  and 10 misses; reverse-after stayed slow at 7.458 s because the prefetch
  direction intentionally mismatched reverse access.
- Intel oneVPL AV1 fMP4, 320x180, 24 frames:
  `output/benchmark-fmp4-decode-access-1778070138.md`.
  `decode_range_iter_contiguous` returned all 24 frames in 1.276 s with min
  PSNR 46.064 dB. Per-sample sequential and reverse no-cache calls took 29.358 s
  / 31.215 s with 300 sample reads. The decoded-frame LRU reduced sequential and
  reverse-before access to 5.957 s / 6.879 s; reverse-after stayed near the
  no-cache reverse cost at 31.230 s. A 90-frame Intel AV1 run exceeded the
  240-second local timeout, so larger Intel runs should be treated as a stress
  case rather than a default smoke.
- Wrapper generation smoke:
  `output/benchmark-fmp4-decode-access-1778070314.md`.
  The one-command `--generate-codec av1 --generate-backend nvidia` path produced
  an 8-frame 320x180 AV1 fMP4 and completed all access cases with min PSNR
  46.045 dB. A 160x90 AV1 smoke generated successfully but failed NVDEC submit
  with `CUDA_ERROR_UNKNOWN`, so 320x180 remains the documented smoke size.

## macOS / VideoToolbox AV1 fMP4 Decode

On this macOS machine, a 30-frame 320x180 AV1 fMP4 input generated for the VT
precise benchmark was measured with `--backend vt --require-hardware` in
`output/benchmark-fmp4-decode-access-1778473184.md`.

Key numbers:

- `decode_range_iter_contiguous`: 0.014 s, 30 returned frames, 30 sample reads,
  min PSNR inf against the sequential baseline.
- `decode_sample_sequential_no_cache`: 0.372 s, 465 sample reads, 984 KB payload
  read.
- `cached_decode_sample_sequential`: 0.063 s with 26 decoded cache hits and 4
  misses for 30 requests.
- `cached_decode_sample_reverse_before`: 0.064 s with 26/4 hit/miss; the
  mismatched `cached_decode_sample_reverse_after` control took 0.364 s.
