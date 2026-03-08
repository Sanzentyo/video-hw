# video-hw Status

更新日: 2026-02-23

## 1. 現在の構成（実装実態）

- workspace 構成へ移行済み
  - `crates/video-hw-core`: 共通型・エラー・契約
  - `crates/video-hw`: facade + VT/NV backend 実装
- backend は `video-hw` crate の feature + target で有効化
  - `backend-vt`: macOS
  - `backend-nvidia`: Linux / Windows

## 2. 実装済み（確認済み）

- 公開APIは `DecodeSession` / `EncodeSession` の submit/reap/flush モデル
- `reap_timeout` は timeout 上限まで待機する契約で実装済み
- decode 入力は `BitstreamInput`（Annex-B chunk / raw NAL AU / length-prefixed sample）
- encode 入力は `EncodeFrame`
- backend 切替は `Backend`（`Auto` / `VideoToolbox` / `Nvidia`）
- VT/NV とも session switch API を公開（`request_session_switch`）
- `PipelineScheduler` と generation 制御を VT/NV encode 経路に接続

## 3. deepresearch 由来で再確認した主要ギャップ

1. 公開契約のねじれ
- `DecodedFrame` は `Metadata/Nv12/Rgb24` を持つ
- `DecoderConfig.output_mode` を導入
- `Metadata`: 実装済み
- `Rgb24`: backend ARGB payload がある場合の変換経路を実装
- `Nv12`: backend ARGB payload がある場合の変換経路を実装
- backend が ARGB payload を返さない条件では `UnsupportedConfig`
- `RawFrameBuffer` の `Nv12/Rgb24` は `unstable-raw-inputs` feature 有効時のみ公開
- `EncodeSession::submit` は現状 ARGB 系のみ受理（`Nv12/Rgb24` は `InvalidInput`）

2. transform 実装差
- `VtTransformAdapter` は現状 pass-through 実装（GPU transform 実体は未接続）
- VT decode callback は `output_mode != Metadata` 時に BGRA pixel buffer から ARGB payload を抽出可能
- `NvidiaTransformAdapter` の NV12->RGB worker 経路はテスト条件での検証中心

3. 配布・運用整備
- CI 導入は後回し（現時点で `.github/workflows` は未運用）
- backend 別 CI（VT 実機 / NVIDIA 実機）の分離運用は未整備
- NVIDIA backend は `nvidia-video-codec-sdk`（ラッパー）経由で SDK を利用
- SDK 本体は各利用者が別途取得し、環境変数でパス指定する前提

## 4. 本日実測した検証結果（この環境）

- `cargo test -- --nocapture`: pass
  - unit: 10 passed
  - integration: 1 passed（`e2e_build_without_enabled_backends_compiles`）
- `cargo check --all-targets --features backend-nvidia`: pass
- `cargo test --features backend-nvidia -- --nocapture`: pass
  - unit: 21 passed
  - integration: 13 passed
- `cargo test --all-features -- --nocapture`: pass
  - unit: 21 passed
  - integration: 13 passed
- `cargo bench -p video-hw --features backend-nvidia --bench decode_bench`: pass
  - H264 hw_optional chunk_4096: 223.60-231.97 ms
  - H264 hw_required chunk_1048576: 225.44-233.16 ms
  - HEVC hw_optional chunk_1048576: 229.68-237.62 ms
  - HEVC hw_required chunk_1048576: 223.12-232.26 ms
- `cargo deny check licenses advisories bans sources`: pass

注記:
- `backend-vt` は target 条件上、非 macOS 環境では VT 本体テストは有効化されない

## 5. 直近優先タスク

1. docs drift 是正
- `docs/spec/TEST_SPEC_INVENTORY.md` と実装テストを完全同期
- 本 `STATUS.md` / `docs/plan/ROADMAP.md` / `docs/README.md` の整合維持

2. 公開契約の明確化
- decode 出力モード（Metadata 固定か、将来の NV12/RGB を含むか）を仕様化
- encode 入力型（ARGB 以外）の扱いを `unstable` か明示 reject のいずれかに整理

3. 配布準備
- CI の導入タイミング見直し（当面は手動検証を継続）
- NVIDIA 依存の配布ポリシー（同梱可否・再配布条件）の明文化強化
- `cargo-deny` 運用の継続（依存更新時の定期実行）

## 6. 関連文書

- `docs/research/deepresearch.md`
- `docs/spec/NVIDIA_SDK_DISTRIBUTION_POLICY.md`
- `docs/spec/TEST_SPEC_INVENTORY.md`
- `docs/plan/ROADMAP.md`
- `docs/plan/NEXT_ACTION_PLAN_2026-02-23.md`
