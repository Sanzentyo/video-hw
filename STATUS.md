# video-hw Status

更新日: 2026-02-18

## 1. 目的と現在位置

`video-hw` は VideoToolbox backend provider の実装検証用プロジェクトとして成立しています。
次フェーズでは、プロジェクト移設と同時に「VT以外（NVIDIA SDK含む）も同一契約で扱える構成」へ再編します。

## 2. 実装済み

- H264/HEVC decode（VideoToolbox）
- H264/HEVC encode（synthetic frame）
- stateful bitstream ingest API（`VtBitstreamDecoder::push_bitstream_chunk`）
- `rtc` ベースの NAL分割/AU組み立て（`src/annexb.rs`）
- 共通 `AccessUnit` + backend別 packer（`src/packer.rs`）
- 契約テスト（chunk収束 + packer出力）

## 3. 直近の検証結果

- H264 chunk decode（4096 bytes/chunk）: `decoded_frames=303`
- HEVC chunk decode（4096 bytes/chunk）: `decoded_frames=303`
- `cargo test --manifest-path video-hw/Cargo.toml -- --nocapture`: pass

## 4. 既知の制約

- bitstream境界判定は AUD 依存度が高く、特殊ストリームで境界精度が下がる可能性がある
- decode summary の `pixel_format` は実行条件により `None` となることがある
- 現状は VT backend 中心で、capability API が共通抽象としては未整理

## 5. 次フェーズの最優先

1. プロジェクト移設後の crate 責務整理
2. 外部抽象層の共通 trait/error/capability の固定
3. NVIDIA backend adapter の追加
4. backend 差し替え contract test と CI 分離整備

## 7. scaffold 実装状況（2026-02-18 追記）

- `rebuild-scaffold` は `video-hw` の責務分離に沿って実装済み
	- `backend-contract`: 共通 trait/type
	- `bitstream-core`: 増分 Annex-B parser + parameter set cache
	- `vt-backend`: standalone VT adapter（root crate 依存なし）
	- `nvidia-backend`: SDK bridge 前段（packer + contract adapter 枠）
- scaffold の E2E tests
	- H264/HEVC decode（chunk=4096/1MB）で `decoded_frames=303`
	- VT encode（synthetic）で packet 出力を確認
- `cargo test --workspace`（rebuild-scaffold）: pass

## 6. 参照

- `README.md`
- `ROADMAP.md`
- `RESEARCH.md`
- `MIGRATION_AND_REBUILD_GUIDE.md`
- `TEST_PLAN_MULTIBACKEND.md`
- `HANDOFF_CONTEXT_2026-02-18.md`
- `highlevel_layer.md`