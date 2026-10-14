//! VideoToolbox backend facade.
//!
//! Decoder, encoder, sample packing, pixel buffers, native callbacks, and configuration live in
//! focused child modules. This facade owns shared imports and the public adapter re-exports.
use std::{
    collections::VecDeque,
    ffi::c_void,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crate::vt_bitstream::{AccessUnit, ParameterSetCache, StatefulBitstreamAssembler};
use core_foundation::{
    base::{CFAllocator, CFType, TCFType, kCFAllocatorSystemDefault},
    boolean::CFBoolean,
    data::CFData,
    dictionary::{CFDictionary, CFMutableDictionary},
    number::CFNumber,
    string::CFString,
};
use core_media::{
    block_buffer::CMBlockBuffer,
    format_description::{
        CMFormatDescription, CMFormatDescriptionRef, CMVideoCodecType, CMVideoFormatDescription,
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
        CMVideoFormatDescriptionGetHEVCParameterSetAtIndex,
        kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms, kCMVideoCodecType_AV1,
        kCMVideoCodecType_H264, kCMVideoCodecType_HEVC,
    },
    sample_buffer::{
        CMSampleBuffer, CMSampleBufferGetFormatDescription, CMSampleBufferRef, CMSampleTimingInfo,
    },
    time::{CMTime, kCMTimeInvalid},
};
use core_video::{
    buffer::TCVBuffer,
    image_buffer::{
        CVColorPrimariesGetIntegerCodePointForString, CVImageBuffer, CVImageBufferKeys,
        CVTransferFunctionGetIntegerCodePointForString, CVYCbCrMatrixGetIntegerCodePointForString,
    },
    pixel_buffer::{CVPixelBuffer, CVPixelBufferKeys, kCVPixelFormatType_32BGRA},
};
use video_hw_core::{
    BackendDecoderOptions, BackendEncoderOptions, BackendError, CapabilityContract,
    CapabilityReport, Codec, ColorMetadata, DecodeCapability, DecodeOutputCapability,
    DecodeOutputMode, DecodeOutputOrigin, DecodeSummary, DecoderConfig, DimensionConstraints,
    EncodeCapability, EncodeInputFormat, EncodedContentKind, EncodedLayout, EncodedPacket,
    EncoderConfig, FallbackPolicy, Frame, RuntimeCapability, RuntimeStatus, SessionSwitchMode,
    SessionSwitchRequest, StreamingMode, VideoDecoder, VideoEncoder, VtDecoderOptions,
    VtEncoderOptions, VtSessionConfig,
};
use video_toolbox::{
    compression_properties::{
        CompressionPropertyKey, EncodeFrameOptionKey, VideoEncoderSpecification,
    },
    compression_session::VTCompressionSession,
    decompression_properties::VideoDecoderSpecification,
    decompression_session::{VTDecompressionOutputCallbackRecord, VTDecompressionSession},
    errors::VTDecodeFrameFlags,
    session::TVTSession,
};

mod capability;
mod decoder;
mod encoder;
mod native;
mod pixel;
mod sample;
#[cfg(test)]
mod tests;

pub use decoder::VtDecoderAdapter;
pub use encoder::VtEncoderAdapter;

use capability::{
    cm_time_from_90k, codec_label, ensure_vt_codec_implemented, frame_encode_properties,
    option_or_env_bool, packed_access_units_bytes, to_cm_codec_type, update_jitter_samples,
    update_peak, vt_capability_report, vt_decoder_options, vt_encoded_layout, vt_encoder_options,
};
use decoder::DecodeOutputState;
use encoder::SampleStats;
use native::{
    cm_error, cv_error, record_vt_encode_callback_error, vt_decode_output_callback, vt_error,
};
use pixel::{
    cm_time_to_90k, copy_argb_payload_from_pixel_buffer, extract_color_metadata, make_bgra_frame,
};
use sample::{
    SamplePacker, VtSamplePacker, create_format_description,
    decode_destination_image_buffer_attributes, detect_keyframe_from_avcc_hvcc_payload,
    empty_dictionary, parameter_sets_from_sample_buffer_ref,
    prepend_length_prefixed_parameter_sets,
};
