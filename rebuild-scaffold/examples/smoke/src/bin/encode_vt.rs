use backend_contract::{Frame, VideoEncoder};
use vt_backend::VtEncoderAdapter;

fn main() {
    let mut encoder = VtEncoderAdapter::new();
    let frame = Frame {
        width: 1,
        height: 1,
        pixel_format: None,
        pts_90k: Some(0),
    };
    let _ = encoder.push_frame(frame);
}
