use std::{
    fs::File,
    io::Write,
    num::{NonZeroU32, NonZeroUsize},
    ptr,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use jni::{
    JNIEnv,
    objects::{JClass, JString},
    sys::{jint, jlong, jobject, jstring},
};
use video_hw::{
    AndroidDecoderOptions, AndroidSurfaceEncoder, AndroidSurfaceEncoderConfig, Backend,
    BackendDecoderOptions, BackendError, BitstreamInput, Codec, DecodeOutputMode, DecodedFrame,
    DecoderConfig, EncodedChunk,
};
use video_hw_fmp4::{
    EncodedTrackConfig, Fmp4Writer, FragmentFrames, FrameRate, FrameSize, Ready, SampleDuration90k,
    SyncEncodedRecording,
};

struct RustCameraRecorder {
    encoder: AndroidSurfaceEncoder,
    writer: Fmp4Writer<SyncEncodedRecording>,
    raw_file: File,
    chunks: Vec<EncodedChunk>,
    width: usize,
    height: usize,
    fps: u32,
    packets: u64,
    encoded_frames: u64,
    bytes: u64,
    keyframes: u64,
    output_path: String,
    raw_path: String,
}

impl RustCameraRecorder {
    fn new(
        env: &mut JNIEnv<'_>,
        output_path: String,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    ) -> Result<Self> {
        let frame_size = FrameSize::new(
            NonZeroU32::new(width).context("width must be non-zero")?,
            NonZeroU32::new(height).context("height must be non-zero")?,
        );
        let fps = fps.max(1);
        let mut encoder_config = AndroidSurfaceEncoderConfig::new(Codec::H264, width, height, fps);
        encoder_config.bitrate = Some(bitrate);
        let encoder = AndroidSurfaceEncoder::new(env, encoder_config)?;
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
            packets: 0,
            encoded_frames: 0,
            bytes: 0,
            keyframes: 0,
            output_path,
            raw_path,
        })
    }

    fn input_surface(&self, env: &mut JNIEnv<'_>) -> Result<jobject> {
        Ok(self.encoder.input_surface(env)?)
    }

    fn drain(&mut self, env: &mut JNIEnv<'_>) -> Result<u64> {
        for chunk in self.encoder.drain(env, false)? {
            self.write_chunk(chunk)?;
        }
        Ok(self.packets)
    }

    fn finish(mut self, env: &mut JNIEnv<'_>) -> Result<String> {
        self.encoder.signal_end_of_input_stream(env)?;
        while !self.encoder.eos_seen() {
            for chunk in self.encoder.drain(env, true)? {
                self.write_chunk(chunk)?;
            }
        }
        self.encoder.stop_and_release(env)?;
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
            "{{\"status\":\"{}\",\"surface_input\":true,\"path\":\"{}\",\"raw_path\":\"{}\",\"mp4_status\":\"{}\",\"mp4_error\":\"{}\",\"decode_status\":\"{}\",\"decode_error\":\"{}\",\"width\":{},\"height\":{},\"fps\":{},\"encoded_frames\":{},\"packets\":{},\"bytes\":{},\"keyframes\":{},\"decoded_frames\":{},\"mp4_bytes\":{},\"duration_90k\":{}}}",
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
            self.encoded_frames,
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
        if chunk.pts_90k.is_some() {
            self.encoded_frames = self.encoded_frames.saturating_add(1);
        }
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

fn decode_chunks(chunks: &[EncodedChunk], width: usize, height: usize, fps: u32) -> Result<usize> {
    let mut config = DecoderConfig::new(Codec::H264, fps as i32, false);
    config.output_mode = DecodeOutputMode::Metadata;
    config.backend_options = BackendDecoderOptions::Android(AndroidDecoderOptions {
        video_width: Some(width.try_into().unwrap_or(u16::MAX)),
        video_height: Some(height.try_into().unwrap_or(u16::MAX)),
        ..Default::default()
    });
    let mut decoder = video_hw::AnyDecodeSession::new(Backend::Android, config)?;
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

fn drain_decoder(
    decoder: &mut video_hw::AnyDecodeSession,
    decoded_frames: &mut usize,
) -> Result<()> {
    while let Some(frame) = decoder.try_reap()? {
        if matches!(frame, DecodedFrame::Metadata { .. }) {
            *decoded_frames = decoded_frames.saturating_add(1);
        }
    }
    Ok(())
}

fn recorder_lock<'a>(handle: jlong) -> Result<MutexGuard<'a, RustCameraRecorder>> {
    if handle == 0 {
        bail!("native recorder handle is null");
    }
    let mutex = unsafe { &*(handle as *mut Mutex<RustCameraRecorder>) };
    mutex
        .lock()
        .map_err(|error| anyhow::anyhow!("native recorder mutex poisoned: {error}"))
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
            &mut env,
            output_path,
            u32::try_from(width).context("negative width")?,
            u32::try_from(height).context("negative height")?,
            u32::try_from(fps).context("negative fps")?,
            u32::try_from(bitrate).context("negative bitrate")?,
        )?;
        Ok(Box::into_raw(Box::new(Mutex::new(recorder))) as jlong)
    })();
    result.unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_videohwcamera_RustRecorder_nativeInputSurface(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jobject {
    let result = (|| -> Result<jobject> { recorder_lock(handle)?.input_surface(&mut env) })();
    result.unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_videohwcamera_RustRecorder_nativeDrain(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jint {
    let result = (|| -> Result<u64> { recorder_lock(handle)?.drain(&mut env) })();
    result.map_or(-1, |packets| packets.min(i32::MAX as u64) as jint)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_videohwcamera_RustRecorder_nativeFinish(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jstring {
    let message = if handle == 0 {
        make_error_json("native recorder handle is null")
    } else {
        let recorder = unsafe { Box::from_raw(handle as *mut Mutex<RustCameraRecorder>) };
        match recorder.into_inner() {
            Ok(recorder) => recorder.finish(&mut env).unwrap_or_else(make_error_json),
            Err(error) => make_error_json(format!("native recorder mutex poisoned: {error}")),
        }
    };
    env.new_string(message)
        .map(|value| value.into_raw())
        .unwrap_or(ptr::null_mut())
}
