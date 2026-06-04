# 05. テスト・検証計画

## 1. ビルド検証

### host側

```bash
cargo fmt --check
cargo check -p video-hw-core
cargo check -p video-hw --no-default-features
```

### Android target側

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android

cargo check -p video-hw \
  --target aarch64-linux-android \
  --no-default-features \
  --features backend-android

cargo check -p video-hw-backend-android \
  --target aarch64-linux-android \
  --no-default-features \
  --features backend-android
```

## 2. Android instrumentation / device test

### 2.1 H.264 decode smoke

入力:

- 320x180、30 frames、H.264 Annex B。
- SPS/PPSを含むIDRから開始。

確認:

- `query_capability(Codec::H264).runtime.status == Available`
- `DecodeOutputMode::Metadata` で frame count / width / height が合う。
- `DecodeOutputMode::Nv12` が有効な端末では、NV12 size / pitch / first frame PSNR を確認。
- `DecodeOutputMode::Rgb24` は NV12 変換後の RGB checksum を確認。

### 2.2 H.264 encode smoke

入力:

- 320x180 / 640x360 の NV12 synthetic frame。
- 30 fps、1 Mbps、IDR interval 1 sec。

確認:

- `EncodedLayout::AnnexB` として SPS/PPS/IDR が出力される。
- host に pull して `ffprobe` で読める。
- `ffmpeg -i output.h264 -f null -` が成功する。

### 2.3 ARGB input encode

入力:

- `RawFrameBuffer::Argb8888`。
- backend内 ARGB→NV12 変換。

確認:

- `EncoderConfig.input_format = Argb8888` でのみ受け付ける。
- mismatch時は既存設計と同じく `BackendError::InvalidInput`。

### 2.4 HEVC / AV1 gated test

HEVC / AV1 は端末 capability に依存します。テストでは capability が Available の場合のみ実行し、Unavailable の場合は skip として記録します。

## 3. device matrix

最低限、以下の系統で試験します。

| 系統 | 目的 |
|---|---|
| Pixel / Tensor | AOSPに近い最新Android挙動の確認 |
| Qualcomm Snapdragon | 市場で多いhardware codec挙動の確認 |
| MediaTek | color format / stride差の確認 |
| Samsung Exynos / Galaxy | vendor codec差の確認 |
| Android Emulator | software codec fallback / CI smoke。hardware性能評価には使わない |

## 4. ログ項目

各 device test で以下をJSONログに残します。

```json
{
  "device": "...",
  "sdk": 35,
  "abi": "arm64-v8a",
  "codec": "video/avc",
  "codec_name": "c2.qti.avc.decoder",
  "is_encoder": false,
  "is_hardware_accelerated": true,
  "is_software_only": false,
  "is_vendor": true,
  "color_format": "...",
  "stride": 640,
  "slice_height": 368,
  "crop": { "left": 0, "top": 0, "right": 639, "bottom": 359 },
  "decode_output_modes": ["metadata", "nv12"],
  "errors": []
}
```

## 5. 品質検証

| 項目 | 方法 | 合格条件 |
|---|---|---|
| decode frame count | 入力frame数と比較 | 一致、またはB-frame reorder込みでflush後一致 |
| decode dimensions | SPS / ffprobeと比較 | 一致 |
| encode bitstream validity | ffprobe / ffmpeg decode | エラーなし |
| PSNR | synthetic raw と decode後raw比較 | H.264 lossyなので閾値はcodec設定ごとに定義 |
| latency | submit→first output | deviceごとに中央値/p95記録 |
| memory | Android Studio profiler / dumpsys meminfo | 継続実行で増加しない |
| resource release | 連続 create/drop 100回 | codec leak / native crashなし |

## 6. CI案

- GitHub Actions host job: format, clippy, normal test。
- Android cross compile job: `cargo check --target aarch64-linux-android --features backend-android`。
- Device lab / Firebase Test Lab: instrumentation test。codec hardware判定はdevice依存なので、CIでは結果をfail固定にせず capability log をartifact化する。
