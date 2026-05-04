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
- `cached_decode_sample_ping_pong`
  - Forward then reverse through the span to expose revisit reuse.

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

The result matches the current implementation model: contiguous decode is
efficient when callers use the streaming range API; exact-frame sparse access is
minimal at the encoded byte-cache layer only unless callers opt into
`CachedFrameDecoder`.
