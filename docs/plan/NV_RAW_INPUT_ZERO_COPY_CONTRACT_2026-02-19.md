# NV Raw Input Zero-Copy Contract
日付: 2026-02-19
対象: Windows + NVIDIA

## 1. 目的
- encode 公平比較の前提となる raw frame 入力契約を明確化する
- copy がどこで発生しているかを可視化し、最適化優先順位を固定する
- 上位 API を壊さず段階的に zero-copy へ近づける

## 2. 現在の契約（実装済み）
- `Frame.argb: Option<Vec<u8>>` を encode 入力として受ける
- `argb` 未指定時のみ synthetic fallback を生成
- NVIDIA encode では input buffer へ `lock.write(&argb)` で upload する
- output は `lock.data().to_vec()` で `EncodedPacket.data` を構築する

注記:
- したがって現状は少なくとも
  - 入力: CPU `Vec<u8>` -> NVENC input buffer への copy
  - 出力: NVENC bitstream lock -> `Vec<u8>` への copy
  が存在する

## 3. copy 計測（このセッションで追加）
- `VIDEO_HW_NV_METRICS=1` で encode metrics に以下を出力
  - `input_copy_bytes`
  - `input_copy_frames`
  - `output_copy_bytes`
  - `output_copy_packets`
- 対象ログ:
  - `[nv.encode] ...`
  - `[nv.encode.safe] ...`

この値で、条件差のあるベンチ間でも copy 量の説明が可能になる。

## 4. zero-copy 契約の段階案
1. Phase A（完了）
- `Frame.argb` の実入力経路を標準化
- copy bytes/packets を計測可能化

2. Phase B（次）
- `Frame` に所有形態を表す入力型を追加
  - 例: `OwnedArgb(Vec<u8>)`, `SharedArgb(Arc<[u8]>)`
- copy 計測を API で取得可能にする（ログ依存を解消）

3. Phase C（将来）
- backend adapter 経由で device memory 入力契約を導入
  - 例: `GpuSurfaceHandle` / `ExternalBufferHandle`
- 可能な経路では host copy を回避

## 5. 受け入れ基準（NV-P1-003/NV-P1-001 関連）
- `--equal-raw-input --verify` で h264/hevc の比較を継続できる
- copy 指標が毎回取得でき、計測レポートと整合する
- safe lifetime 経路/通常経路で copy 指標が説明可能

## 6. 関連
- `docs/plan/NV_BOTTLENECK_REMEDIATION_2026-02-19.md`
- `docs/plan/NV_SESSION_ARCHITECTURE_REDESIGN_2026-02-19.md`
- `docs/status/STATUS.md`
