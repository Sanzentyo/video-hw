# video-hw

`video-toolbox` の低レベルAPIを薄くラップした、workspace非参加の独立サンプルプロジェクトです。

## 現状（2026-02-18）

- stateful decode API（`VtBitstreamDecoder::push_bitstream_chunk`）を実装済み
- `rtc` の `H26xReader` を使った chunk 入力対応の NAL/AU 分割を実装済み
- 共通 `AccessUnit` + backend別 `SamplePacker`（VT向け `AvccHvccPacker` / NVIDIA想定 `AnnexBPacker`）を実装済み
- H264/HEVC の chunk decode で `decoded_frames=303` を確認済み
- 契約テスト（`tests/bitstream_contract.rs`）は pass

次フェーズの再編資料:

- [現状ステータス](./STATUS.md)
- [移設・再構成ガイド](./MIGRATION_AND_REBUILD_GUIDE.md)
- [ロードマップ](./ROADMAP.md)
- [調査メモ](./RESEARCH.md)

## 目的

- `video-toolbox` を扱いやすい API に包む
- H.264 / HEVC の encode / decode サンプルを提供する
- 将来的に `highlevel_layer.md` で想定している外部抽象化層の backend として使える形を先に切り出す

## 前提

- macOS
- `ffmpeg` がインストール済み
- ルートの `sample-videos` ディレクトリに `sample-10s.mp4` があること

## テスト用メディア

- `sample-videos/sample-10s.mp4` を https://home-movie.biz/free_movie.html から取得して `sample-videos` ディレクトリに配置しました（テストで使用します）。

## sample-videos/sample-10s.mp4 から Annex-B 生成

このプロジェクト配下で実行:

```bash
ffmpeg -y -i ../sample-videos/sample-10s.mp4 -an -c:v libx264 -preset veryfast -tune zerolatency -x264-params aud=1:repeat-headers=1 -pix_fmt yuv420p -f h264 ../sample-videos/sample-10s.h264
ffmpeg -y -i ../sample-videos/sample-10s.mp4 -an -c:v libx265 -preset veryfast -x265-params aud=1:repeat-headers=1 -pix_fmt yuv420p -f hevc ../sample-videos/sample-10s.h265
```

## Decode サンプル

```bash
cargo run --example decode_annexb -- --codec h264 --input ../sample-videos/sample-10s.h264 --chunk-bytes 4096
cargo run --example decode_annexb -- --codec hevc --input ../sample-videos/sample-10s.h265 --chunk-bytes 4096
```

`examples/decode_annexb.rs` は `VtBitstreamDecoder::push_bitstream_chunk` を使う stateful 経路です。
`src/annexb.rs` では `rtc` の `H26xReader` を利用して NAL/AU を共通表現へ分割し、
`src/packer.rs` の `AvccHvccPacker` が VideoToolbox 向け length-prefixed 形式へ変換します。

## Encode サンプル

```bash
cargo run --example encode_synthetic -- --codec h264 --output ./sample-videos/encoded-output.h264
cargo run --example encode_synthetic -- --codec hevc --output ./sample-videos/encoded-output.h265
```

## 設計ノート

- ライブラリ側エラーは `thiserror`（`src/error.rs`）
- examples 側は `anyhow`
- Annex-B 解析は `rtc`（`rtc::media::io::h26x_reader`）を利用
- API は backend 実装者向けに最小限（`VtDecoder`, `VtBitstreamDecoder`, `VtEncoder`, `Codec`, option structs）
- 依存方向は `highlevel_layer.md` の方針に合わせ、抽象本体は外部層に置く想定

## 次フェーズ

- プロジェクトを新しい場所へ移設し、`video-hw` を「VT専用backend provider」として整理する
- 別backend（NVIDIA SDK）を同一契約で差し替え可能にするため、外部抽象層を中心に再構成する
- 詳細は `MIGRATION_AND_REBUILD_GUIDE.md` を参照
