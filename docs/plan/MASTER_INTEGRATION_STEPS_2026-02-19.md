# Master Integration Steps (2026-02-19)

対象ブランチ: `feat/nv-precise-performance-analysis-2026-02-19`

## 1. 統合対象

- `src/nv_backend.rs`
  - encode 出力回収の in-flight 化
  - bitstream 再利用
  - NVIDIA backend 固有パラメータ `max_in_flight_outputs` 対応（default=4）
- `scripts/benchmark_ffmpeg_nv_precise.rs`
- ドキュメント更新
  - `docs/status/NV_PRECISE_ANALYSIS_2026-02-19.md`
  - `docs/status/STATUS.md`
  - `docs/plan/NV_LOCK_MS_REDUCTION_EXECUTION_PLAN_2026-02-19.md`
  - `README.md`
  - `scripts/README.md`

## 2. 事前確認（feature branch）

```bash
git checkout feat/nv-precise-performance-analysis-2026-02-19
cargo fmt --all
cargo check --features backend-nvidia
cargo test --features backend-nvidia -- --nocapture
```

主要ベンチ成果物:
- `output/benchmark-nv-precise-h264-1771493200.md`
- `output/benchmark-nv-precise-hevc-1771493244.md`
- `output/benchmark-nv-precise-h264-1771493302.md`
- `output/benchmark-nv-precise-hevc-1771493327.md`

## 3. コミット手順

```bash
git add src/nv_backend.rs
git add scripts/benchmark_ffmpeg_nv_precise.rs
git add docs/status/NV_PRECISE_ANALYSIS_2026-02-19.md
git add docs/status/STATUS.md
git add docs/plan/NV_LOCK_MS_REDUCTION_EXECUTION_PLAN_2026-02-19.md
git add docs/plan/MASTER_INTEGRATION_STEPS_2026-02-19.md
git add README.md scripts/README.md
git commit -m "nvenc: reduce lock wait via in-flight bitstream reap and add precise benchmarks"
```

## 4. master 取り込み（推奨: PR 経由）

```bash
git fetch origin
git rebase origin/master
git push -u origin feat/nv-precise-performance-analysis-2026-02-19
```

PR 作成時チェック:
- Windows + NVIDIA 実機での `backend-nvidia` テスト結果を添付
- `docs/status/NV_PRECISE_ANALYSIS_2026-02-19.md` の before/after 数値を記載
- `max_in_flight=4` 採用理由（H264/HEVC 両方の mean 最良）を記載

## 5. master 反映後の手順

```bash
git checkout master
git pull --ff-only origin master
```

必要ならタグ作成:

```bash
git tag -a nv-lock-opt-2026-02-19 -m "NVENC lock-time reduction validated"
git push origin nv-lock-opt-2026-02-19
```

## 6. ロールバック手順

- PR 反映前: feature branch を修正して再計測
- PR 反映後:
  - revert commit を作成し、NVIDIA backend パラメータを `max_in_flight_outputs=1` とした暫定回避を適用
  - 影響範囲を `docs/status/STATUS.md` に追記
