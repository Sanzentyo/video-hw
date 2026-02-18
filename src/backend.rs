use std::{
    ffi::c_void,
    fs,
    path::{Path, PathBuf},
    slice,
    sync::{Arc, Mutex},
};

use core_foundation::{
    base::{kCFAllocatorSystemDefault, CFAllocator, CFType, TCFType},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFMutableDictionary},
    number::CFNumber,
    string::CFString,
};
use core_media::{
    block_buffer::CMBlockBuffer,
    format_description::{
        kCMVideoCodecType_H264, kCMVideoCodecType_HEVC, CMFormatDescription, CMVideoCodecType,
        CMVideoFormatDescription,
    },
    sample_buffer::{CMSampleBuffer, CMSampleTimingInfo},
    time::{kCMTimeInvalid, CMTime},
};
use core_video::{
    image_buffer::CVImageBuffer,
    pixel_buffer::{kCVPixelFormatType_32BGRA, CVPixelBuffer},
};
use video_toolbox::{
    compression_properties::{CompressionPropertyKey, VideoEncoderSpecification},
    compression_session::VTCompressionSession,
    decompression_properties::VideoDecoderSpecification,
    decompression_session::{VTDecompressionOutputCallbackRecord, VTDecompressionSession},
    errors::VTDecodeFrameFlags,
    session::TVTSession,
};

use crate::{
    annexb::{parse_annexb, AccessUnit, AnnexBStreamParser, BitstreamPrepared},
    error::Result,
    packer::{AvccHvccPacker, SamplePacker},
    VtBackendError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
}

impl Codec {
    pub fn from_str(v: &str) -> Result<Self> {
        match v.to_ascii_lowercase().as_str() {
            "h264" | "avc" => Ok(Self::H264),
            "hevc" | "h265" => Ok(Self::Hevc),
            _ => Err(VtBackendError::UnsupportedCodec(v.to_owned())),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
        }
    }

    fn cm_codec_type(&self) -> CMVideoCodecType {
        match self {
            Codec::H264 => kCMVideoCodecType_H264,
            Codec::Hevc => kCMVideoCodecType_HEVC,
        }
    }

    pub fn file_extension(&self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::Hevc => "h265",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecodeOptions {
    pub codec: Codec,
    pub require_hardware: bool,
}

#[derive(Debug, Clone)]
pub struct DecodeSummary {
    pub decoded_frames: usize,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub pixel_format: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct EncodeOptions {
    pub codec: Codec,
    pub width: usize,
    pub height: usize,
    pub frame_count: usize,
    pub fps: i32,
    pub require_hardware: bool,
}

pub struct VtDecoder {
    session: VTDecompressionSession,
    format_description: CMVideoFormatDescription,
    decode_state: Box<Mutex<DecodeOutputState>>,
    next_pts: Mutex<i64>,
}

pub struct VtBitstreamDecoder {
    options: DecodeOptions,
    fps: i32,
    parser: AnnexBStreamParser,
    reported_decoded_frames: usize,
    decoder: Option<VtDecoder>,
}

#[derive(Debug, Clone, Default)]
struct DecodeOutputState {
    decoded_frames: usize,
    width: Option<usize>,
    height: Option<usize>,
    pixel_format: Option<u32>,
}

impl VtDecoder {
    pub fn new(codec: Codec, parameter_sets: &[Vec<u8>], require_hardware: bool) -> Result<Self> {
        if require_hardware
            && !VTDecompressionSession::is_hardware_decode_supported(codec.cm_codec_type())
        {
            return Err(VtBackendError::UnsupportedCodec(format!(
                "{} hardware decode is not supported on this machine",
                codec.as_str()
            )));
        }

        let format_description = create_format_description(codec, parameter_sets)?;
        let decoder_specification = if require_hardware {
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
        .map_err(|status| VtBackendError::VideoToolbox {
            context: "VTDecompressionSession::new_with_callback",
            status,
        })?;

        Ok(Self {
            session,
            format_description,
            decode_state,
            next_pts: Mutex::new(0),
        })
    }

    pub fn decode_access_units(
        &self,
        access_units: &[AccessUnit],
        fps: i32,
    ) -> Result<DecodeSummary> {
        let mut packer = AvccHvccPacker;
        let mut packed_samples = Vec::with_capacity(access_units.len());
        for access_unit in access_units {
            let packed = packer.pack(access_unit)?;
            packed_samples.push(packed.data);
        }

        self.decode_packed_samples(&packed_samples, fps)
    }

    pub fn decode_packed_samples(
        &self,
        packed_samples: &[Vec<u8>],
        fps: i32,
    ) -> Result<DecodeSummary> {
        for sample_data in packed_samples {
            let block_buffer = unsafe {
                let block_buffer = CMBlockBuffer::new_with_memory_block(
                    None,
                    sample_data.len(),
                    None,
                    0,
                    sample_data.len(),
                    0,
                )
                .map_err(|status| VtBackendError::CoreMedia {
                    context: "CMBlockBuffer::new_with_memory_block",
                    status,
                })?;
                block_buffer
                    .replace_data_bytes(sample_data, 0)
                    .map_err(|status| VtBackendError::CoreMedia {
                        context: "CMBlockBuffer::replace_data_bytes",
                        status,
                    })?;
                Ok::<CMBlockBuffer, VtBackendError>(block_buffer)
            }?;

            let sample_size = [sample_data.len()];

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
            .map_err(|status| VtBackendError::CoreMedia {
                context: "CMSampleBuffer::new_ready",
                status,
            })?;

            unsafe {
                self.session
                    .decode_frame(
                        sample_buffer,
                        VTDecodeFrameFlags::Frame_EnableAsynchronousDecompression,
                        std::ptr::null_mut(),
                    )
                    .map_err(|status| VtBackendError::VideoToolbox {
                        context: "VTDecompressionSession::decode_frame",
                        status,
                    })?;
            }
        }

        Ok(self.snapshot_decode_summary())
    }

    pub fn wait_for_completion(&self) -> Result<()> {
        self.session
            .finish_delayed_frames()
            .map_err(|status| VtBackendError::VideoToolbox {
                context: "VTDecompressionSession::finish_delayed_frames",
                status,
            })?;
        self.session
            .wait_for_asynchronous_frames()
            .map_err(|status| VtBackendError::VideoToolbox {
                context: "VTDecompressionSession::wait_for_asynchronous_frames",
                status,
            })?;

        Ok(())
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

    fn snapshot_decode_summary(&self) -> DecodeSummary {
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

    pub fn decode_annexb_file(
        &self,
        input_path: &Path,
        codec: Codec,
        fps: i32,
    ) -> Result<DecodeSummary> {
        let data = fs::read(input_path).map_err(|source| VtBackendError::ReadFile {
            path: input_path.to_path_buf(),
            source,
        })?;
        let parsed = parse_annexb(&data, codec)?;
        self.decode_access_units(&parsed.access_units, fps)?;
        self.wait_for_completion()?;
        Ok(self.snapshot_decode_summary())
    }
}

impl VtBitstreamDecoder {
    pub fn new(options: DecodeOptions, fps: i32) -> Self {
        Self {
            parser: AnnexBStreamParser::new(options.codec),
            options,
            fps,
            reported_decoded_frames: 0,
            decoder: None,
        }
    }

    pub fn push_bitstream_chunk(&mut self, chunk: &[u8]) -> Result<DecodeSummary> {
        let access_units = self.parser.push_chunk(chunk)?;

        self.try_initialize_decoder()?;

        if self.decoder.is_none() {
            return Ok(DecodeSummary {
                decoded_frames: 0,
                width: None,
                height: None,
                pixel_format: None,
            });
        }

        if !access_units.is_empty() {
            self.decoder
                .as_ref()
                .ok_or_else(|| VtBackendError::MissingParameterSet("decoder not initialized"))?
                .decode_access_units(&access_units, self.fps)?;
        }

        self.take_summary_delta(false)
    }

    pub fn flush(&mut self) -> Result<DecodeSummary> {
        let access_units = self.parser.flush()?;
        self.try_initialize_decoder()?;

        if self.decoder.is_none() {
            return Ok(DecodeSummary {
                decoded_frames: 0,
                width: None,
                height: None,
                pixel_format: None,
            });
        }

        if let Some(decoder) = self.decoder.as_ref() {
            if !access_units.is_empty() {
                decoder.decode_access_units(&access_units, self.fps)?;
            }
        }

        self.take_summary_delta(true)
    }

    fn take_summary_delta(&mut self, wait: bool) -> Result<DecodeSummary> {
        let decoder = self
            .decoder
            .as_ref()
            .ok_or_else(|| VtBackendError::MissingParameterSet("decoder not initialized"))?;

        if wait {
            decoder.wait_for_completion()?;
        }

        let summary = decoder.snapshot_decode_summary();
        let delta = summary
            .decoded_frames
            .saturating_sub(self.reported_decoded_frames);
        self.reported_decoded_frames = summary.decoded_frames;

        Ok(DecodeSummary {
            decoded_frames: delta,
            width: summary.width,
            height: summary.height,
            pixel_format: summary.pixel_format,
        })
    }

    fn try_initialize_decoder(&mut self) -> Result<()> {
        if self.decoder.is_some() {
            return Ok(());
        }

        if let Ok(parameter_sets) =
            required_parameter_sets(&self.parser.parameter_sets(), self.options.codec)
        {
            self.decoder = Some(VtDecoder::new(
                self.options.codec,
                &parameter_sets,
                self.options.require_hardware,
            )?);
        }

        Ok(())
    }
}

pub struct VtEncoder {
    session: VTCompressionSession,
    fps: i32,
}

impl VtEncoder {
    pub fn new(options: &EncodeOptions) -> Result<Self> {
        let mut encoder_specification = CFMutableDictionary::<CFString, CFType>::new();
        if options.require_hardware {
            encoder_specification.add(
                &VideoEncoderSpecification::RequireHardwareAcceleratedVideoEncoder.into(),
                &CFBoolean::true_value().as_CFType(),
            );
        }

        let source_image_buffer_attributes = CFMutableDictionary::<CFString, CFType>::new();
        let allocator = unsafe { CFAllocator::wrap_under_get_rule(kCFAllocatorSystemDefault) };

        let session = VTCompressionSession::new(
            options.width as i32,
            options.height as i32,
            options.codec.cm_codec_type(),
            encoder_specification.to_immutable(),
            source_image_buffer_attributes.to_immutable(),
            allocator,
        )
        .map_err(|status| VtBackendError::VideoToolbox {
            context: "VTCompressionSession::new",
            status,
        })?;

        let session_ref = session.as_session();
        session_ref
            .set_property(
                CompressionPropertyKey::RealTime.into(),
                CFBoolean::false_value().as_CFType(),
            )
            .map_err(|status| VtBackendError::VideoToolbox {
                context: "VTSessionSetProperty(RealTime)",
                status,
            })?;
        session_ref
            .set_property(
                CompressionPropertyKey::ExpectedFrameRate.into(),
                CFNumber::from(options.fps).as_CFType(),
            )
            .map_err(|status| VtBackendError::VideoToolbox {
                context: "VTSessionSetProperty(ExpectedFrameRate)",
                status,
            })?;
        session_ref
            .set_property(
                CompressionPropertyKey::MaxKeyFrameInterval.into(),
                CFNumber::from(options.fps.saturating_mul(2)).as_CFType(),
            )
            .map_err(|status| VtBackendError::VideoToolbox {
                context: "VTSessionSetProperty(MaxKeyFrameInterval)",
                status,
            })?;

        session
            .prepare_to_encode_frames()
            .map_err(|status| VtBackendError::VideoToolbox {
                context: "VTCompressionSession::prepare_to_encode_frames",
                status,
            })?;

        Ok(Self {
            session,
            fps: options.fps,
        })
    }

    pub fn encode_synthetic(
        &self,
        width: usize,
        height: usize,
        frame_count: usize,
    ) -> Result<Vec<Vec<u8>>> {
        let output_packets = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));

        for frame_index in 0..frame_count {
            let pixel_buffer = make_synthetic_bgra_frame(width, height, frame_index)?;
            let image_buffer =
                unsafe { CVImageBuffer::wrap_under_get_rule(pixel_buffer.as_concrete_TypeRef()) };

            let packets_ref = Arc::clone(&output_packets);
            self.session
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
                .map_err(|status| VtBackendError::VideoToolbox {
                    context: "VTCompressionSession::encode_frame_with_closure",
                    status,
                })?;
        }

        self.session
            .complete_frames(unsafe { kCMTimeInvalid })
            .map_err(|status| VtBackendError::VideoToolbox {
                context: "VTCompressionSession::complete_frames",
                status,
            })?;

        output_packets
            .lock()
            .map(|v| v.clone())
            .map_err(|_| VtBackendError::VideoToolbox {
                context: "encode output lock",
                status: -1,
            })
    }

    pub fn write_packets_to_file(output_path: &Path, packets: &[Vec<u8>]) -> Result<()> {
        let mut out = Vec::new();
        for packet in packets {
            out.extend_from_slice(packet);
        }
        fs::write(output_path, out).map_err(|source| VtBackendError::WriteFile {
            path: output_path.to_path_buf(),
            source,
        })
    }
}

pub fn load_and_prepare_annexb(path: &Path, codec: Codec) -> Result<BitstreamPrepared> {
    let data = fs::read(path).map_err(|source| VtBackendError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    parse_annexb(&data, codec)
}

pub fn default_decode_input(codec: Codec) -> PathBuf {
    PathBuf::from(format!(
        "../sample-videos/sample-10s.{}",
        codec.file_extension()
    ))
}

pub fn default_encode_output(codec: Codec) -> PathBuf {
    PathBuf::from(format!("./encoded-output.{}", codec.file_extension()))
}

fn create_format_description(
    codec: Codec,
    parameter_sets: &[Vec<u8>],
) -> Result<CMVideoFormatDescription> {
    let refs = parameter_sets
        .iter()
        .map(|v| v.as_slice())
        .collect::<Vec<_>>();
    match codec {
        Codec::H264 => {
            CMVideoFormatDescription::from_h264_parameter_sets(&refs, 4).map_err(|status| {
                VtBackendError::CoreMedia {
                    context: "CMVideoFormatDescription::from_h264_parameter_sets",
                    status,
                }
            })
        }
        Codec::Hevc => {
            CMVideoFormatDescription::from_hevc_parameter_sets(&refs, 4, Some(&empty_dictionary()))
                .map_err(|status| VtBackendError::CoreMedia {
                    context: "CMVideoFormatDescription::from_hevc_parameter_sets",
                    status,
                })
        }
    }
}

fn required_parameter_sets(parameter_sets: &[Vec<u8>], codec: Codec) -> Result<Vec<Vec<u8>>> {
    match codec {
        Codec::H264 => {
            let sps = parameter_sets
                .iter()
                .find(|ps| !ps.is_empty() && (ps[0] & 0x1f) == 7)
                .cloned()
                .ok_or(VtBackendError::MissingParameterSet("h264 sps"))?;
            let pps = parameter_sets
                .iter()
                .find(|ps| !ps.is_empty() && (ps[0] & 0x1f) == 8)
                .cloned()
                .ok_or(VtBackendError::MissingParameterSet("h264 pps"))?;
            Ok(vec![sps, pps])
        }
        Codec::Hevc => {
            let ntype = |ps: &Vec<u8>| (!ps.is_empty()).then_some((ps[0] >> 1) & 0x3f);
            let vps = parameter_sets
                .iter()
                .find(|ps| ntype(ps) == Some(32))
                .cloned()
                .ok_or(VtBackendError::MissingParameterSet("hevc vps"))?;
            let sps = parameter_sets
                .iter()
                .find(|ps| ntype(ps) == Some(33))
                .cloned()
                .ok_or(VtBackendError::MissingParameterSet("hevc sps"))?;
            let pps = parameter_sets
                .iter()
                .find(|ps| ntype(ps) == Some(34))
                .cloned()
                .ok_or(VtBackendError::MissingParameterSet("hevc pps"))?;
            Ok(vec![vps, sps, pps])
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
) -> Result<CVPixelBuffer> {
    let pixel_buffer =
        CVPixelBuffer::new(kCVPixelFormatType_32BGRA, width, height, None).map_err(|status| {
            VtBackendError::CoreVideo {
                context: "CVPixelBuffer::new",
                status,
            }
        })?;

    let lock_status = pixel_buffer.lock_base_address(0);
    if lock_status != 0 {
        return Err(VtBackendError::CoreVideo {
            context: "CVPixelBuffer::lock_base_address",
            status: lock_status,
        });
    }

    let bytes_per_row = pixel_buffer.get_bytes_per_row();
    let total = bytes_per_row.saturating_mul(height);
    let base_ptr = unsafe { pixel_buffer.get_base_address() } as *mut u8;

    if !base_ptr.is_null() && total > 0 {
        unsafe {
            let buffer = slice::from_raw_parts_mut(base_ptr, total);
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
        return Err(VtBackendError::CoreVideo {
            context: "CVPixelBuffer::unlock_base_address",
            status: unlock_status,
        });
    }

    Ok(pixel_buffer)
}

extern "C" fn vt_decode_output_callback(
    decompression_output_ref_con: *mut std::ffi::c_void,
    _source_frame_ref_con: *mut std::ffi::c_void,
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
