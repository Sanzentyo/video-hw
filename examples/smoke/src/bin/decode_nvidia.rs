use backend_contract::VideoDecoder;
use nvidia_backend::NvidiaDecoderAdapter;

fn main() {
    let mut decoder = NvidiaDecoderAdapter::new();
    let _ = decoder.push_bitstream_chunk(&[], None);
    let _summary = decoder.decode_summary();
}
