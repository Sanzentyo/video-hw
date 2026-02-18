# Rebuild Scaffold Workspace

このディレクトリは、次フェーズ（移設 + マルチbackend再構成）の開始点として使う Cargo workspace 雛形です。

## 目的

- 現在の `video-hw` から責務を段階的に移し替える
- VT backend と NVIDIA backend を同一契約で扱う
- 既存の `push_bitstream_chunk` 設計と `AccessUnit` 共通表現を維持する

## ディレクトリ構成

```text
rebuild-scaffold/
  crates/
    backend-contract/
    bitstream-core/
    vt-backend/
    nvidia-backend/
  examples/
    smoke/
```

## 移行対応表

- `src/annexb.rs` -> `crates/bitstream-core`
- `src/packer.rs` -> `crates/vt-backend` / `crates/nvidia-backend`
- `src/backend.rs` -> `crates/vt-backend`
- NVIDIA 実装 -> `crates/nvidia-backend`
- 共通 trait/error/capability -> `crates/backend-contract`

## 実行メモ

```bash
cargo check --workspace
```

この雛形はまず構造固定のための最小実装です。機能実装は `MIGRATION_AND_REBUILD_GUIDE.md` と `TEST_PLAN_MULTIBACKEND.md` の順で拡張してください。

## 実装状況（2026-02-18 更新）

- `backend-contract`
  - 共通 `Codec` / `Frame` / `EncodedPacket` / `CapabilityReport`
  - `DecoderConfig` / `DecodeSummary`
  - `VideoDecoder` / `VideoEncoder` trait
- `bitstream-core`
  - 増分 Annex-B パーサ（chunk ごとに全体再パースしない）
  - AU 組み立て、parameter set cache、flush 対応
  - chunk 収束と parameter set 抽出の unit test
- `vt-backend`
  - standalone な VideoToolbox adapter 実装（root `video-hw` 依存なし）
  - VT 用 `AvccHvccPacker` を公開
- `nvidia-backend`
  - `bitstream-core` 接続済み（SDK bridge は未接続）
  - NVIDIA 用 `AnnexBPacker` を公開
- `examples/smoke`
  - `decode_vt` / `decode_nvidia` / `encode_vt` / `encode_nvidia` の起動入口を維持

現時点で `cargo check --workspace` と `cargo test --workspace` は通過しています。
