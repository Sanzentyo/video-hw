use clap::Parser;
use video_hw::{default_encode_output, Codec, EncodeOptions, VtEncoder};

#[derive(Parser, Debug)]
#[command(about = "Encode synthetic BGRA frames with VideoToolbox")]
struct Args {
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long, default_value_t = 1280)]
    width: usize,
    #[arg(long, default_value_t = 720)]
    height: usize,
    #[arg(long, default_value_t = 120)]
    frames: usize,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long)]
    output: Option<String>,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let codec = Codec::from_str(&args.codec)?;
    let options = EncodeOptions {
        codec,
        width: args.width,
        height: args.height,
        frame_count: args.frames,
        fps: args.fps,
        require_hardware: args.require_hardware,
    };
    let output_path = args
        .output
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| default_encode_output(codec));

    let encoder = VtEncoder::new(&options)?;
    let packets = encoder.encode_synthetic(options.width, options.height, options.frame_count)?;
    VtEncoder::write_packets_to_file(&output_path, &packets)?;

    println!(
        "encoded_packets={}, bytes={}, output={}",
        packets.len(),
        packets.iter().map(|p| p.len()).sum::<usize>(),
        output_path.display()
    );
    Ok(())
}
