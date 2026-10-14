//! Public NVIDIA adapters backed by private capability, decode, encode, metrics, and session modules.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cudarc::driver::CudaContext;
use nvidia_video_codec_sdk::{DecodeCodec, Encoder, EncoderInitParams, ErrorKind};

use crate::nv_bitstream::{AccessUnit, StatefulBitstreamAssembler};
use crate::nv_meta_decoder::NvMetaDecoder;
use video_hw_core::{
    BackendDecoderOptions, BackendEncoderOptions, BackendError, CapabilityContract,
    CapabilityReport, Codec, DecodeCapability, DecodeOutputCapability, DecodeOutputMode,
    DecodeOutputOrigin, DecodeSummary, DecoderConfig, DimensionConstraints, Dimensions,
    EncodeCapability, EncodeInputFormat, EncodedContentKind, EncodedLayout, EncodedPacket,
    EncoderConfig, FallbackPolicy, Frame, Nv12FramePayload, NvidiaEncoderOptions,
    NvidiaSessionConfig, RuntimeCapability, RuntimeStatus, SessionSwitchMode, SessionSwitchRequest,
    StreamingMode, VideoDecoder, VideoEncoder,
};

mod capability;
mod decoder;
mod encoder;
mod metrics;
mod session;

#[cfg(test)]
mod tests;

pub use decoder::NvDecoderAdapter;
pub use encoder::NvEncoderAdapter;

use capability::*;
use metrics::*;
use session::*;
