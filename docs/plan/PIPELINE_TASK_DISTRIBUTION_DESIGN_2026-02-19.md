# Pipeline Task Distribution Design
日付: 2026-02-19

## 1. 目的
RGB 変換などの重い処理を decode/encode 本線から切り離し、CPU/GPU の役割を明確化したうえで並列実行する。  
対象は VT/NVIDIA 両 backend で、同一の上位抽象契約を維持しつつ backend 差分を adapter 内に閉じる。

## 2. 設計原則
- decode/encode 本線は「submit/reap の継続実行」を最優先し、CPU 変換を同期的に挟まない
- RGB 変換は要求されたときのみ実行し、既定は metadata/surface のまま通す
- backend 差分は `BackendAdapter` 層で吸収し、スケジューラは共通 queue 契約だけを見る
- backpressure は queue 深さと in-flight 数で制御し、flush は EOS 時のみ

## 3. 抽象レイヤ
1. `IngressLayer`
   - byte stream 読み込み、NAL/AU 組み立て、PTS 正規化
2. `DecodeAdapter`
   - backend に AU を投入し、`DecodedUnit` を返す
   - `DecodedUnit` は `MetadataOnly | GpuSurface | CpuFrame`
3. `TransformLayer`
   - 色変換、resize、overlay など CPU/GPU タスクを非同期実行
4. `EncodeAdapter`
   - backend に encode submit/reap
5. `EgressLayer`
   - packet 書き出し、mux、verify
6. `PipelineScheduler`
   - queue、worker、backpressure、metrics を統合管理

## 4. 実行ドメイン（スレッド分散）

| ドメイン | 推奨スレッド | 主タスク | ブロッキング方針 |
|---|---:|---|---|
| Ingress | 1 | 読み込み、AU 組み立て、timestamp 補正 | I/O 待ちのみ許容 |
| Decode Submit | 1 | AU を decode adapter へ submit | queue full 時は backpressure |
| Decode Reap | 1 | decode 完了回収、`DecodedUnit` 発行 | 長時間処理禁止 |
| Transform CPU Pool | `N=物理コア-2` 上限 | RGB 変換、swscale、CPU 前処理 | bounded queue |
| Transform GPU Queue | 1 stream/queue | CUDA/Metal 変換、resize | 非同期実行 |
| Encode Submit | 1 | encode submit | queue full 時は backpressure |
| Encode Reap | 1 | packet 回収 | 長時間処理禁止 |
| Egress | 1 | write/mux/verify | I/O 待ちのみ許容 |
| Control/Telemetry | 1 | rate 制御、queue 監視、drop policy | 非同期 |

## 5. queue と backpressure

| queue | 内容 | 既定容量 | high watermark 動作 |
|---|---|---:|---|
| `au_in` | 完成 AU | 128 | ingress 側を待機 |
| `decoded_out` | `DecodedUnit` | 64 | decode submit を抑制 |
| `transform_in` | 変換要求 | 64 | metadata-only 経路を優先 |
| `encode_in` | encode 入力 | 64 | decode 側を抑制 |
| `packet_out` | encoded packet | 128 | encode submit を抑制 |

制御ルール:
- `decoded_out` と `encode_in` を最重要 queue とし、ここが飽和する前に submit rate を下げる
- tail latency 増大時は transform を間引き、metadata/surface 経路を優先
- drop は ingress ではなく encode 手前で行い、GOP 破壊を避ける

## 6. RGB 変換戦略

### 6.1 方針
- 変換要求がない限り `NV12/P010` のまま通す
- RGB が必要な場合のみ `TransformLayer` に job を投げる
- decode callback 内で RGB 変換しない

### 6.2 実行優先順位
1. GPU 変換
   - NVIDIA: CUDA/NPP カーネル
   - VT: Metal/CoreImage 経路
2. CPU 変換（fallback）
   - 専用 worker pool で実行（decode/encode スレッドでは実行しない）

### 6.3 API 例
```rust
pub enum ColorRequest {
    KeepNative,
    Rgb8,
    Rgba8,
}

pub struct TransformJob {
    pub input: DecodedUnit,
    pub color: ColorRequest,
    pub resize: Option<(u32, u32)>,
}
```

## 7. backend 差分マッピング

| 項目 | NVIDIA | VideoToolbox |
|---|---|---|
| decode 出力の主形態 | `GpuSurface` または metadata | `CVPixelBuffer` |
| RGB 変換推奨経路 | CUDA/NPP | Metal/CoreImage |
| CPU fallback | worker pool で NV12->RGB | worker pool で CVPixelBuffer->RGB |
| encode 入力推奨 | surface 直結（可能なら zero-copy） | pixel buffer 直結 |
| 注意点 | map/unmap 回数と同期点を最小化 | callback thread を詰まらせない |

## 8. CPU タスク分散（入力/出力）

入力側 CPU タスク:
- demux/read
- Annex-B/AVCC 正規化
- AU 組み立て
- PTS 補間・単調増加保証

出力側 CPU タスク:
- packet 境界整形
- mux/write
- verify（`ffprobe` / `ffmpeg -v error`）
- 統計レポート作成

原則:
- 入出力の CPU タスクは decode/encode submit/reap スレッドと分離
- `to_vec` などのコピーは egress 側へ寄せる

## 9. 監視メトリクス
- stage 時間: ingest/decode_submit/decode_reap/transform/encode_submit/encode_reap/egress
- queue: mean/p95/p99/peak
- in-flight: decode/encode 各深さ
- jitter/drop: mean/p95/p99
- utilization: GPU busy、CPU worker busy

SLO 例:
- 1080p30 で `drop_rate < 0.1%`
- `decode_reap_p95 <= 8ms`, `encode_submit_p95 <= 16ms`
- `transform_queue_p95 <= 4`

## 10. 実装段階（提案）
1. 共通 `PipelineScheduler` と bounded queue を導入
2. decode/encode を submit/reap スレッドへ分離
3. `DecodedUnit` 抽象（metadata/surface/cpuframe）を導入
4. `TransformLayer` を worker pool + GPU queue で追加
5. backend adapter ごとに transform 実装（NVIDIA/VT）
6. verify をベンチスクリプト標準フローに統合

## 11. 受け入れ基準
- RGB 要求なしの経路で decode/encode throughput が維持される
- RGB 要求ありでも decode/encode スレッドの待ちが増えない
- backend 切替時に上位 API は不変
- `--verify` 付きベンチで出力妥当性が常時確認できる

## 12. 既知のリスク
- queue 容量過小でスループット低下、過大で遅延増大
- backend ごとの surface 互換差により zero-copy 経路が分岐
- CPU fallback が有効な環境で worker 過負荷になる可能性

## 13. 関連
- `docs/plan/NV_BOTTLENECK_REMEDIATION_2026-02-19.md`
- `docs/status/STATUS.md`
- `docs/research/highlevel_layer.md`
