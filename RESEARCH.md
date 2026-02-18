# Research Notes

## 現状サマリ（2026-02-18）

- `VtBitstreamDecoder::push_bitstream_chunk` により stateful decode を実装済み
- parser は `rtc::media::io::h26x_reader::H26xReader` に統一済み
- `AccessUnit`（raw NAL集合）を共通表現とし、フレーミングは `SamplePacker` に分離済み
- contract test で chunk 入力の収束性と packer 出力を検証済み

## Apple API 観点

- Encode: `VTCompressionSessionCreate` → `VTCompressionSessionPrepareToEncodeFrames` → `VTCompressionSessionEncodeFrame` / `...WithOutputHandler` → `VTCompressionSessionCompleteFrames`
- Decode: `VTDecompressionSessionCreate` → `VTDecompressionSessionDecodeFrame` / `...WithOutputHandler` → `VTDecompressionSessionFinishDelayedFrames` / `VTDecompressionSessionWaitForAsynchronousFrames`
- H.264/HEVC decode 用 format description は parameter sets から構築

## NVIDIA backend 観点（次フェーズ）

- decoder への入力は complete AU 単位に揃える
- AUの最終バイト列化は adapter 側（`AnnexBPacker`）が担う
- capability-first 方針（セッション生成前判定）を維持する
- backend固有エラーは共通 `BackendError` へ明示的に写像する

## 実装上の判断

- `sample-videos/sample-10s.mp4` は ffmpeg で Annex-B ES に変換して利用
- NAL parser は最小実装（AUDベースの access unit 分割 + parameter set 抽出）
- wrapper 層は `Codec` / `VtDecoder` / `VtEncoder` の concrete API 先行

## 2026-02 追加調査: decode 遅延の原因分析

外部調査（Apple公式 + Chromium/FFmpeg 実装）と実測から、以下を原因と判断:

- chunk ごとの全体再パース（入力長に対し O(N^2) 的）
- chunk ごとの `finish_delayed_frames` / `wait_for_asynchronous_frames` による過同期
- `submitted` 件数フォールバックによる decoded 実績の過大評価

採用した対策:

- Annex-B の増分パーサを導入（未完成 NAL/AU のみ保持）
- wait 系 API は `flush` のみで実行
- decoded は callback 集計値のみ採用

## 2026-02 実測（sample-videos/sample-10s, decoded_frames=303）

- 変更前（chunk=4096）
	- H264 decode: real 約 298.85s
	- HEVC decode: real 約 89.79s
- 変更後（chunk=4096）
	- H264 decode: real 0.64s
	- HEVC decode: real 0.49s
- 変更後（chunk=1048576）
	- H264 decode: real 0.47s
	- HEVC decode: real 0.45s

所見:

- chunk サイズ依存はほぼ解消
- `pixel_format` が `Some` で安定し、実デコード callback が機能していることを確認
- 主要ボトルネックは VideoToolbox 自体ではなく、事前 bitstream 処理と同期化戦略だった

## リスク

- 入力 ES が AUD なし・特殊構成だと AU 分割の精度が落ちる
- macOS / ハードウェア差異で HW encode/decode 要件が変わる
- HEVC は環境依存で利用可否が分かれる
- プロジェクト移設時に path 依存（sample配置、manifest指定）が壊れやすい
- NVIDIA backend の CI は GPU 環境前提のため、VT と同一ジョブで管理しにくい

## 後続で行うべきこと

- capability query API を wrapper に追加
- error mapping を highlevel backend 想定の共通型へ拡張
- NVIDIA backend を同契約で差し替え可能にする
- 移設後のディレクトリ基準でドキュメントと実行コマンドを全面更新する
