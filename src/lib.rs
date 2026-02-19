mod backend_transform_adapter;
mod bitstream;
mod contract;
#[cfg(feature = "backend-nvidia")]
mod cuda_transform;
#[cfg(feature = "backend-nvidia")]
mod nv_backend;
#[cfg(feature = "backend-nvidia")]
mod nv_meta_decoder;
mod pipeline;
mod pipeline_scheduler;
mod transform;

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
mod vt_backend;

pub use backend_transform_adapter::{
    BackendTransformAdapter, DecodedUnit, NvidiaTransformAdapter, VtTransformAdapter,
};
pub use contract::{
    BackendEncoderOptions, BackendError, CapabilityReport, Codec, DecodeSummary, DecoderConfig,
    EncodedPacket, EncoderConfig, Frame, NvidiaEncoderOptions, NvidiaSessionConfig,
    SessionSwitchMode, SessionSwitchRequest, VideoDecoder, VideoEncoder,
};
pub use pipeline::{
    BoundedQueueRx, BoundedQueueTx, InFlightCredits, QueueRecvError, QueueSendError, QueueStats,
    bounded_queue,
};
pub use pipeline_scheduler::PipelineScheduler;
pub use transform::{
    ColorRequest, Nv12Frame, RgbFrame, TransformDispatcher, TransformJob, TransformResult,
    make_argb_to_nv12_dummy, nv12_to_rgb24, should_enqueue_transform,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    VideoToolbox,
    Nvidia,
}

pub struct Decoder {
    inner: Box<dyn VideoDecoder>,
}

impl Decoder {
    pub fn new(kind: BackendKind, config: DecoderConfig) -> Self {
        let inner: Box<dyn VideoDecoder> = match kind {
            BackendKind::VideoToolbox => {
                #[cfg(all(target_os = "macos", feature = "backend-vt"))]
                {
                    Box::new(vt_backend::VtDecoderAdapter::new(config))
                }
                #[cfg(not(all(target_os = "macos", feature = "backend-vt")))]
                {
                    let _ = config;
                    Box::new(UnsupportedDecoder::new(
                        "VideoToolbox backend requires macOS + backend-vt feature",
                    ))
                }
            }
            BackendKind::Nvidia => {
                #[cfg(feature = "backend-nvidia")]
                {
                    Box::new(nv_backend::NvDecoderAdapter::new(config))
                }
                #[cfg(not(feature = "backend-nvidia"))]
                {
                    let _ = config;
                    Box::new(UnsupportedDecoder::new(
                        "NVIDIA backend requires backend-nvidia feature",
                    ))
                }
            }
        };
        Self { inner }
    }

    pub fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        self.inner.push_bitstream_chunk(chunk, pts_90k)
    }

    pub fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        self.inner.flush()
    }

    pub fn decode_summary(&self) -> DecodeSummary {
        self.inner.decode_summary()
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.inner.query_capability(codec)
    }
}

pub struct Encoder {
    inner: Box<dyn VideoEncoder>,
}

impl Encoder {
    pub fn new(kind: BackendKind, codec: Codec, fps: i32, require_hardware: bool) -> Self {
        let config = EncoderConfig::new(codec, fps, require_hardware);
        Self::with_config(kind, config)
    }

    pub fn with_config(kind: BackendKind, config: EncoderConfig) -> Self {
        let inner: Box<dyn VideoEncoder> = match kind {
            BackendKind::VideoToolbox => {
                #[cfg(all(target_os = "macos", feature = "backend-vt"))]
                {
                    Box::new(vt_backend::VtEncoderAdapter::with_config(
                        config.codec,
                        config.fps,
                        config.require_hardware,
                    ))
                }
                #[cfg(not(all(target_os = "macos", feature = "backend-vt")))]
                {
                    let _ = config;
                    Box::new(UnsupportedEncoder::new(
                        "VideoToolbox backend requires macOS + backend-vt feature",
                    ))
                }
            }
            BackendKind::Nvidia => {
                #[cfg(feature = "backend-nvidia")]
                {
                    Box::new(nv_backend::NvEncoderAdapter::with_config(
                        config.codec,
                        config.fps,
                        config.require_hardware,
                        config.backend_options,
                    ))
                }
                #[cfg(not(feature = "backend-nvidia"))]
                {
                    let _ = config;
                    Box::new(UnsupportedEncoder::new(
                        "NVIDIA backend requires backend-nvidia feature",
                    ))
                }
            }
        };
        Self { inner }
    }

    pub fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        self.inner.push_frame(frame)
    }

    pub fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        self.inner.flush()
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.inner.query_capability(codec)
    }

    pub fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        self.inner.request_session_switch(request)
    }

    pub fn sync_pipeline_generation(&self, scheduler: &PipelineScheduler) {
        if let Some(generation) = self.inner.pipeline_generation_hint() {
            scheduler.set_generation(generation.max(1));
        }
    }
}

struct UnsupportedDecoder {
    reason: String,
    summary: DecodeSummary,
}

impl UnsupportedDecoder {
    fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
            summary: DecodeSummary {
                decoded_frames: 0,
                width: None,
                height: None,
                pixel_format: None,
            },
        }
    }
}

impl VideoDecoder for UnsupportedDecoder {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec,
            decode_supported: false,
            encode_supported: false,
            hardware_acceleration: false,
        })
    }

    fn push_bitstream_chunk(
        &mut self,
        _chunk: &[u8],
        _pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.reason.clone()))
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.reason.clone()))
    }

    fn decode_summary(&self) -> DecodeSummary {
        self.summary.clone()
    }
}

struct UnsupportedEncoder {
    reason: String,
}

impl UnsupportedEncoder {
    fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
        }
    }
}

impl VideoEncoder for UnsupportedEncoder {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec,
            decode_supported: false,
            encode_supported: false,
            hardware_acceleration: false,
        })
    }

    fn push_frame(&mut self, _frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.reason.clone()))
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.reason.clone()))
    }
}
