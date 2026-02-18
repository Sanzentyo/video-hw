# Roadmap

## フェーズ1（完了）: VT backend provider の最小成立

### 完了した項目

1. 独立プロジェクトを作成（workspaceへは不参加）
2. 低レベルAPI薄ラッパーを `src/backend.rs` に実装
3. `rtc` ベースの bitstream 分割を `src/annexb.rs` に実装
4. decode サンプルを `examples/decode_annexb.rs` に実装
5. encode サンプルを `examples/encode_synthetic.rs` に実装
6. `AccessUnit` 共通表現 + `SamplePacker` 分離を導入
7. stateful decode API（`push_bitstream_chunk`）を実装
8. 契約テスト（`tests/bitstream_contract.rs`）を追加

### 実測確認

- chunk decode: H264/HEVC ともに `decoded_frames=303`
- `cargo test --manifest-path video-hw/Cargo.toml -- --nocapture` pass

## フェーズ2（次作業）: 移設 + マルチbackend再構成

### ゴール

- プロジェクト移設後もビルド/実行/テストが再現できる
- VT backend と NVIDIA backend を同一の外部抽象契約で差し替え可能にする
- backend固有処理（フレーミング/API呼び出し）を adapter に閉じ込める

### タスク分解

1. 新配置（mono-repo もしくは external abstraction repo）を確定
2. crate 責務を再配置（`bitstream-core` / `vt-backend` / `nvidia-backend`）
3. 共通 trait・共通 error・共通 capability API を確定
4. VT adapter 既存実装を新配置へ移設（互換APIを維持）
5. NVIDIA adapter を `push_access_unit` 契約で実装
6. examples を decode/encode の4系統（VT/NVIDIA）で整備
7. tests を二系統で整備

	- NVIDIA SDK 既存 test（upstream）
	- 本プロジェクト新規 test（contract/adapter/sample E2E）
8. backend切替 contract test（同一AU入力で backend 差替え）を追加
9. CI（macOS: VT、Linux+GPU: NVIDIA）を分離整備

### 完了条件

- VT/NVIDIA で同一 decoder trait の `push_bitstream_chunk` / `flush` が動作
- AU境界確定ロジックが backend 非依存モジュールへ分離される
- packer差分が adapter 層に限定される
- 移設後ドキュメント（セットアップ、運用、移行手順）が揃う
- backend ごとの encode examples が実装される
- sample 動画ベース tests が実装される

### 進捗（2026-02-18, rebuild-scaffold）

- [x] `backend-contract` / `bitstream-core` / `vt-backend` / `nvidia-backend` / `examples/smoke` の workspace 構成を実装
- [x] `bitstream-core` に増分 Annex-B parser（chunk -> AU）を移植
- [x] VT adapter で `push_bitstream_chunk` / `flush` の stateful decode を実装
- [x] VT adapter の packer（AVCC/HVCC）を adapter 層に保持
- [x] NVIDIA adapter の packer（Annex-B）を adapter 層に保持
- [x] scaffold で E2E tests（H264/HEVC, chunk=4096/1MB, decoded=303）を追加・通過
- [x] scaffold で encode E2E（VT synthetic encode）を追加・通過
- [ ] NVIDIA SDK bridge（`push_access_unit` 実接続）は未実装（次ステップ）

注記: 受け入れ条件のうち「sample 動画ベース tests」「AU境界の共通化」「packer差分のadapter閉じ込み」は scaffold 側で達成済み。
NVIDIA 実デコード/実エンコードは SDK 接続が残タスク。

## 参照

- `STATUS.md`
- `MIGRATION_AND_REBUILD_GUIDE.md`
- `TEST_PLAN_MULTIBACKEND.md`
- `HANDOFF_CONTEXT_2026-02-18.md`
- `highlevel_layer.md`
