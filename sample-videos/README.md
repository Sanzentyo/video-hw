# Sample Videos

This directory contains sample video files for testing and quality verification.

## Foreman CIF Sequence (CC0 / Public Domain)

The `foreman_cif.*` files are derived from the [Xiph.org](https://xiph.org/) open video test corpus.

- **Source**: <https://media.xiph.org/video/derf/y4m/foreman_cif.y4m>
- **License**: CC0 / Public Domain (Xiph.org derf collection)
- **Resolution**: 352×288 (CIF)
- **Frame rate**: 29.97 fps (30000/1001)
- **Frame count**: 300 frames (~10 seconds)
- **Content**: A person speaking, suitable for codec quality evaluation

### Derived Files

| File | Description |
|------|-------------|
| `foreman_cif.mp4` | H.264 in regular MP4 (libx264, CRF 20) |
| `foreman_cif_fmp4.mp4` | H.264 in fragmented MP4 (fMP4, keyframe-per-fragment) |
| `foreman_cif.h264` | H.264 Annex-B elementary stream |
| `foreman_cif.h265` | HEVC Annex-B elementary stream (libx265, CRF 26) |

All derived files were produced with FFmpeg 8.1 from `foreman_cif.y4m`.

### Regenerating

```sh
FFMPEG="path/to/ffmpeg"
SRC="C:/Temp/foreman_cif.y4m"

# Download source first:
# curl -L https://media.xiph.org/video/derf/y4m/foreman_cif.y4m -o "$SRC"

$FFMPEG -y -i "$SRC" -c:v libx264 -crf 20 -pix_fmt yuv420p -movflags +faststart foreman_cif.mp4
$FFMPEG -y -i foreman_cif.mp4 -c copy -movflags frag_keyframe+empty_moov+default_base_moof foreman_cif_fmp4.mp4
$FFMPEG -y -i foreman_cif.mp4 -c:v copy -bsf:v h264_mp4toannexb -f h264 foreman_cif.h264
$FFMPEG -y -i "$SRC" -c:v libx265 -crf 26 -pix_fmt yuv420p -x265-params "log-level=error" -f hevc foreman_cif.h265
```

---

## Legacy Test Fixtures

| File | Description |
|------|-------------|
| `sample-10s.mp4` | 1920×1080, 303 frames, H.264 MP4 |
| `sample-10s.h264` | 1920×1080, 303 frames, H.264 Annex-B |
| `sample-10s.h265` | 1920×1080, 303 frames, HEVC Annex-B |

These files are used by existing unit and integration tests that assert exactly 303 decoded frames.
Their origin is unknown (encoded by `Lavf58.44.100`) and no clear license is available.
**Do not remove** these files; they are required by existing test assertions.
