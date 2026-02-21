# video-hw 利用ガイド（厳密 I/O 仕様, 自己完結版）

この文書は、`video-hw` をこのページだけで導入して使い始めるための実運用ガイドです。  
対象 API は現行の `DecodeSession` / `EncodeSession` です。

## 1. 導入

`video-hw` は backend 実装を feature で有効化します。

- macOS: `backend-vt`
- Linux/Windows: `backend-nvidia`
- `default = []`（何も有効化しないと backend は使えません）

### 1.1 Cargo.toml（推奨）

```toml
[target.'cfg(target_os = "macos")'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-vt"] }

[target.'cfg(any(target_os = "linux", target_os = "windows"))'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-nvidia"] }
```

## 2. Backend の選択ルール

`Backend` は次を持ちます。

- `Backend::Auto`
- `Backend::VideoToolbox`（macOS + `backend-vt`）
- `Backend::Nvidia`（Linux/Windows + `backend-nvidia`）

`Backend::Auto` は OS 既定 backend を選びます。

- macOS: VideoToolbox
- Linux/Windows: NVIDIA

ただし最終可否は実行時 capability で判定されます。  
利用不可の場合は `BackendError::UnsupportedConfig` を返します。

## 3. 公開 API

### 3.1 Decode

- `DecodeSession::new(Backend, DecoderConfig) -> DecodeSession`
- `submit(BitstreamInput) -> Result<(), BackendError>`
- `try_reap() -> Result<Option<DecodedFrame>, BackendError>`
- `reap_timeout(Duration) -> Result<Option<DecodedFrame>, BackendError>`
- `flush() -> Result<Vec<DecodedFrame>, BackendError>`
- `summary() -> DecodeSummary`
- `query_capability(Codec) -> Result<CapabilityReport, BackendError>`

### 3.2 Encode

- `EncodeSession::new(Backend, EncoderConfig) -> EncodeSession`
- `submit(EncodeFrame) -> Result<(), BackendError>`
- `try_reap() -> Result<Option<EncodedChunk>, BackendError>`
- `reap_timeout(Duration) -> Result<Option<EncodedChunk>, BackendError>`
- `flush() -> Result<Vec<EncodedChunk>, BackendError>`
- `query_capability(Codec) -> Result<CapabilityReport, BackendError>`
- `request_session_switch(SessionSwitchRequest) -> Result<(), BackendError>`

## 4. Decode I/O 契約

### 4.1 入力 `BitstreamInput`

- `AnnexBChunk { chunk, pts_90k }`
  - `00 00 01` / `00 00 00 01` 区切り
  - chunk 境界は任意（途中分割可）
- `AccessUnitRawNal { codec, nalus, pts_90k }`
  - raw NAL 配列（start code なし）
  - 内部で Annex-B にパック
- `LengthPrefixedSample { codec, sample, pts_90k }`
  - `u32be length + NAL` 連結
  - 内部で Annex-B に展開

### 4.2 出力 `DecodedFrame`

- `Metadata { dims, pts_90k, pixel_format, decode_info_flags, color }`
- `Nv12 { dims, pitch, pts_90k, data }`
- `Rgb24 { dims, pts_90k, data }`

現行の標準 decode 経路は `Metadata` を返します。

## 5. Encode I/O 契約

### 5.1 入力 `EncodeFrame`

- `dims`: `NonZeroU32`（0 は不可）
- `buffer`: 現行 encode は `RawFrameBuffer::Argb8888` / `Argb8888Shared` をサポート
- `force_keyframe`: backend の keyframe 指示にマップ

`Argb8888` の長さは厳密に `width * height * 4` です。  
一致しない場合は `BackendError::InvalidInput` です。

### 5.2 出力 `EncodedChunk`

- `codec`
- `layout`
- `data`
- `pts_90k`
- `is_keyframe`

`layout` は backend と codec で決まります。

- VT + H264: `EncodedLayout::Avcc`
- VT + HEVC: `EncodedLayout::Hvcc`
- NV: `EncodedLayout::AnnexB`

## 6. submit / reap / flush の意味

- `submit`: 入力投入のみ（即時に出力が返らないことがある）
- `try_reap` / `reap_timeout`: すでに生成済みの出力を回収
- `flush`: EOS/遅延分の確定回収

推奨ループは「`submit` ごとに `try_reap` で回収、最後に `flush`」です。

## 7. 最小実装例

### 7.1 Decode（Auto backend）

```rust
use video_hw::{Backend, BitstreamInput, Codec, DecodeSession, DecoderConfig};

fn decode_annexb(data: Vec<u8>) -> Result<usize, video_hw::BackendError> {
    let mut sess = DecodeSession::new(
        Backend::Auto,
        DecoderConfig::new(Codec::H264, 30, false),
    );

    sess.submit(BitstreamInput::AnnexBChunk {
        chunk: data,
        pts_90k: None,
    })?;

    let mut count = 0usize;
    while sess.try_reap()?.is_some() {
        count += 1;
    }
    count += sess.flush()?.len();

    let summary = sess.summary();
    assert_eq!(summary.decoded_frames, count);
    Ok(count)
}
```

### 7.2 Encode（Auto backend）

```rust
use video_hw::{
    Backend, Codec, Dimensions, EncodeFrame, EncodeSession, EncoderConfig, RawFrameBuffer,
    Timestamp90k,
};

fn encode_one_frame() -> Result<usize, video_hw::BackendError> {
    let dims = Dimensions {
        width: std::num::NonZeroU32::new(640).expect("non-zero width"),
        height: std::num::NonZeroU32::new(360).expect("non-zero height"),
    };
    let argb = vec![0_u8; (dims.width.get() * dims.height.get() * 4) as usize];

    let mut sess = EncodeSession::new(
        Backend::Auto,
        EncoderConfig::new(Codec::H264, 30, true),
    );

    sess.submit(EncodeFrame {
        dims,
        pts_90k: Some(Timestamp90k(0)),
        buffer: RawFrameBuffer::Argb8888(argb),
        force_keyframe: true,
    })?;

    let mut packets = 0usize;
    while sess.try_reap()?.is_some() {
        packets += 1;
    }
    packets += sess.flush()?.len();
    Ok(packets)
}
```

## 8. CLI ですぐ試す

```bash
# decode（Auto）
cargo run --example decode_annexb -- --backend auto --codec h264 --input sample-videos/sample-10s.h264 --chunk-bytes 4096 --require-hardware

# encode（Auto）
cargo run --example encode_synthetic -- --backend auto --codec h264 --fps 30 --frame-count 120 --require-hardware --output ./encoded-output.bin
```

## 9. 失敗時の見方

- `UnsupportedConfig`
  - backend 実装が環境で利用不可（例: CUDA context 初期化失敗、driver/SDK 不足）
- `InvalidInput`
  - 入力形式不正（例: ARGB サイズ不一致）
- `InvalidBitstream`
  - bitstream 破損または length-prefixed 形式不正
- `TemporaryBackpressure`
  - 一時的な処理飽和
- `DeviceLost`
  - デバイスロスト

## 10. 互換性チェック観点

実装や移植時は次を維持してください。

1. `submit`/`reap`/`flush` の責務分離が崩れないこと。
2. decode frame 数と `summary().decoded_frames` が一致すること。
3. encode 出力の `layout` が backend/codec 契約と一致すること。
4. 入力妥当性エラーが `InvalidInput` として表面化すること。
