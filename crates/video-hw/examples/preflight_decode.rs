use anyhow::{Context, Result};
use clap::Parser;
use video_hw::{Backend, Codec, DecodeOutputMode, DecodePreflightRequest, preflight_decode};

#[derive(Debug, Parser)]
#[command(about = "Report decoder preflight capability")]
struct Args {
    #[arg(long, default_value = "auto")]
    backend: String,

    #[arg(long, default_value = "h264")]
    codec: String,

    #[arg(long, default_value = "metadata")]
    output_mode: String,

    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    require_hardware: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let backend: Backend = parse_backend(&args.backend)?;
    let codec = parse_codec(&args.codec)?;
    let output_mode = parse_output_mode(&args.output_mode)?;
    let report = preflight_decode(DecodePreflightRequest {
        backend,
        codec,
        output_mode,
        require_hardware: args.require_hardware,
    });
    println!("requested_backend={}", report.requested_backend);
    println!(
        "resolved_backend={}",
        report
            .resolved_backend
            .map(|backend| backend.to_string())
            .unwrap_or_else(|| "none".to_string())
    );
    println!("codec={codec}");
    println!("output_mode={output_mode}");
    println!("require_hardware={}", args.require_hardware);
    println!("supported_by_contract={}", report.supported_by_contract);
    println!(
        "usable_in_current_runtime={}",
        report.usable_in_current_runtime
    );
    println!(
        "decode_supported={}",
        format_optional_bool(report.decode_supported)
    );
    println!(
        "hardware_acceleration={}",
        format_optional_bool(report.hardware_acceleration)
    );
    println!(
        "output_mode_supported={}",
        format_optional_bool(report.output_mode_supported)
    );
    if let Some(reason) = report.reason {
        println!("reason={reason}");
    }
    Ok(())
}

fn parse_backend(raw: &str) -> Result<Backend> {
    raw.parse()
        .with_context(|| format!("invalid backend: {raw}"))
}

fn parse_codec(raw: &str) -> Result<Codec> {
    match raw.to_ascii_lowercase().as_str() {
        "h264" => Ok(Codec::H264),
        "hevc" | "h265" => Ok(Codec::Hevc),
        "av1" => Ok(Codec::Av1),
        other => anyhow::bail!("unsupported codec: {other}"),
    }
}

fn parse_output_mode(raw: &str) -> Result<DecodeOutputMode> {
    match raw.to_ascii_lowercase().as_str() {
        "metadata" => Ok(DecodeOutputMode::Metadata),
        "nv12" => Ok(DecodeOutputMode::Nv12),
        "rgb24" => Ok(DecodeOutputMode::Rgb24),
        other => anyhow::bail!("unsupported output mode: {other}"),
    }
}

fn format_optional_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}
