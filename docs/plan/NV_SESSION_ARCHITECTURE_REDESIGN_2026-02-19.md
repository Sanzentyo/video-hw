# NV Session Architecture Redesign
日付: 2026-02-19
ブランチ: `feat/nv-p1-session-architecture-2026-02-19`

## 1. 背景

`video-hw` の NVIDIA backend は、現状 `flush` 単位で `Session` を都度生成しており、
`NV-P1-002`（セッション常駐化）の本丸が未完了である。

現行の主な課題:
- encode `Session` が `flush` ごとに初期化され、起動スパイクが残る
- session 切替が暗黙（サイズ不一致などでエラー）で、明示 API がない
- 共通設定は `max_in_flight_outputs` 中心で、GOP/RC などが contract 層で不足
- backend ごとの差分吸収（NV/VT）の境界が曖昧

参考実装として `../signage-backend/nv-encoder/src/lib.rs` は以下を持つ:
- 専用 `Session` 構造体（blocking/non-blocking）
- `Pin<Box<Session>>` + buffer ring 管理
- submit/output thread 分離
- `gop_length`, `frame_interval_p`, RC 設定の明示適用

## 2. 目標

1. encode/decode のセッションを明示的に開始・切替・終了できるようにする  
2. GOP/RC を含む共通設定を backend 非依存 contract に持ち込む  
3. backend 差分は adapter 内に閉じる（上位 API は不変）  
4. NV backend で session 常駐 + buffer 再利用を実現する  

## 3. 設計原則

- 本線（submit/reap）は停止させない
- セッション切替は generation 管理で明示化する
- 共通設定は `Common*` 構造体へ集約し、backend 固有項目は `Backend*` へ分離する
- unsupported な設定は capability negotiation で事前検出する

## 4. 提案アーキテクチャ

### 4.1 層構成

1. `SessionControl API`（公開層）
2. `SessionManager`（generation 管理・切替）
3. `BackendAdapter`（NV/VT 固有マッピング）
4. `PipelineScheduler`（submit/reap/transform）

### 4.2 新規コア構造体（概念）

```rust
pub struct EncoderSessionHandle {
    pub id: u64,
    pub generation: u64,
}

pub struct DecoderSessionHandle {
    pub id: u64,
    pub generation: u64,
}

pub enum SessionSwitchMode {
    Immediate,
    OnNextKeyframe,
    DrainThenSwap,
}

pub struct SessionSwitchRequest<T> {
    pub target: T,
    pub mode: SessionSwitchMode,
    pub force_idr_on_activate: bool,
}
```

## 5. 共通設定の再設計（Encode/Decode）

### 5.1 Encode 共通設定

```rust
pub struct CommonEncodeConfig {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub low_latency: bool,
    pub gop_length: Option<u32>,
    pub frame_interval_p: Option<i32>,
    pub b_frames: Option<u32>,
    pub rate_control: Option<RateControl>,
    pub force_idr: bool,
}

pub enum RateControl {
    Cbr { bitrate_bps: u32 },
    Vbr { avg_bps: u32, max_bps: Option<u32> },
    ConstQp { intra: u8, inter_p: u8, inter_b: u8 },
}
```

### 5.2 Decode 共通設定

```rust
pub struct CommonDecodeConfig {
    pub codec: Codec,
    pub low_latency: bool,
    pub max_display_delay: Option<u32>,
    pub prefer_zero_copy: bool,
    pub output: DecodeOutputMode,
}

pub enum DecodeOutputMode {
    MetadataOnly,
    NativeSurface,
    CpuRgb,
}
```

### 5.3 backend 固有設定

```rust
pub enum BackendEncodeConfig {
    Nvidia(NvidiaEncodeConfig),
    VideoToolbox(VtEncodeConfig),
}

pub struct NvidiaEncodeConfig {
    pub max_in_flight_outputs: usize,
    pub preset: Option<String>,
    pub tuning: Option<String>,
}
```

## 6. セッション切替モデル（明示 API）

### 6.1 ライフサイクル

- `create_session(config)` -> `Running`
- `request_switch(new_config, mode)` -> `SwitchPending`
- `activate_pending()` -> `Running(new_generation)`
- `close_session(handle)` -> `Closed`

### 6.2 切替手順

1. pending session を事前作成（可能なら prewarm）
2. `mode` に応じて切替:
   - `Immediate`: 次 frame から新 session
   - `OnNextKeyframe`: 次 IDR 境界で切替
   - `DrainThenSwap`: 現 session を drain 後に切替
3. 切替時に `force_idr_on_activate` が true なら IDR 強制
4. 旧 session の buffer/resource を明示解放

### 6.3 失敗時

- pending 作成失敗: current 継続
- activate 失敗: rollback to current generation
- telemetry へ `switch_fail_reason` を記録

## 7. NV 専用 Session 構造（signage-backend 方式の採用）

### 7.1 採用方針

`../signage-backend/nv-encoder` と同様に、NV 側は session 専用構造体を導入する。

```rust
struct NvEncodeSession {
    session: Pin<Box<nvidia_video_codec_sdk::Session>>,
    buffers: NvBufferRing,
    generation: u64,
    config: CommonEncodeConfig,
}
```

### 7.2 安全性ルール

- `Session` と `Buffer<'_>` の lifetime は `NvEncodeSession` に閉じ込める
- submit/reap thread 分離時は `Pin<Box<Session>>` + 専用ラッパーで所有権を固定
- `unsafe impl Send` を使う場合は以下 invariant を文書化:
  - session pointer が移動しない（Pin）
  - SDK 呼び出しスレッドモデルを固定
  - drop 順序（EOS -> drain -> buffer release -> session drop）を保証

## 8. 共通機能マッピング（再検討）

| 機能 | Encode/Decode | NVIDIA | VT | 共通設定化 |
|---|---|---|---|---|
| codec | E/D | 対応 | 対応 | 必須 |
| width/height | E | 対応 | 対応 | 必須 |
| fps | E | 対応 | 対応 | 必須 |
| gop_length | E | 対応 | 対応（keyframe interval） | 必須 |
| frame_interval_p | E | 対応 | 部分 | Optional |
| b_frames | E | 対応 | 部分 | Optional |
| rate control (CBR/VBR/CQP) | E | 対応 | 部分（API差） | Optional |
| force_idr | E | 対応 | 対応 | 必須 |
| low_latency | E/D | 対応 | 対応 | 必須 |
| max_display_delay | D | 対応 | 部分 | Optional |
| output mode (metadata/surface/rgb) | D | 対応 | 対応 | 必須 |

## 9. API 提案（上位）

```rust
pub trait SessionController {
    fn start_encode_session(
        &mut self,
        common: CommonEncodeConfig,
        backend: BackendEncodeConfig,
    ) -> Result<EncoderSessionHandle, BackendError>;

    fn switch_encode_session(
        &mut self,
        req: SessionSwitchRequest<(CommonEncodeConfig, BackendEncodeConfig)>,
    ) -> Result<EncoderSessionHandle, BackendError>;

    fn close_encode_session(&mut self, handle: EncoderSessionHandle) -> Result<(), BackendError>;
}
```

## 10. 実装フェーズ（NV優先）

1. contract 拡張（`CommonEncodeConfig`, `CommonDecodeConfig`）
2. `NvEncodeSession` / `NvDecodeSession` 導入
3. `NvEncoderAdapter` を session-manager 駆動へ置換
4. explicit switch API を追加
5. `PipelineScheduler` へ generation 連携
6. benchmark/verify に switch シナリオ追加

## 11. 受け入れ基準

- 同一 session で `flush` を跨いでも再初期化コストが発生しない
- `switch_encode_session` が `Immediate` / `OnNextKeyframe` / `DrainThenSwap` で動作
- GOP/RC/force-IDR 設定が contract 経由で適用される
- `--verify` を含む h264/hevc ベンチが安定して pass

## 12. このブランチでの判断

- VT 実装は Windows でコンパイル不能のため設計のみに留める
- NV では `Session` 専用構造体への移行を許容する
- `signage-backend` 方式（session struct + ring + split thread）を基盤として採用する

## 13. 関連

- `docs/plan/NV_BOTTLENECK_REMEDIATION_2026-02-19.md`
- `docs/plan/PIPELINE_TASK_DISTRIBUTION_DESIGN_2026-02-19.md`
- `../signage-backend/nv-encoder/src/lib.rs`

## 14. 実装チェックポイント（このブランチ）

- `Frame` に `force_keyframe: bool` を追加
- `NvidiaEncoderOptions` に以下を追加
  - `gop_length: Option<u32>`
  - `frame_interval_p: Option<i32>`
- `NvEncoderAdapter` で上記設定を NVENC preset へ反映
- `EncodePictureParams.encode_pic_flags` で `force_keyframe` を `NV_ENC_PIC_FLAG_FORCEIDR` にマップ
- `examples/encode_synthetic.rs` / `examples/encode_raw_argb.rs` で
  - `--nv-gop-length`
  - `--nv-frame-interval-p`
  を受け取り可能化
- `scripts/benchmark_ffmpeg_nv_precise.rs` で同オプションの pass-through を追加
- `src/nv_backend.rs` に `NvEncodeSession` を導入
  - `Pin<Box<Session>>` を保持
  - reusable input/output buffer pool を保持
  - flush 跨ぎで session/pool を再利用
  - `request_session_switch` による再構成要求時は session を再作成
  - update: `request_session_switch` は `Session::reconfigure` を優先し、失敗時のみ再作成
  - update: `pending_switch` 状態を導入し、`OnNextKeyframe` を明示的に保留適用
  - update: `active_generation` / `config_generation` / `next_generation` を導入し、switch target generation を明示管理
  - update: `VIDEO_HW_NV_SAFE_LIFETIME=1` で per-frame buffer 経路を有効化し、safe API lifetime 制約の回避経路を追加
  - update: safe lifetime 経路は flush 内ローカルプール再利用へ最適化し、per-frame create/destroy オーバーヘッドを削減
  - update: `PipelineScheduler` 側 generation 制御と `NvEncoderAdapter::sync_pipeline_generation` を接続
