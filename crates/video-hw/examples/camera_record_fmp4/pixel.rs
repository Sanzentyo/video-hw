//! Camera pixel conversion, preview frames, and backend/dimension helpers.

use std::{borrow::Cow, num::NonZeroU32};

use anyhow::{Context, Result};
use shiguredo_video_device::{PixelFormat, VideoFrame};
use video_hw::{BackendKind, Dimensions};

#[derive(Debug, Clone)]
pub(super) struct UiFrame {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) timestamp_us: i64,
    pub(super) rgba: Vec<u8>,
}

pub(super) fn rgba_to_argb_bytes(rgba: &[u8]) -> Vec<u8> {
    let mut argb = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        argb.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    argb
}

pub(super) fn strip_stride<'a>(
    data: &'a [u8],
    row_bytes: usize,
    height: usize,
    stride: usize,
) -> Cow<'a, [u8]> {
    if stride == row_bytes {
        Cow::Borrowed(&data[..row_bytes * height])
    } else {
        let mut result = Vec::with_capacity(row_bytes * height);
        for row in 0..height {
            let start = row * stride;
            result.extend_from_slice(&data[start..start + row_bytes]);
        }
        Cow::Owned(result)
    }
}

pub(super) fn frame_to_rgba(frame: &VideoFrame<'_>) -> Option<UiFrame> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let stride = frame.stride as usize;
    let stride_uv = frame.stride_uv as usize;

    let rgba = match frame.pixel_format {
        PixelFormat::Nv12 => {
            let y = strip_stride(frame.data, w, h, stride);
            let uv_data = frame.uv_data?;
            let uv = strip_stride(uv_data, w, h / 2, stride_uv);
            nv12_to_rgba(&y, &uv, w, h)
        }
        PixelFormat::I420 => {
            let y = strip_stride(frame.data, w, h, stride);
            let uv_data = frame.uv_data?;
            let uv_w = w / 2;
            let uv_h = h / 2;
            let u_plane_size = stride_uv * uv_h;
            if uv_data.len() < u_plane_size * 2 {
                return None;
            }
            let u = strip_stride(&uv_data[..u_plane_size], uv_w, uv_h, stride_uv);
            let v = strip_stride(
                &uv_data[u_plane_size..u_plane_size * 2],
                uv_w,
                uv_h,
                stride_uv,
            );
            i420_to_rgba(&y, &u, &v, w, h)
        }
        PixelFormat::Yuy2 => {
            let data = strip_stride(frame.data, w * 2, h, stride);
            yuy2_to_rgba(&data, w, h)
        }
        PixelFormat::Unknown(_) => {
            use std::sync::Once;
            static WARN: Once = Once::new();
            WARN.call_once(|| {
                eprintln!(
                    "warning: unsupported pixel format for preview/recording: {}",
                    frame.pixel_format.name()
                );
            });
            return None;
        }
    };

    Some(UiFrame {
        width: w,
        height: h,
        timestamp_us: frame.timestamp_us,
        rgba,
    })
}

pub(super) fn nv12_to_rgba(y_plane: &[u8], uv_plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let y_row = row * w;
        let uv_row = (row / 2) * w;
        for col in 0..w {
            let y = y_plane[y_row + col];
            let uv_index = uv_row + (col / 2) * 2;
            let u = uv_plane[uv_index];
            let v = uv_plane[uv_index + 1];
            let (r, g, b) = yuv_to_rgb(y, u, v);
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

pub(super) fn i420_to_rgba(
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    w: usize,
    h: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    let uv_w = w / 2;
    for row in 0..h {
        let y_row = row * w;
        let uv_row = (row / 2) * uv_w;
        for col in 0..w {
            let y = y_plane[y_row + col];
            let uv_index = uv_row + (col / 2);
            let u = u_plane[uv_index];
            let v = v_plane[uv_index];
            let (r, g, b) = yuv_to_rgb(y, u, v);
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

pub(super) fn yuy2_to_rgba(data: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let row_start = row * w * 2;
        let row_data = &data[row_start..row_start + w * 2];
        for chunk in row_data.chunks_exact(4) {
            let y0 = chunk[0];
            let u = chunk[1];
            let y1 = chunk[2];
            let v = chunk[3];
            let (r0, g0, b0) = yuv_to_rgb(y0, u, v);
            let (r1, g1, b1) = yuv_to_rgb(y1, u, v);
            out.extend_from_slice(&[r0, g0, b0, 255]);
            out.extend_from_slice(&[r1, g1, b1, 255]);
        }
    }
    out
}

pub(super) fn yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let c = (y as i32 - 16).max(0);
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let r = (298 * c + 409 * e + 128) >> 8;
    let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
    let b = (298 * c + 516 * d + 128) >> 8;
    (clamp_u8(r), clamp_u8(g), clamp_u8(b))
}

pub(super) fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
pub(super) fn backend_is_intel(backend: BackendKind) -> bool {
    matches!(backend, BackendKind::Intel)
}

#[cfg(not(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
)))]
pub(super) fn backend_is_intel(_backend: BackendKind) -> bool {
    false
}

pub(super) fn dims(width: u32, height: u32) -> Result<Dimensions> {
    let width = NonZeroU32::new(width).context("width must be > 0")?;
    let height = NonZeroU32::new(height).context("height must be > 0")?;
    Ok(Dimensions { width, height })
}
