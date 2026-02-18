use backend_contract::{Codec, DecoderConfig, VideoDecoder};
use vt_backend::VtDecoderAdapter;

fn main() {
    let mut decoder = VtDecoderAdapter::new(DecoderConfig {
        codec: Codec::H264,
        fps: 30,
        require_hardware: false,
    });
    let _ = decoder.push_bitstream_chunk(&[], None);
    let _summary = decoder.decode_summary();
}
