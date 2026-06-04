use video_hw_core::BackendError;

pub(crate) fn argb_to_nv12(
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
    let pitch = width;
    let y_size = pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    let mut out = vec![0_u8; y_size + y_size / 2];
    for y in (0..height).step_by(2) {
        let uv_row = y_size + (y / 2) * pitch;
        for x in (0..width).step_by(2) {
            let mut u_acc = 0_i32;
            let mut v_acc = 0_i32;
            let mut count = 0_i32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let px = x + dx;
                    let py = y + dy;
                    if px >= width || py >= height {
                        continue;
                    }
                    let src = (py * width + px) * 4;
                    let r = i32::from(argb[src + 1]);
                    let g = i32::from(argb[src + 2]);
                    let b = i32::from(argb[src + 3]);
                    let yy = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
                    let uu = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                    let vv = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                    out[py * pitch + px] = yy.clamp(0, 255) as u8;
                    u_acc += uu;
                    v_acc += vv;
                    count += 1;
                }
            }
            let dst = uv_row + x;
            let denom = count.max(1);
            out[dst] = (u_acc / denom).clamp(0, 255) as u8;
            if dst + 1 < out.len() {
                out[dst + 1] = (v_acc / denom).clamp(0, 255) as u8;
            }
        }
    }
    Ok((pitch, out))
}

pub(crate) fn copy_semiplanar_yuv420_to_nv12(
    src: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    slice_height: usize,
) -> Result<Vec<u8>, BackendError> {
    let pitch = stride.max(width);
    let src_luma_size = pitch
        .checked_mul(slice_height.max(height))
        .ok_or_else(|| BackendError::InvalidInput("source luma size overflow".to_string()))?;
    let dst_luma_size = pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("destination luma size overflow".to_string()))?;
    let mut out = vec![0_u8; dst_luma_size + dst_luma_size / 2];
    for row in 0..height {
        let src_start = row * pitch;
        let dst_start = row * pitch;
        out[dst_start..dst_start + width].copy_from_slice(&src[src_start..src_start + width]);
    }
    for row in 0..(height / 2) {
        let src_start = src_luma_size + row * pitch;
        let dst_start = dst_luma_size + row * pitch;
        out[dst_start..dst_start + width].copy_from_slice(&src[src_start..src_start + width]);
    }
    Ok(out)
}
