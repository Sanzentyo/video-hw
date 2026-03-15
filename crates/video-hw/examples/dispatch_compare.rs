use std::fs;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use video_hw::{
    Backend, BackendDecoderOptions, BackendEncoderOptions, BackendError, BitstreamInput, Codec,
    DecodeOutputMode, DecodeSession, DecoderConfig, Dimensions, EncodeFrame, EncodeSession,
    EncoderConfig, RawFrameBuffer, Timestamp90k,
};

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::{IntelDecoderAdapter, IntelEncoderAdapter};
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::{NvDecoderAdapter, NvEncoderAdapter};
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use video_hw::{VtDecoderAdapter, VtEncoderAdapter};

#[derive(Debug, Clone, Copy)]
struct Throughput {
    items: usize,
    elapsed: Duration,
}

impl Throughput {
    fn new(items: usize, elapsed: Duration) -> Self {
        Self { items, elapsed }
    }

    fn per_second(self) -> f64 {
        let elapsed_secs = self.elapsed.as_secs_f64();
        if elapsed_secs <= f64::EPSILON {
            0.0
        } else {
            self.items as f64 / elapsed_secs
        }
    }
}

type DecodeRunner = fn(Codec, &[u8], usize, usize, bool) -> Result<Throughput, BackendError>;
type EncodeRunner = fn(Codec, usize, usize, Dimensions, bool) -> Result<Throughput, BackendError>;

#[derive(Clone, Copy)]
struct BackendRunners {
    decode_enum: DecodeRunner,
    decode_static: DecodeRunner,
    encode_enum: EncodeRunner,
    encode_static: EncodeRunner,
}

struct CompareConfig<'a> {
    decode_samples: &'a [(Codec, &'a [u8])],
    codecs: &'a [Codec],
    decode_iterations: usize,
    decode_chunk_bytes: usize,
    encode_iterations: usize,
    encode_frames: usize,
    dims: Dimensions,
    require_hardware: bool,
}

fn main() -> Result<()> {
    let decode_iterations = env_usize("VIDEO_HW_COMPARE_DECODE_ITERS", 3);
    let decode_chunk_bytes = env_usize("VIDEO_HW_COMPARE_DECODE_CHUNK_BYTES", 4096);
    let encode_iterations = env_usize("VIDEO_HW_COMPARE_ENCODE_ITERS", 3);
    let encode_frames = env_usize("VIDEO_HW_COMPARE_ENCODE_FRAMES", 180);
    let require_hardware = env_bool("VIDEO_HW_COMPARE_REQUIRE_HARDWARE", true);
    let dims = Dimensions {
        width: NonZeroU32::new(env_u32("VIDEO_HW_COMPARE_WIDTH", 1280))
            .context("VIDEO_HW_COMPARE_WIDTH must be non-zero")?,
        height: NonZeroU32::new(env_u32("VIDEO_HW_COMPARE_HEIGHT", 720))
            .context("VIDEO_HW_COMPARE_HEIGHT must be non-zero")?,
    };

    let h264 =
        fs::read(sample_path("sample-10s.h264")).context("failed to read sample-10s.h264")?;
    let hevc =
        fs::read(sample_path("sample-10s.h265")).context("failed to read sample-10s.h265")?;
    let decode_samples: [(Codec, &[u8]); 2] = [(Codec::H264, &h264), (Codec::Hevc, &hevc)];
    let codecs = [Codec::H264, Codec::Hevc];

    println!(
        "dispatch-compare config: decode_iters={}, decode_chunk={}, encode_iters={}, encode_frames={}, dims={}x{}, require_hardware={}",
        decode_iterations,
        decode_chunk_bytes,
        encode_iterations,
        encode_frames,
        dims.width,
        dims.height,
        require_hardware
    );

    let mut compared_any = false;

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    {
        let config = CompareConfig {
            decode_samples: &decode_samples,
            codecs: &codecs,
            decode_iterations,
            decode_chunk_bytes,
            encode_iterations,
            encode_frames,
            dims,
            require_hardware,
        };
        compared_any |= compare_backend(
            "nvidia",
            BackendRunners {
                decode_enum: run_decode_enum_nvidia,
                decode_static: run_decode_static_nvidia,
                encode_enum: run_encode_enum_nvidia,
                encode_static: run_encode_static_nvidia,
            },
            &config,
        );
    }

    #[cfg(all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    {
        let config = CompareConfig {
            decode_samples: &decode_samples,
            codecs: &codecs,
            decode_iterations,
            decode_chunk_bytes,
            encode_iterations,
            encode_frames,
            dims,
            require_hardware,
        };
        compared_any |= compare_backend(
            "intel",
            BackendRunners {
                decode_enum: run_decode_enum_intel,
                decode_static: run_decode_static_intel,
                encode_enum: run_encode_enum_intel,
                encode_static: run_encode_static_intel,
            },
            &config,
        );
    }

    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    {
        let config = CompareConfig {
            decode_samples: &decode_samples,
            codecs: &codecs,
            decode_iterations,
            decode_chunk_bytes,
            encode_iterations,
            encode_frames,
            dims,
            require_hardware,
        };
        compared_any |= compare_backend(
            "vt",
            BackendRunners {
                decode_enum: run_decode_enum_vt,
                decode_static: run_decode_static_vt,
                encode_enum: run_encode_enum_vt,
                encode_static: run_encode_static_vt,
            },
            &config,
        );
    }

    if !compared_any {
        bail!("no backend pair available for enum/static comparison on this target");
    }

    Ok(())
}

fn compare_backend(
    backend_label: &str,
    runners: BackendRunners,
    config: &CompareConfig<'_>,
) -> bool {
    let mut compared = false;

    for &(codec, sample) in config.decode_samples {
        match (
            (runners.decode_enum)(
                codec,
                sample,
                config.decode_iterations,
                config.decode_chunk_bytes,
                config.require_hardware,
            ),
            (runners.decode_static)(
                codec,
                sample,
                config.decode_iterations,
                config.decode_chunk_bytes,
                config.require_hardware,
            ),
        ) {
            (Ok(enum_tp), Ok(static_tp)) => {
                compared = true;
                print_result("decode", backend_label, codec, enum_tp, static_tp);
            }
            (Err(enum_err), Err(static_err)) => {
                eprintln!(
                    "decode compare skipped backend={} codec={}: enum error={enum_err}, static error={static_err}",
                    backend_label, codec
                );
            }
            (Err(enum_err), Ok(_)) => {
                eprintln!(
                    "decode compare skipped backend={} codec={}: enum error={enum_err}",
                    backend_label, codec
                );
            }
            (Ok(_), Err(static_err)) => {
                eprintln!(
                    "decode compare skipped backend={} codec={}: static error={static_err}",
                    backend_label, codec
                );
            }
        }
    }

    for &codec in config.codecs {
        match (
            (runners.encode_enum)(
                codec,
                config.encode_iterations,
                config.encode_frames,
                config.dims,
                config.require_hardware,
            ),
            (runners.encode_static)(
                codec,
                config.encode_iterations,
                config.encode_frames,
                config.dims,
                config.require_hardware,
            ),
        ) {
            (Ok(enum_tp), Ok(static_tp)) => {
                compared = true;
                print_result("encode", backend_label, codec, enum_tp, static_tp);
            }
            (Err(enum_err), Err(static_err)) => {
                eprintln!(
                    "encode compare skipped backend={} codec={}: enum error={enum_err}, static error={static_err}",
                    backend_label, codec
                );
            }
            (Err(enum_err), Ok(_)) => {
                eprintln!(
                    "encode compare skipped backend={} codec={}: enum error={enum_err}",
                    backend_label, codec
                );
            }
            (Ok(_), Err(static_err)) => {
                eprintln!(
                    "encode compare skipped backend={} codec={}: static error={static_err}",
                    backend_label, codec
                );
            }
        }
    }

    compared
}

fn print_result(
    stage: &str,
    backend_label: &str,
    codec: Codec,
    enum_tp: Throughput,
    static_tp: Throughput,
) {
    let enum_fps = enum_tp.per_second();
    let static_fps = static_tp.per_second();
    let delta_percent = if enum_fps <= f64::EPSILON {
        0.0
    } else {
        ((static_fps - enum_fps) / enum_fps) * 100.0
    };

    println!(
        "{} backend={} codec={} enum={:.2} fps static={:.2} fps delta={:+.2}% (enum_items={}, static_items={})",
        stage,
        backend_label,
        codec,
        enum_fps,
        static_fps,
        delta_percent,
        enum_tp.items,
        static_tp.items
    );
}

fn run_decode_enum_backend(
    backend: Backend,
    codec: Codec,
    sample: &[u8],
    iterations: usize,
    chunk_bytes: usize,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    let mut decoded_frames = 0usize;
    let started = Instant::now();

    for _ in 0..iterations.max(1) {
        let mut decoder = DecodeSession::new(
            backend,
            DecoderConfig {
                codec,
                fps: 30,
                require_hardware,
                output_mode: DecodeOutputMode::Metadata,
                backend_options: BackendDecoderOptions::Default,
            },
        );

        for chunk in sample.chunks(chunk_bytes.max(1)) {
            loop {
                match decoder.submit(BitstreamInput::AnnexBChunk {
                    chunk: chunk.to_vec(),
                    pts_90k: None,
                }) {
                    Ok(()) => break,
                    Err(BackendError::TemporaryBackpressure(_)) => {
                        while decoder.try_reap()?.is_some() {
                            decoded_frames = decoded_frames.saturating_add(1);
                        }
                    }
                    Err(err) => return Err(err),
                }
            }
        }
        while decoder.try_reap()?.is_some() {
            decoded_frames = decoded_frames.saturating_add(1);
        }
        decoded_frames = decoded_frames.saturating_add(decoder.flush()?.len());
    }

    Ok(Throughput::new(decoded_frames, started.elapsed()))
}

fn run_encode_enum_backend(
    backend: Backend,
    codec: Codec,
    iterations: usize,
    frames_per_iteration: usize,
    dims: Dimensions,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    let mut submitted_frames = 0usize;
    let frame_data = synthetic_argb_frame(dims);
    let started = Instant::now();
    let fps = 30_i64;
    let pts_step = 90_000_i64 / fps;

    for _ in 0..iterations.max(1) {
        let mut encoder = EncodeSession::new(
            backend,
            EncoderConfig {
                codec,
                fps: fps as i32,
                require_hardware,
                backend_options: BackendEncoderOptions::Default,
            },
        );

        for frame_idx in 0..frames_per_iteration.max(1) {
            let frame = EncodeFrame {
                dims,
                pts_90k: Some(Timestamp90k((frame_idx as i64).saturating_mul(pts_step))),
                buffer: RawFrameBuffer::Argb8888Shared(Arc::clone(&frame_data)),
                force_keyframe: frame_idx == 0,
            };
            loop {
                match encoder.submit(frame.clone()) {
                    Ok(()) => {
                        submitted_frames = submitted_frames.saturating_add(1);
                        break;
                    }
                    Err(BackendError::TemporaryBackpressure(_)) => {
                        while encoder.try_reap()?.is_some() {}
                    }
                    Err(err) => return Err(err),
                }
            }
            while encoder.try_reap()?.is_some() {}
        }
        let _ = encoder.flush()?;
    }

    Ok(Throughput::new(submitted_frames, started.elapsed()))
}

fn synthetic_argb_frame(dims: Dimensions) -> Arc<[u8]> {
    let width = dims.width.get() as usize;
    let height = dims.height.get() as usize;
    let mut frame = vec![0_u8; width.saturating_mul(height).saturating_mul(4)];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 4;
            frame[idx] = 255;
            frame[idx + 1] = ((x * 255) / width.max(1)) as u8;
            frame[idx + 2] = ((y * 255) / height.max(1)) as u8;
            frame[idx + 3] = (((x + y) * 255) / (width + height).max(1)) as u8;
        }
    }
    Arc::from(frame)
}

fn sample_path(name: &str) -> PathBuf {
    let local = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("sample-videos")
        .join(name);
    if local.is_file() {
        return local;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join(name)
}

fn env_usize(name: &str, default_value: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default_value)
}

fn env_u32(name: &str, default_value: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default_value)
}

fn env_bool(name: &str, default_value: bool) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(default_value)
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
fn run_decode_enum_nvidia(
    codec: Codec,
    sample: &[u8],
    iterations: usize,
    chunk_bytes: usize,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    run_decode_enum_backend(
        Backend::Nvidia,
        codec,
        sample,
        iterations,
        chunk_bytes,
        require_hardware,
    )
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
fn run_decode_enum_intel(
    codec: Codec,
    sample: &[u8],
    iterations: usize,
    chunk_bytes: usize,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    run_decode_enum_backend(
        Backend::Intel,
        codec,
        sample,
        iterations,
        chunk_bytes,
        require_hardware,
    )
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn run_decode_enum_vt(
    codec: Codec,
    sample: &[u8],
    iterations: usize,
    chunk_bytes: usize,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    run_decode_enum_backend(
        Backend::VideoToolbox,
        codec,
        sample,
        iterations,
        chunk_bytes,
        require_hardware,
    )
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
fn run_encode_enum_nvidia(
    codec: Codec,
    iterations: usize,
    frames_per_iteration: usize,
    dims: Dimensions,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    run_encode_enum_backend(
        Backend::Nvidia,
        codec,
        iterations,
        frames_per_iteration,
        dims,
        require_hardware,
    )
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
fn run_encode_enum_intel(
    codec: Codec,
    iterations: usize,
    frames_per_iteration: usize,
    dims: Dimensions,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    run_encode_enum_backend(
        Backend::Intel,
        codec,
        iterations,
        frames_per_iteration,
        dims,
        require_hardware,
    )
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn run_encode_enum_vt(
    codec: Codec,
    iterations: usize,
    frames_per_iteration: usize,
    dims: Dimensions,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    run_encode_enum_backend(
        Backend::VideoToolbox,
        codec,
        iterations,
        frames_per_iteration,
        dims,
        require_hardware,
    )
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
fn run_decode_static_nvidia(
    codec: Codec,
    sample: &[u8],
    iterations: usize,
    chunk_bytes: usize,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    let mut decoded_frames = 0usize;
    let started = Instant::now();

    for _ in 0..iterations.max(1) {
        let mut decoder = DecodeSession::<NvDecoderAdapter>::new_static(DecoderConfig {
            codec,
            fps: 30,
            require_hardware,
            output_mode: DecodeOutputMode::Metadata,
            backend_options: BackendDecoderOptions::Default,
        });

        for chunk in sample.chunks(chunk_bytes.max(1)) {
            loop {
                match decoder.submit(BitstreamInput::AnnexBChunk {
                    chunk: chunk.to_vec(),
                    pts_90k: None,
                }) {
                    Ok(()) => break,
                    Err(BackendError::TemporaryBackpressure(_)) => {
                        while decoder.try_reap()?.is_some() {
                            decoded_frames = decoded_frames.saturating_add(1);
                        }
                    }
                    Err(err) => return Err(err),
                }
            }
        }
        while decoder.try_reap()?.is_some() {
            decoded_frames = decoded_frames.saturating_add(1);
        }
        decoded_frames = decoded_frames.saturating_add(decoder.flush()?.len());
    }

    Ok(Throughput::new(decoded_frames, started.elapsed()))
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
fn run_decode_static_intel(
    codec: Codec,
    sample: &[u8],
    iterations: usize,
    chunk_bytes: usize,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    let mut decoded_frames = 0usize;
    let started = Instant::now();

    for _ in 0..iterations.max(1) {
        let mut decoder = DecodeSession::<IntelDecoderAdapter>::new_static(DecoderConfig {
            codec,
            fps: 30,
            require_hardware,
            output_mode: DecodeOutputMode::Metadata,
            backend_options: BackendDecoderOptions::Default,
        });

        for chunk in sample.chunks(chunk_bytes.max(1)) {
            loop {
                match decoder.submit(BitstreamInput::AnnexBChunk {
                    chunk: chunk.to_vec(),
                    pts_90k: None,
                }) {
                    Ok(()) => break,
                    Err(BackendError::TemporaryBackpressure(_)) => {
                        while decoder.try_reap()?.is_some() {
                            decoded_frames = decoded_frames.saturating_add(1);
                        }
                    }
                    Err(err) => return Err(err),
                }
            }
        }
        while decoder.try_reap()?.is_some() {
            decoded_frames = decoded_frames.saturating_add(1);
        }
        decoded_frames = decoded_frames.saturating_add(decoder.flush()?.len());
    }

    Ok(Throughput::new(decoded_frames, started.elapsed()))
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn run_decode_static_vt(
    codec: Codec,
    sample: &[u8],
    iterations: usize,
    chunk_bytes: usize,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    let mut decoded_frames = 0usize;
    let started = Instant::now();

    for _ in 0..iterations.max(1) {
        let mut decoder = DecodeSession::<VtDecoderAdapter>::new_static(DecoderConfig {
            codec,
            fps: 30,
            require_hardware,
            output_mode: DecodeOutputMode::Metadata,
            backend_options: BackendDecoderOptions::Default,
        });

        for chunk in sample.chunks(chunk_bytes.max(1)) {
            loop {
                match decoder.submit(BitstreamInput::AnnexBChunk {
                    chunk: chunk.to_vec(),
                    pts_90k: None,
                }) {
                    Ok(()) => break,
                    Err(BackendError::TemporaryBackpressure(_)) => {
                        while decoder.try_reap()?.is_some() {
                            decoded_frames = decoded_frames.saturating_add(1);
                        }
                    }
                    Err(err) => return Err(err),
                }
            }
        }
        while decoder.try_reap()?.is_some() {
            decoded_frames = decoded_frames.saturating_add(1);
        }
        decoded_frames = decoded_frames.saturating_add(decoder.flush()?.len());
    }

    Ok(Throughput::new(decoded_frames, started.elapsed()))
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
fn run_encode_static_nvidia(
    codec: Codec,
    iterations: usize,
    frames_per_iteration: usize,
    dims: Dimensions,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    let mut submitted_frames = 0usize;
    let frame_data = synthetic_argb_frame(dims);
    let started = Instant::now();
    let fps = 30_i64;
    let pts_step = 90_000_i64 / fps;

    for _ in 0..iterations.max(1) {
        let mut encoder = EncodeSession::<NvEncoderAdapter>::new_static(EncoderConfig {
            codec,
            fps: fps as i32,
            require_hardware,
            backend_options: BackendEncoderOptions::Default,
        });

        for frame_idx in 0..frames_per_iteration.max(1) {
            let frame = EncodeFrame {
                dims,
                pts_90k: Some(Timestamp90k((frame_idx as i64).saturating_mul(pts_step))),
                buffer: RawFrameBuffer::Argb8888Shared(Arc::clone(&frame_data)),
                force_keyframe: frame_idx == 0,
            };
            loop {
                match encoder.submit(frame.clone()) {
                    Ok(()) => {
                        submitted_frames = submitted_frames.saturating_add(1);
                        break;
                    }
                    Err(BackendError::TemporaryBackpressure(_)) => {
                        while encoder.try_reap()?.is_some() {}
                    }
                    Err(err) => return Err(err),
                }
            }
            while encoder.try_reap()?.is_some() {}
        }
        let _ = encoder.flush()?;
    }

    Ok(Throughput::new(submitted_frames, started.elapsed()))
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
fn run_encode_static_intel(
    codec: Codec,
    iterations: usize,
    frames_per_iteration: usize,
    dims: Dimensions,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    let mut submitted_frames = 0usize;
    let frame_data = synthetic_argb_frame(dims);
    let started = Instant::now();
    let fps = 30_i64;
    let pts_step = 90_000_i64 / fps;

    for _ in 0..iterations.max(1) {
        let mut encoder = EncodeSession::<IntelEncoderAdapter>::new_static(EncoderConfig {
            codec,
            fps: fps as i32,
            require_hardware,
            backend_options: BackendEncoderOptions::Default,
        });

        for frame_idx in 0..frames_per_iteration.max(1) {
            let frame = EncodeFrame {
                dims,
                pts_90k: Some(Timestamp90k((frame_idx as i64).saturating_mul(pts_step))),
                buffer: RawFrameBuffer::Argb8888Shared(Arc::clone(&frame_data)),
                force_keyframe: frame_idx == 0,
            };
            loop {
                match encoder.submit(frame.clone()) {
                    Ok(()) => {
                        submitted_frames = submitted_frames.saturating_add(1);
                        break;
                    }
                    Err(BackendError::TemporaryBackpressure(_)) => {
                        while encoder.try_reap()?.is_some() {}
                    }
                    Err(err) => return Err(err),
                }
            }
            while encoder.try_reap()?.is_some() {}
        }
        let _ = encoder.flush()?;
    }

    Ok(Throughput::new(submitted_frames, started.elapsed()))
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn run_encode_static_vt(
    codec: Codec,
    iterations: usize,
    frames_per_iteration: usize,
    dims: Dimensions,
    require_hardware: bool,
) -> Result<Throughput, BackendError> {
    let mut submitted_frames = 0usize;
    let frame_data = synthetic_argb_frame(dims);
    let started = Instant::now();
    let fps = 30_i64;
    let pts_step = 90_000_i64 / fps;

    for _ in 0..iterations.max(1) {
        let mut encoder = EncodeSession::<VtEncoderAdapter>::new_static(EncoderConfig {
            codec,
            fps: fps as i32,
            require_hardware,
            backend_options: BackendEncoderOptions::Default,
        });

        for frame_idx in 0..frames_per_iteration.max(1) {
            let frame = EncodeFrame {
                dims,
                pts_90k: Some(Timestamp90k((frame_idx as i64).saturating_mul(pts_step))),
                buffer: RawFrameBuffer::Argb8888Shared(Arc::clone(&frame_data)),
                force_keyframe: frame_idx == 0,
            };
            loop {
                match encoder.submit(frame.clone()) {
                    Ok(()) => {
                        submitted_frames = submitted_frames.saturating_add(1);
                        break;
                    }
                    Err(BackendError::TemporaryBackpressure(_)) => {
                        while encoder.try_reap()?.is_some() {}
                    }
                    Err(err) => return Err(err),
                }
            }
            while encoder.try_reap()?.is_some() {}
        }
        let _ = encoder.flush()?;
    }

    Ok(Throughput::new(submitted_frames, started.elapsed()))
}
