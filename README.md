# video-hw

`video-hw` は、複数のハードウェア backend（VideoToolbox / NVIDIA）を同一 API で扱う単一 crate です。

## 主要構成

```text
src/
  lib.rs              # 公開API + backend切替
  contract.rs         # 共通 trait / type / error
  bitstream.rs        # Annex-B 増分パースと AU 組み立て
  vt_backend.rs       # VideoToolbox 実装（macOS + feature）
  nv_backend.rs       # NVIDIA 実装（Windows/Linux + backend-nvidia）
examples/
  decode_annexb.rs
  encode_synthetic.rs
tests/
  e2e_video_hw.rs
scripts/
  benchmark_ffmpeg_nv.rs
  README.md
```

## feature / platform 切替

- デフォルト: `backend-vt`（macOS 前提）
- NVIDIA を有効化: `--features backend-nvidia`
- 実行時は `BackendKind` で backend を選択

## NVIDIA backend 依存

`backend-nvidia` では次の依存を固定しています。

- `nvidia-video-codec-sdk`
  - `git = "https://github.com/Sanzentyo/nvidia-video-codec-sdk"`
  - `rev = "d2d0fec631365106d26adfe462f3ce15b043b879"`
- `cudarc = 0.19.2`（`driver` + `cuda-version-from-build-system`）

### NVIDIA Video Codec SDK ビルド前提（Windows）

- NVIDIA Driver / CUDA が有効であること
- `nvidia-video-codec-sdk` の build script が SDK の lib を見つけられること
- 必要に応じて環境変数を設定

```powershell
$env:NVIDIA_VIDEO_CODEC_SDK_PATH = "C:\Path\To\Video_Codec_SDK\Lib\x64"
```

`NVIDIA_VIDEO_CODEC_SDK_PATH` は `nvEncodeAPI.lib` / `nvcuvid.lib` があるディレクトリを指します。

## 検証コマンド

```bash
cargo fmt --all
cargo check
cargo check --features backend-nvidia
cargo test -- --nocapture
cargo test --features backend-nvidia -- --nocapture
```

## 実行例

```bash
# NVDEC decode
cargo run --features backend-nvidia --example decode_annexb -- --backend nv --codec h264 --input sample-videos/sample-10s.h264 --chunk-bytes 4096 --require-hardware

# NVENC encode
cargo run --features backend-nvidia --example encode_synthetic -- --backend nv --codec h264 --fps 30 --frame-count 300 --require-hardware --output output/video-hw-h264.bin

# NVENC encode (backend 固有パラメータ)
cargo run --features backend-nvidia --example encode_synthetic -- --backend nv --codec h264 --fps 30 --frame-count 300 --require-hardware --nv-max-in-flight 4 --output output/video-hw-h264.bin
```

Rust API から設定する場合:

```rust
use video_hw::{
    BackendEncoderOptions, BackendKind, Codec, Encoder, EncoderConfig, NvidiaEncoderOptions,
};

let mut config = EncoderConfig::new(Codec::H264, 30, true);
config.backend_options = BackendEncoderOptions::Nvidia(NvidiaEncoderOptions {
    max_in_flight_outputs: 4,
});
let _encoder = Encoder::with_config(BackendKind::Nvidia, config);
```

## ffmpeg 比較ベンチ

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv.rs --codec h264 --release
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv.rs --codec hevc --release

# 反復実行 + 統計（mean/p95/CV）
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --release --warmup 2 --repeat 9
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --release --warmup 2 --repeat 9
```

## スクリプト実装方針

- `scripts/` は RFC 3424 / Cargo issue #12207 の `cargo -Zscript` 形式を基本とします。
- 新規スクリプトは原則 `scripts/*.rs` で追加してください。
- 詳細: `scripts/README.md`

## ドキュメント

- インデックス: `docs/README.md`
- 全体状態: `docs/status/STATUS.md`
- VT 比較: `docs/status/FFMPEG_VT_COMPARISON_2026-02-19.md`
- NV 比較: `docs/status/FFMPEG_NV_COMPARISON_2026-02-19.md`
