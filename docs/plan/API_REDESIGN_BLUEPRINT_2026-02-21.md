# API Redesign Blueprint

更新日: 2026-02-23  
方針: 互換性は維持しない。設計品質を優先する。

## 0. 現況ステータス（2026-02-23）

- `video-hw-core` / `video-hw` の workspace 分割は実施済み
- 公開 API は submit/reap/flush モデルで稼働済み
- 現行契約は decode=`Metadata` 中心、encode=`ARGB` 系中心で運用中
- 本書は「vNext API をどう仕上げるか」の設計台帳として扱う

## 1. ゴール

1. I/O を型で厳密化し、`layout` と `pixel format` の曖昧さを排除する。  
2. encode/decode を「submit/reap」モデルに統一し、`flush` 前提の回収制約を外す。  
3. backend 差分は adapter 内に閉じ込め、上位 API は共通契約のみ露出する。  
4. `docs/spec/TEST_SPEC_INVENTORY.md` の E2E同等性要件を満たす。

## 2. 非ゴール

1. 既存 API (`Frame`, `EncodedPacket`, `push_frame`) の互換維持。  
2. 現行 workspace 構成（`crates/video-hw-core` + `crates/video-hw`）の巻き戻し。  
3. mux/container 生成（MP4/MKV）を本 crate の責務にすること。

## 3. 新しい公開 API

```rust
pub enum Backend {
    VideoToolbox,
    Nvidia,
}

pub struct VideoConfig {
    pub codec: Codec,
    pub fps: u32,
    pub require_hardware: bool,
}

pub struct DecodeSession;
pub struct EncodeSession;

pub enum DecodeInput {
    AnnexBChunk { data: Vec<u8>, pts_90k: Option<i64> },
    AccessUnit { codec: Codec, nalus: Vec<Vec<u8>>, pts_90k: Option<i64> },
    LengthPrefixedSample { codec: Codec, data: Vec<u8>, pts_90k: Option<i64> },
}

pub enum DecodeOutput {
    Metadata(DecodedMetadata),
    Nv12(Nv12Frame),
    Rgb24(RgbFrame),
}

pub struct EncodeInput {
    pub dims: Dimensions,
    pub pts_90k: Option<i64>,
    pub pixel: PixelBuffer,
    pub force_keyframe: bool,
}

pub struct EncodeOutput {
    pub codec: Codec,
    pub layout: BitstreamLayout,
    pub pts_90k: Option<i64>,
    pub is_keyframe: bool,
    pub data: Vec<u8>,
}

impl DecodeSession {
    pub fn submit(&mut self, input: DecodeInput) -> Result<(), VideoError>;
    pub fn try_reap(&mut self) -> Result<Option<DecodeOutput>, VideoError>;
    pub fn reap_timeout(&mut self, timeout: std::time::Duration) -> Result<Option<DecodeOutput>, VideoError>;
    pub fn flush(&mut self) -> Result<Vec<DecodeOutput>, VideoError>;
    pub fn summary(&self) -> DecodeSummary;
}

impl EncodeSession {
    pub fn submit(&mut self, input: EncodeInput) -> Result<(), VideoError>;
    pub fn try_reap(&mut self) -> Result<Option<EncodeOutput>, VideoError>;
    pub fn reap_timeout(&mut self, timeout: std::time::Duration) -> Result<Option<EncodeOutput>, VideoError>;
    pub fn flush(&mut self) -> Result<Vec<EncodeOutput>, VideoError>;
}
```

## 4. 型設計の決定

1. `Dimensions` は `NonZeroU32` で表現し、0寸法をコンパイル時型で防ぐ。  
2. decode と encode で型を分離する（`DecodeOutput` / `EncodeInput`）。  
3. bitstream 出力は `BitstreamLayout` を必須にする。  
4. `PixelBuffer` は `Argb8888 | ArgbShared | Nv12 | Rgb24` を持つ。  
5. `VideoError` は `Unsupported | InvalidInput | InvalidBitstream | Backpressure | DeviceLost | Backend` の 6分類に固定。

## 5. 実行モデル

1. submit は「投入のみ」を責務にする。  
2. reap は non-blocking (`try_reap`) と timeout (`reap_timeout`) の二系統を提供する。  
3. `flush` は EOS/遅延フレーム回収のみを責務にする。  
4. 1 回のセッションで解像度変更を許容するかは backend ごとに再設定戦略へ委譲し、APIとしては禁止しない。  
5. backpressure は `VideoError::Backpressure` で返す。

## 6. 内部アーキテクチャ

## 6.1 モジュール境界

1. `api/`: 公開型とセッション facade。  
2. `bitstream/`: Annex-B parser / AU assembler / parameter set cache。  
3. `packer/`: `AnnexBPacker` と `LengthPrefixedPacker`。  
4. `backend/`: `vt` と `nv` の adapter 実装。  
5. `pipeline/`: scheduler / transform / generation。  
6. `metrics/`: backend 非依存の計測集約。

## 6.2 Adapter trait

```rust
trait DecoderBackend {
    fn submit_au(&mut self, au: AccessUnit) -> Result<(), VideoError>;
    fn try_reap(&mut self) -> Result<Option<DecodeOutput>, VideoError>;
    fn flush(&mut self) -> Result<Vec<DecodeOutput>, VideoError>;
    fn summary(&self) -> DecodeSummary;
}

trait EncoderBackend {
    fn submit_frame(&mut self, frame: EncodeInput) -> Result<(), VideoError>;
    fn try_reap(&mut self) -> Result<Option<EncodeOutput>, VideoError>;
    fn flush(&mut self) -> Result<Vec<EncodeOutput>, VideoError>;
    fn request_session_switch(&mut self, request: SessionSwitchRequest) -> Result<(), VideoError>;
}
```

## 7. backend 差分の固定ルール

1. VT encode 出力は `Avcc`/`Hvcc` を返す。  
2. NV encode 出力は `AnnexB` を返す（SDK raw payload の契約を明示）。  
3. VT decode は parameter set 未到達時に投入を保持し、decoder 初期化は遅延作成する。  
4. NV decode は AU 単位で `push_access_unit` へ橋渡しする。  
5. session switch は VT/NV で `Immediate | OnNextKeyframe | DrainThenSwap` を同一契約で公開する。

## 8. E2E同等性の受け入れマッピング

| 現行要件 | 新設計での検証 |
| --- | --- |
| VT decode 303 frames | `DecodeSession` + Annex-B chunk matrix で 303 |
| decode summary 一致 | `summary().decoded_frames == observed` |
| 空 flush | 入力無し `flush()` が空 |
| ARGB サイズ不正 | `EncodeSession::submit` が `InvalidInput` |
| encode packet 非空 | 30 frame submit 後 `flush` 非空 |
| PTS 単調 | reaped `EncodeOutput` の `pts_90k` non-decreasing |
| unsupported backend | capability=false + `Unsupported` |
| session switch 呼び出し可能 | VT/NV で `request_session_switch` が `Ok` |

## 9. テスト再編方針

1. `crates/video-hw/tests/e2e_video_hw.rs` を canonical E2E とする。  
2. E2E 名称は backend/機能/期待を含む命名へ統一する。  
3. 環境依存テストは `skip` 理由を明文化して早期 return する。  
4. 文言依存 assertion（`contains("...")`）は error variant assertion へ置換する。

## 10. 実装フェーズ

1. Phase A（完了）: API 型分離・adapter 方針の土台整備、workspace 分割。  
2. Phase B（完了）: VT/NV backend を submit/reap/flush モデルへ接続。  
3. Phase C（完了）: E2E を `crates/video-hw/tests/e2e_video_hw.rs` 中心に移行。  
4. Phase D（進行中）: docs drift 解消（deepresearch と現行実装の時点差管理）。  
5. Phase E（未着手）: vNext API（`DecodeInput/DecodeOutput/EncodeInput/EncodeOutput`）へ公開面を一本化。  

## 11. 次にやる実装タスク（この設計から逆算）

1. encode 入力契約の明確化（完了）
- `RawFrameBuffer::Nv12/Rgb24` を `unstable-raw-inputs` feature で隔離
- 既存経路での `InvalidInput` 契約を維持

2. decode 出力モードの明示 API 導入（Phase 2 完了）
- `DecoderConfig.output_mode` を導入し、Metadata 運用を API 契約として明示
- `Rgb24` は backend ARGB payload がある場合の変換経路を追加
- `Nv12` は backend ARGB payload がある場合の変換経路を追加
- backend payload 非提供時は `UnsupportedConfig` を返す契約で明示

3. エラー契約テストの強化（Phase 1 完了）
- E2E の主要判定を文字列一致から error variant 判定へ移行
- `UnsupportedConfig` / `DeviceLost` を runtime unavailable 判定として共通化
- ライブラリ側に `BackendError::kind()` / `is_runtime_unavailable()` を導入
- 詳細な skip reason taxonomy は次フェーズで整備する

4. `reap_timeout` の backend 直接 poll 連携（Phase 1 完了）
- `VideoDecoder::try_reap` / `VideoEncoder::try_reap` を導入
- `DecodeSession::reap_timeout` は内部 ready queue + backend poll で待機
- NV decode は `NvMetaDecoder::try_drain` で non-EOS drain をサポート

## 12. 完了定義

1. `cargo test -p video-hw` で unit/e2e が通る。  
2. `docs/spec/TEST_SPEC_INVENTORY.md` の最小受け入れ条件 1-4 を満たす。  
3. 公開 API から `layout 不明` と `decode/encode 混在 Frame` が消えている。  
4. README と docs が新 API を唯一の正として記載している。
