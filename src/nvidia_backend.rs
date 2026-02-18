use backend_contract::{BackendError, CapabilityReport, Codec, DecodeSummary, EncodedPacket, Frame, VideoDecoder, VideoEncoder};
use bitstream_core::{AccessUnit, StatefulBitstreamAssembler};

pub struct PackedSample { pub data: Vec<u8> }

pub trait SamplePacker { fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample, BackendError>; }

#[derive(Debug, Default)]
pub struct AnnexBPacker;

impl SamplePacker for AnnexBPacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample, BackendError> {
        let total_size: usize = access_unit.nalus.iter().map(|nal| nal.len().saturating_add(4)).sum();
        let mut data = Vec::with_capacity(total_size);
        for nal in &access_unit.nalus {
            data.extend_from_slice(&[0,0,0,1]);
            data.extend_from_slice(nal);
        }
        Ok(PackedSample { data })
    }
}

pub struct NvidiaDecoderAdapter { assembler: StatefulBitstreamAssembler, last_summary: DecodeSummary }

impl NvidiaDecoderAdapter {
    pub fn new() -> Self {
        Self { assembler: StatefulBitstreamAssembler::new(), last_summary: DecodeSummary { decoded_frames: 0, width: None, height: None, pixel_format: None } }
    }
}

impl Default for NvidiaDecoderAdapter { fn default() -> Self { Self::new() } }

impl VideoDecoder for NvidiaDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport { codec, decode_supported: true, encode_supported: true, hardware_acceleration: true })
    }

    fn push_bitstream_chunk(&mut self, chunk: &[u8], pts_90k: Option<i64>) -> Result<Vec<Frame>, BackendError> {
        let _ = self.assembler.push_chunk(chunk, Codec::H264, pts_90k)?;
        Err(BackendError::UnsupportedConfig("nvidia-sdk bridge is not wired in scaffold yet".to_string()))
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> { Ok(Vec::new()) }

    fn decode_summary(&self) -> DecodeSummary { self.last_summary.clone() }
}

pub struct NvidiaEncoderAdapter;

impl NvidiaEncoderAdapter { pub fn new() -> Self { Self } }
impl Default for NvidiaEncoderAdapter { fn default() -> Self { Self::new() } }

impl VideoEncoder for NvidiaEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> { Ok(CapabilityReport { codec, decode_supported: true, encode_supported: true, hardware_acceleration: true }) }

    fn push_frame(&mut self, _frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> { Err(BackendError::UnsupportedConfig("nvidia-sdk bridge is not wired in scaffold yet".to_string())) }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> { Ok(Vec::new()) }
}
