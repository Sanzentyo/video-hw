use std::{thread, time::Duration};

use cudarc::driver::CudaContext;
use nvidia_video_codec_sdk::{
    DecodeCodec, DecodeError, DecodeOptions, Encoder, EncoderInitParams, ErrorKind,
};

use crate::bitstream::{AccessUnit, StatefulBitstreamAssembler};
use crate::{
    BackendError, CapabilityReport, Codec, DecodeSummary, DecoderConfig, EncodedPacket, Frame,
    VideoDecoder, VideoEncoder,
};

const NVENC_RETRY_SLEEP_MS: u64 = 1;
const NVENC_FINAL_DRAIN_ATTEMPTS: usize = 16;

pub struct PackedSample {
    pub data: Vec<u8>,
}

pub trait SamplePacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample, BackendError>;
}

#[derive(Debug, Default)]
pub struct AnnexBPacker;

impl SamplePacker for AnnexBPacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample, BackendError> {
        let total_size: usize = access_unit
            .nalus
            .iter()
            .map(|nal| nal.len().saturating_add(4))
            .sum();
        let mut data = Vec::with_capacity(total_size);

        for nal in &access_unit.nalus {
            data.extend_from_slice(&[0, 0, 0, 1]);
            data.extend_from_slice(nal);
        }

        Ok(PackedSample { data })
    }
}

pub struct NvDecoderAdapter {
    config: DecoderConfig,
    assembler: StatefulBitstreamAssembler,
    decoder: Option<nvidia_video_codec_sdk::Decoder>,
    next_pts_90k: i64,
    last_summary: DecodeSummary,
}

impl NvDecoderAdapter {
    pub fn new(config: DecoderConfig) -> Self {
        Self {
            assembler: StatefulBitstreamAssembler::with_codec(config.codec),
            config,
            decoder: None,
            next_pts_90k: 0,
            last_summary: DecodeSummary {
                decoded_frames: 0,
                width: None,
                height: None,
                pixel_format: None,
            },
        }
    }

    fn ensure_decoder(&mut self) -> Result<(), BackendError> {
        if self.decoder.is_some() {
            return Ok(());
        }

        let cuda_ctx = CudaContext::new(0).map_err(|err| {
            BackendError::UnsupportedConfig(format!("failed to initialize CUDA context: {err}"))
        })?;
        let decoder = nvidia_video_codec_sdk::Decoder::new(
            cuda_ctx,
            to_decode_codec(self.config.codec),
            DecodeOptions::default(),
        )
        .map_err(map_decode_error)?;

        self.decoder = Some(decoder);
        Ok(())
    }

    fn decode_access_units(
        &mut self,
        access_units: &[AccessUnit],
        fallback_pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        if access_units.is_empty() {
            return Ok(Vec::new());
        }

        self.ensure_decoder()?;
        let mut packer = AnnexBPacker;
        let mut frames = Vec::new();

        for au in access_units {
            let packed = packer.pack(au)?;
            let pts_90k = au
                .pts_90k
                .or(fallback_pts_90k)
                .unwrap_or_else(|| self.bump_pts_90k());

            let decoded = {
                let decoder = self.decoder.as_mut().ok_or_else(|| {
                    BackendError::Backend("decoder should be initialized".to_string())
                })?;
                decoder
                    .push_access_unit(&packed.data, pts_90k)
                    .map_err(map_decode_error)?
            };
            self.apply_decoded_summary(&decoded);
            frames.extend(decoded.into_iter().map(to_frame));
        }

        Ok(frames)
    }

    fn bump_pts_90k(&mut self) -> i64 {
        let current = self.next_pts_90k;
        let step = if self.config.fps > 0 {
            (90_000 / i64::from(self.config.fps)).max(1)
        } else {
            3_000
        };
        self.next_pts_90k = self.next_pts_90k.saturating_add(step);
        current
    }

    fn apply_decoded_summary(&mut self, decoded: &[nvidia_video_codec_sdk::DecodedRgbFrame]) {
        self.last_summary.decoded_frames = self
            .last_summary
            .decoded_frames
            .saturating_add(decoded.len());

        if let Some(last) = decoded.last() {
            self.last_summary.width = Some(last.width as usize);
            self.last_summary.height = Some(last.height as usize);
        }
    }
}

impl VideoDecoder for NvDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec,
            decode_supported: matches!(codec, Codec::H264 | Codec::Hevc),
            encode_supported: matches!(codec, Codec::H264 | Codec::Hevc),
            hardware_acceleration: true,
        })
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        let (access_units, _cache) =
            self.assembler
                .push_chunk(chunk, self.config.codec, pts_90k)?;
        self.decode_access_units(&access_units, pts_90k)
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        let (access_units, _cache) = self.assembler.flush()?;
        let mut frames = self.decode_access_units(&access_units, None)?;

        if let Some(decoder) = self.decoder.as_mut() {
            let drained = decoder.flush().map_err(map_decode_error)?;
            self.apply_decoded_summary(&drained);
            frames.extend(drained.into_iter().map(to_frame));
        }

        Ok(frames)
    }

    fn decode_summary(&self) -> DecodeSummary {
        self.last_summary.clone()
    }
}

pub struct NvEncoderAdapter {
    codec: Codec,
    fps: i32,
    require_hardware: bool,
    pending_frames: Vec<Frame>,
    width: Option<usize>,
    height: Option<usize>,
}

impl NvEncoderAdapter {
    pub fn with_config(codec: Codec, fps: i32, require_hardware: bool) -> Self {
        Self {
            codec,
            fps,
            require_hardware,
            pending_frames: Vec::new(),
            width: None,
            height: None,
        }
    }

    fn make_session(
        &self,
        width: usize,
        height: usize,
    ) -> Result<nvidia_video_codec_sdk::Session, BackendError> {
        let _ = self.require_hardware;

        let cuda_ctx = CudaContext::new(0).map_err(|err| {
            BackendError::UnsupportedConfig(format!("failed to initialize CUDA context: {err}"))
        })?;

        let encoder = Encoder::initialize_with_cuda(cuda_ctx).map_err(map_encode_error)?;
        let encode_guid = to_encode_guid(self.codec);

        let encode_guids = encoder.get_encode_guids().map_err(map_encode_error)?;
        if !encode_guids.contains(&encode_guid) {
            return Err(BackendError::UnsupportedCodec(self.codec));
        }

        let preset_guid = nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_PRESET_P1_GUID;
        let tuning_info =
            nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;

        let mut preset_config = encoder
            .get_preset_config(encode_guid, preset_guid, tuning_info)
            .map_err(map_encode_error)?;

        let mut init_params = EncoderInitParams::new(encode_guid, width as u32, height as u32);
        init_params
            .preset_guid(preset_guid)
            .tuning_info(tuning_info)
            .framerate(self.fps.max(1) as u32, 1)
            .enable_picture_type_decision()
            .encode_config(&mut preset_config.presetCfg);

        encoder
            .start_session(
                nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB,
                init_params,
            )
            .map_err(map_encode_error)
    }
}

impl VideoEncoder for NvEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec,
            decode_supported: matches!(codec, Codec::H264 | Codec::Hevc),
            encode_supported: matches!(codec, Codec::H264 | Codec::Hevc),
            hardware_acceleration: true,
        })
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        if frame.width == 0 || frame.height == 0 {
            return Err(BackendError::InvalidInput(
                "frame dimensions must be positive".to_string(),
            ));
        }

        if let Some(width) = self.width {
            if frame.width != width {
                return Err(BackendError::InvalidInput(
                    "all frames in one flush cycle must have the same width".to_string(),
                ));
            }
        } else {
            self.width = Some(frame.width);
        }

        if let Some(height) = self.height {
            if frame.height != height {
                return Err(BackendError::InvalidInput(
                    "all frames in one flush cycle must have the same height".to_string(),
                ));
            }
        } else {
            self.height = Some(frame.height);
        }

        self.pending_frames.push(frame);
        Ok(Vec::new())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        if self.pending_frames.is_empty() {
            return Ok(Vec::new());
        }

        let pending_frames = std::mem::take(&mut self.pending_frames);
        let width = self.width.take().unwrap_or(640);
        let height = self.height.take().unwrap_or(360);

        let session = self.make_session(width, height)?;
        let mut input = session.create_input_buffer().map_err(map_encode_error)?;
        let mut output = session
            .create_output_bitstream()
            .map_err(map_encode_error)?;

        let mut packets = Vec::new();

        for (index, frame) in pending_frames.iter().enumerate() {
            let argb = generate_synthetic_argb(width, height, index);
            {
                let mut lock = input.lock().map_err(map_encode_error)?;
                unsafe {
                    lock.write_pitched(&argb, width.saturating_mul(4), height);
                }
            }

            let input_timestamp = frame
                .pts_90k
                .unwrap_or_else(|| (index as i64).saturating_mul(3_000))
                .max(0) as u64;

            let produced_output = loop {
                match session.encode_picture(
                    &mut input,
                    &mut output,
                    nvidia_video_codec_sdk::EncodePictureParams {
                        input_timestamp,
                        ..Default::default()
                    },
                ) {
                    Ok(()) => break true,
                    Err(err) if err.kind() == ErrorKind::EncoderBusy => {
                        thread::sleep(Duration::from_millis(NVENC_RETRY_SLEEP_MS));
                    }
                    Err(err) if err.kind() == ErrorKind::NeedMoreInput => break false,
                    Err(err) => return Err(map_encode_error(err)),
                }
            };

            if produced_output {
                let data = loop {
                    match output.lock() {
                        Ok(lock) => break lock.data().to_vec(),
                        Err(err) if err.kind() == ErrorKind::LockBusy => {
                            thread::sleep(Duration::from_millis(NVENC_RETRY_SLEEP_MS));
                        }
                        Err(err) => return Err(map_encode_error(err)),
                    }
                };

                if !data.is_empty() {
                    packets.push(EncodedPacket {
                        codec: self.codec,
                        data,
                        pts_90k: frame.pts_90k,
                        is_keyframe: index == 0,
                    });
                }
            }
        }

        loop {
            match session.end_of_stream() {
                Ok(()) => break,
                Err(err)
                    if err.kind() == ErrorKind::EncoderBusy
                        || err.kind() == ErrorKind::NeedMoreInput =>
                {
                    thread::sleep(Duration::from_millis(NVENC_RETRY_SLEEP_MS));
                }
                Err(err) => return Err(map_encode_error(err)),
            }
        }

        for _ in 0..NVENC_FINAL_DRAIN_ATTEMPTS {
            match output.try_lock() {
                Ok(lock) => {
                    let data = lock.data().to_vec();
                    if !data.is_empty() {
                        packets.push(EncodedPacket {
                            codec: self.codec,
                            data,
                            pts_90k: None,
                            is_keyframe: false,
                        });
                    }
                }
                Err(err)
                    if err.kind() == ErrorKind::LockBusy
                        || err.kind() == ErrorKind::EncoderBusy =>
                {
                    break;
                }
                Err(err) => return Err(map_encode_error(err)),
            }
        }

        Ok(packets)
    }
}

fn to_decode_codec(codec: Codec) -> DecodeCodec {
    match codec {
        Codec::H264 => DecodeCodec::H264,
        Codec::Hevc => DecodeCodec::H265,
    }
}

fn to_encode_guid(codec: Codec) -> nvidia_video_codec_sdk::sys::nvEncodeAPI::GUID {
    match codec {
        Codec::H264 => nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_CODEC_H264_GUID,
        Codec::Hevc => nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_CODEC_HEVC_GUID,
    }
}

fn map_decode_error(error: DecodeError) -> BackendError {
    match error {
        DecodeError::Unsupported(message) => BackendError::UnsupportedConfig(message),
        DecodeError::InvalidInput(message) => BackendError::InvalidInput(message),
        DecodeError::Cuda(err) => BackendError::DeviceLost(format!("cuda decode error: {err}")),
        DecodeError::Nvdec { operation, code } => {
            BackendError::Backend(format!("nvdec({operation}) failed: {code:?}"))
        }
        DecodeError::Internal(message) => BackendError::Backend(message),
    }
}

fn map_encode_error(error: nvidia_video_codec_sdk::EncodeError) -> BackendError {
    match error.kind() {
        ErrorKind::NeedMoreInput | ErrorKind::EncoderBusy | ErrorKind::LockBusy => {
            BackendError::TemporaryBackpressure(error.to_string())
        }
        ErrorKind::DeviceNotExist => BackendError::DeviceLost(error.to_string()),
        ErrorKind::UnsupportedDevice
        | ErrorKind::UnsupportedParam
        | ErrorKind::NoEncodeDevice
        | ErrorKind::InvalidEncoderDevice => BackendError::UnsupportedConfig(error.to_string()),
        ErrorKind::InvalidParam | ErrorKind::InvalidCall => {
            BackendError::InvalidInput(error.to_string())
        }
        _ => BackendError::Backend(error.to_string()),
    }
}

fn to_frame(frame: nvidia_video_codec_sdk::DecodedRgbFrame) -> Frame {
    Frame {
        width: frame.width as usize,
        height: frame.height as usize,
        pixel_format: None,
        pts_90k: Some(frame.timestamp_90k),
    }
}

fn generate_synthetic_argb(width: usize, height: usize, frame_index: usize) -> Vec<u8> {
    let mut buffer = vec![0_u8; width.saturating_mul(height).saturating_mul(4)];

    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;
            buffer[offset] = ((x + frame_index) % 256) as u8;
            buffer[offset + 1] = ((y + frame_index * 2) % 256) as u8;
            buffer[offset + 2] = ((frame_index * 5) % 256) as u8;
            buffer[offset + 3] = 255;
        }
    }

    buffer
}
