# I/O Format Contract（Binary + Type Level）

更新日: 2026-02-21

## 1. この文書の目的

`video-hw` の入出力契約を、以下の 2 層で明確化する。

- バイナリ形式（wire/byte layout）
- 型レベル形式（Rust API での表現）

本書は、`README.md` / `docs/USAGE_STRICT.md` / `docs/status/STATUS.md` の現行仕様を前提に、公開I/O契約を定義する。

## 2. プロジェクト概要（ドキュメント要約）

- 単一 crate で VideoToolbox / NVIDIA を実行時 `BackendKind` で切替。
- decode は Annex-B chunk 入力を受け、内部で AU 組み立てを行う。
- encode は `Frame` 入力を受け、backend raw payload を返す。
- 現行 decode 出力はメタデータ中心（`Frame.argb = None`）。
- transform 系では `Nv12Frame` / `RgbFrame` が別型で実装済み。

## 3. 想定ユースケースと必要I/O

| ユースケース | 入力形式 | 出力形式 |
| --- | --- | --- |
| U1: オフライン transcode（ES->ES） | Annex-B / ARGB | Annex-B / AVCC/HVCC |
| U2: 低遅延配信（1AU単位） | Access Unit（raw NAL） | layout 明示済み packet |
| U3: 解析用途（metadata only） | Annex-B | metadata frame |
| U4: 画素変換パイプライン | NV12 | RGB24 / RGBA |
| U5: muxer 連携（MP4/MKV 等） | ARGB/NV12 + codec | AVCC/HVCC or Annex-B 明示 |
| U6: 将来 zero-copy 拡張 | 共有バッファ/外部ハンドル | 共有/所有を選べる packet |

この 6 用途を満たすため、本書では 13 種の I/O フォーマットを規定する。

## 4. バイナリ形式（Binary Contract）

### 4.1 Bitstream 入力

| ID | 形式 | 定義 | 用途 | 現行実装 |
| --- | --- | --- | --- | --- |
| `BIN-BS-01` | Annex-B byte stream | `00 00 01` または `00 00 00 01` 区切り | U1/U3 | 対応済み（decode入口） |
| `BIN-BS-02` | Access Unit / raw NAL list | NAL は生 bytes（prefix 無し） | U2 | 内部表現で対応済み |
| `BIN-BS-03` | length-prefixed sample | 各 NAL が `u32be length + payload` | U2/U5 | VT 側 pack/unpack で対応 |

### 4.2 Raw frame 入力

| ID | 形式 | 定義 | 用途 | 現行実装 |
| --- | --- | --- | --- | --- |
| `BIN-RF-01` | ARGB8888 packed | 1 pixel = 4 bytes (A,R,G,B), `len = w*h*4` | U1/U5 | 対応済み（encode入力） |
| `BIN-RF-02` | NV12 pitch-linear | Y plane + interleaved UV, `len >= pitch*h*3/2` | U4/U6 | transform層で対応済み |
| `BIN-RF-03` | RGB24 packed | 1 pixel = 3 bytes (R,G,B) | U4 | transform出力で対応済み |

### 4.3 Encode 出力

| ID | 形式 | 定義 | 用途 | 現行実装 |
| --- | --- | --- | --- | --- |
| `BIN-EP-01` | Annex-B互換 packet | start code 付き NAL 連結（NVENC raw想定） | U1/U2 | NV encode 出力（SDK raw payload） |
| `BIN-EP-02` | AVCC packet | 4-byte BE length-prefix NAL 列 | U5 | VT H.264 encode 出力 |
| `BIN-EP-03` | HVCC packet | 4-byte BE length-prefix NAL 列 | U5 | VT HEVC encode 出力 |
| `BIN-EP-04` | Opaque packet | backend raw（layout不明時の退避） | U6 | 将来/互換用 |

### 4.4 Decode 出力

| ID | 形式 | 定義 | 用途 | 現行実装 |
| --- | --- | --- | --- | --- |
| `BIN-DF-01` | metadata frame | width/height/pts 等のみ | U3 | 対応済み（標準） |
| `BIN-DF-02` | NV12 frame | pitch 付き生 NV12 | U4/U6 | transform経路で対応 |
| `BIN-DF-03` | RGB frame | RGB24 または RGBA | U4 | transform経路で対応 |

## 5. 型レベル形式（Type Contract）

以下は現行 API の型レベル契約である。

```rust
pub struct Dimensions {
    pub width: std::num::NonZeroU32,
    pub height: std::num::NonZeroU32,
}

pub struct Timestamp90k(pub i64);

pub enum BitstreamInput<'a> {
    AnnexBChunk(&'a [u8]),                    // BIN-BS-01
    AccessUnitRawNal {                        // BIN-BS-02
        codec: Codec,
        nalus: Vec<&'a [u8]>,
        pts: Option<Timestamp90k>,
    },
    LengthPrefixedSample {                    // BIN-BS-03
        codec: Codec,
        sample: &'a [u8],
        pts: Option<Timestamp90k>,
    },
}

pub enum RawFrameBuffer {
    Argb8888(Vec<u8>),                        // BIN-RF-01
    Argb8888Shared(std::sync::Arc<[u8]>),     // U6 (copy削減)
    Nv12 { pitch: usize, data: Vec<u8> },     // BIN-RF-02
    Rgb24(Vec<u8>),                           // BIN-RF-03
}

pub struct EncodeFrame {
    pub dims: Dimensions,
    pub pts: Option<Timestamp90k>,
    pub buffer: RawFrameBuffer,
    pub force_keyframe: bool,
}

pub enum EncodedLayout {
    AnnexB,                                   // BIN-EP-01
    Avcc,                                     // BIN-EP-02
    Hvcc,                                     // BIN-EP-03
    Opaque,                                   // BIN-EP-04
}

pub struct EncodedChunk {
    pub codec: Codec,
    pub layout: EncodedLayout,
    pub pts: Option<Timestamp90k>,
    pub is_keyframe: bool,
    pub data: Vec<u8>,
}

pub enum DecodedFrame {
    Metadata {
        dims: Dimensions,
        pts: Option<Timestamp90k>,
        pixel_format: Option<u32>,
        decode_info_flags: Option<u32>,
        color: Option<ColorMetadata>,
    },                                        // BIN-DF-01
    Nv12 {
        dims: Dimensions,
        pitch: usize,
        pts: Option<Timestamp90k>,
        data: Vec<u8>,
    },                                        // BIN-DF-02
    Rgb24 {
        dims: Dimensions,
        pts: Option<Timestamp90k>,
        data: Vec<u8>,
    },                                        // BIN-DF-03
}

pub struct ColorMetadata {
    pub color_primaries: Option<i32>,
    pub transfer_function: Option<i32>,
    pub ycbcr_matrix: Option<i32>,
}
```

## 6. 現行 API との対応

| 現行型 | 問題点 | 対応方針 |
| --- | --- | --- |
| `Frame { pixel_format: Option<u32>, argb: Option<Vec<u8>> }` | 意味が多重（decode metadata と encode raw を同居） | `DecodedFrame` / `EncodeFrame` に分離 |
| `EncodedPacket { data: Vec<u8> }` | layout が型上で不明 | `EncodedChunk.layout` を必須化 |
| `push_bitstream_chunk(&[u8], pts)` | Annex-B 専用で拡張しにくい | `BitstreamInput` を受ける追加 API を導入 |

## 7. 方向性（決定事項）

1. I/O は常に「layout 明示」を原則とする（特に encode 出力）。
2. `Frame` の単一型運用は互換維持しつつ、用途別型へ段階移行する。
3. decode は metadata fast-path を維持し、必要時のみ画素 payload を返す。
4. 共有バッファ (`Arc<[u8]>`) を first-class にし、copy 削減経路を確保する。

## 8. 実装方針

1. `layout` 明示を必須とし、曖昧 payload を公開型から排除する。
2. decode/encode で型を分離し、`Frame` 多義性を持ち込まない。
3. submit/reap/flush 契約で I/O のタイミングを明示する。

## 9. 参照

- `README.md`
- `docs/USAGE_STRICT.md`
- `docs/status/STATUS.md`
- `docs/plan/NV_RAW_INPUT_ZERO_COPY_CONTRACT_2026-02-19.md`
