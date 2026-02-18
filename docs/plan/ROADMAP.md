# Roadmap

更新日: 2026-02-19

## 現在の到達点

- root 単一 crate 構成へ整理済み
- backend 切替は `BackendKind` + feature で運用
- VideoToolbox の decode/encode は E2E まで通過
- `decode_annexb` / `encode_synthetic` の examples で実行確認済み
- Criterion ベンチで `hw_optional` / `hw_required` の比較が可能
- `ffmpeg`（VideoToolbox）との同一素材比較レポートを作成済み
- 重複していた旧 `crates/` と `legacy-root-backup/` は削除済み

## 直近の優先タスク

1. NVIDIA SDK bridge の実接続
   - `backend-nvidia` feature で実デコード/実エンコード経路を有効化
2. E2E の NVIDIA 実機検証
   - Linux + GPU 環境で sample video ベースの回帰テストを追加
3. CI 分離
   - macOS (VT) / Linux+GPU (NVIDIA) を分離して安定運用
4. encode 比較の公平化
   - `video-hw` に同一素材入力 encode 経路を追加し、`ffmpeg` 比較条件を統一

## 受け入れ条件

- VT/NVIDIA の双方で同一 trait API で decode/encode が呼べる
- sample video ベースで backend ごとの E2E が再現可能
- README と docs の実行手順が実装と一致している

## 関連文書

- `docs/status/STATUS.md`
- `docs/status/FFMPEG_VT_COMPARISON_2026-02-19.md`
- `docs/plan/TEST_PLAN_MULTIBACKEND.md`
- `docs/plan/MIGRATION_AND_REBUILD_GUIDE.md`
- `docs/research/RESEARCH.md`
