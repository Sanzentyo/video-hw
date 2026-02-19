use std::{
    ffi::c_void,
    sync::{Arc, Mutex},
};

use crate::bitstream::{AccessUnit, ParameterSetCache, StatefulBitstreamAssembler};
use crate::{
    BackendError, CapabilityReport, Codec, DecodeSummary, DecoderConfig, EncodedPacket, Frame,
    VideoDecoder, VideoEncoder,
};
use core_foundation::{
    base::{CFAllocator, CFType, TCFType, kCFAllocatorSystemDefault},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFMutableDictionary},
    number::CFNumber,
    string::CFString,
};
use core_media::{
    block_buffer::CMBlockBuffer,
    format_description::{
        CMFormatDescription, CMVideoCodecType, CMVideoFormatDescription, kCMVideoCodecType_H264,
        kCMVideoCodecType_HEVC,
    },
    sample_buffer::{CMSampleBuffer, CMSampleTimingInfo},
    time::{CMTime, kCMTimeInvalid},
};
use core_video::{
    image_buffer::CVImageBuffer,
    pixel_buffer::{CVPixelBuffer, kCVPixelFormatType_32BGRA},
};
use video_toolbox::{
    compression_properties::{CompressionPropertyKey, VideoEncoderSpecification},
    compression_session::VTCompressionSession,
    decompression_properties::VideoDecoderSpecification,
    decompression_session::{VTDecompressionOutputCallbackRecord, VTDecompressionSession},
    errors::VTDecodeFrameFlags,
    session::TVTSession,
};

pub struct PackedSample {
    pub data: Vec<u8>,
}

pub trait SamplePacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample, BackendError>;
}

#[derive(Debug, Default)]
pub struct AvccHvccPacker;

impl SamplePacker for AvccHvccPacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample, BackendError> {
        let total_size: usize = access_unit
            .nalus
            .iter()
            .map(|nal| nal.len().saturating_add(4))
            .sum();
        let mut data = Vec::with_capacity(total_size);

        for nal in &access_unit.nalus {
            let len = (nal.len() as u32).to_be_bytes();
            data.extend_from_slice(&len);
            data.extend_from_slice(nal);
        }

        Ok(PackedSample { data })
    }
}

#[derive(Debug, Clone, Default)]
struct DecodeOutputState {
    decoded_frames: usize,
    width: Option<usize>,
    height: Option<usize>,
    pixel_format: Option<u32>,
}

struct VtDecoderSession {
    session: VTDecompressionSession,
    format_description: CMVideoFormatDescription,
    decode_state: Box<Mutex<DecodeOutputState>>,
    next_pts: Mutex<i64>,
}

impl VtDecoderSession {
    fn new(config: &DecoderConfig, parameter_sets: &[Vec<u8>]) -> Result<Self, BackendError> {
        let codec_type = to_cm_codec_type(config.codec);
        if config.require_hardware
            && !VTDecompressionSession::is_hardware_decode_supported(codec_type)
        {
            return Err(BackendError::UnsupportedConfig(format!(
                "{} hardware decode is not supported on this machine",
                codec_label(config.codec)
            )));
        }

        let format_description = create_format_description(config.codec, parameter_sets)?;

        let decoder_specification = if config.require_hardware {
            let mut spec = CFMutableDictionary::<CFString, CFType>::new();
            spec.add(
                &VideoDecoderSpecification::RequireHardwareAcceleratedVideoDecoder.into(),
                &CFBoolean::true_value().as_CFType(),
            );
            Some(spec.to_immutable())
        } else {
            None
        };

        let mut decode_state = Box::new(Mutex::new(DecodeOutputState::default()));
        let decode_state_ptr =
            (&mut *decode_state as *mut Mutex<DecodeOutputState>).cast::<c_void>();
        let callback = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(vt_decode_output_callback),
            decompressionOutputRefCon: decode_state_ptr,
        };

        let session = unsafe {
            VTDecompressionSession::new_with_callback(
                format_description.clone(),
                decoder_specification,
                None,
                Some(&callback as *const VTDecompressionOutputCallbackRecord),
            )
        }
        .map_err(|status| vt_error("VTDecompressionSession::new_with_callback", status))?;

        Ok(Self {
            session,
            format_description,
            decode_state,
            next_pts: Mutex::new(0),
        })
    }

    fn decode_access_units(
        &self,
        access_units: &[AccessUnit],
        fps: i32,
    ) -> Result<(), BackendError> {
        let mut packer = AvccHvccPacker;
        for access_unit in access_units {
            let packed = packer.pack(access_unit)?;

            let block_buffer = unsafe {
                let block_buffer = CMBlockBuffer::new_with_memory_block(
                    None,
                    packed.data.len(),
                    None,
                    0,
                    packed.data.len(),
                    0,
                )
                .map_err(|status| cm_error("CMBlockBuffer::new_with_memory_block", status))?;
                block_buffer
                    .replace_data_bytes(&packed.data, 0)
                    .map_err(|status| cm_error("CMBlockBuffer::replace_data_bytes", status))?;
                Ok::<CMBlockBuffer, BackendError>(block_buffer)
            }?;

            let sample_size = [packed.data.len()];
            let format_description: CMFormatDescription = unsafe {
                CMFormatDescription::wrap_under_get_rule(
                    self.format_description.as_concrete_TypeRef(),
                )
            };
            let timing = CMSampleTimingInfo {
                duration: CMTime::make(1, fps),
                presentationTimeStamp: CMTime::make(self.next_pts(), fps),
                decodeTimeStamp: unsafe { kCMTimeInvalid },
            };
            let sample_buffer = CMSampleBuffer::new_ready(
                &block_buffer,
                Some(&format_description),
                1,
                Some(&[timing]),
                Some(&sample_size),
            )
            .map_err(|status| cm_error("CMSampleBuffer::new_ready", status))?;

            unsafe {
                self.session
                    .decode_frame(
                        sample_buffer,
                        VTDecodeFrameFlags::Frame_EnableAsynchronousDecompression,
                        std::ptr::null_mut(),
                    )
                    .map_err(|status| vt_error("VTDecompressionSession::decode_frame", status))?;
            }
        }

        Ok(())
    }

    fn wait_for_completion(&self) -> Result<(), BackendError> {
        self.session
            .finish_delayed_frames()
            .map_err(|status| vt_error("VTDecompressionSession::finish_delayed_frames", status))?;
        self.session
            .wait_for_asynchronous_frames()
            .map_err(|status| {
                vt_error(
                    "VTDecompressionSession::wait_for_asynchronous_frames",
                    status,
                )
            })?;
        Ok(())
    }

    fn snapshot_summary(&self) -> DecodeSummary {
        let state = self
            .decode_state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        let dims = self.format_description.get_dimensions();
        let fallback_width = usize::try_from(dims.width).ok().filter(|v| *v > 0);
        let fallback_height = usize::try_from(dims.height).ok().filter(|v| *v > 0);

        DecodeSummary {
            decoded_frames: state.decoded_frames,
            width: state.width.or(fallback_width),
            height: state.height.or(fallback_height),
            pixel_format: state.pixel_format,
        }
    }

    fn next_pts(&self) -> i64 {
        match self.next_pts.lock() {
            Ok(mut v) => {
                let current = *v;
                *v = v.saturating_add(1);
                current
            }
            Err(_) => 0,
        }
    }
}

pub struct VtDecoderAdapter {
    config: DecoderConfig,
    assembler: StatefulBitstreamAssembler,
    decoder: Option<VtDecoderSession>,
    last_summary: DecodeSummary,
    reported_decoded_frames: usize,
}

impl VtDecoderAdapter {
    pub fn new(config: DecoderConfig) -> Self {
        Self {
            assembler: StatefulBitstreamAssembler::with_codec(config.codec),
            config,
            decoder: None,
            last_summary: DecodeSummary {
                decoded_frames: 0,
                width: None,
                height: None,
                pixel_format: None,
            },
            reported_decoded_frames: 0,
        }
    }

    fn ensure_decoder(&mut self, cache: &ParameterSetCache) -> Result<(), BackendError> {
        if self.decoder.is_some() {
            return Ok(());
        }
        if let Some(parameter_sets) = cache.required_for_codec(self.config.codec) {
            self.decoder = Some(VtDecoderSession::new(&self.config, &parameter_sets)?);
        }
        Ok(())
    }

    fn take_delta(&mut self, wait: bool) -> Result<Vec<Frame>, BackendError> {
        if let Some(decoder) = self.decoder.as_ref() {
            if wait {
                decoder.wait_for_completion()?;
            }
            let summary = decoder.snapshot_summary();
            let delta = summary
                .decoded_frames
                .saturating_sub(self.reported_decoded_frames);
            self.reported_decoded_frames = summary.decoded_frames;
            self.last_summary = summary.clone();
            return Ok(summary_to_frames(delta, &summary));
        }

        Ok(Vec::new())
    }
}

impl VideoDecoder for VtDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        let cm_codec = to_cm_codec_type(codec);
        Ok(CapabilityReport {
            codec,
            decode_supported: true,
            encode_supported: true,
            hardware_acceleration: VTDecompressionSession::is_hardware_decode_supported(cm_codec),
        })
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        let (access_units, cache) = self
            .assembler
            .push_chunk(chunk, self.config.codec, pts_90k)?;
        self.ensure_decoder(&cache)?;

        if let Some(decoder) = self.decoder.as_ref() {
            if !access_units.is_empty() {
                decoder.decode_access_units(&access_units, self.config.fps)?;
            }
        }

        self.take_delta(false)
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        let (access_units, cache) = self.assembler.flush()?;
        self.ensure_decoder(&cache)?;

        if let Some(decoder) = self.decoder.as_ref() {
            if !access_units.is_empty() {
                decoder.decode_access_units(&access_units, self.config.fps)?;
            }
        }

        self.take_delta(true)
    }

    fn decode_summary(&self) -> DecodeSummary {
        self.last_summary.clone()
    }
}

pub struct VtEncoderAdapter {
    codec: Codec,
    fps: i32,
    require_hardware: bool,
    pending_frame_count: usize,
    width: Option<usize>,
    height: Option<usize>,
}

impl VtEncoderAdapter {
    pub fn with_config(codec: Codec, fps: i32, require_hardware: bool) -> Self {
        Self {
            codec,
            fps,
            require_hardware,
            pending_frame_count: 0,
            width: None,
            height: None,
        }
    }
}

impl VideoEncoder for VtEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec,
            decode_supported: true,
            encode_supported: true,
            hardware_acceleration: true,
        })
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        self.pending_frame_count = self.pending_frame_count.saturating_add(1);
        if self.width.is_none() {
            self.width = Some(frame.width);
        }
        if self.height.is_none() {
            self.height = Some(frame.height);
        }
        Ok(Vec::new())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        if self.pending_frame_count == 0 {
            return Ok(Vec::new());
        }

        let mut encoder_specification = CFMutableDictionary::<CFString, CFType>::new();
        if self.require_hardware {
            encoder_specification.add(
                &VideoEncoderSpecification::RequireHardwareAcceleratedVideoEncoder.into(),
                &CFBoolean::true_value().as_CFType(),
            );
        }

        let source_image_buffer_attributes = CFMutableDictionary::<CFString, CFType>::new();
        let allocator = unsafe { CFAllocator::wrap_under_get_rule(kCFAllocatorSystemDefault) };

        let session = VTCompressionSession::new(
            self.width.unwrap_or(640) as i32,
            self.height.unwrap_or(360) as i32,
            to_cm_codec_type(self.codec),
            encoder_specification.to_immutable(),
            source_image_buffer_attributes.to_immutable(),
            allocator,
        )
        .map_err(|status| vt_error("VTCompressionSession::new", status))?;

        let session_ref = session.as_session();
        session_ref
            .set_property(
                CompressionPropertyKey::RealTime.into(),
                CFBoolean::false_value().as_CFType(),
            )
            .map_err(|status| vt_error("VTSessionSetProperty(RealTime)", status))?;
        session_ref
            .set_property(
                CompressionPropertyKey::ExpectedFrameRate.into(),
                CFNumber::from(self.fps).as_CFType(),
            )
            .map_err(|status| vt_error("VTSessionSetProperty(ExpectedFrameRate)", status))?;
        session_ref
            .set_property(
                CompressionPropertyKey::MaxKeyFrameInterval.into(),
                CFNumber::from(self.fps.saturating_mul(2)).as_CFType(),
            )
            .map_err(|status| vt_error("VTSessionSetProperty(MaxKeyFrameInterval)", status))?;

        session
            .prepare_to_encode_frames()
            .map_err(|status| vt_error("VTCompressionSession::prepare_to_encode_frames", status))?;

        let output_packets = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        for frame_index in 0..self.pending_frame_count {
            let pixel_buffer = make_synthetic_bgra_frame(
                self.width.unwrap_or(640),
                self.height.unwrap_or(360),
                frame_index,
            )?;
            let image_buffer =
                unsafe { CVImageBuffer::wrap_under_get_rule(pixel_buffer.as_concrete_TypeRef()) };

            let packets_ref = Arc::clone(&output_packets);
            session
                .encode_frame_with_closure(
                    image_buffer,
                    CMTime::make(frame_index as i64, self.fps),
                    CMTime::make(1, self.fps),
                    empty_dictionary(),
                    move |status, _info_flags, sample_buffer_ref| {
                        if status != 0 || sample_buffer_ref.is_null() {
                            return;
                        }
                        let sample_buffer =
                            unsafe { CMSampleBuffer::wrap_under_get_rule(sample_buffer_ref) };
                        if let Some(data_buffer) = sample_buffer.get_data_buffer() {
                            let len = data_buffer.get_data_length();
                            let mut bytes = vec![0u8; len];
                            if data_buffer.copy_data_bytes(0, &mut bytes).is_ok() {
                                if let Ok(mut packets) = packets_ref.lock() {
                                    packets.push(bytes);
                                }
                            }
                        }
                    },
                )
                .map_err(|status| {
                    vt_error("VTCompressionSession::encode_frame_with_closure", status)
                })?;
        }

        session
            .complete_frames(unsafe { kCMTimeInvalid })
            .map_err(|status| vt_error("VTCompressionSession::complete_frames", status))?;

        self.pending_frame_count = 0;

        let packets = output_packets
            .lock()
            .map(|v| v.clone())
            .map_err(|_| BackendError::Backend("encode output lock".to_string()))?;

        Ok(packets
            .into_iter()
            .map(|data| EncodedPacket {
                codec: self.codec,
                data,
                pts_90k: None,
                is_keyframe: false,
            })
            .collect())
    }
}

fn summary_to_frames(count: usize, summary: &DecodeSummary) -> Vec<Frame> {
    let width = summary.width.unwrap_or_default();
    let height = summary.height.unwrap_or_default();
    let pixel_format = summary.pixel_format;

    (0..count)
        .map(|_| Frame {
            width,
            height,
            pixel_format,
            pts_90k: None,
            argb: None,
            force_keyframe: false,
        })
        .collect()
}

fn to_cm_codec_type(codec: Codec) -> CMVideoCodecType {
    match codec {
        Codec::H264 => kCMVideoCodecType_H264,
        Codec::Hevc => kCMVideoCodecType_HEVC,
    }
}

fn codec_label(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "h264",
        Codec::Hevc => "hevc",
    }
}

fn create_format_description(
    codec: Codec,
    parameter_sets: &[Vec<u8>],
) -> Result<CMVideoFormatDescription, BackendError> {
    let refs = parameter_sets
        .iter()
        .map(|v| v.as_slice())
        .collect::<Vec<_>>();
    match codec {
        Codec::H264 => {
            CMVideoFormatDescription::from_h264_parameter_sets(&refs, 4).map_err(|status| {
                cm_error("CMVideoFormatDescription::from_h264_parameter_sets", status)
            })
        }
        Codec::Hevc => {
            CMVideoFormatDescription::from_hevc_parameter_sets(&refs, 4, Some(&empty_dictionary()))
                .map_err(|status| {
                    cm_error("CMVideoFormatDescription::from_hevc_parameter_sets", status)
                })
        }
    }
}

fn empty_dictionary() -> CFDictionary<CFString, CFType> {
    CFMutableDictionary::<CFString, CFType>::new().to_immutable()
}

fn make_synthetic_bgra_frame(
    width: usize,
    height: usize,
    frame_index: usize,
) -> Result<CVPixelBuffer, BackendError> {
    let pixel_buffer = CVPixelBuffer::new(kCVPixelFormatType_32BGRA, width, height, None)
        .map_err(|status| cv_error("CVPixelBuffer::new", status))?;

    let lock_status = pixel_buffer.lock_base_address(0);
    if lock_status != 0 {
        return Err(cv_error("CVPixelBuffer::lock_base_address", lock_status));
    }

    let bytes_per_row = pixel_buffer.get_bytes_per_row();
    let total = bytes_per_row.saturating_mul(height);
    let base_ptr = unsafe { pixel_buffer.get_base_address() } as *mut u8;

    if !base_ptr.is_null() && total > 0 {
        unsafe {
            let buffer = std::slice::from_raw_parts_mut(base_ptr, total);
            for y in 0..height {
                for x in 0..width {
                    let offset = y * bytes_per_row + x * 4;
                    if offset + 3 >= buffer.len() {
                        continue;
                    }
                    buffer[offset] = ((x + frame_index) % 256) as u8;
                    buffer[offset + 1] = ((y + frame_index * 2) % 256) as u8;
                    buffer[offset + 2] = ((frame_index * 5) % 256) as u8;
                    buffer[offset + 3] = 255;
                }
            }
        }
    }

    let unlock_status = pixel_buffer.unlock_base_address(0);
    if unlock_status != 0 {
        return Err(cv_error(
            "CVPixelBuffer::unlock_base_address",
            unlock_status,
        ));
    }

    Ok(pixel_buffer)
}

fn vt_error(context: &str, status: i32) -> BackendError {
    BackendError::Backend(format!("videotoolbox({context}): {status}"))
}

fn cm_error(context: &str, status: i32) -> BackendError {
    BackendError::Backend(format!("coremedia({context}): {status}"))
}

fn cv_error(context: &str, status: i32) -> BackendError {
    BackendError::Backend(format!("corevideo({context}): {status}"))
}

extern "C" fn vt_decode_output_callback(
    decompression_output_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: i32,
    _info_flags: video_toolbox::errors::VTDecodeInfoFlags,
    image_buffer: core_video::image_buffer::CVImageBufferRef,
    _presentation_time_stamp: CMTime,
    _presentation_duration: CMTime,
) {
    if status != 0 || decompression_output_ref_con.is_null() || image_buffer.is_null() {
        return;
    }

    let state = unsafe { &*(decompression_output_ref_con as *const Mutex<DecodeOutputState>) };
    let pixel_buffer = unsafe { CVPixelBuffer::wrap_under_get_rule(image_buffer) };

    if let Ok(mut s) = state.lock() {
        s.decoded_frames += 1;
        if s.width.is_none() {
            s.width = Some(pixel_buffer.get_width());
        }
        if s.height.is_none() {
            s.height = Some(pixel_buffer.get_height());
        }
        if s.pixel_format.is_none() {
            s.pixel_format = Some(pixel_buffer.get_pixel_format());
        }
    }
}
