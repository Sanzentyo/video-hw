# patch_skeleton について

このディレクトリは、実装開始時に使える「変更方針と雛形」です。完全なコンパイル済み実装ではありません。

主な内容:

- `facade_changes.patch.md`: `video-hw` / `video-hw-core` 側に入れる変更例。
- `crates/video-hw-backend-android`: 新規 Android backend crate の雛形。
- `android-app`: Android Gradle / CMake / cargo-ndk 連携例。

実装時は、実際の `video-hw-core` の option enum 定義位置、`Frame` / `EncodedPacket` の private/public 境界、bitstream helper の公開範囲に合わせて調整してください。
