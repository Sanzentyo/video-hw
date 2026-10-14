use crate::{
    BackendError, CapabilityReport, Codec, DecodeSummary, EncodedPacket, Frame,
    SessionSwitchRequest,
};

pub trait VideoDecoder {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError>;

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError>;

    fn try_reap(&mut self) -> Result<Vec<Frame>, BackendError> {
        Ok(Vec::new())
    }

    fn decode_summary(&self) -> DecodeSummary;
}

pub trait VideoEncoder {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError>;

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError>;

    fn try_reap(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        Ok(Vec::new())
    }

    fn request_session_switch(
        &mut self,
        _request: SessionSwitchRequest,
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "session switching is not supported by this backend".to_string(),
        ))
    }

    fn pipeline_generation_hint(&self) -> Option<u64> {
        None
    }
}
