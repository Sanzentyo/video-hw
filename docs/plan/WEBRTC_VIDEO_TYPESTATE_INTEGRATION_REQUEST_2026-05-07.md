# WebRTC video integration request: typestate and newtype API additions

作成日: 2026-05-07
背景リポジトリ: `../webrtc-video`
前提: `video-hw` / `video-hw-fmp4` の公開 API は typestate pattern と new type pattern を積極採用する。

## 1. 背景

`webrtc-video` は `video-hw` を使って camera capture -> hardware encode -> WebRTC RTP -> relay decode/re-encode -> return decode/preview を行っている。
この実装を俯瞰すると、WebRTC/signaling/GUI 以外の処理として、次の video bitstream 周辺ロジックが上位アプリ側に残っている。

- H.264/HEVC の Annex-B NALU split
- AVCC/HVCC length-prefixed sample から Annex-B への変換
- RTP depacketize 後の access unit 組み立て
- `EncodedChunk` の `EncodedLayout` ごとの NALU 化
- dump/再送信用の Annex-B 正規化
- `DecodedFrame::Nv12` / `Rgb24` から GUI/再エンコード用 RGBA/ARGB への変換
- `DecodeOutputMode::Nv12` が実際に pixel payload を返せない場合の fallback 判断

一方、`video-hw-fmp4` にはすでに `EncodedSample::to_annexb()`、`EncodedSample::to_decode_payload()`、sample entry description、parameter set 抽出、reader decode diagnostics がある。
そのため、新規に別系統の高水準 API を増やすより、既存 `video-hw-core` / `video-hw` / `video-hw-fmp4` 内部の bitstream/sample utility を public API として整理し、typestate session の入口を追加するのがよい。

## 2. 設計原則

1. typestate は session lifecycle と「構成済み/未構成」の境界に使う。
2. newtype は codec payload、時刻、寸法、長さ、sample id など意味のある値に積極採用する。
3. hot path で newtype が余計な allocation/copy を生む場合は、borrowed view / zero-copy option / validation level を用意する。
4. WebRTC/RTP/signaling/GUI は `video-hw` の責務にしない。
5. fMP4/container は引き続き `video-hw-fmp4` の責務にする。

## 3. ゴール

1. `webrtc-video` 側の H.264/HEVC/AV1 bitstream utility 重複を `video-hw-core` へ寄せる。
2. `EncodedChunk` と `EncodedSample` の変換 API を揃える。
3. `video-hw-fmp4` writer が raw frame だけでなく encoded stream も typestate session として受け取れるようにする。
4. live/WebRTC decode で pixel output が利用可能かを事前に判断しやすくする。
5. newtype によって `pts_90k`、RTP timestamp、NAL length size、Annex-B AU、length-prefixed sample の混同を防ぐ。

## 4. 非ゴール

- WebRTC signaling を `video-hw` に入れない。
- RTP packetizer/depacketizer 自体を `video-hw` に入れない。
- `webrtc-video` 固有の telemetry schema や GUI preview 型を `video-hw` に入れない。
- frame ごとの hot path API を過剰な consuming typestate にしない。
- `video-hw-fmp4` を `video-hw-core` に統合しない。

## 5. 現状の重複箇所

`webrtc-video` 側:

- `src/h26x.rs`
  - `avcc_or_hvcc_to_annexb`
  - `split_annexb_nalus`
  - `split_annexb_access_units`
  - `encoded_payload_to_nalus`
  - `to_annexb_for_dump`
  - H.264/HEVC NAL classification
- `src/bin/webrtc-h26x-relay-reencode-gui-main.rs`
  - RTP depacketize chunk -> pending access unit
  - `EncodedChunk` -> RTP packetize 用 NALU list
  - decode pixel output 不在時の fallback 判断
- `src/bin/camera-webrtc-h26x-roundtrip-gui-main.rs`
  - encoded output -> NALU list
  - returned RTP -> pending access unit -> decode
- `src/camera_convert.rs`
  - pitch 付き NV12 -> ARGB/RGBA
  - RGBA -> ARGB
  - BT.601/BT.709 limited-range conversion

`video-hw` 側にも private utility がある:

- `crates/video-hw/src/lib.rs`
  - `normalize_bitstream_input`
  - `pack_access_unit_nalus_to_annexb`
  - `unpack_length_prefixed_sample_to_annexb`
- `crates/video-hw-fmp4/src/fmp4_writer/core.rs`
  - Annex-B / AVCC / HVCC / AV1 sample conversion
  - parameter set 抽出
  - sample entry 生成
- `crates/video-hw-fmp4/src/fmp4_reader/config.rs`
  - `EncodedSample::to_annexb()`
  - `EncodedSample::to_decode_payload()`

## 6. 要求 API

### 6.1 `video-hw-core::bitstream`

`video-hw-core` に backend 非依存の public module を追加する。
すべて生 `Vec<u8>` / `&[u8]` だけで受け渡さず、意味のある newtype を用意する。

想定型:

```rust
pub struct AnnexBAccessUnit(Vec<u8>);
pub struct AnnexBAccessUnitRef<'a>(&'a [u8]);
pub struct NalUnit(Vec<u8>);
pub struct NalUnitRef<'a>(&'a [u8]);
pub struct LengthPrefixedSample(Vec<u8>);
pub struct LengthPrefixedSampleRef<'a>(&'a [u8]);
pub struct ParameterSets { /* H264/H265/AV1 specific storage */ }
pub enum NalLengthSize { One, Two, Four }
```

想定 API:

```rust
pub fn split_annexb_nalus(data: AnnexBAccessUnitRef<'_>) -> Result<Vec<NalUnitRef<'_>>, BitstreamError>;
pub fn annexb_to_length_prefixed(data: AnnexBAccessUnitRef<'_>, nal_length_size: NalLengthSize) -> Result<LengthPrefixedSample, BitstreamError>;
pub fn length_prefixed_to_annexb(data: LengthPrefixedSampleRef<'_>, nal_length_size: NalLengthSize) -> Result<AnnexBAccessUnit, BitstreamError>;
pub fn append_annexb_nalu(out: &mut Vec<u8>, nalu_or_start_coded_nalu: &[u8]);
pub fn split_access_units(codec: Codec, annexb: AnnexBAccessUnitRef<'_>) -> Result<Vec<AnnexBAccessUnit>, BitstreamError>;
pub fn extract_parameter_sets(codec: Codec, payload: EncodedPayloadRef<'_>) -> Result<ParameterSets, BitstreamError>;
```

効率オプション:

```rust
pub struct BitstreamParseOptions {
    pub validation: ValidationLevel,
    pub copy_policy: CopyPolicy,
}

pub enum ValidationLevel {
    Full,
    StructuralOnly,
    TrustCaller,
}

pub enum CopyPolicy {
    BorrowWhenPossible,
    AlwaysOwned,
}
```

`TrustCaller` は `unsafe` API にしない。名前通り検証を減らすだけで、panic/UB を発生させない範囲に限定する。
borrowed view は lifetime で安全性を保ち、owned が必要な場面では明示的に `to_owned_access_unit()` などへ進める。

### 6.2 typestate access unit assembler

RTP depacketizer から得た chunk を access unit としてまとめる helper を提供する。
RTP 自体は扱わず、「chunk が来た」「marker 相当で AU を閉じる」だけを扱う。

```rust
pub struct AccessUnitAssembler<State> { /* ... */ }
pub struct Idle;
pub struct Collecting;

impl AccessUnitAssembler<Idle> {
    pub fn new(codec: Codec, options: AccessUnitAssemblerOptions) -> Self;
    pub fn push_chunk(self, chunk: NalUnitRef<'_>, pts_90k: Option<Timestamp90k>) -> AccessUnitAssembler<Collecting>;
}

impl AccessUnitAssembler<Collecting> {
    pub fn push_chunk(&mut self, chunk: NalUnitRef<'_>);
    pub fn finish(self) -> Result<(AnnexBAccessUnit, AccessUnitAssembler<Idle>), BitstreamError>;
    pub fn discard(self) -> AccessUnitAssembler<Idle>;
}
```

hot path で毎 packet 所有権遷移が重い場合は、`push_chunk(&mut self)` 型の alternative API を追加してよい。
ただし「codec 未設定で chunk を入れられない」「空の AU を finish できない」境界は型か明確な typed error で表現する。

効率オプション:

```rust
pub struct AccessUnitAssemblerOptions {
    pub preallocate_bytes: Option<usize>,
    pub max_access_unit_bytes: Option<usize>,
    pub copy_policy: CopyPolicy,
}
```

### 6.3 `EncodedChunk` conversion API

`video_hw::EncodedChunk` に `video-hw-fmp4::EncodedSample` と近い変換 API を追加する。

```rust
impl EncodedChunk {
    pub fn payload_ref(&self) -> EncodedPayloadRef<'_>;
    pub fn to_annexb(&self) -> Result<AnnexBAccessUnit, BitstreamError>;
    pub fn to_nalus(&self) -> Result<Vec<NalUnit>, BitstreamError>;
    pub fn to_decode_payload(&self) -> Result<DecodePayload, BitstreamError>;
    pub fn to_length_prefixed_sample(&self, nal_length_size: NalLengthSize) -> Result<LengthPrefixedSample, BitstreamError>;
}
```

注意:

- `EncodedLayout::Av1` は Annex-B ではない。互換上 passthrough にする API と、正式な `to_decode_payload()` の違いを docs に明記する。
- `Opaque` は必ず typed error。
- `EncodedChunk` は sample entry を持たないため、keyframe 時の SPS/PPS/VPS 前置が必要な API では `ParameterSets` newtype を別引数で受ける overload も検討する。

### 6.4 `video-hw-fmp4` encoded writer session

現行 public writer は raw frame input の typestate session として設計されている。

```rust
Fmp4Writer<Ready> -> Fmp4Writer<SyncRecording> -> Fmp4Writer<Finished>
```

これに encoded input 用の session を追加する。

```rust
pub struct EncodedTrackConfig {
    pub output_path: PathBuf,
    pub frame_size: FrameSize,
    pub frame_rate: FrameRate,
    pub codec: Codec,
    pub fragment_frames: FragmentFrames,
    pub initial_parameter_sets: Option<ParameterSets>,
}

pub struct SyncEncodedRecording;
pub struct AsyncEncodedRecording;

impl Fmp4Writer<Ready> {
    pub fn into_sync_encoded_session(self, config: EncodedTrackConfig) -> Result<Fmp4Writer<SyncEncodedRecording>>;
}

impl Fmp4Writer<SyncEncodedRecording> {
    pub fn write_encoded_chunk(&mut self, chunk: EncodedChunk, duration_90k: Option<SampleDuration90k>) -> Result<()>;
    pub fn write_encoded_sample(&mut self, sample: EncodedSampleInput, duration_90k: Option<SampleDuration90k>) -> Result<()>;
    pub fn finish(self) -> Result<Fmp4Writer<Finished>>;
}
```

newtype 要件:

- `SampleDuration90k`
- `CompositionOffset90k`
- `TrackTimescale`
- `NalLengthSize`
- `ParameterSets`
- `EncodedSampleInput`

typestate 方針:

- `Ready` から raw recording と encoded recording を明確に分岐する。
- `SyncRecording` に encoded 書き込みを混ぜない。
- parameter set が未観測の状態は runtime state として持つ。
- frame ごとに `NeedParameterSets -> Writing` を所有権遷移させる API は避ける。
- `finish()` 時に sample entry が確定していなければ typed error にする。

### 6.5 decoded frame conversion

`DecodedFrame` / `Nv12Frame` 周辺に color conversion helper を追加する。

```rust
pub enum PixelOutputLayout {
    Rgb24,
    Rgba8888,
    Argb8888,
    Bgra8888,
}

pub struct ColorConvertOptions {
    pub matrix: ColorMatrix,
    pub range: ColorRange,
}

impl DecodedFrame {
    pub fn to_pixel_buffer(&self, layout: PixelOutputLayout, options: ColorConvertOptions) -> Result<PixelBufferOwned, BackendError>;
    pub fn try_as_nv12(&self) -> Option<Nv12FrameRef<'_>>;
}
```

newtype 要件:

- `PixelBufferOwned`
- `Nv12FrameRef<'a>`
- `PitchBytes`
- `PixelWidth`
- `PixelHeight`
- `ColorMatrix`
- `ColorRange`

効率オプション:

- `try_as_nv12()` / `try_as_rgb24()` の borrowed view を用意する。
- 変換が不要な場合は copy しない。
- `to_pixel_buffer()` は owned output が必要な GUI/encode path 用とする。
- SIMD/GPU 高速化は後続計画でよいが、API は `ColorConvertOptions` に backend-independent に載せる。

### 6.6 live decode preflight diagnostics

`video-hw-fmp4` reader には `DecodeDiagnostics` があるが、live decode の `AnyDecodeSession` でも pixel output availability を判断したい。

```rust
pub struct DecodePreflightRequest {
    pub backend: Backend,
    pub codec: Codec,
    pub output_mode: DecodeOutputMode,
    pub require_hardware: bool,
}

pub struct DecodePreflightReport {
    pub requested_backend: Backend,
    pub resolved_backend: Option<BackendKind>,
    pub output_mode: DecodeOutputMode,
    pub supported_by_contract: bool,
    pub usable_in_current_runtime: bool,
    pub reason: Option<String>,
}
```

`webrtc-video` の relay は `DecodeOutputMode::Nv12` を要求し、pixel payload が出なければ fallback へ移る。
この判断を文字列 error や「数 frame decode して `argb` が出ない」推測に寄せないため、structured preflight を提供する。

## 7. typestate / newtype 運用方針

typestate の対象:

1. session lifecycle
   - `Ready`
   - `SyncRecording`
   - `SyncEncodedRecording`
   - `AsyncRecording`
   - `AsyncEncodedRecording`
   - `Finished`
2. codec/config 未設定と設定済みの境界
   - `AccessUnitAssembler<Idle>`
   - `AccessUnitAssembler<Collecting>`
3. reader/writer の open 前/処理中/finish 後の境界

newtype の対象:

1. 時刻
   - `Timestamp90k`
   - `SampleDuration90k`
   - `CompositionOffset90k`
   - `MediaTime`
2. サイズ
   - `Dimensions`
   - `FrameSize`
   - `PitchBytes`
   - `NalLengthSize`
3. payload
   - `AnnexBAccessUnit`
   - `LengthPrefixedSample`
   - `NalUnit`
   - `DecodePayload`
   - `EncodedPayloadRef`
4. codec config
   - `ParameterSets`
   - `Av1ConfigObus`
   - `H264ParameterSets`
   - `HevcParameterSets`

typestate にしないもの:

- RTP packet 1個ごとの状態遷移
- encoded chunk 1個ごとの state ownership
- parameter set 観測済み/未観測の全 frame 消費 API

理由:

- live video hot path では `&mut self` で状態を進める API の方が現実的。
- typestate を細かくしすぎると、RTP の欠損・空 chunk・marker only packet のような runtime 現象を型に押し込みすぎる。
- typestate は lifecycle の誤用を防ぐ用途に留める。

## 8. 効率に関する要求

newtype によって安全性を上げる一方、以下の option を用意して効率悪化を避ける。

1. borrowed view と owned 型を両方用意する。
   - `AnnexBAccessUnitRef<'a>` / `AnnexBAccessUnit`
   - `NalUnitRef<'a>` / `NalUnit`
   - `LengthPrefixedSampleRef<'a>` / `LengthPrefixedSample`
2. validation level を選べるようにする。
   - `Full`
   - `StructuralOnly`
   - `TrustCaller`
3. copy policy を選べるようにする。
   - `BorrowWhenPossible`
   - `AlwaysOwned`
4. hot path の assembler は preallocation を受けられる。
5. `EncodedChunk` からの `payload_ref()` は allocation しない。
6. fMP4 writer の encoded path は、sample entry 生成に必要な parameter set だけを抽出し、payload 全体の不要な再コピーを避ける。

## 9. `webrtc-video` 側の移行イメージ

移行後、`webrtc-video` 側から削れるもの:

- `src/h26x.rs` の大半
- `encoded_chunk_to_nalus`
- `to_annexb_for_dump`
- RTP depacketized chunk の raw `Vec<u8>` 組み立て補助
- pitch 付き NV12 -> ARGB/RGBA の独自実装の一部

残すもの:

- WebRTC signaling
- RTP packetizer/depacketizer の crate 選定と呼び出し
- GUI preview
- telemetry
- relay policy
- fallback strategy の最終判断

## 10. 受け入れ条件

1. `video-hw-core` に bitstream utility が public API として追加される。
2. 意味のある payload/time/size 値には newtype が採用される。
3. newtype による allocation/copy 増加を避ける borrowed view / option が用意される。
4. `EncodedChunk` と `EncodedSample` の変換 API の命名・意味が揃う。
5. `video-hw-fmp4` に encoded input 用 typestate session が追加される。
6. raw frame writer 既存 API は維持する。
7. `webrtc-video` の H26x utility を削っても camera roundtrip / relay roundtrip が成立する。
8. H.264 / HEVC / AV1 の `EncodedLayout` ごとの error contract が明文化される。
9. `Opaque` layout は明示 error になる。
10. docs に `video-hw-fmp4` は container、`video-hw-core::bitstream` は codec payload utility、WebRTC/RTP は上位アプリ責務であることを明記する。

## 11. 実装順序

1. `video-hw-core::bitstream` を追加し、既存 private helper をそこへ移す。
2. newtype と borrowed view の基本型を追加する。
3. `EncodedChunk` conversion API を追加する。
4. `video-hw-fmp4::EncodedSample` を `video-hw-core::bitstream` 実装へ寄せる。
5. `Fmp4Writer<SyncEncodedRecording>` を追加し、内部 `handle_chunk` 相当を public session から使えるようにする。
6. `DecodedFrame` conversion helper を追加する。
7. live decode preflight diagnostics を追加する。
8. `webrtc-video` を新 API に移行して重複実装を削る。

## 12. 注意点

- `video-hw-fmp4::EncodedSample` は fMP4 sample entry を持てるが、`video_hw::EncodedChunk` は encoder output なので sample entry を持たない。この差分は API 名で明確にする。
- AV1 は Annex-B ではない。互換名としての `to_annexb()` を増やす場合は、AV1 payload passthrough の意味を docs に明記する。
- H.264/HEVC keyframe 判定は backend の `is_keyframe` だけで足りない場合がある。IDR/CRA/BLA など codec-specific frame type を将来拡張できる型にする。
- color conversion はまず CPU helper でよい。Metal/Vulkan/CUDA などの高速化は別計画にする。
