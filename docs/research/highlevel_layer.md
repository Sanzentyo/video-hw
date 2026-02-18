# High-Level Abstraction Integration Design v2（外部抽象層への統合設計・VT/NVIDIA両立版）

## 1. 前提

本設計は「抽象化層をこのリポジトリ内に作る」前提ではない。
目的は **外部抽象層（別crate/別プロジェクト）が複数 backend（VideoToolbox / NVIDIA）を同一契約で取り込めること**。

依存方向（重要）:

```text
External Abstraction Layer
    ├─ depends on vt backend provider (uses rust-media/apple-media-rs/video-toolbox)
    └─ depends on nvidia-video-codec-sdk
```

前提ライブラリ:

* VT: `rust-media/apple-media-rs` の `video-toolbox` crate を利用（VideoToolbox の safe bindings）([GitHub][3])
* NVIDIA: `Sanzentyo/nvidia-video-codec-sdk` を利用（NVDEC/NVENC wrapper）([GitHub][2])

基本方針（v2）:

* **chunk → NAL → AU（Access Unit）境界の確定までは外部抽象層で共通化**
* **AU 内のフレーミング（AVCC/HVCC の length-prefix か、Annex-B start code か）は API 由来の差として adapter 側に置く**
* adapter は「共通契約 ↔ backend 固有 API」だけでなく、**backend が要求するバイト列フレーミング（SamplePacker）**も担当する

---

## 2. 両立要件（今回の合意）

1. decode は「連続ビットストリーム投入（chunk push）」を第一級に扱う
2. 呼び出し側は NAL bitstream を順次渡すだけで、継続 decode できることを期待する
3. NAL 分割/AU 組み立て/parameter set 管理は外部抽象層へ切り分ける（stateful）
4. VT/NVIDIA のどちらでも同じ上位 trait で使えること
5. NVIDIA adapter の方針（capability-first、明示的エラー変換）を維持すること
6. **AU 境界の確定は共通化するが、AU を backend 用 1 バイト列にする方法は adapter が責務を持つ**

   * VT: length-prefixed（AVCC/HVCC）
   * NVIDIA: Annex-B start code 付き（慣行）＋ **nvidia-video-codec-sdk の `push_access_unit` 契約に従い “complete AU” を渡す**([GitHub][2])

---

## 3. 推奨アーキテクチャ（v2）

```text
App
 ↓
External Abstraction Crate
 ├─ common traits/types/errors
 ├─ bitstream module (stateful, backend-agnostic)
 │   ├─ NalReader        (rtc::media::io::h26x_reader)
 │   ├─ AuAssembler      (AUD優先 + codec補助ルール)
 │   ├─ ParameterSetCache
 │   └─ AccessUnit model (raw NAL bytes, no framing)
 ├─ vt_adapter      (depends on vt backend provider)
 │   └─ AvccHvccPacker   (AU -> VT input bytes -> CMSampleBuffer)
 └─ nvidia_adapter  (depends on nvidia-video-codec-sdk)
     └─ AnnexBPacker     (AU -> NVDEC input bytes -> push_access_unit)
```

---

## 4. 責務分割（v2）

### 4.1 外部抽象層（必須・共通化する範囲）

* byte stream 入力の受け口（`push_bitstream_chunk` の入口）
* `rtc` parser による NAL 分割（chunk 境界は任意）

  * `rtc::media::io::h26x_reader::H26xReader`
* NAL -> AU 組み立て（stateful）
* parameter set（H264: SPS/PPS, HEVC: VPS/SPS/PPS）保持
* **共通中間表現 `AccessUnit` を生成**

  * **重要**: `AccessUnit` 内の NAL は **start code も length prefix も付けない raw bytes**
  * これにより、backend 依存のフレーミングを外部層から排除できる（統一点が最大化される）

#### AccessUnit（例）

```rust
pub struct AccessUnit {
    pub nalus: Vec<Vec<u8>>,      // raw NAL bytes (no start code / no length)
    pub codec: Codec,             // H264 / HEVC / AV1 ...
    pub pts_90k: Option<i64>,
    pub is_keyframe: bool,
}
```

### 4.2 backend provider 側（薄いラップ）

* セッション生成/破棄
* backend API 呼び出し（decode/encode）

  * VT: `VTDecompressionSessionDecodeFrame(… CMSampleBuffer …)` を呼べる形を提供([GitHub][4])
  * NVIDIA: `nvidia-video-codec-sdk::Decoder` / `Encoder` の呼び出し
* flush/finish
* backend 固有エラー返却（外部層では共通 `BackendError` に写像）

---

## 5. 共通 trait（外部層）

（インターフェースは原案を維持。意味づけだけ v2 に合わせて明確化）

```rust
pub trait VideoDecoder: Send {
    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError>;

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError>;
}
```

実装意図（v2）:

* `push_bitstream_chunk` は NAL 断片を許容
* AU 完成判定は外部層 bitstream module が持つ
* **完成した `AccessUnit` を adapter が受け取り、backend 仕様に合わせてフレーミングして投入する**

  * VT: length-prefixed（AVCC/HVCC）
  * NVIDIA: Annex-B start code（＋ `push_access_unit` 契約により complete AU）([GitHub][2])

---

## 6. Adapter マッピング仕様（v2）

### 6.0 SamplePacker の導入（v2 の主変更点）

AU は共通表現だが、**backend が受け取る 1 バイト列**は異なるため、adapter 内に packer を置く。

```rust
pub trait SamplePacker {
    fn pack(&mut self, au: &AccessUnit) -> Result<PackedSample, BackendError>;
}

pub struct PackedSample {
    pub data: Vec<u8>,            // backend-ready bytes
    // 追加情報が必要ならここに:
    // pub nal_lengths: Option<Vec<u32>>,
}
```

### 6.1 VideoToolbox adapter（v2）

* `create_decoder`

  * parameter sets から format description を構築
  * VT セッション生成（`VTDecompressionSessionCreate`）([GitHub][4])
  * capability: `VTIsHardwareDecodeSupported(codecType)` 等で事前確認可能([GitHub][4])
* `push_bitstream_chunk`

  * 外部層 bitstream module で `AccessUnit` 化
  * `AvccHvccPacker` で **各 NAL に 4byte length header を付けて 1 sample を構成**

    * Chromium 実装でも `kNALUHeaderLength = 4` として length header を挿入している([Chromium][1])
  * `CMBlockBuffer` / `CMSampleBuffer` を組み立てて `decode_frame` に渡す（`CMSampleBuffer` が入力契約）([GitHub][4])
* `flush`

  * `finish_delayed_frames` / `wait_for_asynchronous_frames` を呼ぶ([GitHub][4])

### 6.2 NVIDIA adapter（v2, 元設計の意図を維持）

* `create_decoder`

  * `Decoder::new(codec, options)`（capability-first の probe も維持）
* `push_bitstream_chunk`

  * 外部層 bitstream module で `AccessUnit` 化
  * `AnnexBPacker` で **各 NAL に start code（例: 00 00 00 01）を付け、1 AU バイト列に連結**
  * `Decoder::push_access_unit(&bytes, timestamp_90k)` に投入

    * `push_access_unit` は **“one complete access unit”** を要求し、内部で `CUVID_PKT_ENDOFPICTURE` を立てて `cuvidParseVideoData` に渡す([GitHub][2])
    * `CUVID_PKT_ENDOFPICTURE` は **packet がちょうど 1 frame/field を含む場合に MUST**（NVDEC ガイド）([NVIDIA Docs][5])
* `flush`

  * `Decoder::flush`
* `reconfigure`（encode 側）

  * `Session::reconfigure`（原案維持）

注記（v2）:

* 「NVIDIA 側 AU 形式」は **外部層ではなく adapter の packer が最終責務を持つ**
  （外部層は raw NAL の集合としての AU を返すだけ）

---

## 7. `annexb.rs` の位置づけ（v2）

* 旧: `video-hw/src/annexb.rs` 相当が “NAL/AU 整形” まで含んでいた
* v2:

  * **NAL 分割/AU 組み立て（境界確定）**は外部層 `bitstream` に集約
  * **Annex-B への“パッキング”**は NVIDIA adapter の `AnnexBPacker` に限定
  * **AVCC/HVCC への“パッキング”**は VT adapter の `AvccHvccPacker` に限定

これにより、「annexb」という名前が “境界確定” と “フレーミング” を混同させる問題を回避する。

---

## 8. Capability 方針（Capability First）

### 共通

* セッション生成前に `query_capability` を実施
* 不可構成は生成前に弾く

### NVIDIA（元設計を継承）

* decode:

  * adapter 側 probe ロジックで `Decoder::new` 可否を事前判定
* encode:

  * 既存の capability 取得手順を維持

### VT

* `VTIsHardwareDecodeSupported(codecType)` 等で事前確認([GitHub][4])

---

## 9. エラー変換ルール（v2）

基本は原案維持。ただし v2 では “packer 層” が増えたため、分類を明確化する。

* parser/AU 段階（外部層）:

  * `BackendError::InvalidBitstream`
* packer 段階（adapter）:

  * AU が backend に変換できない（例: length header overflow / unsupported NAL set）
    → `BackendError::InvalidInput` または `InvalidBitstream`
* VT 実行:

  * `BackendError::Backend("videotoolbox: ...")`
* NVIDIA 実行:

  * `TemporaryBackpressure` / `DeviceLost` / `UnsupportedConfig` など原案の写像を維持

---

## 10. 機能差分の扱い（抽象漏れ対策）

原案維持:

* backend 固有機能は共通 trait に無理に入れない
* `Ext` trait を backend 別に分離、または downcast 可能な拡張インターフェースで露出

---

## 11. 受け入れ基準（v2）

1. chunk 境界が任意でも decode 継続できる（stateful）
2. AU 未完成 chunk は内部保持される
3. AU 完成時のみ backend decode が呼ばれる
4. VT/NVIDIA 両 adapter が同一 trait で動作する
5. capability 不一致はセッション生成前に失敗する
6. backend provider 単体利用が可能（外部層 annexb 依存なし）
7. **packer 差分は adapter に閉じ、外部層は `AccessUnit` までを共通化できている**

---

## 12. 実装タスク（v2）

1. 共通 trait/type/error を定義
2. `bitstream` モジュールを作成（`H26xReader` + `AuAssembler` + `ParameterSetCache` + `AccessUnit`）
3. `SamplePacker` trait と `PackedSample` を導入
4. `vt_adapter` に `AvccHvccPacker` を実装し、`CMSampleBuffer` → `decode_frame` で接続([GitHub][4])
5. `nvidia_adapter` に `AnnexBPacker` を実装し、`Decoder::push_access_unit` で接続([GitHub][2])
6. contract test：

   * chunk ランダム分割 + backend 差替え
   * 同一 AU から **VT は length-prefix、NVIDIA は Annex-B** が生成されることを検証

---

## 13. 次フェーズ実行ガイド（移設 + 再構成）

### 13.1 方針

- 現在の `video-hw` は「VT backend provider の先行実装」として維持する
- そのうえで、配置を移した先で **共通契約を先に固定** し、VT/NVIDIA を adapter として再配置する

### 13.2 推奨順序

1. 現行実装の実測結果を固定（status化）
2. プロジェクトを移設し、VT単独で再現確認
3. `bitstream-core` / `backend-contract` を抽出
4. VT adapter を新契約へ適合
5. NVIDIA adapter を同契約で追加
6. backend 差替え contract test を通す

### 13.3 失敗しやすい点

- 相対パス前提の sample 入力が移設後に壊れる
- `AccessUnit` と backend入力バイト列の責務が再び混ざる
- capability 判定を後ろ倒しにして実行時エラーを増やしてしまう

### 13.4 完了判定

- 上位コードは backend 種別を変えても同一 trait 呼び出しのまま
- AU 境界確定は共通層にのみ存在
- VT/NVIDIA の framing 差分は各 adapter packer に閉じている

実作業チェックリストは `video-hw/MIGRATION_AND_REBUILD_GUIDE.md` を参照。

---

## 14. 実運用パフォーマンス規約（2026-02 検証反映）

VideoToolbox の decode を chunk 入力で運用する場合、以下を規約化する。

1. **bitstream は増分パースを必須**

  - chunk ごとに全バッファを再パースしない
  - 未完成 NAL/AU のみ内部保持し、完成 AU だけを decode へ渡す

2. **`finish_delayed_frames` / `wait_for_asynchronous_frames` は flush/EOS のみ**

  - 各 chunk で wait すると非同期性が潰れ、呼び出し回数分だけ大幅に遅くなる

3. **decode 成功判定は callback 実績ベース**

  - `submitted` 件数を decoded とみなすフォールバックは使わない
  - `decoded_frames` / `dropped` / `submitted` を分離集計する

4. **AU 境界と packer 責務は厳密分離**

  - 外部共通層: AU 境界確定まで
  - adapter: AU→backend 入力バイト列（AVCC/HVCC / Annex-B）

5. **時刻情報は単調増加で供給**

  - chunk 分割に依存せず AU 単位で PTS を単調増加させる
  - chunk 内 index 由来の非連続値を使わない

この規約により、chunk サイズ依存の極端な性能劣化（O(N^2) 的挙動）を回避し、
外部抽象層での VT/NVIDIA 共通契約にも整合する。