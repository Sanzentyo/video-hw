# API Redesign Blueprint

更新日: 2026-02-21  
方針: 互換性は維持しない。設計品質を優先する。

## 1. ゴール

1. I/O を型で厳密化し、`layout` と `pixel format` の曖昧さを排除する。  
2. encode/decode を「submit/reap」モデルに統一し、`flush` 前提の回収制約を外す。  
3. backend 差分は adapter 内に閉じ込め、上位 API は共通契約のみ露出する。  
4. `docs/spec/TEST_SPEC_INVENTORY.md` の E2E同等性要件を満たす。

## 2. 非ゴール

1. 既存 API (`Frame`, `EncodedPacket`, `push_frame`) の互換維持。  
2. 旧 `crates/` 構成の維持。  
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

1. root の E2E を canonical にし、`crates/` 配下テストは削除する。  
2. E2E 名称は backend/機能/期待を含む命名へ統一する。  
3. 環境依存テストは `skip` 理由を明文化して早期 return する。  
4. 文言依存 assertion（`contains("...")`）は error variant assertion へ置換する。

## 10. 実装フェーズ

1. Phase A: 新 API 型と adapter trait を導入。旧 API は削除。  
2. Phase B: VT/NV backend を新 trait へ接続。  
3. Phase C: E2E を新 API へ全面移行。  
4. Phase D: `crates/` 配下の旧資産を削除。  
5. Phase E: ドキュメントを `USAGE_STRICT` と本設計に一本化。

## 11. 完了定義

1. `cargo test` で root unit/e2e が通る。  
2. `docs/spec/TEST_SPEC_INVENTORY.md` の最小受け入れ条件 1-4 を満たす。  
3. 公開 API から `layout 不明` と `decode/encode 混在 Frame` が消えている。  
4. README と docs が新 API を唯一の正として記載している。
