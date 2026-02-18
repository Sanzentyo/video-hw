use backend_contract::{Codec, Frame, VideoEncoder};
use vt_backend::VtEncoderAdapter;

fn main() {
    let mut encoder = VtEncoderAdapter::with_config(Codec::H264, 30, false);
    let frame = Frame {
        width: 1,
        height: 1,
        pixel_format: None,
        pts_90k: Some(0),
    };
    let _ = encoder.push_frame(frame);
}
