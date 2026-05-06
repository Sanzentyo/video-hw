#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
anyhow = "1"
clap = { version = "4.5", features = ["derive"] }
---

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};

#[derive(Debug, Parser)]
#[command(about = "Inspect AV1 low-overhead OBU frame headers for GOP/inter-frame diagnostics")]
struct Args {
    #[arg(long)]
    input: Option<PathBuf>,

    #[arg(long, value_enum, default_value_t = InputFormat::Obu)]
    input_format: InputFormat,

    #[arg(long, default_value = "ffmpeg")]
    ffmpeg: PathBuf,

    #[arg(long, default_value = "output/av1-frame-types")]
    output_dir: PathBuf,

    #[arg(long, default_value_t = 320)]
    width: u32,

    #[arg(long, default_value_t = 180)]
    height: u32,

    #[arg(long, default_value_t = 8)]
    frames: u32,

    #[arg(long, default_value_t = 30)]
    fps: u32,

    #[arg(long, default_value_t = 30)]
    gop_size: u32,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum InputFormat {
    Obu,
    Fmp4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObuType {
    TemporalDelimiter,
    SequenceHeader,
    FrameHeader,
    TileGroup,
    Frame,
    Other(u8),
}

#[derive(Debug)]
struct ObuRecord {
    obu_type: ObuType,
    payload_start: usize,
    payload_end: usize,
    temporal_unit_index: usize,
}

#[derive(Debug)]
struct FrameHeaderSummary {
    temporal_unit_index: usize,
    obu_type: ObuType,
    show_existing_frame: bool,
    frame_type: Option<u8>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.width == 0 || args.height == 0 || args.frames == 0 || args.fps == 0 || args.gop_size == 0 {
        bail!("--width/--height/--frames/--fps/--gop-size must be non-zero");
    }
    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create output dir: {}", args.output_dir.display()))?;
    let run_id = epoch_millis()?;
    let input = match &args.input {
        Some(path) => path.clone(),
        None => generate_input(&args, run_id)?,
    };
    let obu_input = match args.input_format {
        InputFormat::Obu => input.clone(),
        InputFormat::Fmp4 => extract_obu_from_fmp4(&args, &input, run_id)?,
    };
    let bitstream = fs::read(&obu_input)
        .with_context(|| format!("read AV1 OBU input: {}", obu_input.display()))?;
    let records = parse_obus(&bitstream).map_err(anyhow::Error::msg)?;
    let frame_headers = inspect_frame_headers(&bitstream, &records).map_err(anyhow::Error::msg)?;
    let report = write_report(&args, &input, &obu_input, &records, &frame_headers, run_id)?;
    println!(
        "av1_frame_types frame_headers={} has_inter_frame={} report={}",
        frame_headers.len(),
        frame_headers
            .iter()
            .any(|header| header.frame_type.is_some_and(|frame_type| frame_type != 0)),
        report.display()
    );
    Ok(())
}

fn generate_input(args: &Args, run_id: u128) -> Result<PathBuf> {
    let extension = match args.input_format {
        InputFormat::Obu => "obu",
        InputFormat::Fmp4 => "mp4",
    };
    let output = args
        .output_dir
        .join(format!("av1-frame-types-{run_id}.{extension}"));
    let source = format!("testsrc2=size={}x{}:rate={}", args.width, args.height, args.fps);
    let mut command = Command::new(&args.ffmpeg);
    command.args([
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        &source,
        "-frames:v",
        &args.frames.to_string(),
        "-an",
        "-c:v",
        "libaom-av1",
        "-cpu-used",
        "8",
        "-g",
        &args.gop_size.to_string(),
        "-lag-in-frames",
        "0",
    ]);
    match args.input_format {
        InputFormat::Obu => {
            command.args(["-f", "obu", &output.to_string_lossy()]);
        }
        InputFormat::Fmp4 => {
            command.args([
                "-movflags",
                "+frag_keyframe+empty_moov+delay_moov+default_base_moof",
                "-f",
                "mp4",
                &output.to_string_lossy(),
            ]);
        }
    }
    run_command(command, "generate FFmpeg AV1 input")?;
    Ok(output)
}

fn extract_obu_from_fmp4(args: &Args, input: &Path, run_id: u128) -> Result<PathBuf> {
    let output = args
        .output_dir
        .join(format!("av1-frame-types-{run_id}-extracted.obu"));
    let mut command = Command::new(&args.ffmpeg);
    command
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(input)
        .args(["-an", "-c:v", "copy", "-f", "obu"])
        .arg(&output);
    run_command(command, "extract AV1 OBU from fMP4")?;
    Ok(output)
}

fn run_command(mut command: Command, label: &str) -> Result<()> {
    let output = command.output().with_context(|| format!("spawn {label}"))?;
    if !output.status.success() {
        bail!(
            "{label} failed: status={}; stdout={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn parse_obus(bitstream: &[u8]) -> Result<Vec<ObuRecord>, String> {
    if bitstream.starts_with(b"DKIF") {
        return Err("AV1 IVF container input is not a low-overhead OBU stream".to_string());
    }
    let mut records = Vec::new();
    let mut cursor = 0usize;
    let mut temporal_unit_index = 0usize;
    let mut current_temporal_unit_has_payload = false;
    while cursor < bitstream.len() {
        let obu_start = cursor;
        let header = bitstream[cursor];
        cursor += 1;
        if header & 0x80 != 0 {
            return Err(format!("AV1 OBU at offset {obu_start} sets obu_forbidden_bit"));
        }
        if header & 0x01 != 0 {
            return Err(format!("AV1 OBU at offset {obu_start} sets reserved bit"));
        }
        let obu_type = obu_type((header >> 3) & 0x0f);
        if header & 0x04 != 0 {
            if cursor >= bitstream.len() {
                return Err(format!(
                    "AV1 OBU at offset {obu_start} is truncated before extension header"
                ));
            }
            cursor += 1;
        }
        if header & 0x02 == 0 {
            return Err(format!(
                "AV1 OBU at offset {obu_start} lacks low-overhead size field"
            ));
        }
        let (payload_len, leb_len) = read_leb128(&bitstream[cursor..])
            .map_err(|err| format!("AV1 OBU at offset {obu_start} has invalid size: {err}"))?;
        cursor += leb_len;
        let payload_start = cursor;
        let payload_end = payload_start
            .checked_add(payload_len)
            .ok_or_else(|| format!("AV1 OBU at offset {obu_start} size overflows usize"))?;
        if payload_end > bitstream.len() {
            return Err(format!(
                "AV1 OBU at offset {obu_start} is truncated: payload_end={payload_end}, len={}",
                bitstream.len()
            ));
        }
        if obu_type == ObuType::TemporalDelimiter && current_temporal_unit_has_payload {
            temporal_unit_index = temporal_unit_index.saturating_add(1);
            current_temporal_unit_has_payload = false;
        }
        records.push(ObuRecord {
            obu_type,
            payload_start,
            payload_end,
            temporal_unit_index,
        });
        if obu_type != ObuType::TemporalDelimiter {
            current_temporal_unit_has_payload = true;
        }
        cursor = payload_end;
    }
    if records.is_empty() {
        return Err("AV1 bitstream contains no OBUs".to_string());
    }
    Ok(records)
}

fn inspect_frame_headers(
    bitstream: &[u8],
    records: &[ObuRecord],
) -> Result<Vec<FrameHeaderSummary>, String> {
    records
        .iter()
        .filter(|record| matches!(record.obu_type, ObuType::Frame | ObuType::FrameHeader))
        .map(|record| {
            let payload = bitstream
                .get(record.payload_start..record.payload_end)
                .ok_or_else(|| "AV1 OBU payload range exceeds bitstream".to_string())?;
            let mut bits = BitReader::new(payload);
            let show_existing_frame = bits.read_bool("show_existing_frame")?;
            let frame_type = if show_existing_frame {
                None
            } else {
                Some(bits.read_bits_u8(2, "frame_type")?)
            };
            Ok(FrameHeaderSummary {
                temporal_unit_index: record.temporal_unit_index,
                obu_type: record.obu_type,
                show_existing_frame,
                frame_type,
            })
        })
        .collect()
}

fn write_report(
    args: &Args,
    input: &Path,
    obu_input: &Path,
    records: &[ObuRecord],
    frame_headers: &[FrameHeaderSummary],
    run_id: u128,
) -> Result<PathBuf> {
    let path = args
        .output_dir
        .join(format!("av1-frame-types-{run_id}.md"));
    let mut text = String::new();
    writeln!(&mut text, "# AV1 Frame Type Inspection")?;
    writeln!(&mut text, "epoch_millis: {run_id}")?;
    writeln!(&mut text, "input: {}", input.display())?;
    writeln!(&mut text, "obu_input: {}", obu_input.display())?;
    writeln!(&mut text, "input_format: {:?}", args.input_format)?;
    writeln!(&mut text, "width: {}", args.width)?;
    writeln!(&mut text, "height: {}", args.height)?;
    writeln!(&mut text, "frames: {}", args.frames)?;
    writeln!(&mut text, "gop_size: {}", args.gop_size)?;
    writeln!(&mut text, "obu_count: {}", records.len())?;
    writeln!(&mut text, "frame_header_count: {}", frame_headers.len())?;
    writeln!(
        &mut text,
        "has_inter_frame: {}",
        frame_headers
            .iter()
            .any(|header| header.frame_type.is_some_and(|frame_type| frame_type != 0))
    )?;
    writeln!(&mut text)?;
    writeln!(&mut text, "| Temporal Unit | OBU Type | show_existing_frame | frame_type |")?;
    writeln!(&mut text, "|---:|---|---:|---:|")?;
    for header in frame_headers {
        writeln!(
            &mut text,
            "| {} | {:?} | {} | {} |",
            header.temporal_unit_index,
            header.obu_type,
            header.show_existing_frame,
            header
                .frame_type
                .map(|frame_type| frame_type.to_string())
                .unwrap_or_else(|| "-".to_string())
        )?;
    }
    fs::write(&path, text).with_context(|| format!("write report: {}", path.display()))?;
    Ok(path)
}

fn obu_type(raw: u8) -> ObuType {
    match raw {
        1 => ObuType::SequenceHeader,
        2 => ObuType::TemporalDelimiter,
        3 => ObuType::FrameHeader,
        4 => ObuType::TileGroup,
        6 => ObuType::Frame,
        other => ObuType::Other(other),
    }
}

fn read_leb128(bytes: &[u8]) -> Result<(usize, usize), String> {
    let mut value = 0usize;
    for (index, byte) in bytes.iter().copied().enumerate().take(8) {
        let low = usize::from(byte & 0x7f);
        let shift = index
            .checked_mul(7)
            .ok_or_else(|| "LEB128 shift overflow".to_string())?;
        value |= low
            .checked_shl(u32::try_from(shift).map_err(|_| "LEB128 shift exceeds u32")?)
            .ok_or_else(|| "LEB128 value overflow".to_string())?;
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err("truncated or oversized LEB128".to_string())
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bool(&mut self, field: &str) -> Result<bool, String> {
        Ok(self.read_bits_u8(1, field)? != 0)
    }

    fn read_bits_u8(&mut self, count: usize, field: &str) -> Result<u8, String> {
        if count > 8 {
            return Err(format!("{field} requests more than 8 bits"));
        }
        let mut value = 0u8;
        for _ in 0..count {
            let byte_index = self.bit_pos / 8;
            let bit_in_byte = 7 - (self.bit_pos % 8);
            let byte = *self
                .data
                .get(byte_index)
                .ok_or_else(|| format!("{field} exceeds payload bits"))?;
            value = (value << 1) | ((byte >> bit_in_byte) & 1);
            self.bit_pos = self.bit_pos.saturating_add(1);
        }
        Ok(value)
    }
}

fn epoch_millis() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?
        .as_millis())
}
