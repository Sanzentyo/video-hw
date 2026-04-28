# video-hw-fmp4

`video-hw` の encoder/decoder を使って fragmented MP4 を扱うための crate です。

- Writer は typestate
  - `Fmp4Writer<Ready>`
  - `Fmp4Writer<SyncRecording>`
  - `Fmp4Writer<AsyncRecording>`
  - `Fmp4Writer<Finished>`
- Reader も typestate
  - `Fmp4Reader<ReaderReady>`
  - `Fmp4Reader<SyncReading>`
  - `Fmp4Reader<AsyncReading>`
  - `Fmp4Reader<Finished>`

## Features

- `async-session`
  - async API を有効化します
  - 内部実装は worker thread + channel です
- `backend-vt`
- `backend-nvidia`
- `backend-intel`
- `backend-vulkan`

## Writer Example

```rust
use std::num::{NonZeroU32, NonZeroUsize};

use anyhow::Result;
use video_hw::{Backend, Codec};
use video_hw_fmp4::{
    Fmp4Writer, Fmp4WriterConfig, FragmentFrames, FrameRate, FrameSize, Pts90k, Ready, RgbaFrame,
};

fn main() -> Result<()> {
    let frame_size = FrameSize::new(
        NonZeroU32::new(640).expect("non-zero width"),
        NonZeroU32::new(360).expect("non-zero height"),
    );
    let config = Fmp4WriterConfig {
        output_path: "output/example.mp4".into(),
        frame_size,
        frame_rate: FrameRate::new(NonZeroU32::new(30).expect("non-zero fps")),
        backend: Backend::Auto,
        codec: Codec::H264,
        require_hardware: false,
        intel_force_software: false,
        fragment_frames: FragmentFrames::new(
            NonZeroUsize::new(30).expect("non-zero fragment frames"),
        ),
    };

    let mut writer = Fmp4Writer::<Ready>::new(config).into_sync_session()?;
    let rgba = vec![0_u8; frame_size.pixel_count() * 4];
    writer.write_rgba(RgbaFrame::new(rgba, frame_size)?, Pts90k::new(0))?;
    let summary = writer.finish()?.into_summary();
    println!("wrote {}", summary.output_path.display());
    Ok(())
}
```

## Reader Example

```rust
use anyhow::Result;
use video_hw_fmp4::{Fmp4Reader, Fmp4ReaderConfig, Fmp4ReaderReady};

fn main() -> Result<()> {
    let mut reader = Fmp4Reader::<Fmp4ReaderReady>::new(Fmp4ReaderConfig {
        input_path: "output/example.mp4".into(),
    })
    .into_sync_session()?;

    while let Some(sample) = reader.next_sample()? {
        let _annexb = sample.to_annexb()?;
        println!("keyframe={}", sample.keyframe);
    }
    Ok(())
}
```

## Examples

- synthetic write

```bash
cargo run -p video-hw-fmp4 --example write_synthetic_fmp4 --features backend-vt
```

- file read

```bash
cargo run -p video-hw-fmp4 --example read_fmp4_file --features backend-vt -- output/synthetic-fmp4.mp4
```

- slider GUI read (seek/playback + decoded preview + backend select)

```bash
cargo run -p video-hw-fmp4 --example read_fmp4_slider_gui --features 'backend-nvidia backend-intel backend-vulkan' -- output/camera-fmp4/camera-recording-xxxxx.mp4 --backend auto --require-hardware
```

補足:
- preview decode は UI thread ではなく worker thread 側で実行します（UI の引っかかり軽減）。
- decode 結果は sample ごとにキャッシュし、次 sample は先読みします。
- 既定では backend fallback 有効で decode し、選択 backend で pixel 出力できないときは他 backend へフォールバックして preview 継続を試みます。
- `--strict-backend` を付けると選択 backend 固定で decode します。
- status は tracing で出力します。既定ログレベルは `warn`（warn/error のみ）で、`RUST_LOG=info` などを指定すると詳細ログを確認できます。
- 通常 MP4（non-fragmented）も読めます（例: `sample-videos/sample-10s.mp4`）。

ライトな確認だけ行う場合:

```bash
cargo run -p video-hw-fmp4 --example read_fmp4_slider_gui --features 'backend-nvidia backend-intel backend-vulkan' -- output/camera-fmp4/camera-recording-xxxxx.mp4 --smoke-test
```

通常 MP4 の例:

```bash
cargo run -p video-hw-fmp4 --example read_fmp4_slider_gui --features 'backend-nvidia backend-intel backend-vulkan' -- sample-videos/sample-10s.mp4 --backend nvidia --smoke-test
```

- headless camera record

```bash
cargo run -p video-hw-fmp4 --example camera_record_fmp4_headless --features 'backend-vt async-session' -- --list-devices
```

- GUI camera record

```bash
cargo run -p video-hw-fmp4 --example camera_record_fmp4_gui --features 'backend-vt async-session'
```

## Backend Test Steps

backend ごとに見るべきポイントは同じです。

1. build / test / clippy
2. synthetic writer で MP4 生成
3. reader で再読込
4. camera recorder を使う場合は実機で duration と再生可否を確認

### VideoToolbox (macOS)

```bash
cargo check -p video-hw-fmp4 --all-targets --features 'backend-vt async-session'
cargo test -p video-hw-fmp4 --all-targets --features 'backend-vt async-session'
cargo clippy -p video-hw-fmp4 --all-targets --features 'backend-vt async-session' -- -D warnings
```

```bash
cargo run -p video-hw-fmp4 --example write_synthetic_fmp4 --features 'backend-vt async-session' -- --output output/vt-h264.mp4 --codec h264
cargo run -p video-hw-fmp4 --example read_fmp4_file --features 'backend-vt async-session' -- output/vt-h264.mp4
ffprobe -hide_banner output/vt-h264.mp4
```

camera:

```bash
cargo run -p video-hw-fmp4 --example camera_record_fmp4_headless --features 'backend-vt async-session' -- --list-devices
cargo run -p video-hw-fmp4 --example camera_record_fmp4_gui --features 'backend-vt async-session'
```

確認項目:
- `ffprobe` の duration
- QuickTime/Finder の duration
- H.264 / HEVC の再生可否

### NVIDIA (Linux / Windows)

前提:
- NVIDIA GPU
- 対応 driver
- `video-hw` の NVIDIA backend が動く環境

```bash
cargo check -p video-hw-fmp4 --all-targets --features 'backend-nvidia async-session'
cargo test -p video-hw-fmp4 --all-targets --features 'backend-nvidia async-session'
cargo clippy -p video-hw-fmp4 --all-targets --features 'backend-nvidia async-session' -- -D warnings
```

```bash
cargo run -p video-hw-fmp4 --example write_synthetic_fmp4 --features 'backend-nvidia async-session' -- --output output/nvidia-h264.mp4 --codec h264
cargo run -p video-hw-fmp4 --example read_fmp4_file --features 'backend-nvidia async-session' -- output/nvidia-h264.mp4
ffprobe -hide_banner output/nvidia-h264.mp4
```

必要なら `--backend nvidia` を camera examples に明示します。

### Intel (Linux / Windows)

前提:
- Intel media stack / QSV が使える環境

```bash
cargo check -p video-hw-fmp4 --all-targets --features 'backend-intel async-session'
cargo test -p video-hw-fmp4 --all-targets --features 'backend-intel async-session'
cargo clippy -p video-hw-fmp4 --all-targets --features 'backend-intel async-session' -- -D warnings
```

```bash
cargo run -p video-hw-fmp4 --example write_synthetic_fmp4 --features 'backend-intel async-session' -- --output output/intel-h264.mp4 --codec h264
cargo run -p video-hw-fmp4 --example read_fmp4_file --features 'backend-intel async-session' -- output/intel-h264.mp4
ffprobe -hide_banner output/intel-h264.mp4
```

必要なら software fallback も見る:

```bash
cargo run -p video-hw-fmp4 --example camera_record_fmp4_headless --features 'backend-intel async-session' -- --backend intel --intel-force-software
```

### Vulkan (Linux / Windows)

前提:
- Vulkan encode/decode 対応環境

```bash
cargo check -p video-hw-fmp4 --all-targets --features 'backend-vulkan async-session'
cargo test -p video-hw-fmp4 --all-targets --features 'backend-vulkan async-session'
cargo clippy -p video-hw-fmp4 --all-targets --features 'backend-vulkan async-session' -- -D warnings
```

```bash
cargo run -p video-hw-fmp4 --example write_synthetic_fmp4 --features 'backend-vulkan async-session' -- --output output/vulkan-h264.mp4 --codec h264
cargo run -p video-hw-fmp4 --example read_fmp4_file --features 'backend-vulkan async-session' -- output/vulkan-h264.mp4
ffprobe -hide_banner output/vulkan-h264.mp4
```

## What To Compare Across Backends

- MP4 が生成されるか
- `Fmp4Reader` で再読込できるか
- `ffprobe` で codec / duration / fps が期待通りか
- camera record 時に duration がずれないか
- keyframe 間隔を変えたときに fragment 数が期待通りか
- H.264 / HEVC の両方で再生できるか

## Notes

- 録画 PTS は capture wallclock ではなく、設定 fps の固定刻みで生成する方が安定します。
  camera source によっては wallclock ベースの PTS で duration がずれます。
- QuickTime/Finder 向けの fragmented MP4 では、最終 duration は `mehd` にだけ入れ、
  `mvhd`/`tkhd`/`mdhd` は `0` にします。
  両方に入れると `ffprobe` は正しくても QuickTime/Finder がほぼ 2 倍で表示することがあります。
