//! Intel oneVPL backend facade.
//!
//! Decoder, encoder, surface conversion, native adaptation, and tests are isolated in focused
//! child modules. This facade owns only shared imports and the two public adapter re-exports.
use std::collections::VecDeque;
use std::io::{Cursor, Read, Write};

use onevpl::MfxStatus;
use onevpl::bitstream::Bitstream;
use onevpl::constants::{
    ChromaFormat, Codec as OneVplCodec, CodingOptionValue, FourCC, FrameType, IoPattern,
    MemoryFlag, PicStruct, RateControlMethod, TargetUsage,
};
use onevpl::encode::EncodeCtrl;
use onevpl::vpp::VppVideoParams;
use tokio::runtime::Builder as RuntimeBuilder;

use video_hw_core::{
    BackendDecoderOptions, BackendEncoderOptions, BackendError, CapabilityContract,
    CapabilityReport, Codec, DecodeCapability, DecodeOutputCapability, DecodeOutputMode,
    DecodeOutputOrigin, DecodeSummary, DecoderConfig, DimensionConstraints, Dimensions,
    EncodeCapability, EncodeInputFormat, EncodedContentKind, EncodedLayout, EncodedPacket,
    EncoderConfig, FallbackPolicy, Frame, IntelDecoderOptions, IntelEncoderOptions,
    Nv12FramePayload, RuntimeCapability, RuntimeStatus, SessionSwitchRequest, StreamingMode,
    VideoDecoder, VideoEncoder,
};

mod capability;
mod decoder;
mod encoder;
mod native;
mod surface;
#[cfg(test)]
mod tests;

pub use decoder::IntelDecoderAdapter;
pub use encoder::IntelEncoderAdapter;

use capability::{intel_capability_report, intel_encoded_layout};
use decoder::bump_decode_pts;
#[cfg(test)]
use native::catch_onevpl_loader_initialization;
use native::{
    OneVplOperation, configured_onevpl_loader, default_rate_control_method, default_target_kbps,
    env_bool, map_onevpl_status, parse_rate_control_method, to_onevpl_codec,
};
use surface::{
    argb_to_i420, argb_to_yv12, metadata_frame_from_surface, surface_to_backend_frame,
    write_argb_to_bgra_surface, write_argb_to_nv12_surface, write_nv12_to_surface,
};
