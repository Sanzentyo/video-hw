use backend_contract::{Codec, Frame};
use video_hw::{BackendKind, Encoder};

fn main() {
    let mut encoder = Encoder::new(BackendKind::Nvidia, Codec::H264, 30, true);
    let frame = Frame {
        width: 1,
        height: 1,
        pixel_format: None,
        pts_90k: Some(0),
    };
    let _ = encoder.push_frame(frame);
}
