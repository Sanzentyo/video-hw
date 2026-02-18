# Migration And Rebuild Guide

更新日: 2026-02-18

> 注記: 本文書は workspace 再構成フェーズ時点の履歴文書です。
> 現行状態は `docs/status/STATUS.md` と `docs/plan/ROADMAP.md` を参照してください。

本書は、次フェーズで予定している

1. プロジェクトの場所移動
2. VT + NVIDIA SDK を同一契約で扱える再構成

の実作業手順をまとめたものです。

## 0. 目的

- 現在の `video-hw` で得た知見を保持したまま、新配置へ安全に移す
- backend 実装（VT/NVIDIA）を adapter に閉じ込め、上位層から同じ API で使える状態にする

## 1. 推奨ターゲット構成

最小構成（推奨）:

```text
media-backends/
  crates/
    bitstream-core/      # chunk->NAL->AU, parameter set, common model
    backend-contract/    # trait/error/capability model
    vt-backend/          # VideoToolbox adapter + packer(AVCC/HVCC)
    nvidia-backend/      # NVIDIA adapter + packer(Annex-B)
  examples/
    decode_vt.rs
    decode_nvidia.rs
    encode_vt.rs
    encode_nvidia.rs
```

  開始時の雛形は `rebuild-scaffold/` を参照。

## 2. 移設フェーズ（壊さない移動）

### 2.1 事前固定

- 現行の検証結果を `STATUS.md` に固定
- 依存バージョン（`Cargo.lock`）を保存
- 実行に使ったサンプルファイルの配置ルールを記録

### 2.2 物理移動

- まず `video-hw` をそのまま移動（コード改変なし）
- `cargo run --manifest-path ...` で動作再確認
- README の相対パスを新配置に合わせて更新

### 2.3 移動完了の判定

- H264/HEVC chunk decode が再現
- 既存テストが pass
- サンプル生成・実行手順がドキュメントどおりに通る

## 3. 再構成フェーズ（マルチbackend化）

### 3.1 共通契約を先に固定

- `VideoDecoder` trait
- `VideoEncoder` trait
- `BackendError` と分類ルール
- `CapabilityReport`（decode/encode, hw/sw, codec/profile）

先に契約を固定し、VT/NVIDIA 実装はそれに合わせて adapter 化する。

### 3.2 bitstream-core 抽出

- `H26xReader` 入力
- stateful chunk buffering
- AU 完成判定
- parameter set cache
- 共通 `AccessUnit` 出力（raw NAL、framingなし）

### 3.3 backend adapter 実装

- VT:
  - `AvccHvccPacker` で length-prefixed sample 化
  - `CMSampleBuffer` 経由で decode
- NVIDIA:
  - `AnnexBPacker` で complete AU byte stream 化
  - `push_access_unit` 契約に従って投入

### 3.4 contract test

- 同一 chunk 入力を `bitstream-core` に渡したとき AU が一致する
- 同一 AU に対して
  - VT packer は length-prefix 出力
  - NVIDIA packer は Annex-B 出力
- backend 切替時に上位 API の呼び出しコードが変わらない

### 3.5 tests 分離（必須）

- NVIDIA SDK 既存テスト（upstream）
  - https://github.com/Sanzentyo/nvidia-video-codec-sdk
  - SDK の正しさ検証は原則 upstream 側で実施
- 本プロジェクト新規テスト
  - 共通契約の担保
  - adapter 境界の担保
  - sample 動画 E2E

詳細は `TEST_PLAN_MULTIBACKEND.md` を参照。

## 4. 設計ルール（破綻防止）

- ルール1: backend 固有のバイト列仕様を共通層へ持ち込まない
- ルール2: parser 失敗と backend 実行失敗を別エラー系列で扱う
- ルール3: capability 不一致はセッション生成前に失敗させる
- ルール4: AU 未完成 chunk は decode しない（内部保持）
- ルール5: 既存VT API名は可能な限り維持し、移行時の破壊変更を最小化する

## 5. CI/運用方針

- macOS job: VT backend の build/test
- Linux+GPU job: NVIDIA backend の build/test
- 共通 job: `bitstream-core` と `backend-contract` の contract test

## 6. 実行チェックリスト

### A. 移設直後

- [ ] `cargo check`（VT backend）
- [ ] H264 chunk decode
- [ ] HEVC chunk decode
- [ ] 既存契約テスト

### B. 再構成後

- [ ] 共通 trait で VT を呼べる
- [ ] 共通 trait で NVIDIA を呼べる
- [ ] 同一入力で backend 切替検証が通る
- [ ] ドキュメントと実コマンドが一致する
- [ ] backend ごとの decode/encode examples が揃う
- [ ] sample 動画ベース integration tests が揃う

## 7. 移行時に持ち出すべき資産

- `src/annexb.rs`（stateful parse/AU）
- `src/packer.rs`（packer 契約）
- `tests/bitstream_contract.rs`（契約テスト）
- `src/backend.rs`（VT adapter 実装の土台）

これらを新構成へ段階的に再配置し、互換確認をしながら責務分離を進める。

## 8. 引き継ぎ文書

- `HANDOFF_CONTEXT_2026-02-18.md`
- `TEST_PLAN_MULTIBACKEND.md`