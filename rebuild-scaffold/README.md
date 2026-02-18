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
