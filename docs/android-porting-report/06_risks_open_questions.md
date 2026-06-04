# 06. リスクと未確定事項

## 1. Android codec の端末差

Android の codec 実装は vendor / SoC / OS version に強く依存します。特に以下は端末差が出ます。

- H.264 / HEVC / AV1 の encode 対応有無。
- profile / level / bitrate mode の対応。
- CPU output の color format。
- stride / slice-height / crop の扱い。
- Annex B と length-prefixed sample の受け入れ可否。
- CSD (`csd-0`, `csd-1`, `csd-2`) の期待形式。

対策: `CapabilityReport.runtime` を必ず probe 結果ベースにし、device matrix のログを蓄積します。

## 2. CPU output とNV12のギャップ

`video-hw` の decode payload は `Nv12` / `Rgb24` を持ちますが、Android codec output は `YUV_420_888` / vendor-specific semi-planar / planar / tiled 形式になる可能性があります。MVPでは以下の順で対応します。

1. Metadata only を必ず安定化。
2. flexible YUV / known color format の CPU buffer を NV12 に正規化。
3. 未対応 color format は `UnsupportedConfig` ではなく capability 上 `Nv12` 非対応として報告。
4. HardwareBuffer / Surface output は Phase 2 で native frame API を設計。

## 3. 既存APIとの互換性

`RawFrameBuffer` / `DecodedFrame` に Android native handle を追加すると、公開APIの意味が広がります。破壊的変更を避けるには次の選択肢があります。

- MVPでは既存 enum を変更しない。
- native handle は backend-specific extension API として別に出す。
- 将来的に `DecodedFrame::Native(NativeDecodedFrame)` / `RawFrameBuffer::Native(NativeFrame)` を追加する場合は feature gate し、既存matchが壊れないよう `#[non_exhaustive]` 検討。

## 4. async callback の扱い

NDK async callback は API 28 から利用できますが、callback有効時には sync dequeue を呼んではいけません。また callback thread で重い処理をしてはいけないという制約があります。

対策: MVPは sync dequeue。Phase 2 で async backend を追加する場合は callback→event queue→worker thread の3層にします。

## 5. hardware/software fallback

Android の software codec は存在することがありますが、性能保証がありません。`require_hardware` を true にする場合は、JNI の `MediaCodecInfo.isHardwareAccelerated()` / `isSoftwareOnly()` を使って選別するのが最も明確です。ただしこれらの属性は API 29 以降です。

MVP判断:

- NDK-only / minSdk 21: codec名 heuristics と configure probe。
- 実用版 / minSdk 29: JNI capability provider を使い、hardware/software/vendor を明示。

## 6. Vulkan Video on Android

既存 Vulkan backend を Android へ広げることは可能ですが、次が課題です。

- 現行 code と Cargo 依存が Linux/Windows cfg。
- Android Vulkan Video 対応は端末/driver依存。
- `AHardwareBuffer` / external memory / queue family / surface interop の追加設計が必要。
- 現行 Vulkan capability report はH.264中心。

結論: Android対応の第一段階には不向きです。MediaCodec backend を先に実装し、将来の optional backend として扱うのが安全です。

## 7. 未確定事項

実装前に決めるべき事項です。

1. 最低 minSdk を 21 / 28 / 29 のどれにするか。
2. JNI を MVP に含めるか、Phase 2 に回すか。
3. decode `Nv12` を必須要件にするか、Metadata-only を許容するか。
4. encode output layout を常に `AnnexB` に正規化するか、Android native sample layoutを追加するか。
5. Surface / HardwareBuffer の公開APIを core に入れるか、Android専用 extension にするか。
6. H.264 のみで先にmergeするか、HEVC / AV1 の skeleton まで入れるか。
