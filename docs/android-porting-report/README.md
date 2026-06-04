# video-hw Android 対応 調査・設計パッケージ

調査日: 2026-06-04（Asia/Tokyo）
対象: `Sanzentyo/video-hw` main branch（GitHub 公開ページおよび主要ファイルを確認）

## 結論

`Sanzentyo/video-hw` を Android で利用可能にする最短かつ保守しやすい方針は、既存の Vulkan backend を Android に無理に移植するのではなく、Android 公式の `MediaCodec` / NDK `AMediaCodec` を使う新規 backend crate `video-hw-backend-android` を追加することです。

理由は次の通りです。

- 現行の `video-hw` は `video-hw-core`、facade の `video-hw`、各 backend crate に分離され、`DecodeSession::<Adapter>::new(...)` / `EncodeSession::<Adapter>::new(...)` による static generic backend 選択を前提にしています。この構造には Android 用 adapter の追加が自然に収まります。
- 現行 README の platform 切替は macOS=VideoToolbox、Linux/Windows=NVIDIA/Intel/Vulkan で、Android は含まれていません。
- 現行 Vulkan backend は Cargo 依存と Rust module が `target_os = "linux" | "windows"` に限定されており、Android はビルド対象外です。
- Android の公開 codec API は `MediaCodec` / NDK `AMediaCodec` です。NDK の `AMediaCodec_createDecoderByType` / `AMediaCodec_createEncoderByType` / `AMediaCodec_configure` は API 21 から利用でき、encoder input surface は API 26、async callback は API 28 から利用できます。

## ZIP内の構成

```text
README.md
01_current_repo_analysis.md       # 現行リポジトリ構成とAndroid未対応点
02_android_backend_design.md      # Android backend の具体設計
03_dependency_matrix.md           # Rust / Android / API level / Gradle / linker 依存
04_implementation_plan.md         # 段階的な実装手順と変更diff例
05_testing_validation_plan.md     # テスト・検証・ベンチ設計
06_risks_open_questions.md        # リスク、未確定事項、判断ポイント
sources.md                        # 参照元URL一覧

diagrams/
  architecture.mmd                # Mermaid構成図

patch_skeleton/
  README.md                       # skeletonの位置づけ
  facade_changes.patch.md         # video-hw facade/core に必要な変更例
  crates/video-hw-backend-android/
    Cargo.toml
    src/lib.rs
    src/ffi.rs
    src/codec.rs
    src/capability.rs
    src/color.rs
  android-app/
    build.gradle.kts
    CMakeLists.txt
    cargo-ndk.md
```

## この成果物の前提

このZIPは「調査・設計・実装スケルトン」です。実デバイスでの `cargo check --target aarch64-linux-android` や Android instrumentation test まではこの環境では実行していません。特に Android codec は端末差が大きいため、最終実装では複数端末で capability と色フォーマットをログ化して検証してください。
