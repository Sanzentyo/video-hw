# Feature Gating and Dead Code Policy

## Goal

- `allow(dead_code)` に頼らず、`feature + target` で有効化された backend だけを型・モジュールレベルで有効化する。
- no-backend 構成でも API が破綻せず、E2E が実質同等の検証を維持できること。

## Current Design

- `Backend`（=`BackendKind`）の列挙子は compile-time gating 済み。
  - `VideoToolbox`: `all(target_os = "macos", feature = "backend-vt")`
  - `Nvidia`: `all(feature = "backend-nvidia", any(target_os = "linux", target_os = "windows"))`
- backend 実装モジュール（`vt_backend`, `nv_backend`, `cuda_transform`）は同じ条件で module-level gating。
- `build_decoder_inner` / `build_encoder_inner` は有効な列挙子のみを受けるため、無効 backend の runtime 分岐を持たない。

## Dead Code Handling Policy

- `allow(dead_code)` は使わない。
- 使われない実装は優先順位で整理する。
  1. `feature/target` で型・関数・モジュールごと外す
  2. テスト専用なら `#[cfg(test)]` に寄せる
  3. それでも残る場合のみ「構造上の残存」として記録する（この文書に追記）

## Residual Warnings

- 現時点では、`cargo test -q --all-features`（macOS）で構造由来の `dead_code` 警告は残っていない。

## Verification

- `cargo test -q` : pass
- `cargo test -q --all-features` : pass
- `rg -n "allow\\(dead_code\\)" src tests examples docs README.md` : no match
