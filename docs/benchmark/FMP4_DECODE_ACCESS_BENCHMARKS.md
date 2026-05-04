# fMP4 Decode Access Benchmarks

`video-hw-fmp4` has a byte-range cache and GOP replay helpers. It does not keep
a crate-level decoded-frame cache. Long contiguous reads should use
`decode_range_iter`, while sparse preview or revisit-heavy workloads should keep
their own decoded-frame cache around `decode_window`.

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
- `decode_window_sequential_lru`
  - Caller-side LRU cache with `decode_window`.
  - Misses decode a small presentation window and retain nearby frames.
- `decode_sample_random_no_cache`
  - Sparse deterministic sample order without decoded-frame cache.
- `decode_window_random_lru`
  - Sparse deterministic sample order with caller-side decoded-frame cache.
- `decode_window_ping_pong_lru`
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
- `app cache hit/miss` is only populated by the LRU window-cache cases. These
  hits avoid a decoder call entirely.
- If contiguous `decode_range_iter` is much faster than repeated
  `decode_sample`, the implementation is behaving as designed: efficient for
  long sequential ranges, but decoded-frame caching is a caller policy.

## Current Observation

On this Windows machine, a 30-frame H.264 run against
`sample-videos/foreman_cif.mp4` with `--backend auto` produced
`output/benchmark-fmp4-decode-access-1777903296.md`.

Key numbers:

- `decode_range_iter_contiguous`: 30 encoded sample reads, 88 KB payload read,
  29/1 byte-cache hit/miss, min PSNR 45.512 dB.
- `decode_sample_sequential_no_cache`: 465 encoded sample reads, 1.63 MB
  payload read, same min PSNR. This confirms repeated single-sample decode
  replays GOP payloads even though the byte range cache avoids disk misses.
- `decode_window_sequential_lru`: 22 app-cache hits and 8 misses for 30
  requests, 190 encoded sample reads. This is materially better for preview-like
  access because cache hits skip decoder calls.
- `decode_window_ping_pong_lru`: 48 app-cache hits for 60 requests, showing the
  expected reuse when a caller revisits recently decoded frames.

The result matches the current implementation model: contiguous decode is
efficient when callers use the streaming range API; exact-frame sparse access is
minimal at the encoded byte-cache layer only; decoded-frame reuse requires a
caller-side policy such as an LRU around `decode_window`.
