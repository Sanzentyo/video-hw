# Test Plan (VT + NVIDIA)

更新日: 2026-02-18

## 1. 目的

- 移設後に VT backend / NVIDIA backend の両方を同一契約で検証する
- テスト資産を「既存SDKテスト」と「本プロジェクト新規テスト」に明確に分離する
- sample 動画ベースの再現性ある回帰テストを準備する

## 2. テスト資産の分離方針

### A. 既存 NVIDIA SDK 側テスト（再利用）

対象: https://github.com/Sanzentyo/nvidia-video-codec-sdk

- 目的: SDKラッパ自体の正しさを担保
- 管理場所: upstream 側（原則として upstream の test を尊重）
- 本プロジェクトでの扱い:
  - バージョン固定した依存として取り込む
  - 必要に応じて CI で upstream test 実行結果を参照
  - 互換性破壊が疑われるときのみ patch として提案

### B. 本プロジェクト新規テスト（今回作る）

- 目的: 共通契約・adapter境界・サンプル入力再現性を担保
- 管理場所: 移設先 workspace の `tests/` または各crate内
- 主眼: backend 差替え時に上位 API の意味が変わらないこと

## 3. 新規テストスイート構成

### 3.1 Contract tests（backend 非依存）

- `chunk` ランダム分割でも AU 出力が収束する
- 同一 AU 入力で
  - VT packer は length-prefix
  - NVIDIA packer は Annex-B
- parser エラーと backend エラーの分類が崩れない

### 3.2 Adapter tests（backend 依存）

- VT adapter:
  - decode/encode セッション生成の capability 判定
  - `push_bitstream_chunk` / `flush` の呼び出し契約
- NVIDIA adapter:
  - `push_access_unit` に complete AU が渡る
  - capability-first で不可設定を早期拒否

### 3.3 Sample video integration tests（E2E）

- 入力: `sample-videos/sample-10s.h264`, `sample-videos/sample-10s.h265`
- 期待:
  - decode フレーム数が既知ベースライン（現在 303）を満たす
  - chunk サイズを変えても結果が大きく変動しない
- 追加予定:
  - encode 例（VT/NVIDIA）で作った出力を再度 decode して整合性確認

## 4. encode examples 追加計画

移設後の `examples` は decode/encode を backend ごとに明示する。

- `decode_vt`
- `decode_nvidia`
- `encode_vt`
- `encode_nvidia`

それぞれで同一CLI引数体系（`--codec`, `--input/--output`, `--chunk-bytes`, `--require-hardware` など）を採用し、比較しやすくする。

## 5. CI 分離

- Job A (macOS): VT の unit/contract/integration
- Job B (Linux + NVIDIA GPU): NVIDIA adapter + sample integration
- Job C (any OS): backend 非依存 contract tests

## 6. 受け入れ条件

1. upstream NVIDIA SDK test と本プロジェクト新規 test の責務が分離されている
2. sample 動画ベースの decode テストが VT/NVIDIA で成立する
3. encode examples が backend ごとに用意される
4. backend 差替えで上位 trait 呼び出しが変わらない
