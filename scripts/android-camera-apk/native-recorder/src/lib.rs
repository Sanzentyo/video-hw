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
    objects::{GlobalRef, JByteBuffer, JClass, JObject, JString, JValue},
    sys::{JNI_FALSE, jint, jlong, jobject, jstring},
};
use video_hw::{
    AndroidDecoderOptions, Backend, BackendDecoderOptions, BackendError, BitstreamInput, Codec,
    DecodeOutputMode, DecodedFrame, DecoderConfig, EncodedChunk, EncodedLayout, Timestamp90k,
};
use video_hw_fmp4::{
    EncodedTrackConfig, Fmp4Writer, FragmentFrames, FrameRate, FrameSize, Ready, SampleDuration90k,
    SyncEncodedRecording,
};

const MEDIACODEC_INFO_TRY_AGAIN_LATER: i32 = -1;
const MEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED: i32 = -2;
const MEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED: i32 = -3;
const MEDIACODEC_BUFFER_FLAG_KEY_FRAME: i32 = 1;
const MEDIACODEC_BUFFER_FLAG_CODEC_CONFIG: i32 = 2;
const MEDIACODEC_BUFFER_FLAG_END_OF_STREAM: i32 = 4;
const MEDIACODEC_CONFIGURE_FLAG_ENCODE: i32 = 1;
const COLOR_FORMAT_SURFACE: i32 = 0x7f00_0789;
const OUTPUT_TIMEOUT_US: i64 = 10_000;

struct RustCameraRecorder {
    encoder: SurfaceEncoder,
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
        let encoder = SurfaceEncoder::new(env, width as usize, height as usize, fps, bitrate)?;
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
        self.encoder.input_surface(env)
    }

    fn drain(&mut self, env: &mut JNIEnv<'_>) -> Result<u64> {
        for chunk in self.encoder.drain(env, false)? {
            self.write_chunk(chunk)?;
        }
        Ok(self.packets)
    }

    fn finish(mut self, env: &mut JNIEnv<'_>) -> Result<String> {
        self.encoder.signal_eos(env)?;
        while !self.encoder.eos_seen {
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

struct SurfaceEncoder {
    codec: Option<GlobalRef>,
    input_surface: GlobalRef,
    eos_signaled: bool,
    eos_seen: bool,
}

impl SurfaceEncoder {
    fn new(
        env: &mut JNIEnv<'_>,
        width: usize,
        height: usize,
        fps: u32,
        bitrate: u32,
    ) -> Result<Self> {
        let mime = env.new_string("video/avc")?;
        let format = env
            .call_static_method(
                "android/media/MediaFormat",
                "createVideoFormat",
                "(Ljava/lang/String;II)Landroid/media/MediaFormat;",
                &[
                    JValue::Object(&JObject::from(mime)),
                    JValue::Int(checked_i32(width, "width")?),
                    JValue::Int(checked_i32(height, "height")?),
                ],
            )?
            .l()?;

        set_media_format_i32(env, &format, "bitrate", checked_i32(bitrate, "bitrate")?)?;
        set_media_format_i32(env, &format, "frame-rate", checked_i32(fps, "fps")?)?;
        set_media_format_i32(env, &format, "i-frame-interval", 1)?;
        set_media_format_i32(env, &format, "color-format", COLOR_FORMAT_SURFACE)?;
        set_media_format_i32(env, &format, "prepend-sps-pps-to-idr-frames", 1)?;

        let mime = env.new_string("video/avc")?;
        let codec = env
            .call_static_method(
                "android/media/MediaCodec",
                "createEncoderByType",
                "(Ljava/lang/String;)Landroid/media/MediaCodec;",
                &[JValue::Object(&JObject::from(mime))],
            )?
            .l()?;
        let null = JObject::null();
        env.call_method(
            &codec,
            "configure",
            "(Landroid/media/MediaFormat;Landroid/view/Surface;Landroid/media/MediaCrypto;I)V",
            &[
                JValue::Object(&format),
                JValue::Object(&null),
                JValue::Object(&null),
                JValue::Int(MEDIACODEC_CONFIGURE_FLAG_ENCODE),
            ],
        )?;
        let input_surface = env
            .call_method(
                &codec,
                "createInputSurface",
                "()Landroid/view/Surface;",
                &[],
            )?
            .l()?;
        env.call_method(&codec, "start", "()V", &[])?;

        let codec = env.new_global_ref(codec)?;
        let input_surface = env.new_global_ref(input_surface)?;
        Ok(Self {
            codec: Some(codec),
            input_surface,
            eos_signaled: false,
            eos_seen: false,
        })
    }

    fn input_surface(&self, env: &mut JNIEnv<'_>) -> Result<jobject> {
        Ok(env.new_local_ref(self.input_surface.as_obj())?.into_raw())
    }

    fn signal_eos(&mut self, env: &mut JNIEnv<'_>) -> Result<()> {
        if self.eos_signaled {
            return Ok(());
        }
        let codec = self.codec()?;
        env.call_method(codec, "signalEndOfInputStream", "()V", &[])?;
        self.eos_signaled = true;
        Ok(())
    }

    fn drain(&mut self, env: &mut JNIEnv<'_>, wait_for_eos: bool) -> Result<Vec<EncodedChunk>> {
        let mut out = Vec::new();
        let mut spins = 0_u32;
        let buffer_info = env.new_object("android/media/MediaCodec$BufferInfo", "()V", &[])?;
        loop {
            spins = spins.saturating_add(1);
            let timeout_us = if wait_for_eos { OUTPUT_TIMEOUT_US } else { 0 };
            let codec = self.codec()?;
            let event = env
                .call_method(
                    codec,
                    "dequeueOutputBuffer",
                    "(Landroid/media/MediaCodec$BufferInfo;J)I",
                    &[JValue::Object(&buffer_info), JValue::Long(timeout_us)],
                )?
                .i()?;
            match event {
                index if index >= 0 => {
                    let info = OutputBufferInfo::read(env, &buffer_info)?;
                    if info.size > 0 {
                        out.push(self.output_chunk(env, index, &info)?);
                    }
                    let codec = self.codec()?;
                    env.call_method(
                        codec,
                        "releaseOutputBuffer",
                        "(IZ)V",
                        &[JValue::Int(index), JValue::Bool(JNI_FALSE)],
                    )?;
                    if (info.flags & MEDIACODEC_BUFFER_FLAG_END_OF_STREAM) != 0 {
                        self.eos_seen = true;
                        break;
                    }
                }
                MEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED => {
                    out.extend(self.format_config_chunks(env)?);
                }
                MEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED => {}
                MEDIACODEC_INFO_TRY_AGAIN_LATER => {
                    if !wait_for_eos || spins > 10_000 {
                        break;
                    }
                }
                other => bail!("MediaCodec.dequeueOutputBuffer returned {other}"),
            }
        }
        Ok(out)
    }

    fn output_chunk(
        &mut self,
        env: &mut JNIEnv<'_>,
        index: i32,
        info: &OutputBufferInfo,
    ) -> Result<EncodedChunk> {
        let codec = self.codec()?;
        let buffer = env
            .call_method(
                codec,
                "getOutputBuffer",
                "(I)Ljava/nio/ByteBuffer;",
                &[JValue::Int(index)],
            )?
            .l()?;
        if buffer.is_null() {
            bail!("MediaCodec.getOutputBuffer returned null");
        }
        let buffer = JByteBuffer::from(buffer);
        let buffer_ptr = env.get_direct_buffer_address(&buffer)?;
        let buffer_size = env.get_direct_buffer_capacity(&buffer)?;
        let offset = usize::try_from(info.offset.max(0)).context("negative output offset")?;
        let size = usize::try_from(info.size).context("negative output size")?;
        let end = offset
            .checked_add(size)
            .context("encoder output range overflow")?;
        if end > buffer_size {
            bail!("encoder output range exceeds buffer: end={end}, len={buffer_size}");
        }
        let data = unsafe { std::slice::from_raw_parts(buffer_ptr.add(offset), size) }.to_vec();
        Ok(EncodedChunk {
            codec: Codec::H264,
            layout: EncodedLayout::AnnexB,
            data,
            pts_90k: ((info.flags & MEDIACODEC_BUFFER_FLAG_CODEC_CONFIG) == 0)
                .then_some(Timestamp90k(us_to_pts_90k(info.presentation_time_us))),
            is_keyframe: (info.flags & MEDIACODEC_BUFFER_FLAG_KEY_FRAME) != 0
                || (info.flags & MEDIACODEC_BUFFER_FLAG_CODEC_CONFIG) != 0,
        })
    }

    fn format_config_chunks(&mut self, env: &mut JNIEnv<'_>) -> Result<Vec<EncodedChunk>> {
        let codec = self.codec()?;
        let format = env
            .call_method(
                codec,
                "getOutputFormat",
                "()Landroid/media/MediaFormat;",
                &[],
            )?
            .l()?;
        if format.is_null() {
            return Ok(Vec::new());
        }
        ["csd-0", "csd-1"]
            .into_iter()
            .filter_map(|key| media_format_buffer(env, &format, key).transpose())
            .map(|buffer| {
                buffer.map(|data| EncodedChunk {
                    codec: Codec::H264,
                    layout: EncodedLayout::AnnexB,
                    data: annexb_prefixed(data),
                    pts_90k: None,
                    is_keyframe: true,
                })
            })
            .collect::<Result<Vec<_>>>()
    }

    fn stop_and_release(&mut self, env: &mut JNIEnv<'_>) -> Result<()> {
        if let Some(codec) = self.codec.take() {
            let _ = env.call_method(codec.as_obj(), "stop", "()V", &[]);
            env.call_method(codec.as_obj(), "release", "()V", &[])?;
        }
        Ok(())
    }

    fn codec(&self) -> Result<&JObject<'static>> {
        self.codec
            .as_ref()
            .map(GlobalRef::as_obj)
            .context("MediaCodec has already been released")
    }
}

impl Drop for SurfaceEncoder {
    fn drop(&mut self) {
        // The Java MediaCodec is released explicitly from nativeFinish while a JNIEnv is available.
    }
}

#[derive(Debug)]
struct OutputBufferInfo {
    offset: i32,
    size: i32,
    presentation_time_us: i64,
    flags: i32,
}

impl OutputBufferInfo {
    fn read(env: &mut JNIEnv<'_>, buffer_info: &JObject<'_>) -> Result<Self> {
        Ok(Self {
            offset: env.get_field(buffer_info, "offset", "I")?.i()?,
            size: env.get_field(buffer_info, "size", "I")?.i()?,
            presentation_time_us: env.get_field(buffer_info, "presentationTimeUs", "J")?.j()?,
            flags: env.get_field(buffer_info, "flags", "I")?.i()?,
        })
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

fn checked_i32<T>(value: T, name: &str) -> Result<i32>
where
    T: TryInto<i32> + Copy + std::fmt::Display,
{
    value
        .try_into()
        .map_err(|_| anyhow::anyhow!("{name} value out of i32 range: {value}"))
}

fn set_media_format_i32(
    env: &mut JNIEnv<'_>,
    format: &JObject<'_>,
    key: &str,
    value: i32,
) -> Result<()> {
    let key = env.new_string(key)?;
    env.call_method(
        format,
        "setInteger",
        "(Ljava/lang/String;I)V",
        &[JValue::Object(&JObject::from(key)), JValue::Int(value)],
    )?;
    Ok(())
}

fn media_format_buffer(
    env: &mut JNIEnv<'_>,
    format: &JObject<'_>,
    key: &str,
) -> Result<Option<Vec<u8>>> {
    let key_string = env.new_string(key)?;
    let has_key = env
        .call_method(
            format,
            "containsKey",
            "(Ljava/lang/String;)Z",
            &[JValue::Object(&JObject::from(key_string))],
        )?
        .z()?;
    if !has_key {
        return Ok(None);
    }

    let key_string = env.new_string(key)?;
    let buffer = env
        .call_method(
            format,
            "getByteBuffer",
            "(Ljava/lang/String;)Ljava/nio/ByteBuffer;",
            &[JValue::Object(&JObject::from(key_string))],
        )?
        .l()?;
    if buffer.is_null() {
        return Ok(None);
    }
    let data = byte_buffer_remaining(env, &buffer)?;
    Ok((!data.is_empty()).then_some(data))
}

fn byte_buffer_remaining(env: &mut JNIEnv<'_>, buffer: &JObject<'_>) -> Result<Vec<u8>> {
    let duplicate = env
        .call_method(buffer, "duplicate", "()Ljava/nio/ByteBuffer;", &[])?
        .l()?;
    let len = env.call_method(&duplicate, "remaining", "()I", &[])?.i()?;
    if len <= 0 {
        return Ok(Vec::new());
    }
    let array = env.new_byte_array(len)?;
    let array_object: &JObject<'_> = array.as_ref();
    env.call_method(
        &duplicate,
        "get",
        "([B)Ljava/nio/ByteBuffer;",
        &[JValue::Object(array_object)],
    )?;
    Ok(env.convert_byte_array(&array)?)
}

fn annexb_prefixed(buffer: Vec<u8>) -> Vec<u8> {
    if buffer.starts_with(&[0, 0, 1]) || buffer.starts_with(&[0, 0, 0, 1]) {
        buffer
    } else {
        let mut out = Vec::with_capacity(buffer.len() + 4);
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&buffer);
        out
    }
}

fn us_to_pts_90k(value: i64) -> i64 {
    value.saturating_mul(90_000) / 1_000_000
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
