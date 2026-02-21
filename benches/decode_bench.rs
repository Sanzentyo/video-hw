#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
use std::fs;

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
use std::time::Duration;

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
use video_hw::{
    Backend, BackendDecoderOptions, BackendError, BitstreamInput, Codec, DecodeSession,
    DecoderConfig,
};

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn run_decode(
    backend: Backend,
    codec: Codec,
    data: &[u8],
    chunk_bytes: usize,
    require_hardware: bool,
) -> Result<(), BackendError> {
    let mut decoder = DecodeSession::new(
        backend,
        DecoderConfig {
            codec,
            fps: 30,
            require_hardware,
            backend_options: BackendDecoderOptions::Default,
        },
    );

    for chunk in data.chunks(chunk_bytes.max(1)) {
        decoder.submit(BitstreamInput::AnnexBChunk {
            chunk: chunk.to_vec(),
            pts_90k: None,
        })?;
        while decoder.try_reap()?.is_some() {}
    }
    let _ = decoder.flush()?;
    Ok(())
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn decode_benchmark(c: &mut Criterion) {
    let h264 = fs::read("sample-videos/sample-10s.h264")
        .expect("missing sample-videos/sample-10s.h264 for benchmark");
    let hevc = fs::read("sample-videos/sample-10s.h265")
        .expect("missing sample-videos/sample-10s.h265 for benchmark");

    let mut group = c.benchmark_group("decode_annexb");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(2));

    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    let backends = vec![("vt", Backend::VideoToolbox)];
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    let backends = vec![("nv", Backend::Nvidia)];

    for (backend_label, backend) in backends {
        for (label, codec, data) in [("h264", Codec::H264, &h264), ("hevc", Codec::Hevc, &hevc)] {
            for require_hardware in [false, true] {
                let mode = if require_hardware {
                    "hw_required"
                } else {
                    "hw_optional"
                };
                for chunk_bytes in [4096usize, 1024 * 1024] {
                    group.throughput(Throughput::Bytes(data.len() as u64));
                    group.bench_with_input(
                        BenchmarkId::new(
                            format!("{backend_label}/{label}"),
                            format!("{mode}/chunk_{chunk_bytes}"),
                        ),
                        &chunk_bytes,
                        |b, &chunk| {
                            b.iter(|| {
                                run_decode(backend, codec, data, chunk, require_hardware)
                                    .expect("decode should succeed in benchmark");
                            });
                        },
                    );
                }
            }
        }
    }

    group.finish();
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
criterion_group!(benches, decode_benchmark);
#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
criterion_main!(benches);

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
fn main() {}
