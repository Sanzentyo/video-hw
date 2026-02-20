# video-hw 利用ガイド（厳密 I/O 仕様つき）

この文書は `src/` 実装（`lib.rs`, `contract.rs`, `bitstream.rs`, `nv_backend.rs`, `vt_backend.rs` ほか）に基づく、**現行実装の厳密な利用方法**です。

## 1. まず押さえる API

公開 API の入口は次の 2 つです。

- `video_hw::Decoder`
  - `push_bitstream_chunk(&[u8], Option<i64>) -> Result<Vec<Frame>, BackendError>`
  - `flush() -> Result<Vec<Frame>, BackendError>`
- `video_hw::Encoder`
  - `push_frame(Frame) -> Result<Vec<EncodedPacket>, BackendError>`
  - `flush() -> Result<Vec<EncodedPacket>, BackendError>`

backend は `BackendKind::{VideoToolbox, Nvidia}` で実行時に切り替えます。

## 2. 入出力仕様（厳密）

### 2.1 Decode 入力（`Decoder::push_bitstream_chunk`）

- 入力は **Annex-B 形式の H.264 / HEVC elementary stream**
  - NAL 区切りは `00 00 01` または `00 00 00 01`
- chunk 分割は任意（途中分割可）
  - 内部 `StatefulBitstreamAssembler` が増分復元
- codec は `DecoderConfig.codec` に一致している必要あり

注意:

- VideoToolbox decode は parameter set が必要
  - H264: SPS + PPS
  - HEVC: VPS + SPS + PPS
- `push_bitstream_chunk` の `pts_90k` は AU ごとの厳密時刻ではなく、現状は fallback 的に扱われます（chunk 内複数 AU の個別時刻は分離されません）。

### 2.2 Decode 出力（`Vec<Frame>`）

現行実装の decode 出力は **メタデータ中心**です。

- `Frame.width`, `Frame.height`: デコード結果の寸法
- `Frame.pts_90k`: backend/経路により付与
- `Frame.argb`: **常に `None`（decode 生画素は返さない）**
- `Frame.pixel_format`:
  - VT: `Some(...)` になる場合あり
  - NV: `None`

### 2.3 Encode 入力（`Encoder::push_frame`）

- `Frame.width > 0 && Frame.height > 0` 必須
- 1 回の flush サイクル内で寸法固定
  - 同一サイクルで width/height が変わると `InvalidInput`
- `Frame.argb` を与える場合、長さは厳密に `width * height * 4`

`Frame.argb` のバイト順は **ARGB（1 pixel = 4 bytes）**:

- byte[0] = A
- byte[1] = R
- byte[2] = G
- byte[3] = B

`argb: None` の場合は backend 内部で synthetic 画像が使われます（主に疎通確認向け）。

### 2.4 Encode 出力（`Vec<EncodedPacket>`）

`EncodedPacket`:

- `codec`: 入力 codec
- `data`: backend 生 payload
- `pts_90k`: 入力 `Frame.pts_90k` が基本的に引き継がれる
- `is_keyframe`: backend 判定結果

`data` の形式は backend ごとに異なります。

1) VideoToolbox (`BackendKind::VideoToolbox`)

- **AVCC/HVCC 形式（length-prefixed NAL）**
- 各 NAL = `4-byte big-endian 長さ` + `NAL payload`
- start code (`00 00 01` / `00 00 00 01`) は付きません

2) NVIDIA (`BackendKind::Nvidia`)

- NVENC SDK から取得した elementary payload をそのまま返却
- 追加の mux / container 化はしません

実運用上は backend 混在で payload 形式を揃えるため、必要なら自前で変換層を入れてください。

### 2.5 Encode の取り出しタイミング（streaming 利用時の厳密挙動）

現行実装の `Encoder` は、**`push_frame` 呼び出し時には packet を返さず**、内部キューにフレームを積みます。

- `push_frame(...)` の返り値は通常 `Vec::new()`
- 実際の `EncodedPacket` 回収は `flush()` で行う

したがって、1 フレームごとに「投入してすぐ取り出す」運用は次の形になります。

- `push_frame(frame)`
- 直後に `flush()`

この方式で 1 フレーム単位の回収は可能ですが、毎回 flush するため backend 内部のバッチ効率は下がる可能性があります。

## 3. 最小コード例

### 3.1 Annex-B を decode する

```rust
use std::fs;
use video_hw::{BackendKind, Codec, Decoder, DecoderConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut decoder = Decoder::new(
        BackendKind::VideoToolbox,
        DecoderConfig::new(Codec::H264, 30, false),
    );

    let bitstream = fs::read("sample-videos/sample-10s.h264")?;
    let mut all_frames = Vec::new();

    for chunk in bitstream.chunks(65_536) {
        let frames = decoder.push_bitstream_chunk(chunk, None)?;
        all_frames.extend(frames);
    }
    all_frames.extend(decoder.flush()?);

    let summary = decoder.decode_summary();
    println!(
        "decoded={}, width={:?}, height={:?}, pixel_format={:?}",
        summary.decoded_frames, summary.width, summary.height, summary.pixel_format
    );
    Ok(())
}
```

### 3.2 ARGB 生フレームを encode する

```rust
use video_hw::{BackendKind, Codec, Encoder, EncoderConfig, Frame};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let width = 640usize;
    let height = 360usize;
    let frame_size = width * height * 4; // ARGB

    let mut encoder = Encoder::with_config(
        BackendKind::Nvidia,
        EncoderConfig::new(Codec::H264, 30, true),
    );

    let mut out = Vec::new();
    for i in 0..30usize {
        let mut argb = vec![0u8; frame_size];
        for px in argb.chunks_exact_mut(4) {
            px[0] = 255; // A
            px[1] = (i * 7 % 255) as u8; // R
            px[2] = 64; // G
            px[3] = 192; // B
        }

        let packets = encoder.push_frame(Frame {
            width,
            height,
            pixel_format: None,
            pts_90k: Some((i as i64) * 3000),
            argb: Some(argb),
            force_keyframe: i == 0,
        })?;

        for p in packets {
            out.extend_from_slice(&p.data);
        }
    }

    for p in encoder.flush()? {
        out.extend_from_slice(&p.data);
    }

    std::fs::write("encoded-output.bin", out)?;
    Ok(())
}
```

### 3.3 VT 出力（AVCC/HVCC）を Annex-B へ変換する

VideoToolbox encode の `EncodedPacket.data` は length-prefixed なので、Annex-B が必要なら次で変換できます。

```rust
fn avcc_or_hvcc_to_annexb(mut payload: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    while payload.len() >= 4 {
        let n = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
        payload = &payload[4..];
        if n == 0 || payload.len() < n {
            return Err("invalid length-prefixed payload".to_string());
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&payload[..n]);
        payload = &payload[n..];
    }
    if !payload.is_empty() {
        return Err("trailing bytes after NAL parse".to_string());
    }
    Ok(out)
}
```

### 3.4 streaming 適性を測る（backend 共通 probe）

`examples/encode_streaming_probe.rs` は、次の 2 モードを同条件で比較します。

- `batch_flush_once`: 全フレーム投入後に 1 回だけ flush
- `streaming_flush_each_frame`: 毎フレーム `push_frame -> flush`

実行例（VT）:

```bash
cargo run --example encode_streaming_probe -- --backend vt --codec h264 --fps 30 --width 640 --height 360 --frame-count 120
```

実行例（NV）:

```bash
cargo run --features backend-nvidia --example encode_streaming_probe -- --backend nv --codec h264 --fps 30 --width 640 --height 360 --frame-count 120
```

## 4. backend 別の注意点

### VideoToolbox

- feature: `backend-vt`（macOSで明示的に有効化）
- 前提: macOS
- encode 入力は ARGB、内部で BGRA ピクセルバッファへ変換して VT に投入
- encode 出力は AVCC/HVCC

### NVIDIA

- feature: `backend-nvidia`（Linux/Windowsで明示的に有効化）
- 前提: CUDA / NVENC / NVDEC が利用可能
- decode は NVDEC メタデータ経路（現状 `Frame.argb=None`）
- encode は ARGB 入力を NVENC バッファへ upload

## 5. Metrics（取得できる情報）

メトリクスは標準エラー出力（`stderr`）に 1 行ログとして出ます。

### 5.1 有効化方法

NVIDIA:

- 環境変数: `VIDEO_HW_NV_METRICS=1`
- または API 設定:
  - decode: `BackendDecoderOptions::Nvidia(NvidiaDecoderOptions { report_metrics: Some(true) })`
  - encode: `BackendEncoderOptions::Nvidia(NvidiaEncoderOptions { report_metrics: Some(true), .. })`

VideoToolbox:

- 環境変数: `VIDEO_HW_VT_METRICS=1`

### 5.2 NVIDIA decode（`[nv.decode]`）

主な項目:

- `access_units`, `frames`
- `pack_ms`, `sdk_ms`, `map_ms`（総時間, ms）
- `pack_p95_ms`, `pack_p99_ms`, `sdk_p95_ms`, `sdk_p99_ms`, `map_p95_ms`, `map_p99_ms`
- `queue_depth_peak`, `queue_depth_p95`, `queue_depth_p99`
- `jitter_ms_mean`, `jitter_ms_p95`, `jitter_ms_p99`

補足:

- `jitter_ms_*` は `pts_90k` 差分から計算したフレーム間隔のゆらぎ（期待フレーム間隔との差の絶対値, ms）です。

### 5.3 NVIDIA encode（`[nv.encode]` / `[nv.encode.safe]`）

通常経路（`[nv.encode]`）:

- `frames`, `packets`, `queue_peak`, `max_in_flight`
- `synth_ms`, `upload_ms`, `submit_ms`, `reap_ms`, `encode_ms`, `lock_ms`
- `queue_p95`, `queue_p99`
- `jitter_ms_mean`, `jitter_ms_p95`, `jitter_ms_p99`
- `input_copy_bytes`, `input_copy_frames`, `output_copy_bytes`, `output_copy_packets`

safe lifetime 経路（`[nv.encode.safe]`）:

- `frames`, `packets`
- `synth_ms`, `upload_ms`, `submit_ms`, `reap_ms`, `lock_ms`
- `queue_p95`, `queue_p99`
- `jitter_ms_mean`, `jitter_ms_p95`, `jitter_ms_p99`
- `input_copy_bytes`, `input_copy_frames`, `output_copy_bytes`, `output_copy_packets`

### 5.4 VideoToolbox decode（`[vt.decode.submit]` / `[vt.decode]`）

submit 側（`[vt.decode.submit]`）:

- `flush`（`true/false`）
- `access_units`
- `input_copy_bytes`
- `submit_ms`

出力側（`[vt.decode]`）:

- `wait`, `delta_frames`, `total_frames`
- `width`, `height`
- `elapsed_ms`
- `jitter_ms_mean`, `jitter_ms_p95`, `jitter_ms_p99`
- `output_copy_frames`

### 5.5 VideoToolbox encode（`[vt.encode]`）

主な項目:

- `frames`, `packets`, `output_bytes`, `width`, `height`
- `ensure_ms`, `frame_prep_ms`, `submit_ms`, `complete_ms`, `total_ms`
- `queue_peak`, `queue_p95`, `queue_p99`
- `jitter_ms_mean`, `jitter_ms_p95`, `jitter_ms_p99`
- `input_copy_bytes`, `input_copy_frames`, `output_copy_bytes`, `output_copy_packets`

## 6. エラーの見方

- `BackendError::InvalidInput`
  - 寸法 0、ARGB サイズ不一致、bitstream 形式不整合
- `BackendError::UnsupportedConfig`
  - backend/feature/platform 不一致、デバイス能力不足
- `BackendError::TemporaryBackpressure`
  - in-flight / queue 過負荷、一時的 busy
- `BackendError::DeviceLost`
  - GPU デバイス喪失

## 7. 既存 example の参照先

- decode: `examples/decode_annexb.rs`
- encode (synthetic): `examples/encode_synthetic.rs`
- encode (raw ARGB): `examples/encode_raw_argb.rs`
- encode (streaming probe): `examples/encode_streaming_probe.rs`
- transform: `examples/transform_nv12_rgb.rs`
