//! Color conversion skeleton.
//!
//! Android decoders may output planar, semi-planar, flexible YUV, or vendor-specific formats.
//! The MVP should accept only known CPU-accessible layouts and convert them into video-hw's NV12.

use video_hw_core::BackendError;

#[derive(Debug, Clone, Copy)]
pub struct AndroidYuvLayout {
    pub width: usize,
    pub height: usize,
    pub y_stride: usize,
    pub u_stride: usize,
    pub v_stride: usize,
    pub u_pixel_stride: usize,
    pub v_pixel_stride: usize,
}

pub fn yuv420_to_nv12(
    y: &[u8],
    u: &[u8],
    v: &[u8],
    layout: AndroidYuvLayout,
) -> Result<(usize, Vec<u8>), BackendError> {
    if layout.width == 0 || layout.height == 0 {
        return Err(BackendError::InvalidInput("invalid yuv dimensions".to_string()));
    }

    let pitch = layout.width;
    let y_size = pitch * layout.height;
    let uv_size = y_size / 2;
    let mut out = vec![0u8; y_size + uv_size];

    for row in 0..layout.height {
        let src = row * layout.y_stride;
        let dst = row * pitch;
        out[dst..dst + layout.width].copy_from_slice(&y[src..src + layout.width]);
    }

    let uv_base = y_size;
    for row in 0..(layout.height / 2) {
        for col in 0..(layout.width / 2) {
            let u_idx = row * layout.u_stride + col * layout.u_pixel_stride;
            let v_idx = row * layout.v_stride + col * layout.v_pixel_stride;
            let dst = uv_base + row * pitch + col * 2;
            out[dst] = u[u_idx];
            out[dst + 1] = v[v_idx];
        }
    }

    Ok((pitch, out))
}
