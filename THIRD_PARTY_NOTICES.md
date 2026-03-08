# Third-Party Notices

更新日: 2026-02-23

この文書は `video-hw` が依存する主要な第三者ソフトウェアのライセンス参照先を記録する。

## Direct Dependencies (Representative)

- Rust crates.io dependencies
  - 各crateのライセンスは `Cargo.toml` / crates.io / upstream repository を参照
- `nvidia-video-codec-sdk`（optional, `backend-nvidia`）
  - Repository: `https://github.com/Sanzentyo/nvidia-video-codec-sdk`
  - Rust から NVIDIA Video Codec SDK を扱うためのラッパー層
  - SDK 本体は各利用者が NVIDIA から取得し、環境に配置する前提
  - 利用時は upstream のライセンス/利用条件を確認すること

## NVIDIA SDK に関する注意

`backend-nvidia` は NVIDIA 関連コンポーネントに依存する。配布・再配布時は、
NVIDIA 側のライセンス条件と利用規約を確認し、同梱可否や再配布条件を必ず検証すること。
また、ビルド時には `NVIDIA_VIDEO_CODEC_SDK_PATH` 等の環境設定が必要になる。

## Maintenance Policy

- 新規依存追加時に本ファイルを更新する
- リリース前に依存ライセンス一覧を再確認する
