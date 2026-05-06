//! Decode a compressed video stream and write raw pixel frames to a file.
//!
//! Supports:
//! - AnnexB input (`.h264`, `.h265`): submitted as `BitstreamInput::AnnexBChunk`
//! - MP4/fMP4 input (`--input-format mp4`): demuxed per-sample via `shiguredo_mp4`
//!   and submitted as `BitstreamInput::LengthPrefixedSample` (exercises the AVCC/HVCC path)
//!
//! Output modes:
//! - `rgb24` – raw R8G8B8 bytes, one frame after another (no pitch padding)
//! - `nv12`  – raw NV12 bytes, pitch = width (no padding)
//! - `metadata` – no pixel output; only prints frame/dimension statistics

use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Parser;
use shiguredo_mp4::{TrackKind, boxes::SampleEntry};
use video_hw::{
    AnyDecodeSession, Backend, BackendDecoderOptions, BackendKind, BitstreamInput, Codec,
    DecodeOutputMode, DecodedFrame, DecoderConfig, IntelDecoderOptions, NvidiaDecoderOptions,
    VulkanDecoderOptions,
};

#[derive(Parser, Debug)]
#[command(about = "Decode a stream and write raw pixel output")]
struct Args {
    #[arg(long, default_value = "auto")]
    backend: String,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long)]
    input: PathBuf,
    /// annexb (default) or mp4
    #[arg(long, default_value = "annexb")]
    input_format: String,
    /// rgb24, nv12, or metadata (default: rgb24)
    #[arg(long, default_value = "rgb24")]
    output_mode: String,
    /// Output file for raw pixel data. Required unless --output-mode metadata.
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long, default_value_t = 65536)]
    chunk_bytes: usize,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
    #[arg(long, default_value_t = false)]
    intel_force_software: bool,
    #[arg(long)]
    nv_report_metrics: Option<bool>,
    #[arg(long)]
    vulkan_adapter_index: Option<usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let codec = parse_codec(&args.codec)?;
    let output_mode = parse_output_mode(&args.output_mode)?;
    let input_format = parse_input_format(&args.input_format)?;
    let backend: Backend = args.backend.parse()?;

    let mut config = DecoderConfig {
        codec,
        fps: args.fps,
        require_hardware: args.require_hardware,
        output_mode,
        backend_options: BackendDecoderOptions::Default,
    };
    let resolved = backend
        .resolve_decoder(&config)
        .context("failed to resolve decoder backend")?;
    config.backend_options = build_backend_options(resolved, &args);

    let mut output_writer: Option<BufWriter<File>> = match output_mode {
        DecodeOutputMode::Metadata => None,
        _ => {
            let path = args
                .output
                .as_ref()
                .context("--output is required unless --output-mode metadata")?;
            let f = File::create(path)
                .with_context(|| format!("failed to create output: {}", path.display()))?;
            Some(BufWriter::new(f))
        }
    };

    let stats = match input_format {
        InputFormat::AnnexB => decode_annexb(
            &args.input,
            args.chunk_bytes,
            resolved,
            config,
            &mut output_writer,
        )?,
        InputFormat::Mp4 => decode_mp4(&args.input, codec, resolved, config, &mut output_writer)?,
    };

    if let Some(writer) = output_writer.as_mut() {
        writer
            .flush()
            .context("failed to flush pixel output file")?;
    }

    println!(
        "frames={} width={} height={} output_mode={} backend={} codec={}",
        stats.frame_count,
        stats.width.unwrap_or(0),
        stats.height.unwrap_or(0),
        args.output_mode,
        resolved,
        args.codec,
    );

    Ok(())
}

#[derive(Debug, Default)]
struct DecodeStats {
    frame_count: usize,
    width: Option<usize>,
    height: Option<usize>,
}

fn decode_annexb(
    input: &PathBuf,
    chunk_bytes: usize,
    resolved: BackendKind,
    config: DecoderConfig,
    output: &mut Option<BufWriter<File>>,
) -> Result<DecodeStats> {
    let data = fs::read(input).with_context(|| format!("failed to read {}", input.display()))?;
    let step = chunk_bytes.max(1);
    let mut decoder = AnyDecodeSession::with_backend_kind(resolved, config)?;
    let mut stats = DecodeStats::default();

    for chunk in data.chunks(step) {
        loop {
            match decoder.submit(BitstreamInput::AnnexBChunk {
                chunk: chunk.to_vec(),
                pts_90k: None,
            }) {
                Ok(()) => break,
                Err(video_hw::BackendError::TemporaryBackpressure(_)) => {
                    drain_frames(&mut decoder, &mut stats, output)?;
                }
                Err(err) => return Err(err).context("submit AnnexBChunk failed"),
            }
        }
        drain_frames(&mut decoder, &mut stats, output)?;
    }
    flush_frames(&mut decoder, &mut stats, output)?;
    Ok(stats)
}

fn decode_mp4(
    input: &PathBuf,
    codec: Codec,
    resolved: BackendKind,
    config: DecoderConfig,
    output: &mut Option<BufWriter<File>>,
) -> Result<DecodeStats> {
    use shiguredo_mp4::demux::{DemuxError, Fmp4FileDemuxer};

    let bytes = fs::read(input).with_context(|| format!("failed to read {}", input.display()))?;

    let mut demuxer = Fmp4FileDemuxer::new();
    // Feed the entire file to the demuxer up front.
    feed_demuxer(&mut demuxer, &bytes)?;

    let mut decoder = AnyDecodeSession::with_backend_kind(resolved, config)?;
    let mut stats = DecodeStats::default();
    let mut pts_counter = 0i64;
    let mut current_sample_entry: Option<SampleEntry> = None;

    loop {
        let sample = match demuxer.next_sample() {
            Ok(Some(s)) => s,
            Ok(None) => break,
            Err(DemuxError::InputRequired(_)) => {
                // Feed any remaining required input and retry.
                feed_demuxer(&mut demuxer, &bytes)?;
                continue;
            }
            Err(err) => {
                return Err(anyhow::anyhow!("demux error: {err:?}"));
            }
        };

        if sample.track.kind != TrackKind::Video {
            continue;
        }

        // Update sample entry when the demuxer provides a new one.
        if let Some(entry) = sample.sample_entry.cloned() {
            current_sample_entry = Some(entry);
        }

        let start =
            usize::try_from(sample.data_offset).context("sample data offset exceeds usize")?;
        let end = start.saturating_add(sample.data_size);
        let raw_sample = bytes
            .get(start..end)
            .context("sample data range outside file")?;

        // For keyframes, prepend parameter sets (SPS/PPS for H.264, VPS/SPS/PPS for H.265)
        // as length-prefixed NALs so the decoder can initialize its codec context.
        let sample_bytes: Vec<u8> = if sample.keyframe {
            let mut buf = current_sample_entry
                .as_ref()
                .map(parameter_sets_as_length_prefixed)
                .unwrap_or_default();
            buf.extend_from_slice(raw_sample);
            buf
        } else {
            raw_sample.to_vec()
        };

        let pts_90k = Some(video_hw::Timestamp90k(pts_counter * 3000));
        pts_counter += 1;

        loop {
            match decoder.submit(BitstreamInput::LengthPrefixedSample {
                codec,
                sample: sample_bytes.clone(),
                pts_90k,
            }) {
                Ok(()) => break,
                Err(video_hw::BackendError::TemporaryBackpressure(_)) => {
                    drain_frames(&mut decoder, &mut stats, output)?;
                }
                Err(err) => {
                    return Err(err).context("submit LengthPrefixedSample failed");
                }
            }
        }
        drain_frames(&mut decoder, &mut stats, output)?;
    }
    flush_frames(&mut decoder, &mut stats, output)?;
    Ok(stats)
}

/// Feed the demuxer until it no longer requires more input from the given byte slice.
fn feed_demuxer(demuxer: &mut shiguredo_mp4::demux::Fmp4FileDemuxer, bytes: &[u8]) -> Result<()> {
    use shiguredo_mp4::demux::Input;
    while let Some(required) = demuxer.required_input() {
        let start = usize::try_from(required.position)
            .context("demuxer required input offset exceeds usize")?;
        let end = match required.size {
            Some(size) => start.saturating_add(size),
            None => bytes.len(),
        }
        .min(bytes.len());
        demuxer.handle_input(Input {
            position: required.position,
            data: bytes.get(start..end).unwrap_or(&[]),
        });
    }
    Ok(())
}

/// Build length-prefixed NAL units for all parameter sets stored in a sample entry.
///
/// For H.264 (`avc1`): SPS then PPS from the AVCC box.  
/// For H.265 (`hev1`/`hvc1`): every NAL array in the HVCC box.  
/// The resulting bytes are suitable for prepending to an AVCC/HVCC-format sample before
/// passing it to [`BitstreamInput::LengthPrefixedSample`].
fn parameter_sets_as_length_prefixed(entry: &SampleEntry) -> Vec<u8> {
    fn append_nalu(out: &mut Vec<u8>, nalu: &[u8]) {
        let len = u32::try_from(nalu.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nalu);
    }

    let mut out = Vec::new();
    match entry {
        SampleEntry::Avc1(avc1) => {
            for sps in &avc1.avcc_box.sps_list {
                append_nalu(&mut out, sps);
            }
            for pps in &avc1.avcc_box.pps_list {
                append_nalu(&mut out, pps);
            }
        }
        SampleEntry::Hev1(hev1) => {
            for arr in &hev1.hvcc_box.nalu_arrays {
                for nalu in &arr.nalus {
                    append_nalu(&mut out, nalu);
                }
            }
        }
        SampleEntry::Hvc1(hvc1) => {
            for arr in &hvc1.hvcc_box.nalu_arrays {
                for nalu in &arr.nalus {
                    append_nalu(&mut out, nalu);
                }
            }
        }
        _ => {}
    }
    out
}

fn drain_frames(
    decoder: &mut AnyDecodeSession,
    stats: &mut DecodeStats,
    output: &mut Option<BufWriter<File>>,
) -> Result<()> {
    while let Some(frame) = decoder.try_reap()? {
        process_frame(frame, stats, output)?;
    }
    Ok(())
}

fn flush_frames(
    decoder: &mut AnyDecodeSession,
    stats: &mut DecodeStats,
    output: &mut Option<BufWriter<File>>,
) -> Result<()> {
    for frame in decoder.flush()? {
        process_frame(frame, stats, output)?;
    }
    Ok(())
}

fn process_frame(
    frame: DecodedFrame,
    stats: &mut DecodeStats,
    output: &mut Option<BufWriter<File>>,
) -> Result<()> {
    stats.frame_count += 1;
    match frame {
        DecodedFrame::Metadata { dims, .. } => {
            if let Some(d) = dims {
                stats.width = Some(d.width.get() as usize);
                stats.height = Some(d.height.get() as usize);
            }
        }
        DecodedFrame::Rgb24 { data, dims, .. } => {
            stats.width = Some(dims.width.get() as usize);
            stats.height = Some(dims.height.get() as usize);
            if let Some(writer) = output.as_mut() {
                writer
                    .write_all(&data)
                    .context("failed to write RGB24 frame")?;
            }
        }
        DecodedFrame::Nv12 {
            data, pitch, dims, ..
        } => {
            let w = dims.width.get() as usize;
            let h = dims.height.get() as usize;
            stats.width = Some(w);
            stats.height = Some(h);
            if let Some(writer) = output.as_mut() {
                write_nv12_packed(writer, &data, pitch, w, h)?;
            }
        }
    }
    Ok(())
}

/// Write NV12 data to the output, removing any pitch padding so each row is exactly `width` bytes.
fn write_nv12_packed(
    writer: &mut BufWriter<File>,
    data: &[u8],
    pitch: usize,
    width: usize,
    height: usize,
) -> Result<()> {
    // Y plane
    for row in 0..height {
        let start = row * pitch;
        let end = start + width;
        let row_data = data.get(start..end).context("NV12 Y plane out of bounds")?;
        writer.write_all(row_data).context("write NV12 Y row")?;
    }
    // UV plane (height/2 rows of width bytes interleaved U,V)
    let uv_base = pitch * height;
    for row in 0..(height / 2) {
        let start = uv_base + row * pitch;
        let end = start + width;
        let row_data = data
            .get(start..end)
            .context("NV12 UV plane out of bounds")?;
        writer.write_all(row_data).context("write NV12 UV row")?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum InputFormat {
    AnnexB,
    Mp4,
}

fn parse_input_format(raw: &str) -> Result<InputFormat> {
    match raw.to_ascii_lowercase().as_str() {
        "annexb" | "h264" | "h265" | "hevc" => Ok(InputFormat::AnnexB),
        "mp4" | "fmp4" => Ok(InputFormat::Mp4),
        other => anyhow::bail!("unsupported input_format: {other}"),
    }
}

fn parse_output_mode(raw: &str) -> Result<DecodeOutputMode> {
    match raw.to_ascii_lowercase().as_str() {
        "metadata" => Ok(DecodeOutputMode::Metadata),
        "nv12" => Ok(DecodeOutputMode::Nv12),
        "rgb24" => Ok(DecodeOutputMode::Rgb24),
        other => anyhow::bail!("unsupported output_mode: {other}"),
    }
}

fn parse_codec(raw: &str) -> Result<Codec> {
    match raw.to_ascii_lowercase().as_str() {
        "h264" => Ok(Codec::H264),
        "hevc" | "h265" => Ok(Codec::Hevc),
        "av1" => Ok(Codec::Av1),
        other => anyhow::bail!("unsupported codec: {other}"),
    }
}

fn build_backend_options(resolved: BackendKind, args: &Args) -> BackendDecoderOptions {
    if backend_is_nvidia(resolved) {
        BackendDecoderOptions::Nvidia(NvidiaDecoderOptions {
            report_metrics: args.nv_report_metrics,
        })
    } else if backend_is_intel(resolved) {
        BackendDecoderOptions::Intel(IntelDecoderOptions {
            force_software: args.intel_force_software,
        })
    } else if backend_is_vulkan(resolved) {
        BackendDecoderOptions::Vulkan(VulkanDecoderOptions {
            adapter_index: args.vulkan_adapter_index,
            ..Default::default()
        })
    } else {
        BackendDecoderOptions::Default
    }
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
fn backend_is_nvidia(backend: BackendKind) -> bool {
    matches!(backend, BackendKind::Nvidia)
}

#[cfg(not(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
)))]
fn backend_is_nvidia(_backend: BackendKind) -> bool {
    false
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
fn backend_is_intel(backend: BackendKind) -> bool {
    matches!(backend, BackendKind::Intel)
}

#[cfg(not(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
)))]
fn backend_is_intel(_backend: BackendKind) -> bool {
    false
}

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
fn backend_is_vulkan(backend: BackendKind) -> bool {
    matches!(backend, BackendKind::Vulkan)
}

#[cfg(not(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
)))]
fn backend_is_vulkan(_backend: BackendKind) -> bool {
    false
}
