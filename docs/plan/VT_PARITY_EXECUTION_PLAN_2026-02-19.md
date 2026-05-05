# VT Parity Execution Plan
日付: 2026-02-19
対象: macOS + VideoToolbox（別セッション）

## 1. 目的

NVIDIA backend で到達した実装水準（submit/reap 分離、generation 制御、変換分離、比較計測基盤）を、
VideoToolbox backend へ同等レベルで適用する。

本計画は「NV を基準とした backend parity」を目的とし、単なる動作確認ではなく
性能・運用性・比較可能性まで揃える。

## 2. 前提

- 現セッション環境（Windows）では VT 実装の本体検証ができない
- 実装と計測は macOS セッションで実施する
- 上位 API 契約（`Encoder` / `Decoder` / `PipelineScheduler`）は backend 間で不変を維持する

## 3. 現状ギャップ（VT vs NV）

1. scheduler 本統合
- NV: `VIDEO_HW_NV_PIPELINE=1` で encode 本線前処理まで接続済み
- VT: decode/encode とも `PipelineScheduler` 接続済み（`VIDEO_HW_VT_PIPELINE=1`）

2. transform adapter 実体
- NV: CUDA 優先 + CPU fallback
- VT: CPU worker fallback 実装済み（NV12->RGB）
- VT: Metal compute 優先 + CPU worker fallback 実装済み（NV12->RGB）

3. session / generation 運用
- NV: `pending_switch` + generation 制御を実装
- VT: encode session 再利用は実装済み（flush 跨ぎ）
- VT: encode の session switch + generation 制御は実装済み（`Immediate` / `OnNextKeyframe` / `DrainThenSwap`）

4. 比較基盤
- NV: ffmpeg 比較（verify/equal-raw-input）を継続運用
- VT: `VIDEO_HW_VT_METRICS=1` による簡易計測を追加済み
- VT: encode/decode とも queue/jitter/copy 指標を追加済み

## 4. VT 同等化タスク（NV 1対1対応）

| NV 側項目 | VT 側対応タスク | 完了条件 |
|---|---|---|
| NV-P1-004 | `PipelineScheduler` を VT decode/encode 本線へ接続 | 完了（`VIDEO_HW_VT_PIPELINE=1`） |
| NV-P1-005 | VT TransformLayer（GPU優先 + CPU fallback） | 完了（Metal compute + CPU fallback） |
| NV-P1-006 | `VtTransformAdapter` 実体（Metal/CoreImage） | 完了（Metal compute） |
| session generation | VT 側 session switch + generation 制御 | 完了（encode 経路） |
| metrics parity | VT encode/decode に stage + queue + jitter + copy 指標 | 完了（`VIDEO_HW_VT_METRICS=1`） |
| benchmark parity | ffmpeg VT 比較スクリプトの repeat/verify/equal-input 運用 | h264/hevc で継続再現可能 |

### 2026-05-04 追記

- VT bitstream assembler は timestamped AnnexB chunk を AU 境界として確定し、AU に `pts_90k` を保持する。これにより NV decode と同じく、呼び出し側が渡した chunk PTS を backend decode timestamp に反映できる。
- VT decode は non-metadata output mode で BGRA destination image buffer を要求し、`DecodeOutputMode::Nv12` / `Rgb24` を e2e で検証する。
- `VtTransformAdapter` は passthrough stub ではなく `TransformDispatcher` worker を保持し、NV12->RGB の Metal compute 優先 + CPU worker fallback を `metal_nv12_to_rgb_matches_cpu_path` / `vt_nv12_rgb_request_runs_worker` で検証する。
- `BackendDecoderOptions::VideoToolbox` / `BackendEncoderOptions::VideoToolbox` を追加し、VT も metrics / pipeline scheduler / queue capacity を config から指定できる。環境変数 `VIDEO_HW_VT_METRICS` / `VIDEO_HW_VT_PIPELINE` / `VIDEO_HW_VT_PIPELINE_QUEUE` は fallback として維持する。
- 短時間 smoke として `benchmark_ffmpeg_vt_precise.rs --warmup 0 --repeat 1 --frame-count 30 --verify --equal-raw-input` を h264/hevc で通過。

## 5. 実装フェーズ（VTセッション）

1. VT-P2: Transform 実体化（完了）
- `VtTransformAdapter` に Metal compute 経路を実装
- CPU fallback を worker で維持し callback thread を保護

2. VT-P4: 計測基盤の同等化（完了）
- VT 経路に queue/jitter/copy 指標を追加
- 既存 NV レポートとの比較軸を一致させる

3. VT-P5: 検証固定化
- ffmpeg VT 比較を `warmup/repeat/verify/equal-raw-input` で定常化
- soak test を定期実行できるスクリプトを整備

## 6. 受け入れ基準

- VT backend で scheduler 統合経路が安定動作する
- VT transform が stub ではなく GPU 実経路で動作する
- VT session switch が generation 制御下で動作する
- h264/hevc で `ffprobe` + `ffmpeg -v error` の verify を継続通過
- NV/VT の比較レポートが同一フォーマットで取得できる

## 7. 保留タスクとの関係

- 本計画は `NV-P1-002` の追加最適化（保留）とは独立して進める
- 先に VT parity を達成し、backend 間の機能格差を縮小する
- その後、NV/VT 双方で運用タスク（P2系）を横断的に仕上げる

## 8. 次セッション開始手順（macOS）

1. `cargo check --all-targets --features backend-vt`
2. `cargo test --all-targets --features backend-vt -- --nocapture`
3. VT 比較レポート更新（`docs/status/FFMPEG_VT_COMPARISON_2026-02-19.md`）
4. VT-P2 -> VT-P5 の順で着手

## 9. 関連

- `docs/status/STATUS.md`
- `docs/plan/NV_BOTTLENECK_REMEDIATION_2026-02-19.md`
- `docs/plan/NV_SESSION_ARCHITECTURE_REDESIGN_2026-02-19.md`
- `docs/status/FFMPEG_VT_COMPARISON_2026-02-19.md`
