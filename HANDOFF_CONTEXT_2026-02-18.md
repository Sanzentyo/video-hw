# Handoff Context (2026-02-18)

## 1. 目的

この文書は、移設とマルチbackend再構成に進む次担当者が、設計意図・実装状態・検証結果・未完了タスクを欠落なく引き継ぐためのコンテキストです。

## 2. 現在の到達点

- `video-hw` は VideoToolbox backend provider として動作済み
- `VtBitstreamDecoder::push_bitstream_chunk` により stateful decode 入力を実装済み
- `rtc::media::io::h26x_reader::H26xReader` を使って NAL/AU 分割を実装済み
- `AccessUnit`（raw NAL bytes）を共通表現にして packer を分離済み
  - VT想定: `AvccHvccPacker`
  - NVIDIA想定: `AnnexBPacker`
- chunk decode の実測は H264/HEVC ともに 303 frames

## 3. 既存主要ファイル

- `src/backend.rs`
  - `VtDecoder`, `VtBitstreamDecoder`, `VtEncoder`
- `src/annexb.rs`
  - `parse_annexb_for_stream`, `AccessUnit`, parameter set 抽出
- `src/packer.rs`
  - `SamplePacker`, `AvccHvccPacker`, `AnnexBPacker`
- `tests/bitstream_contract.rs`
  - chunk収束性と packer 出力の契約テスト

## 4. 直近検証コマンド

```bash
cargo run --example decode_annexb -- --codec h264 --input ../sample-videos/sample-10s.h264 --chunk-bytes=4096
cargo run --example decode_annexb -- --codec hevc --input ../sample-videos/sample-10s.h265 --chunk-bytes=4096
cargo test --manifest-path video-hw/Cargo.toml -- --nocapture
```

## 5. 重要な設計合意

1. AU境界確定は共通層で行う
2. backend 用フレーミングは adapter に閉じる
3. capability-first（不可設定はセッション前で失敗）
4. parser失敗と backend失敗はエラー分類を分ける
5. NVIDIA 側は `push_access_unit` 契約（complete AU）を守る

## 6. 次フェーズで追加するもの

- プロジェクト移設（まずは無改変移動 + 再現確認）
- 共通契約 crate（trait/error/capability）
- bitstream-core 抽出
- VT/NVIDIA adapter 分離
- backend ごとの decode/encode examples
- sample 動画ベース integration tests

## 7. テスト方針（厳守）

- NVIDIA SDK 側既存テストと本プロジェクト新規テストを分離管理する
- 新規テストは「共通契約」「adapter動作」「sample動画E2E」を担当する
- 詳細は `TEST_PLAN_MULTIBACKEND.md` を参照

## 8. 雛形

移設開始用に `rebuild-scaffold/` を追加済み。

- `crates/backend-contract`
- `crates/bitstream-core`
- `crates/vt-backend`
- `crates/nvidia-backend`
- `examples/smoke`（decode/encode の VT/NVIDIA 入口を確保）

## 9. 実装順序（推奨）

1. `backend-contract` の型を確定
2. `bitstream-core` に現 `annexb.rs` の実ロジックを移植
3. `vt-backend` に現 `backend.rs` の decode/encode を移植
4. `nvidia-backend` を SDK 接続で実装
5. sample 動画 integration tests を VT -> NVIDIA の順で有効化
6. CI を macOS / Linux+GPU に分離

## 10. 既知の注意点

- path 依存（sample ファイル位置、`--manifest-path`）は移設時に壊れやすい
- AUD 依存ストリームでは AU 判定精度に注意
- `pixel_format` が実行環境によって `None` の場合がある
