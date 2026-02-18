# video-hw Status

更新日: 2026-02-19

## 1. 現在の構成

- root の `src/` に実装を集約した単一 crate 構成
- `BackendKind`（VideoToolbox / NVIDIA）で実行時切替
- feature で backend 実装を有効化
  - default: `backend-vt`
  - optional: `backend-nvidia`

## 2. 実装済み

- VideoToolbox decode/encode 実装
- 増分 Annex-B parser + AU 組み立て
- root examples
  - `examples/decode_annexb.rs`
  - `examples/encode_synthetic.rs`
- E2E
  - `tests/e2e_video_hw.rs`（VT経路）
- decode benchmark（Criterion）
  - `benches/decode_bench.rs`
  - `hw_optional` / `hw_required` を分離して計測可能

## 3. 検証結果（最新）

- `cargo check`: pass
- `cargo test -- --nocapture`: pass
- `cargo run --example decode_annexb`（H264/HEVC）: pass
- `cargo run --example encode_synthetic`（H264）: pass
- `cargo check --features backend-nvidia`: pass
  - 実行時は NVIDIA SDK bridge 未接続のため UnsupportedConfig を返す（想定どおり）
- `cargo bench --bench decode_bench -- --noplot`: pass
  - `hw_required` 条件を含む計測に更新済み

## 4. パフォーマンス比較の現状

- `video-hw` 側
  - 関数レベル（Criterion）とプロセス実行（`decode_annexb`）の両計測を実施
- `ffmpeg` 側
  - `videotoolbox` の hwaccel と `h264_videotoolbox` / `hevc_videotoolbox` の利用可を確認
  - 同一素材で decode/encode 計測を実施
- 主要な知見
  - 比較対象の粒度（関数ベンチ vs 外部コマンド）で差が出るため、絶対値比較は注意が必要
  - decode の同一素材比較では `video-hw` は `ffmpeg VT decode` より速いが `ffmpeg SW decode` には届かない

## 5. クリーンアップ

- 旧重複実装の `crates/` を退避検証後に削除
- `legacy-root-backup/` を削除
- Markdown は `docs/` 配下へ再配置

## 6. 残課題

- NVIDIA SDK bridge の実装
- NVIDIA 実機での E2E 回帰テスト
- CI の macOS / Linux+GPU 分離
- encode の公平比較用に「同一素材入力 encode」経路を `video-hw` 側へ追加

## 7. 関連文書

- `README.md`
- `docs/README.md`
- `docs/status/BENCHMARK_2026-02-18.md`
- `docs/status/FFMPEG_VT_COMPARISON_2026-02-19.md`
- `docs/plan/ROADMAP.md`
- `docs/plan/TEST_PLAN_MULTIBACKEND.md`
- `docs/research/RESEARCH.md`
