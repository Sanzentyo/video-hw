# video-hw Status

更新日: 2026-06-25

## 1. 現在の構成（実装実態）

- workspace 構成へ移行済み
  - `crates/video-hw-core`: 共通型・エラー・契約
  - `crates/video-hw`: facade + backend adapter 統合
- backend は `video-hw` crate の feature + target で有効化
  - `backend-android`: Android
  - `backend-vt`: macOS
  - `backend-nvidia`: Linux / Windows
  - `backend-intel`: Linux / Windows
  - `backend-vulkan`: Linux / Windows

## 2. 実装済み（確認済み）

- 公開APIは `DecodeSession` / `EncodeSession` の submit/reap/flush モデル
- `reap_timeout` は timeout 上限まで待機する契約で実装済み
- decode 入力は `BitstreamInput`（Annex-B chunk / raw NAL AU / length-prefixed sample）
- encode 入力は `EncodeFrame`
- backend 切替は `Backend`（`Auto` / `Android` / `VideoToolbox` / `Nvidia` / `Intel` / `Vulkan`）
- VT/NV とも session switch API を公開（`request_session_switch`）
- `PipelineScheduler` と generation 制御を VT/NV encode 経路に接続

## 3. deepresearch 由来で再確認した主要ギャップ

1. 公開契約のねじれ
- `DecodedFrame` は `Metadata/Nv12/Rgb24` を持つ
- `DecoderConfig.output_mode` を導入
- `Metadata`: 実装済み
- `Rgb24`: backend ARGB/NV12 payload がある場合の変換経路を実装
- `Nv12`: backend ARGB/NV12 payload がある場合の変換経路を実装
- Android decoder は現時点では `Metadata` のみを公開契約として広告し、`video_width` / `video_height` side data を要求する
- backend が ARGB payload を返さない条件では `UnsupportedConfig`
- `RawFrameBuffer` は `Argb8888/Argb8888Shared/Nv12` を公開
- `EncoderConfig.input_format` と投入 frame の形式一致を検証
- VideoToolbox は ARGB のみ、NVIDIA / Intel / Vulkan は ARGB/NV12 を契約入力として扱う
- Android encoder は ARGB または tightly packed NV12 を受ける。pitch 付き NV12 の汎用 repack は未公開契約
- 非対応または payload 欠落時は synthetic 画像へ置き換えず `InvalidInput` / `UnsupportedConfig`

2. transform 実装差
- `VtTransformAdapter` は Metal compute 優先 + CPU worker fallback の NV12->RGB 経路を持つ
- VT decode callback は `output_mode != Metadata` 時に BGRA pixel buffer から ARGB payload を抽出可能
- `NvidiaTransformAdapter` は CUDA 優先 + CPU worker fallback、VT は Metal 優先 + CPU worker fallback で transform adapter の運用形を揃えている

3. 配布・運用整備
- GitHub CI は導入しない方針。backend 別の確認は実機で手動コマンドを実行して記録する
- NVIDIA backend は `nvidia-video-codec-sdk`（ラッパー）経由で SDK を利用
- SDK 本体は各利用者が別途取得し、環境変数でパス指定する前提

## 4. 本日実測した検証結果（この環境）

- `cargo fmt --all -- --check`: pass
- `cargo test --workspace -- --nocapture`: pass
  - unit: 11 passed
  - integration: 1 passed（`e2e_build_without_enabled_backends_compiles`）
- `cargo check --all-targets --features backend-nvidia`: pass
- `cargo clippy --workspace --all-targets --features backend-nvidia`: pass
- `cargo test --workspace --features backend-nvidia -- --nocapture`: pass
  - unit: 26 passed
  - integration: 13 passed
- `cargo test --workspace --all-features -- --nocapture`: pass
  - unit: 27 passed
  - integration: 13 passed
- `cargo bench -p video-hw --features backend-nvidia --bench decode_bench`: pass
  - H264 hw_optional chunk_4096: 223.60-231.97 ms
  - H264 hw_required chunk_1048576: 225.44-233.16 ms
  - HEVC hw_optional chunk_1048576: 229.68-237.62 ms
  - HEVC hw_required chunk_1048576: 223.12-232.26 ms
- `cargo deny check licenses advisories bans sources`: pass
- `cargo check --workspace --all-targets --all-features`: pass（2026-05-05, macOS）
- `cargo test -p video-hw-backend-vt --features backend-vt -- --nocapture`: pass（26 passed, 2026-05-16）
- `cargo test -p video-hw --features backend-vt e2e_vt -- --nocapture --test-threads=1`: pass（9 passed, 2026-05-16）
- `cargo test -p video-hw --features backend-vt preflight_encode_rejects_vt_nv12_by_contract -- --nocapture`: pass（1 passed, 2026-05-16）
- `cargo fmt --all -- --check`: pass（2026-06-25, Windows）
- `cargo test --workspace --all-targets`: pass（2026-06-25, Windows）
- `cargo clippy --workspace --all-targets -- -D warnings`: pass（2026-06-25, Windows）
- `cargo deny check licenses bans sources`: pass（2026-06-25, Windows; warnings remain for existing Sanzentyo onevpl fork metadata and duplicate transitive crates）
- Android camera smoke APK: pass（2026-06-25, Samsung device over `192.168.0.244:42133`; `4080x3060@30fps`, Rust surface recorder, MP4 write PASS, decode PASS）

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
- GitHub CI は不要。NV/Intel/Vulkan/VT の backend 別手動 test/bench 手順と結果文書を維持する
- NVIDIA 依存の配布ポリシー（同梱可否・再配布条件）の明文化強化
- `cargo-deny` 運用の継続（依存更新時の定期実行）

## 6. 関連文書

- `docs/research/deepresearch.md`
- `docs/spec/NVIDIA_SDK_DISTRIBUTION_POLICY.md`
- `docs/spec/TEST_SPEC_INVENTORY.md`
- `docs/plan/ROADMAP.md`
- `docs/plan/NEXT_ACTION_PLAN_2026-02-23.md`
- `docs/status/ANDROID_BACKEND_MERGE_READINESS_2026-06-25.md`
