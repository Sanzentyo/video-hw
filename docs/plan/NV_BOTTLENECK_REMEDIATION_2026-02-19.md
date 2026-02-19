# NV Bottleneck Remediation Plan
日付: 2026-02-19

## 1. スコープと目的
本計画は NVIDIA バックエンドにおける decode/encode の実効性能ボトルネックを特定し、WebRTC/配信用途で重要な低遅延性（フレーム遅延、ジッタ、ドロップ率）を改善することを目的とする。  
対象は主に単一ストリームのリアルタイム処理（1080p30 起点）で、単純なスループット最大化よりも、安定した低遅延と tail latency の抑制を優先する。

## 2. 最新ベンチマークスナップショット

### 2.1 最新比較（通常条件）
| Codec | video-hw decode | video-hw encode | ffmpeg decode | ffmpeg encode |
|---|---:|---:|---:|---:|
| h264 | 2.958s | 0.745s | 0.485s | 0.203s |
| hevc | 2.773s | 0.713s | 0.491s | 0.201s |

### 2.2 追加最新 h264（外れ値/条件差あり）
| Codec | video-hw decode | video-hw encode | ffmpeg decode | ffmpeg encode | 扱い |
|---|---:|---:|---:|---:|---|
| h264 | 24.677s | 2.881s | 0.544s | 0.230s | 外れ値または実行条件差の可能性が高く、主要比較には直接採用しない |

補足: 外れ値の再現条件（GPU 負荷、電源モード、ビルド差分、同時実行プロセス、入力条件）を別実験で切り分ける。

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

## 9. Issue 分解に使える具体タスク

- [ ] NV-P0-001: stage 別計測（decode pack/decode map, encode submit/reap）を追加
- [ ] NV-P0-002: queue depth, jitter, p95/p99 を収集するメトリクス導入
- [ ] NV-P0-003: encode push 時逐次送出パイプライン実装（flush 依存削減）
- [ ] NV-P0-004: decode の RGB 経路を optional 化し、不要変換を停止
- [ ] NV-P0-005: 外れ値条件（24.677s ケース）の再現スクリプト化と要因切り分け
- [ ] NV-P1-001: Frame 契約の拡張案（raw frame payload / zero-copy 方針）設計
- [ ] NV-P1-002: encode/decode セッション常駐化とバッファプール再利用
- [ ] NV-P1-003: ffmpeg 同条件比較ベンチ（同一入力・同一フレーム列）作成
- [ ] NV-P2-001: マルチストリーム時の backpressure 制御としきい値調整
- [ ] NV-P2-002: canary + rollback 運用手順書（SLO/アラート）整備

---
期待成果: 低遅延配信で重要な p95 遅延・ジッタ・ドロップ率を維持しつつ、decode/encode の実効性能差を段階的に縮小する。
