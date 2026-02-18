use backend_contract::{Codec, DecoderConfig, EncodedPacket, Frame, VideoDecoder, VideoEncoder};

pub use backend_contract::{BackendError, CapabilityReport, DecodeSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    VideoToolbox,
    Nvidia,
}

impl Default for BackendKind {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        {
            BackendKind::VideoToolbox
        }
        #[cfg(all(not(target_os = "macos"), target_os = "linux"))]
        {
            BackendKind::Nvidia
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            BackendKind::VideoToolbox
        }
    }
}

#[cfg(target_os = "macos")]
mod vt_backend;
mod nvidia_backend;

struct UnsupportedDecoder(&'static str);
struct UnsupportedEncoder(&'static str);

impl UnsupportedDecoder {
    fn new(msg: &'static str) -> Self {
        Self(msg)
    }
}
impl UnsupportedEncoder {
    fn new(msg: &'static str) -> Self {
        Self(msg)
    }
}

impl VideoDecoder for UnsupportedDecoder {
    fn query_capability(&self, _codec: Codec) -> Result<CapabilityReport, BackendError> {
        Err(BackendError::UnsupportedConfig(self.0.to_string()))
    }

    fn push_bitstream_chunk(
        &mut self,
        _chunk: &[u8],
        _pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.0.to_string()))
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.0.to_string()))
    }

    fn decode_summary(&self) -> DecodeSummary {
        DecodeSummary {
            decoded_frames: 0,
            width: None,
            height: None,
            pixel_format: None,
        }
    }
}

impl VideoEncoder for UnsupportedEncoder {
    fn query_capability(&self, _codec: Codec) -> Result<CapabilityReport, BackendError> {
        Err(BackendError::UnsupportedConfig(self.0.to_string()))
    }

    fn push_frame(&mut self, _frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.0.to_string()))
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.0.to_string()))
    }
}

pub struct Decoder {
    inner: Box<dyn VideoDecoder>,
}

impl Decoder {
    pub fn new(kind: BackendKind, config: DecoderConfig) -> Self {
        let inner: Box<dyn VideoDecoder> = match kind {
            BackendKind::VideoToolbox => {
                #[cfg(target_os = "macos")]
                {
                    Box::new(vt_backend::VtDecoderAdapter::new(config))
                }
                #[cfg(not(target_os = "macos"))]
                {
                    Box::new(UnsupportedDecoder::new("vt backend not supported on this platform"))
                }
            }
            BackendKind::Nvidia => {
                Box::new(nvidia_backend::NvidiaDecoderAdapter::new())
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
        let inner: Box<dyn VideoEncoder> = match kind {
            BackendKind::VideoToolbox => {
                #[cfg(target_os = "macos")]
                {
                    Box::new(vt_backend::VtEncoderAdapter::with_config(
                        codec,
                        fps,
                        require_hardware,
                    ))
                }
                #[cfg(not(target_os = "macos"))]
                {
                    Box::new(UnsupportedEncoder::new("vt backend not supported on this platform"))
                }
            }
            BackendKind::Nvidia => {
                Box::new(nvidia_backend::NvidiaEncoderAdapter::new())
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
}
