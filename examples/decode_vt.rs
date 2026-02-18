use backend_contract::{Codec, DecoderConfig};
use video_hw::{BackendKind, Decoder};

fn main() {
    let mut decoder = Decoder::new(
        BackendKind::VideoToolbox,
        DecoderConfig {
            codec: Codec::H264,
            fps: 30,
            require_hardware: false,
        },
    );
    let _ = decoder.push_bitstream_chunk(&[], None);
    let _summary = decoder.decode_summary();
}
