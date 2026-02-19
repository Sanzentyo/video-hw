# NV Bottleneck Remediation Plan
日付: 2026-02-19

## 1. スコープと目的
本計画は NVIDIA バックエンドにおける decode/encode の実効性能ボトルネックを特定し、WebRTC/配信用途で重要な低遅延性（フレーム遅延、ジッタ、ドロップ率）を改善することを目的とする。  
対象は主に単一ストリームのリアルタイム処理（1080p30 起点）で、単純なスループット最大化よりも、安定した低遅延と tail latency の抑制を優先する。

## 2. 最新ベンチマークスナップショット

### 2.1 最新比較（repeat=5, include-internal-metrics）
| Codec | video-hw decode | video-hw encode | ffmpeg decode | ffmpeg encode |
|---|---:|---:|---:|---:|
| h264 | 0.289s | 0.271s | 0.588s | 0.229s |
| hevc | 0.305s | 0.258s | 0.543s | 0.208s |

### 2.2 追加最新 h264（外れ値/条件差あり）
| Codec | video-hw decode | video-hw encode | ffmpeg decode | ffmpeg encode | 扱い |
|---|---:|---:|---:|---:|---|
| h264 | 24.677s | 2.881s | 0.544s | 0.230s | 外れ値または実行条件差の可能性が高く、主要比較には直接採用しない |

補足: `warmup 0 --repeat 1 --verify` の軽試行（`output/benchmark-nv-precise-h264-1771514429.md`）では外れ値は再現せず、本計画では優先度を下げる。

## 3. ボトルネック仮説（コード/API 制約紐付け）

| ID | 仮説 | コード/API 根拠 | 重大度 | 確信度 |
|---|---|---|---|---|
| H1 | decode パスで RGB 化と不要データ経路が発生し、null sink 比較で不利 | src/nv_backend.rs:132 で DecodedRgbFrame、src/nv_backend.rs:442 でメタデータ化のみ（実データ未活用） | Critical | High |
| H2 | AU ごとの再パックで CPU メモリコピーが過多 | src/nv_backend.rs:26-39 の AnnexB 再構築、src/nv_backend.rs:86-116 のループ | High | High |
| H3 | encode が flush 一括処理でストリーミング化されていない | src/nv_backend.rs:270 で push は蓄積のみ、src/nv_backend.rs:301 で flush 実行 | Critical | High |
| H4 | Frame 契約に画素バッファがなく、実入力比較が不公平 | src/contract.rs:8-13 の Frame が寸法/PTSのみ、src/nv_backend.rs:320/451 で synthetic 生成 | Critical | High |
| H5 | encode セッション初期化/バッファ生成コストが周期的に発生 | src/nv_backend.rs:204-265 で make_session、src/nv_backend.rs:311-318 でバッファ作成 | Medium | Medium |
| H6 | 同期 lock と to_vec コピーで出力回収に追加オーバーヘッド | src/nv_backend.rs:406-416 の lock と data コピー | Medium | Medium |
| H7 | API 上、キュー深さ・ジッタ・遅延を観測する契約が不足 | src/contract.rs 全体に統計/テレメトリ契約なし | High | High |

## 4. NVIDIA SDK サンプル知見と本リポジトリへのマッピング
調査対象: c:/Users/sanze/Downloads/Video_Codec_SDK_13.0.37/Video_Codec_SDK_13.0.37/Samples

| サンプル/クラス | 典型パターン | 本リポジトリ現状 | 適用方針 |
|---|---|---|---|
| AppEncLowLatency | 低遅延向け設定（B-frame/Lookahead 抑制、即時回収、浅いキュー） | ULTRA_LOW_LATENCY は設定済みだが flush 一括 | push 時点送出＋浅いリングバッファ化 |
| AppDecLowLatency | 低遅延 decode（最小バッファ、逐次取り出し） | AU 再パック＋RGB 経路が重い | bitstream 供給を軽量化し、色変換を必要時のみ |
| AppEncPerf | 高スループット（事前確保、再利用、計測分離） | 入出力バッファ再利用が限定的 | セッション常駐＋バッファプール導入 |
| AppDecPerf | decode 計測の純化（I/O/変換と分離） | parser/pack/copy の混在 | decode 純処理時間と周辺処理時間を分離計測 |
| NvEncoder クラス | resource 登録/再利用、送出と回収の明確化 | flush 時にまとめて処理 | submit/reap の常時パイプライン化 |
| NvDecoder クラス | parser/decode の責務分離、バッファ管理 | assembler + packer の CPU 作業が相対的に重い | parser 境界の最適化、コピー削減 |

## 5. 外部公開リファレンス（テーマ）
正確 URL は本書では固定せず、公開一次資料のテーマを列挙する。

- NVIDIA Video Codec SDK Programming Guide（NVENC/NVDEC API 全体）
- NVIDIA Video Encoder API Programming Guide（低遅延設定、レート制御、バッファ運用）
- NVIDIA Video Decoder 関連ガイド（parser/decode queue、surface 管理）
- FFmpeg HWAccel/NVENC 利用ガイダンス（hwaccel、cuvid、nvenc、プリセット/チューニング）
- FFmpeg の transcoding 最適化一般論（I/O 分離、フィルタ最小化、比較条件統一）

## 6. 優先度付きリメディエーション計画（P0/P1/P2）

| 優先度 | 施策 | 受け入れ基準（計測可能） |
|---|---|---|
| P0 | 計測基盤追加（stage 別時間、キュー深さ、p95/p99、ドロップ率、ジッタ）と比較条件統一 | 1080p30 連続 10 分で drop rate < 0.1%、queue depth p95 <= 3、jitter p95 <= 4ms |
| P0 | encode を flush 一括から逐次送出へ変更（submit/reap パイプライン） | encode p95 frame latency <= 16ms、queue depth p95 <= 2 |
| P0 | decode で不要 RGB 経路の回避（必要時のみ変換） | decode throughput を現状比 2.0x 以上、p95 decode latency 40% 以上改善 |
| P1 | Frame/契約拡張（raw frame 入力経路、統計取得 API） | ffmpeg と同等入力で apples-to-apples 比較が成立、指標を API で取得可能 |
| P1 | セッション/バッファプール再利用の強化 | encode/decode の起動直後スパイクを 30% 以上削減 |
| P2 | マルチストリーム最適化、負荷時劣化特性の平準化 | 2-4 stream 時も drop rate < 1%、jitter p95 <= 8ms |
| P2 | 運用向けロールアウト（カナリア、フェイルバック） | 異常時に自動退避し、SLO 逸脱時間を 5 分未満に抑制 |

## 7. 検証実験マトリクス

| 実験軸 | 変化させる要素 | 観測指標 | Pass/Fail |
|---|---|---|---|
| Codec | h264 / hevc | fps、p95/p99 latency、drop rate | いずれも P0 基準を満たす |
| 解像度 | 720p / 1080p / 1440p | fps、GPU 使用率、queue depth | 1080p30 で p95 <= 16ms |
| fps | 30 / 60 | jitter、drop rate | 30fps で <0.1%、60fps で <0.5% |
| 入力経路 | synthetic / raw frame / 実素材 | 各 stage 時間 | raw と synthetic の差分要因を説明可能 |
| バッファ方針 | 現状 / pool 再利用 | queue depth、tail latency | p95 queue depth 改善 25% 以上 |
| セッション運用 | flush 一括 / 常駐逐次 | E2E latency、起動スパイク | 起動スパイク 30% 以上改善 |
| 負荷条件 | 単一 / 並列 2-4 | drop/jitter、再現性 | SLO 超過を規定内に維持 |
| 比較対象 | video-hw / ffmpeg | 相対比、再現性 | 3 run の分散が許容範囲内（CV <= 10%） |

## 8. リスクとロールアウト戦略

### 主なリスク
- 契約変更（Frame 拡張）で API 互換性影響が出る。
- 低遅延最適化が品質（圧縮効率）や安定性に副作用を持つ。
- GPU/ドライバ差異で再現性が崩れる。

### ロールアウト方針
1. 計測機能を先行導入し、現状の可視化を固定。
2. P0 変更は feature flag で段階有効化（既存経路を保持）。
3. canary ワークロードで 24-72 時間監視後、段階展開。
4. SLO 逸脱時は自動で旧経路へフェイルバック。
5. 週次で性能/安定性レビューし、P1/P2 の着手判定。

### 8.1 追補: スレッド分散設計

- RGB 変換を decode callback / encode submit から切り離し、専用 worker へオフロードする設計を追加。
- 入出力 CPU タスク（ingress/egress）と GPU 本線（decode/encode submit-reap）を分離し、queue/backpressure で制御。
- backend 差分（NVIDIA / VideoToolbox）は adapter 層に閉じ、上位契約は共通化。
- 詳細: `docs/plan/PIPELINE_TASK_DISTRIBUTION_DESIGN_2026-02-19.md`

## 9. Issue 分解に使える具体タスク

- [x] NV-P0-001: stage 別計測（decode pack/decode map, encode submit/reap）を追加
- [x] NV-P0-002: queue depth, jitter, p95/p99 を収集するメトリクス導入
- [x] NV-P0-004: decode の RGB 経路を optional 化し、不要変換を停止
- [x] NV-P0-005: 外れ値条件（24.677s ケース）の再現スクリプト化と要因切り分け（軽試行 1 回で非再現のため本件ではクローズ）
- [x] NV-P1-001: Frame 契約の拡張案（raw frame payload / zero-copy 方針）設計
  - `Frame.argb: Option<Vec<u8>>` を導入し、NVIDIA encode で実入力経路を接続
- [ ] NV-P1-002: encode/decode セッション常駐化とバッファプール再利用
  - 進捗: CUDA context を `NvEncoderAdapter` 内で flush 跨ぎ再利用
  - 未完: NVENC `Session` の完全常駐化（safe API の buffer lifetime 制約あり）
- [x] NV-P1-003: ffmpeg 同条件比較ベンチ（同一入力・同一フレーム列）作成
  - `scripts/benchmark_ffmpeg_nv_precise.rs` に `--equal-raw-input` を追加
  - `examples/encode_raw_argb.rs` を追加（同一 ARGB 入力列を video-hw 側へ投入）
- [ ] NV-P1-004: `PipelineScheduler` 導入（submit/reap/transform/egress のスレッド分離）
  - 進捗: `src/pipeline_scheduler.rs` を追加（submit/reap + transform 分離）
- [ ] NV-P1-005: `TransformLayer` 導入（RGB/resize を非同期 worker 化、GPU優先・CPU fallback）
  - 進捗: `PipelineScheduler` が `BackendTransformAdapter` を駆動し、KeepNative fast-path / NV12->RGB 非同期 reap を実行
- [ ] NV-P1-006: backend adapter 差分実装（NVIDIA: CUDA変換、VT: Metal/CoreImage 経路）
  - Phase 1 実装済み: `src/backend_transform_adapter.rs`
    - 共通 `BackendTransformAdapter` trait
    - `DecodedUnit`（`MetadataOnly | Nv12Cpu | RgbCpu`）
    - NVIDIA 側: KeepNative fast-path + NV12->RGB 非同期 dispatch（`TransformDispatcher` 連携）
    - VT 側: パススルー stub（Metal/CoreImage 実装は未着手）
  - Phase 2（部分）実装済み:
    - `src/cuda_transform.rs` を追加（NVRTC + CUDA kernel による NV12->RGB）
    - `NvidiaTransformAdapter` で CUDA 経路を優先使用し、失敗時のみ CPU worker fallback
  - 追補（今回）:
    - `src/contract.rs` の `Frame` に `argb: Option<Vec<u8>>` を追加
    - `src/nv_backend.rs` encode で `frame.argb` を優先投入（未指定時のみ synthetic fallback）
    - 運用乖離のある「1回だけ入力 upload」最適化は採用しない
  - 追補（今回2）:
    - `src/pipeline_scheduler.rs` を追加し、NVIDIA adapter との submit/reap 駆動を実装
- [ ] NV-P2-001: マルチストリーム時の backpressure 制御としきい値調整
- [ ] NV-P2-002: canary + rollback 運用手順書（SLO/アラート）整備

注記（2026-02-19 再計測）:
- `output/benchmark-nv-precise-h264-1771496033.md`: encode mean は video-hw 0.303s / ffmpeg 0.221s（約 1.37x）
- `output/benchmark-nv-precise-hevc-1771496033.md`: encode mean は video-hw 0.286s / ffmpeg 0.213s（約 1.34x）
- encode については「ffmpeg と同等に近い水準」に到達したため、旧 `NV-P0-003` は本チェックリストから除外（実装済み扱い）。
- `NV-P0-004` 反映後の再計測（`output/benchmark-nv-precise-h264-1771498123.md`, `output/benchmark-nv-precise-hevc-1771498128.md`）では decode mean が 0.300s / 0.329s まで短縮。

---
期待成果: 低遅延配信で重要な p95 遅延・ジッタ・ドロップ率を維持しつつ、decode/encode の実効性能差を段階的に縮小する。

## 10. 次セッション実行チェックリスト

- [x] Step 1: `NV-P0-001`, `NV-P0-002` を先に着手（実装順固定）
- [x] Step 2: `cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --warmup 1 --repeat 5 --include-internal-metrics` を実行
- [x] Step 3: `cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --warmup 1 --repeat 5 --include-internal-metrics` を実行
- [x] Step 4: `NV-P0-004`（decode RGB optional 化）を反映し、Step 2/3 を再実行
- [x] Step 5: decode mean の before/after と ffmpeg 比を `docs/status/FFMPEG_NV_COMPARISON_2026-02-19.md` に追記
- [x] Step 6: `NV-P0-005`（外れ値再現）は軽試行 1 回で非再現のため追加実装は保留でクローズ

注記（今回実施）:
- `warmup 0 --repeat 1 --verify` で h264 を 1 回実施し、24.677s 外れ値は非再現（`output/benchmark-nv-precise-h264-1771514429.md`）。
- `NV-P0-004` 実装後に `--warmup 1 --repeat 5 --include-internal-metrics` を h264/hevc で実行済み。
- `NV-P1-004/005` の基盤として `src/pipeline.rs`（bounded queue）と `src/transform.rs`（CPU worker 変換）を追加済み。backend への本統合は次段。
- レビュー指摘の反映:
  - スロット制（in-flight credits）を `src/pipeline.rs` に追加
  - KeepNative fast-path 判定を `src/transform.rs` に追加
  - decode/encode submit-reap 分離を `src/nv_backend.rs` に反映（thread 分離）
  - decode metadata 経路で call 単位 reaper thread 生成を除去（`src/nv_backend.rs`）
  - encode 入力/出力バッファを in-flight 数に応じてプール再利用（`src/nv_backend.rs`）
  - （取り消し）synthetic 固定入力向けの「input 一度 upload」最適化は運用乖離のため撤回
- 再計測（`--warmup 1 --repeat 3 --include-internal-metrics --verify`）:
  - `output/benchmark-nv-precise-h264-1771499558.md`
  - `output/benchmark-nv-precise-hevc-1771499564.md`
  - `output/benchmark-nv-precise-h264-1771500342.md`
  - `output/benchmark-nv-precise-hevc-1771500342.md`
  - `output/benchmark-nv-precise-h264-1771500639.md`
  - `output/benchmark-nv-precise-hevc-1771500639.md`
  - `output/benchmark-nv-precise-h264-1771501008.md`
  - `output/benchmark-nv-precise-hevc-1771501008.md`
  - `output/benchmark-nv-precise-h264-1771505433.md`
  - `output/benchmark-nv-precise-hevc-1771505433.md`
  - `output/benchmark-nv-precise-h264-1771513976.md`
  - `output/benchmark-nv-precise-hevc-1771513976.md`
  - `output/benchmark-nv-precise-h264-1771514429.md`（外れ値軽試行）
  - `output/benchmark-nv-precise-h264-1771514448.md`（repeat=5）
  - `output/benchmark-nv-precise-hevc-1771514450.md`（repeat=5）
  - `output/benchmark-nv-precise-h264-1771514780.md`（repeat=3, verify）
  - `output/benchmark-nv-precise-hevc-1771514780.md`（repeat=3, verify）
  - `output/benchmark-nv-precise-h264-1771515974.md`（repeat=3, verify）
  - `output/benchmark-nv-precise-hevc-1771515974.md`（repeat=3, verify）
  - `output/benchmark-nv-precise-h264-1771515386.md`（repeat=3, verify, equal-raw-input）
  - `output/benchmark-nv-precise-hevc-1771515398.md`（repeat=3, verify, equal-raw-input）
  - mean（最新）:
    - h264（repeat=3, verify）: video-hw decode 0.291s / encode 0.324s, ffmpeg decode 0.547s / encode 0.217s
    - hevc（repeat=3, verify）: video-hw decode 0.312s / encode 0.316s, ffmpeg decode 0.536s / encode 0.254s
  - mean（equal-raw-input）:
    - h264（repeat=3, verify, equal-raw-input）: video-hw decode 0.286s / encode 0.467s, ffmpeg decode 0.493s / encode 0.228s
    - hevc（repeat=3, verify, equal-raw-input）: video-hw decode 0.326s / encode 0.435s, ffmpeg decode 0.495s / encode 0.218s
  - verify:
    - h264/hevc とも `ffprobe` + `ffmpeg -v error` 検証で decode=ok
