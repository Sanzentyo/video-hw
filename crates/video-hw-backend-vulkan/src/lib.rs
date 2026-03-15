pub use video_hw_core::*;

#[derive(Debug, Clone)]
pub struct Nv12Frame {
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
    pub pts_90k: Option<i64>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RgbFrame {
    pub width: usize,
    pub height: usize,
    pub pts_90k: Option<i64>,
    pub data: Vec<u8>,
}

pub fn nv12_to_rgb24(frame: &Nv12Frame) -> Result<RgbFrame, BackendError> {
    let width = frame.width;
    let height = frame.height;
    let pitch = frame.pitch.max(width);
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "nv12 frame dimensions must be positive".to_string(),
        ));
    }
    if width > pitch {
        return Err(BackendError::InvalidInput(
            "nv12 width exceeds pitch".to_string(),
        ));
    }
    let luma_size = pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    let total_size = luma_size
        .checked_add(luma_size / 2)
        .ok_or_else(|| BackendError::InvalidInput("nv12 total size overflow".to_string()))?;
    if frame.data.len() < total_size {
        return Err(BackendError::InvalidInput(
            "nv12 data is smaller than expected".to_string(),
        ));
    }

    let uv_base = luma_size;
    let mut rgb = vec![0_u8; width.saturating_mul(height).saturating_mul(3)];
    for y in 0..height {
        let y_row = y * pitch;
        let uv_row = uv_base + (y / 2) * pitch;
        let dst_row = y * width * 3;
        for x in 0..width {
            let y_value = i32::from(frame.data[y_row + x]);
            let uv_index = uv_row + (x & !1);
            let u_value = i32::from(frame.data[uv_index]);
            let v_value = i32::from(frame.data[uv_index + 1]);

            let c = (y_value - 16).max(0);
            let d = u_value - 128;
            let e = v_value - 128;
            let r = clip_to_u8((298 * c + 409 * e + 128) >> 8);
            let g = clip_to_u8((298 * c - 100 * d - 208 * e + 128) >> 8);
            let b = clip_to_u8((298 * c + 516 * d + 128) >> 8);

            let dst = dst_row + x * 3;
            rgb[dst] = r;
            rgb[dst + 1] = g;
            rgb[dst + 2] = b;
        }
    }

    Ok(RgbFrame {
        width,
        height,
        pts_90k: frame.pts_90k,
        data: rgb,
    })
}

#[inline]
fn clip_to_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

pub fn argb_to_nv12(
    argb: &[u8],
    width: usize,
    height: usize,
) -> Result<(usize, Vec<u8>), BackendError> {
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("argb size overflow".to_string()))?;
    if argb.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "argb payload size mismatch: expected {}, got {}",
            expected,
            argb.len()
        )));
    }
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "argb frame dimensions must be positive".to_string(),
        ));
    }
    let pitch = width;
    let y_size = pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    let uv_size = y_size / 2;
    let mut out = vec![0_u8; y_size + uv_size];

    let mut y_plane = vec![0_u8; y_size];
    let mut u_plane = vec![0_u8; width * height];
    let mut v_plane = vec![0_u8; width * height];

    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 4;
            let r = argb[src + 1] as f32;
            let g = argb[src + 2] as f32;
            let b = argb[src + 3] as f32;

            let yy = (0.257 * r + 0.504 * g + 0.098 * b + 16.0).round() as i32;
            let uu = (-0.148 * r - 0.291 * g + 0.439 * b + 128.0).round() as i32;
            let vv = (0.439 * r - 0.368 * g - 0.071 * b + 128.0).round() as i32;

            y_plane[y * pitch + x] = yy.clamp(0, 255) as u8;
            u_plane[y * width + x] = uu.clamp(0, 255) as u8;
            v_plane[y * width + x] = vv.clamp(0, 255) as u8;
        }
    }

    out[..y_size].copy_from_slice(&y_plane);
    let uv_base = y_size;
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let idx = y * width + x;
            let idx1 = idx;
            let idx2 = idx + (x + 1 < width) as usize;
            let idx3 = idx + if y + 1 < height { width } else { 0 };
            let idx4 = idx3 + (x + 1 < width) as usize;

            let u_avg = ((u_plane[idx1] as u32
                + u_plane[idx2] as u32
                + u_plane[idx3] as u32
                + u_plane[idx4] as u32)
                / 4) as u8;
            let v_avg = ((v_plane[idx1] as u32
                + v_plane[idx2] as u32
                + v_plane[idx3] as u32
                + v_plane[idx4] as u32)
                / 4) as u8;

            let uv_row = (y / 2) * pitch;
            let uv_col = x & !1;
            let dst = uv_base + uv_row + uv_col;
            out[dst] = u_avg;
            if dst + 1 < out.len() {
                out[dst + 1] = v_avg;
            }
        }
    }

    Ok((pitch, out))
}

mod vulkan_hevc_decode;

mod vulkan_backend;

pub use vulkan_backend::{VulkanDecoderAdapter, VulkanEncoderAdapter};
