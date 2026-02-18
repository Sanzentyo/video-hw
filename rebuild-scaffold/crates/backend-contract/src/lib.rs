#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub pixel_format: Option<u32>,
    pub pts_90k: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct EncodedPacket {
    pub codec: Codec,
    pub data: Vec<u8>,
    pub pts_90k: Option<i64>,
    pub is_keyframe: bool,
}

#[derive(Debug, Clone)]
pub struct CapabilityReport {
    pub codec: Codec,
    pub decode_supported: bool,
    pub encode_supported: bool,
    pub hardware_acceleration: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("unsupported codec: {0:?}")]
    UnsupportedCodec(Codec),
    #[error("unsupported config: {0}")]
    UnsupportedConfig(String),
    #[error("invalid bitstream: {0}")]
    InvalidBitstream(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("temporary backpressure: {0}")]
    TemporaryBackpressure(String),
    #[error("device lost: {0}")]
    DeviceLost(String),
    #[error("backend error: {0}")]
    Backend(String),
}

pub trait VideoDecoder: Send {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError>;

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError>;
}

pub trait VideoEncoder: Send {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError>;

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError>;
}
