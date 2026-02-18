use backend_contract::{
    BackendError, CapabilityReport, Codec, EncodedPacket, Frame, VideoDecoder,
    VideoEncoder,
};
use bitstream_core::StatefulBitstreamAssembler;

pub struct VtDecoderAdapter {
    assembler: StatefulBitstreamAssembler,
}

impl VtDecoderAdapter {
    pub fn new() -> Self {
        Self {
            assembler: StatefulBitstreamAssembler::new(),
        }
    }
}

impl Default for VtDecoderAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoDecoder for VtDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec,
            decode_supported: true,
            encode_supported: true,
            hardware_acceleration: true,
        })
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        let _ = self.assembler.push_chunk(chunk, Codec::H264, pts_90k);
        Ok(Vec::new())
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        Ok(Vec::new())
    }
}

pub struct VtEncoderAdapter;

impl VtEncoderAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VtEncoderAdapter {
    fn default() -> Self {
        Self::new()
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

    fn push_frame(&mut self, _frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        Ok(Vec::new())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        Ok(Vec::new())
    }
}
