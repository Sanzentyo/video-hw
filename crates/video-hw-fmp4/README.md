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
- `serde`
  - reader metadata/report 型の `Serialize` / `Deserialize` を有効化します
  - `shiguredo_mp4::SampleEntry` は外部 runtime 型のため serde 対象外です。`Fmp4Track` / `EncodedSample` の `sample_entry` は serialization では省略され、deserialization では `None` になります
  - `Mp4IndexSnapshot::track_descriptions` には codec / layout / NAL length size / parameter sets / basic audio-video fields を含む軽量 description が入ります
  - `DecodeDiagnostics` は backend と output mode を文字列として serialize します。deserialize では有効化済み backend feature に含まれる `resolved_backend` だけを復元できます

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
use video_hw_fmp4::{Fmp4Reader, Fmp4ReaderConfig, Fmp4ReaderReady, TrackKind};

fn main() -> Result<()> {
    let mut reader = Fmp4Reader::<Fmp4ReaderReady>::new(Fmp4ReaderConfig::new(
        "output/example.mp4",
    ))
    .into_sync_session()?;

    let tracks = reader.tracks().to_vec();
    for track in &tracks {
        println!("track={} samples={}", track.track_id, reader.samples(track.track_id)?.len());
    }

    let Some(video_track) = tracks
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .map(|track| track.track_id)
    else {
        return Ok(());
    };
    let Some(target) = reader.samples(video_track)?.first().map(|sample| sample.sample_id) else {
        return Ok(());
    };

    for sample in reader.iter_gop_for_sample(target)? {
        let sample = sample?;
        let _annexb = sample.to_annexb()?;
        println!("sample={} keyframe={}", sample.meta.sample_id, sample.meta.keyframe);
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

任意の sample payload だけ読む場合:

```bash
cargo run -p video-hw-fmp4 --example read_fmp4_file -- sample-videos/sample-10s.mp4 --sample-id 0
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
- GOP replay + decode submit/reap は crate 側の `FrameDecoder` が担当し、example 側は preview/cache 方針だけを持ちます。
- 人物検出、HISDF 解釈、bbox crop、tracking、検証 artifact 保存は `video-hw-fmp4` の責務外です。上位層は `sample_at_pts`、`GopCursor`、`decode_range`、`cache_stats` を組み合わせて必要な frame/window を取得します。
- 既定は `IndexMode::Eager` です。`IndexMode::Lazy` は `next_sample` や `read_sample` で必要分だけ metadata index を延長し、`samples(track)` のような完全な slice を返す API では EOF まで index 化します。
- `sample_at_pts_with_delta` は `SampleLookupMatch::{Exact,Previous,FirstAfter}` で完全一致/近傍一致を返します。長い区間は `decode_range_iter`、中心sample周辺の短窓は `decode_window` を使えます。
- `FrameDecoder::decode_window` と `decode_range` / `decode_sample` の `frames` は fMP4 sample metadata の PTS 順、つまり presentation order で返します。H.264 B-frame などで sample id / DTS / decoder submit order が前後しても、返却順は backend 由来の `DecodedFrame::pts_90k` だけに依存せず、reader が保持する `SampleMeta` の `pts` と `sample_id` から決まります。
- `DecodedSampleFrame` は `sample_id`、fMP4 由来の `sample_meta`（`pts` / `dts` / `duration` など）、返却ベクタ内の `presentation_index`、backend の `frame` を持ちます。decode-order が必要な caller は `sample_meta.dts` または `sample_id` / `sample_meta` から明示的に並べ替えてください。
- `DecodeDiagnostics` は `returned_frame_order`、要求 sample 数、decoder から得た frame 数、返却 frame 数、sample metadata 付き frame 数、drop/unmatched 数、不確実な fallback sample association 数、missing sample ids を返します。`decode_window` / `decode_range` / `decode_sample` の返却ベクタは `ReturnedFrameOrder::Presentation` として報告されます。
- `EncodedSample::to_annexb()` は MP4 sample entry の NAL length size（1/2/4 byte）に従います。`index_snapshot()` は metadata report 用、`clear_cache()` は range cache の明示解放用です。
- `status()` は global / track別 / sample別の payload read stats と range cache stats を返します。`DecodeDiagnostics` は要求 backend、実解決 backend、output mode、fallback 有無と理由も返します。
- decode access pattern の性能を見る場合は `scripts/benchmark_fmp4_decode_access.rs` を使って、`decode_range_iter`、単発 `decode_sample`、caller-side LRU 付き `decode_window` を比較できます。レポートには encoded payload read 数、byte range cache hit/miss、FFmpeg RGB24 reference との MSE/PSNR が入ります。
- `serde` feature 有効時は `SampleMeta`、`Mp4IndexSnapshot`、`SampleLookup`、`Fmp4ReaderStatus` などの metadata/report 型を JSON 等へ保存できます。
- async reader は `samples` / `sample_meta` / `sample_at_pts_with_delta` / `read_sample` / `read_gop` / `read_segment` / `index_snapshot` / cache/status 系 API を worker 経由で利用できます。borrow を返せないため metadata slice は `Vec<SampleMeta>` として返します。

ライトな確認だけ行う場合:

```bash
cargo run -p video-hw-fmp4 --example read_fmp4_slider_gui --features 'backend-nvidia backend-intel backend-vulkan' -- output/camera-fmp4/camera-recording-xxxxx.mp4 --smoke-test
```

通常 MP4 の例:

```bash
cargo run -p video-hw-fmp4 --example read_fmp4_slider_gui --features 'backend-nvidia backend-intel backend-vulkan' -- sample-videos/sample-10s.mp4 --backend nvidia --smoke-test
```

Lazy/Eager index と任意の decode smoke をまとめて見る場合:

```bash
cargo +nightly -Zscript scripts/verify_fmp4_lazy.rs sample-videos/sample-10s.mp4
cargo +nightly -Zscript scripts/verify_fmp4_lazy.rs sample-videos/sample-10s.mp4 --decode-features 'backend-nvidia backend-intel backend-vulkan' --decode-backend auto
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
