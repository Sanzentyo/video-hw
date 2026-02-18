use backend_contract::VideoDecoder;
use vt_backend::VtDecoderAdapter;

fn main() {
    let mut decoder = VtDecoderAdapter::new();
    let _ = decoder.push_bitstream_chunk(&[], None);
}
