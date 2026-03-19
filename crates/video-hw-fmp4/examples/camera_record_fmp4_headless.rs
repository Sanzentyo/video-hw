use std::{
    borrow::Cow,
    num::{NonZeroU32, NonZeroUsize},
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::Parser;
use shiguredo_video_device::{
    PixelFormat, VideoCapture, VideoCaptureConfig, VideoDeviceList, VideoFrame,
};
use video_hw::{Backend, Codec};
use video_hw_fmp4::{
    Fmp4Writer, Fmp4WriterConfig, FragmentFrames, FrameRate, FrameSize, Pts90k, Ready, RgbaFrame,
};

#[derive(Debug, Parser)]
struct Cli {
    #[arg(long)]
    list_devices: bool,
    #[arg(long)]
    video_device_id: Option<String>,
    #[arg(long, default_value = "640x360")]
    resolution: String,
    #[arg(long, default_value_t = 30)]
    fps: u32,
    #[arg(long, default_value_t = 8.0)]
    duration: f64,
    #[arg(long, default_value = "auto")]
    backend: String,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
    #[arg(long, default_value_t = false)]
    intel_force_software: bool,
    #[arg(long, default_value_t = 30)]
    fragment_frames: usize,
    #[arg(long, default_value = "output/camera-fmp4-headless")]
    output_dir: PathBuf,
}

#[derive(Debug)]
struct CapturedFrame {
    rgba: Vec<u8>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.list_devices {
        list_devices();
        return Ok(());
    }
    let (width, height) = parse_resolution(&cli.resolution)
        .with_context(|| format!("invalid resolution: {}", cli.resolution))?;
    let frame_size = FrameSize::new(
        NonZeroU32::new(width).context("width must be > 0")?,
        NonZeroU32::new(height).context("height must be > 0")?,
    );
    let codec = parse_codec(&cli.codec)?;
    let backend: Backend = cli.backend.parse()?;
    let output_path = cli.output_dir.join("camera-recording.mp4");
    let config = Fmp4WriterConfig {
        output_path,
        frame_size,
        frame_rate: FrameRate::new(NonZeroU32::new(cli.fps).context("fps must be > 0")?),
        backend,
        codec,
        require_hardware: cli.require_hardware,
        intel_force_software: cli.intel_force_software,
        fragment_frames: FragmentFrames::new(
            NonZeroUsize::new(cli.fragment_frames).context("fragment_frames must be > 0")?,
        ),
    };
    let mut writer = Fmp4Writer::<Ready>::new(config).into_sync_session()?;

    let (tx, rx) = mpsc::channel::<CapturedFrame>();
    let mut capture = VideoCapture::new(
        VideoCaptureConfig {
            device_id: cli.video_device_id,
            width: i32::try_from(width).context("width must fit in i32")?,
            height: i32::try_from(height).context("height must fit in i32")?,
            fps: i32::try_from(cli.fps).context("fps must fit in i32")?,
            pixel_format: None,
        },
        move |frame: VideoFrame<'_>| {
            if let Some(rgba) = frame_to_rgba(&frame) {
                let _ = tx.send(CapturedFrame { rgba });
            }
        },
    )
    .context("failed to create video capture")?;
    capture.start().context("failed to start video capture")?;

    let mut recording_started_at = None;
    let mut deadline: Option<Instant> = None;
    let mut submitted_frames = 0_u64;
    let frame_duration_90k = fps_frame_duration_90k(cli.fps);
    loop {
        let timeout = deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|| Duration::from_millis(250));
        if deadline.is_some() && timeout.is_zero() {
            break;
        }
        let Ok(frame) = rx.recv_timeout(timeout.min(Duration::from_millis(250))) else {
            continue;
        };
        let started_at = recording_started_at.get_or_insert_with(Instant::now);
        // Derive PTS from the configured FPS cadence instead of camera wallclock. Device timestamps
        // were not stable enough for container duration on every capture source.
        let pts_90k = submitted_frames.saturating_mul(frame_duration_90k);
        writer.write_rgba(
            RgbaFrame::new(frame.rgba, frame_size)?,
            Pts90k::new(pts_90k),
        )?;
        submitted_frames = submitted_frames.saturating_add(1);
        if deadline.is_none() {
            *started_at = Instant::now();
            deadline = Some(*started_at + Duration::from_secs_f64(cli.duration));
        }
    }
    capture.stop();
    let summary = writer.finish()?.into_summary();
    println!(
        "wrote {} (segments={}, packets={}, bytes={}, duration_90k={})",
        summary.output_path.display(),
        summary.segments_written,
        summary.packets_seen,
        summary.bytes_written,
        summary.duration_90k,
    );
    Ok(())
}

fn parse_resolution(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn parse_codec(raw: &str) -> Result<Codec> {
    match raw.to_ascii_lowercase().as_str() {
        "h264" => Ok(Codec::H264),
        "hevc" | "h265" => Ok(Codec::Hevc),
        other => anyhow::bail!("unsupported codec: {other}"),
    }
}

fn list_devices() {
    println!("=== video devices ===");
    match VideoDeviceList::enumerate() {
        Ok(devices) => {
            if devices.is_empty() {
                println!("no video devices found");
            } else {
                for device in devices.devices() {
                    let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
                    let id = device.unique_id().unwrap_or_else(|_| "Unknown".to_string());
                    println!("{name}");
                    println!("  id: {id}");
                    for fmt in device.formats() {
                        println!(
                            "  {}x{} @ {:.0}-{:.0} fps ({})",
                            fmt.width,
                            fmt.height,
                            fmt.min_fps,
                            fmt.max_fps,
                            fmt.pixel_format.name()
                        );
                    }
                }
            }
        }
        Err(err) => eprintln!("failed to enumerate video devices: {err:?}"),
    }
}

fn fps_frame_duration_90k(fps: u32) -> u64 {
    90_000 / u64::from(fps.max(1))
}

fn frame_to_rgba(frame: &VideoFrame<'_>) -> Option<Vec<u8>> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let stride = frame.stride as usize;
    let stride_uv = frame.stride_uv as usize;
    Some(match frame.pixel_format {
        PixelFormat::Nv12 => {
            let y = strip_stride(frame.data, w, h, stride);
            let uv_data = frame.uv_data?;
            let uv = strip_stride(uv_data, w, h / 2, stride_uv);
            nv12_to_rgba(&y, &uv, w, h)
        }
        PixelFormat::I420 => {
            let y = strip_stride(frame.data, w, h, stride);
            let uv_data = frame.uv_data?;
            let uv_w = w / 2;
            let uv_h = h / 2;
            let u_plane_size = stride_uv * uv_h;
            if uv_data.len() < u_plane_size * 2 {
                return None;
            }
            let u = strip_stride(&uv_data[..u_plane_size], uv_w, uv_h, stride_uv);
            let v = strip_stride(
                &uv_data[u_plane_size..u_plane_size * 2],
                uv_w,
                uv_h,
                stride_uv,
            );
            i420_to_rgba(&y, &u, &v, w, h)
        }
        PixelFormat::Yuy2 => {
            let data = strip_stride(frame.data, w * 2, h, stride);
            yuy2_to_rgba(&data, w, h)
        }
        PixelFormat::Unknown(_) => return None,
    })
}

fn strip_stride<'a>(
    data: &'a [u8],
    row_bytes: usize,
    height: usize,
    stride: usize,
) -> Cow<'a, [u8]> {
    if stride == row_bytes {
        Cow::Borrowed(&data[..row_bytes * height])
    } else {
        let mut result = Vec::with_capacity(row_bytes * height);
        for row in 0..height {
            let start = row * stride;
            result.extend_from_slice(&data[start..start + row_bytes]);
        }
        Cow::Owned(result)
    }
}

fn nv12_to_rgba(y_plane: &[u8], uv_plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let y_row = row * w;
        let uv_row = (row / 2) * w;
        for col in 0..w {
            let y = y_plane[y_row + col];
            let uv_index = uv_row + (col / 2) * 2;
            let u = uv_plane[uv_index];
            let v = uv_plane[uv_index + 1];
            let (r, g, b) = yuv_to_rgb(y, u, v);
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

fn i420_to_rgba(y_plane: &[u8], u_plane: &[u8], v_plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    let uv_w = w / 2;
    for row in 0..h {
        let y_row = row * w;
        let uv_row = (row / 2) * uv_w;
        for col in 0..w {
            let y = y_plane[y_row + col];
            let uv_index = uv_row + (col / 2);
            let u = u_plane[uv_index];
            let v = v_plane[uv_index];
            let (r, g, b) = yuv_to_rgb(y, u, v);
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

fn yuy2_to_rgba(data: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let row_start = row * w * 2;
        let row_data = &data[row_start..row_start + w * 2];
        for chunk in row_data.chunks_exact(4) {
            let y0 = chunk[0];
            let u = chunk[1];
            let y1 = chunk[2];
            let v = chunk[3];
            let (r0, g0, b0) = yuv_to_rgb(y0, u, v);
            let (r1, g1, b1) = yuv_to_rgb(y1, u, v);
            out.extend_from_slice(&[r0, g0, b0, 255]);
            out.extend_from_slice(&[r1, g1, b1, 255]);
        }
    }
    out
}

fn yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let c = (y as i32 - 16).max(0);
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let r = (298 * c + 409 * e + 128) >> 8;
    let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
    let b = (298 * c + 516 * d + 128) >> 8;
    (clamp_u8(r), clamp_u8(g), clamp_u8(b))
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}
