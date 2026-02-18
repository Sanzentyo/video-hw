#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use std::fs;

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use std::time::Duration;

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use video_hw::{BackendKind, Codec, Decoder, DecoderConfig};

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn run_decode(codec: Codec, data: &[u8], chunk_bytes: usize, require_hardware: bool) {
    let mut decoder = Decoder::new(
        BackendKind::VideoToolbox,
        DecoderConfig {
            codec,
            fps: 30,
            require_hardware,
        },
    );

    for chunk in data.chunks(chunk_bytes.max(1)) {
        let _ = decoder
            .push_bitstream_chunk(chunk, None)
            .expect("decode chunk should succeed in benchmark");
    }
    let _ = decoder.flush().expect("flush should succeed in benchmark");
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn decode_benchmark(c: &mut Criterion) {
    let h264 = fs::read("sample-videos/sample-10s.h264")
        .expect("missing sample-videos/sample-10s.h264 for benchmark");
    let hevc = fs::read("sample-videos/sample-10s.h265")
        .expect("missing sample-videos/sample-10s.h265 for benchmark");

    let mut group = c.benchmark_group("decode_annexb");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(2));

    for (label, codec, data) in [
        ("h264", Codec::H264, &h264),
        ("hevc", Codec::Hevc, &hevc),
    ] {
        for require_hardware in [false, true] {
            let mode = if require_hardware { "hw_required" } else { "hw_optional" };
            for chunk_bytes in [4096usize, 1024 * 1024] {
            group.throughput(Throughput::Bytes(data.len() as u64));
            group.bench_with_input(
                BenchmarkId::new(label, format!("{mode}/chunk_{chunk_bytes}")),
                &chunk_bytes,
                |b, &chunk| {
                    b.iter(|| run_decode(codec, data, chunk, require_hardware));
                },
            );
            }
        }
    }

    group.finish();
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
criterion_group!(benches, decode_benchmark);
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
criterion_main!(benches);

#[cfg(not(all(target_os = "macos", feature = "backend-vt")))]
fn main() {}
