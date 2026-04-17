set shell := ["powershell", "-NoLogo", "-Command"]

FFMPEG_BIN := "C:\\Users\\sanze\\AppData\\Local\\Microsoft\\WinGet\\Packages\\Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe\\ffmpeg-8.1-full_build\\bin"

# List available recipes
default:
    @just --list

# Format all crates
fmt:
    cargo fmt --all

# Lint all crates (all features, deny warnings)
clippy:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run all tests (all features, single-threaded, with output)
test:
    cargo test --workspace --all-features -- --nocapture --test-threads=1

# Run fmt + clippy + test
check: fmt clippy test

# Build all examples (release)
build-examples:
    cargo build --examples --all-features --release

# Run the NVIDIA decode benchmark (requires backend-nvidia)
bench-decode:
    cargo bench --package video-hw --features backend-nvidia --bench decode_bench -- --noplot

# Full CI validation (fmt + clippy + test + bench)
ci: check bench-decode

# Run the PSNR quality check script (requires nightly + built examples)
quality-check: build-examples
    $env:PATH = "{{FFMPEG_BIN}};$env:PATH"; cargo +nightly -Zscript scripts/quality_check.rs

# Generate sample videos from a Y4M source
# Usage: just gen-samples Y4M=C:\Temp\foreman_cif.y4m
gen-samples Y4M="C:\\Temp\\foreman_cif.y4m":
    $env:PATH = "{{FFMPEG_BIN}};$env:PATH"; ffmpeg -y -i "{{Y4M}}" -c:v libx264 -crf 20 -pix_fmt yuv420p -movflags +faststart sample-videos/foreman_cif.mp4
    $env:PATH = "{{FFMPEG_BIN}};$env:PATH"; ffmpeg -y -i sample-videos/foreman_cif.mp4 -c copy -movflags frag_keyframe+empty_moov+default_base_moof sample-videos/foreman_cif_fmp4.mp4
    $env:PATH = "{{FFMPEG_BIN}};$env:PATH"; ffmpeg -y -i sample-videos/foreman_cif.mp4 -c:v copy -bsf:v h264_mp4toannexb -f h264 sample-videos/foreman_cif.h264
    $env:PATH = "{{FFMPEG_BIN}};$env:PATH"; ffmpeg -y -i "{{Y4M}}" -c:v libx265 -crf 26 -pix_fmt yuv420p -x265-params "log-level=error" -f hevc sample-videos/foreman_cif.h265

# Decode and dump pixel frames using the decode_to_yuv example
# Usage: just decode-yuv BACKEND=intel CODEC=h264 MODE=rgb24
decode-yuv BACKEND="intel" CODEC="h264" MODE="rgb24":
    cargo run --example decode_to_yuv --all-features -- \
        --backend {{BACKEND}} --codec {{CODEC}} \
        --input sample-videos/foreman_cif.h264 \
        --output-mode {{MODE}} --output output/decoded_{{BACKEND}}_{{CODEC}}.{{MODE}}
