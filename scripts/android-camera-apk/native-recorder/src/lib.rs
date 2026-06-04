use std::{
    fs::File,
    io::Write,
    num::{NonZeroU32, NonZeroUsize},
    slice,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use jni::{
    JNIEnv,
    objects::{JByteBuffer, JClass, JString},
    sys::{jboolean, jint, jlong, jstring},
};
use video_hw::{
    AndroidDecoderOptions, AndroidEncoderOptions, AnyDecodeSession, AnyEncodeSession, Backend,
    BackendDecoderOptions, BackendEncoderOptions, BackendError, BitstreamInput, Codec,
    DecodeOutputMode, DecodedFrame, DecoderConfig, Dimensions, EncodeFrame, EncodeInputFormat,
    EncodedChunk, EncoderConfig, RawFrameBuffer, Timestamp90k,
};
use video_hw_fmp4::{
    EncodedTrackConfig, Fmp4Writer, FragmentFrames, FrameRate, FrameSize, Ready, SampleDuration90k,
    SyncEncodedRecording,
};

struct RustCameraRecorder {
    encoder: AnyEncodeSession,
    writer: Fmp4Writer<SyncEncodedRecording>,
    raw_file: File,
    chunks: Vec<EncodedChunk>,
    width: usize,
    height: usize,
    fps: u32,
    frame_index: u64,
    packets: u64,
    bytes: u64,
    keyframes: u64,
    output_path: String,
    raw_path: String,
}

impl RustCameraRecorder {
    fn new(output_path: String, width: u32, height: u32, fps: u32, bitrate: u32) -> Result<Self> {
        let frame_size = FrameSize::new(
            NonZeroU32::new(width).context("width must be non-zero")?,
            NonZeroU32::new(height).context("height must be non-zero")?,
        );
        let fps = fps.max(1);
        let mut config =
            EncoderConfig::new(Codec::H264, fps as i32, false, EncodeInputFormat::Nv12);
        config.backend_options = BackendEncoderOptions::Android(AndroidEncoderOptions {
            bitrate: Some(bitrate),
            i_frame_interval_sec: Some(1),
            ..Default::default()
        });
        let encoder = AnyEncodeSession::new(Backend::Android, config)?;
        let raw_path = output_path.strip_suffix(".mp4").map_or_else(
            || format!("{output_path}.h264"),
            |base| format!("{base}.h264"),
        );
        let raw_file = File::create(&raw_path)?;
        let writer = Fmp4Writer::<Ready>::new(video_hw_fmp4::Fmp4WriterConfig {
            output_path: output_path.clone().into(),
            frame_size,
            frame_rate: FrameRate::new(NonZeroU32::new(fps).context("fps must be non-zero")?),
            backend: Backend::Android,
            codec: Codec::H264,
            require_hardware: false,
            intel_force_software: false,
            fragment_frames: FragmentFrames::new(NonZeroUsize::new(fps as usize).unwrap()),
        })
        .into_sync_encoded_session(EncodedTrackConfig {
            output_path: output_path.clone().into(),
            frame_size,
            frame_rate: FrameRate::new(NonZeroU32::new(fps).unwrap()),
            codec: Codec::H264,
            fragment_frames: FragmentFrames::new(NonZeroUsize::new(fps as usize).unwrap()),
            initial_parameter_sets: None,
        })?;
        Ok(Self {
            encoder,
            writer,
            raw_file,
            chunks: Vec::new(),
            width: width as usize,
            height: height as usize,
            fps,
            frame_index: 0,
            packets: 0,
            bytes: 0,
            keyframes: 0,
            output_path,
            raw_path,
        })
    }

    fn push_yuv420(
        &mut self,
        y: Plane<'_>,
        u: Plane<'_>,
        v: Plane<'_>,
        pts_ns: i64,
        force_keyframe: bool,
    ) -> Result<u64> {
        let nv12 = yuv420_to_nv12(self.width, self.height, y, u, v)?;
        let dims = Dimensions {
            width: NonZeroU32::new(self.width as u32).unwrap(),
            height: NonZeroU32::new(self.height as u32).unwrap(),
        };
        let pts_90k = if pts_ns > 0 {
            Some(Timestamp90k(pts_ns.saturating_mul(90_000) / 1_000_000_000))
        } else {
            Some(Timestamp90k(
                i64::try_from(self.frame_index.saturating_mul(90_000) / u64::from(self.fps))
                    .unwrap_or(i64::MAX),
            ))
        };
        while let Some(chunk) = self.encoder.try_reap()? {
            self.write_chunk(chunk)?;
        }
        match self.encoder.submit(EncodeFrame {
            dims,
            pts_90k,
            buffer: RawFrameBuffer::Nv12 {
                pitch: self.width,
                data: nv12,
            },
            force_keyframe: force_keyframe || self.frame_index == 0,
        }) {
            Ok(()) => {}
            Err(BackendError::TemporaryBackpressure(_)) => return Ok(self.frame_index),
            Err(err) => return Err(err.into()),
        }
        self.frame_index = self.frame_index.saturating_add(1);
        while let Some(chunk) = self.encoder.try_reap()? {
            self.write_chunk(chunk)?;
        }
        Ok(self.frame_index)
    }

    fn finish(mut self) -> Result<String> {
        for chunk in self.encoder.flush()? {
            self.write_chunk(chunk)?;
        }
        self.raw_file.flush()?;
        let (mp4_status, mp4_error, mp4_bytes, duration_90k) = match self.writer.finish() {
            Ok(finished) => {
                let summary = finished.into_summary();
                (
                    "PASS".to_string(),
                    String::new(),
                    summary.bytes_written,
                    summary.duration_90k,
                )
            }
            Err(error) => ("FAIL".to_string(), escape_json(&error.to_string()), 0, 0),
        };
        let (decode_status, decode_error, decoded_frames) =
            match decode_chunks(&self.chunks, self.width, self.height, self.fps) {
                Ok(frames) if frames > 0 => ("PASS".to_string(), String::new(), frames),
                Ok(_) => ("FAIL".to_string(), "decoded no frames".to_string(), 0_usize),
                Err(error) => ("FAIL".to_string(), escape_json(&error.to_string()), 0_usize),
            };
        let status = if mp4_status == "PASS" && decode_status == "PASS" {
            "PASS"
        } else {
            "FAIL"
        };
        Ok(format!(
            "{{\"status\":\"{}\",\"path\":\"{}\",\"raw_path\":\"{}\",\"mp4_status\":\"{}\",\"mp4_error\":\"{}\",\"decode_status\":\"{}\",\"decode_error\":\"{}\",\"width\":{},\"height\":{},\"fps\":{},\"frames_in\":{},\"packets\":{},\"bytes\":{},\"keyframes\":{},\"decoded_frames\":{},\"mp4_bytes\":{},\"duration_90k\":{}}}",
            status,
            escape_json(&self.output_path),
            escape_json(&self.raw_path),
            mp4_status,
            mp4_error,
            decode_status,
            decode_error,
            self.width,
            self.height,
            self.fps,
            self.frame_index,
            self.packets,
            self.bytes,
            self.keyframes,
            decoded_frames,
            mp4_bytes,
            duration_90k
        ))
    }

    fn write_chunk(&mut self, chunk: EncodedChunk) -> Result<()> {
        self.bytes = self.bytes.saturating_add(chunk.data.len() as u64);
        self.packets = self.packets.saturating_add(1);
        if chunk.is_keyframe {
            self.keyframes = self.keyframes.saturating_add(1);
        }
        self.raw_file.write_all(&chunk.data)?;
        self.writer.write_encoded_chunk(
            chunk.clone(),
            Some(SampleDuration90k::new(90_000 / self.fps.max(1))),
        )?;
        self.chunks.push(chunk);
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Plane<'a> {
    data: &'a [u8],
    row_stride: usize,
    pixel_stride: usize,
}

fn yuv420_to_nv12(
    width: usize,
    height: usize,
    y: Plane<'_>,
    u: Plane<'_>,
    v: Plane<'_>,
) -> Result<Vec<u8>> {
    if y.row_stride < width {
        bail!(
            "Y row stride {} is smaller than width {}",
            y.row_stride,
            width
        );
    }
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let mut out = vec![0_u8; width * height + chroma_width * chroma_height * 2];
    for row in 0..height {
        let src = row
            .checked_mul(y.row_stride)
            .context("Y row offset overflow")?;
        let dst = row * width;
        out[dst..dst + width].copy_from_slice(
            y.data
                .get(src..src + width)
                .context("Y plane is shorter than expected")?,
        );
    }
    let uv_base = width * height;
    for row in 0..chroma_height {
        for col in 0..chroma_width {
            let u_index = row
                .checked_mul(u.row_stride)
                .and_then(|base| base.checked_add(col.saturating_mul(u.pixel_stride)))
                .context("U plane offset overflow")?;
            let v_index = row
                .checked_mul(v.row_stride)
                .and_then(|base| base.checked_add(col.saturating_mul(v.pixel_stride)))
                .context("V plane offset overflow")?;
            let dst = uv_base + (row * chroma_width + col) * 2;
            out[dst] = *u
                .data
                .get(u_index)
                .context("U plane is shorter than expected")?;
            out[dst + 1] = *v
                .data
                .get(v_index)
                .context("V plane is shorter than expected")?;
        }
    }
    Ok(out)
}

fn decode_chunks(chunks: &[EncodedChunk], width: usize, height: usize, fps: u32) -> Result<usize> {
    let mut config = DecoderConfig::new(Codec::H264, fps as i32, false);
    config.output_mode = DecodeOutputMode::Metadata;
    config.backend_options = BackendDecoderOptions::Android(AndroidDecoderOptions {
        video_width: Some(width.try_into().unwrap_or(u16::MAX)),
        video_height: Some(height.try_into().unwrap_or(u16::MAX)),
        ..Default::default()
    });
    let mut decoder = AnyDecodeSession::new(Backend::Android, config)?;
    let mut decoded_frames = 0_usize;
    for chunk in chunks {
        let mut backpressure_spins = 0_u32;
        loop {
            match decoder.submit(BitstreamInput::AnnexBChunk {
                chunk: chunk.data.clone(),
                pts_90k: chunk.pts_90k,
            }) {
                Ok(()) => break,
                Err(BackendError::TemporaryBackpressure(_)) => {
                    drain_decoder(&mut decoder, &mut decoded_frames)?;
                    backpressure_spins = backpressure_spins.saturating_add(1);
                    if backpressure_spins > 1_000 {
                        bail!("decoder input buffer remained unavailable");
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(err) => return Err(err.into()),
            }
        }
        drain_decoder(&mut decoder, &mut decoded_frames)?;
    }
    let mut flush_spins = 0_u32;
    loop {
        match decoder.flush() {
            Ok(frames) => {
                decoded_frames += frames
                    .into_iter()
                    .filter(|frame| matches!(frame, DecodedFrame::Metadata { .. }))
                    .count();
                break;
            }
            Err(BackendError::TemporaryBackpressure(_)) => {
                drain_decoder(&mut decoder, &mut decoded_frames)?;
                flush_spins = flush_spins.saturating_add(1);
                if flush_spins > 1_000 {
                    bail!("decoder flush input buffer remained unavailable");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => return Err(err.into()),
        }
    }
    Ok(decoded_frames)
}

fn drain_decoder(decoder: &mut AnyDecodeSession, decoded_frames: &mut usize) -> Result<()> {
    while let Some(frame) = decoder.try_reap()? {
        if matches!(frame, DecodedFrame::Metadata { .. }) {
            *decoded_frames = decoded_frames.saturating_add(1);
        }
    }
    Ok(())
}

fn direct_plane<'local>(
    env: &JNIEnv<'local>,
    buffer: &JByteBuffer<'local>,
    length: jint,
    row_stride: jint,
    pixel_stride: jint,
) -> Result<Plane<'local>> {
    let ptr = env.get_direct_buffer_address(buffer)?;
    let capacity = env.get_direct_buffer_capacity(buffer)?;
    let length = usize::try_from(length).context("negative buffer length")?;
    if length > capacity {
        bail!("direct buffer length {length} exceeds capacity {capacity}");
    }
    let data = unsafe { slice::from_raw_parts(ptr.cast_const(), length) };
    Ok(Plane {
        data,
        row_stride: usize::try_from(row_stride).context("negative row stride")?,
        pixel_stride: usize::try_from(pixel_stride).context("negative pixel stride")?,
    })
}

fn recorder_mut<'a>(handle: jlong) -> Result<&'a mut RustCameraRecorder> {
    if handle == 0 {
        bail!("native recorder handle is null");
    }
    Ok(unsafe { &mut *(handle as *mut RustCameraRecorder) })
}

fn make_error_json(error: impl std::fmt::Display) -> String {
    format!(
        "{{\"status\":\"FAIL\",\"error\":\"{}\"}}",
        escape_json(&error.to_string())
    )
}

fn escape_json(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect(),
            '\n' => "\\n".chars().collect(),
            '\r' => "\\r".chars().collect(),
            '\t' => "\\t".chars().collect(),
            other => vec![other],
        })
        .collect()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_videohwcamera_RustRecorder_nativeStart(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    output_path: JString<'_>,
    width: jint,
    height: jint,
    fps: jint,
    bitrate: jint,
) -> jlong {
    let result = (|| -> Result<jlong> {
        let output_path: String = env.get_string(&output_path)?.into();
        let recorder = RustCameraRecorder::new(
            output_path,
            u32::try_from(width).context("negative width")?,
            u32::try_from(height).context("negative height")?,
            u32::try_from(fps).context("negative fps")?,
            u32::try_from(bitrate).context("negative bitrate")?,
        )?;
        Ok(Box::into_raw(Box::new(recorder)) as jlong)
    })();
    result.unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_videohwcamera_RustRecorder_nativePushYuv(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    y_buffer: JByteBuffer<'_>,
    y_length: jint,
    y_row_stride: jint,
    u_buffer: JByteBuffer<'_>,
    u_length: jint,
    u_row_stride: jint,
    u_pixel_stride: jint,
    v_buffer: JByteBuffer<'_>,
    v_length: jint,
    v_row_stride: jint,
    v_pixel_stride: jint,
    pts_ns: jlong,
    force_keyframe: jboolean,
) -> jint {
    let result = (|| -> Result<u64> {
        let y = direct_plane(&env, &y_buffer, y_length, y_row_stride, 1)?;
        let u = direct_plane(&env, &u_buffer, u_length, u_row_stride, u_pixel_stride)?;
        let v = direct_plane(&env, &v_buffer, v_length, v_row_stride, v_pixel_stride)?;
        recorder_mut(handle)?.push_yuv420(y, u, v, pts_ns, force_keyframe != 0)
    })();
    result.map_or(-1, |frames| frames.min(i32::MAX as u64) as jint)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_videohwcamera_RustRecorder_nativeFinish(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jstring {
    let message = if handle == 0 {
        make_error_json("native recorder handle is null")
    } else {
        let recorder = unsafe { Box::from_raw(handle as *mut RustCameraRecorder) };
        recorder.finish().unwrap_or_else(make_error_json)
    };
    env.new_string(message)
        .map(|value| value.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
