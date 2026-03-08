# I/O Format Contract（Current Runtime Contract）

更新日: 2026-02-23

## 1. 目的

この文書は `video-hw` の現行公開APIにおける I/O 契約を、実装準拠で定義する。

- バイナリ形式（bitstream / packet）
- 型レベル形式（`BitstreamInput` / `EncodeFrame` / `DecodedFrame` / `EncodedChunk`）
- 実行時制約（現在サポートされる入力と出力）

## 2. 現行 API の前提

- decode/encode は `submit` / `try_reap` / `reap_timeout` / `flush` で統一
- `reap_timeout` は timeout 上限まで待機する実装（内部キュー + backend poll、空なら `None`）
- decode 出力は `DecoderConfig.output_mode` でモード指定（現行は `Metadata` のみ実装）
- backend は feature + target で有効化
  - macOS: `backend-vt`
  - Linux/Windows: `backend-nvidia`

### 2.1 NVIDIA 依存の境界条件

- `backend-nvidia` は `nvidia-video-codec-sdk`（Rust bindings）を利用するラッパー構成
- NVIDIA Video Codec SDK 本体は利用者が別途取得・配置する前提
- ビルド時は SDK ライブラリ探索のため `NVIDIA_VIDEO_CODEC_SDK_PATH` 等の環境設定が必要になる
- 本プロジェクトの配布方針として、SDK 本体の同梱は前提にしない

## 3. Binary Contract

### 3.1 Decode 入力

- `BIN-BS-01`: Annex-B chunk
  - start code: `00 00 01` または `00 00 00 01`
- `BIN-BS-02`: raw NAL Access Unit
  - API入力時は prefix なし NAL 配列
  - 内部で Annex-B にパック
- `BIN-BS-03`: length-prefixed sample
  - 各 NAL が `u32be length + payload`
  - 内部で Annex-B に展開

### 3.2 Encode 入力

- `BIN-RF-01`: ARGB8888 packed
  - `len == width * height * 4`
  - 現行 encode で受理される唯一の実入力系列
- `BIN-RF-02`: NV12
  - 型としては存在するが、現行 encode では `InvalidInput`
- `BIN-RF-03`: RGB24
  - 型としては存在するが、現行 encode では `InvalidInput`

### 3.3 Encode 出力

- `BIN-EP-01`: Annex-B（NV）
- `BIN-EP-02`: AVCC（VT + H264）
- `BIN-EP-03`: HVCC（VT + HEVC）

## 4. Type Contract（実装準拠）

```rust
pub struct Dimensions {
    pub width: std::num::NonZeroU32,
    pub height: std::num::NonZeroU32,
}

pub struct Timestamp90k(pub i64);

pub enum BitstreamInput {
    AnnexBChunk {
        chunk: Vec<u8>,
        pts_90k: Option<Timestamp90k>,
    },
    AccessUnitRawNal {
        codec: Codec,
        nalus: Vec<Vec<u8>>,
        pts_90k: Option<Timestamp90k>,
    },
    LengthPrefixedSample {
        codec: Codec,
        sample: Vec<u8>,
        pts_90k: Option<Timestamp90k>,
    },
}

pub enum RawFrameBuffer {
    Argb8888(Vec<u8>),
    Argb8888Shared(std::sync::Arc<[u8]>),
    #[cfg(feature = "unstable-raw-inputs")]
    Nv12 { pitch: usize, data: Vec<u8> },
    #[cfg(feature = "unstable-raw-inputs")]
    Rgb24(Vec<u8>),
}

pub struct EncodeFrame {
    pub dims: Dimensions,
    pub pts_90k: Option<Timestamp90k>,
    pub buffer: RawFrameBuffer,
    pub force_keyframe: bool,
}

pub enum EncodedLayout {
    AnnexB,
    Avcc,
    Hvcc,
    Opaque,
}

pub struct EncodedChunk {
    pub codec: Codec,
    pub layout: EncodedLayout,
    pub data: Vec<u8>,
    pub pts_90k: Option<Timestamp90k>,
    pub is_keyframe: bool,
}

pub enum DecodedFrame {
    Metadata {
        dims: Option<Dimensions>,
        pts_90k: Option<Timestamp90k>,
        pixel_format: Option<u32>,
        decode_info_flags: Option<u32>,
        color: Option<ColorMetadata>,
    },
    Nv12 {
        dims: Dimensions,
        pitch: usize,
        pts_90k: Option<Timestamp90k>,
        data: Vec<u8>,
    },
    Rgb24 {
        dims: Dimensions,
        pts_90k: Option<Timestamp90k>,
        data: Vec<u8>,
    },
}
```

## 5. 現行の厳密制約

1. decode 出力
- 公開 decode 経路の標準出力は `DecodedFrame::Metadata` 中心
- `DecoderConfig.output_mode=Metadata` は常用サポート
- `output_mode=Nv12` / `Rgb24` は backend ARGB payload がある場合のみ変換出力する
- backend が ARGB payload を提供しない場合は `BackendError::UnsupportedConfig`

2. encode 入力
- `RawFrameBuffer::Argb8888` / `Argb8888Shared` のみ受理
- `RawFrameBuffer::Nv12` / `Rgb24` は `unstable-raw-inputs` 有効時のみ型として公開される
- 上記 variant は現行 encode 経路では `BackendError::InvalidInput`

3. timeout 契約
- `reap_timeout` は内部キューと backend poll を用いて timeout 上限まで待機し、回収できない場合は `None` を返す

## 6. エラー契約

- `UnsupportedConfig`: backend/環境で利用不可
- `InvalidBitstream`: bitstream 形式不正
- `InvalidInput`: frame 入力不正（ARGBサイズ不一致、未対応 buffer など）
- `TemporaryBackpressure`: 一時的飽和
- `DeviceLost`: デバイスロスト
- `Backend`: backend 内部エラー

## 7. 今後の拡張対象

- decode 出力モードの正式契約化（Metadata/NV12/RGB）
- encode での NV12 直接入力契約
- `reap_timeout` の backend 直接 poll 連携（将来最適化）

## 8. 参照

- `docs/USAGE_STRICT.md`
- `docs/spec/TEST_SPEC_INVENTORY.md`
- `docs/status/STATUS.md`
- `crates/video-hw-core/src/lib.rs`
- `crates/video-hw/src/lib.rs`
