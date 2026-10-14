use video_hw_core::BackendError;

pub(crate) fn copy_semiplanar_yuv420_to_nv12(
    src: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    slice_height: usize,
) -> Result<Vec<u8>, BackendError> {
    validate_nv12_dimensions(width, height)?;
    if stride < width {
        return Err(BackendError::InvalidInput(format!(
            "source stride is smaller than width: stride={stride}, width={width}"
        )));
    }
    if slice_height < height {
        return Err(BackendError::InvalidInput(format!(
            "source slice height is smaller than height: slice_height={slice_height}, height={height}"
        )));
    }
    let pitch = stride;
    let src_luma_size = pitch
        .checked_mul(slice_height)
        .ok_or_else(|| BackendError::InvalidInput("source luma size overflow".to_string()))?;
    let src_chroma_size = pitch
        .checked_mul(height / 2)
        .ok_or_else(|| BackendError::InvalidInput("source chroma size overflow".to_string()))?;
    let required_src_len = src_luma_size
        .checked_add(src_chroma_size)
        .ok_or_else(|| BackendError::InvalidInput("source nv12 size overflow".to_string()))?;
    if src.len() < required_src_len {
        return Err(BackendError::InvalidInput(format!(
            "source nv12 payload is too short: expected at least {required_src_len}, got {}",
            src.len()
        )));
    }
    let dst_luma_size = pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("destination luma size overflow".to_string()))?;
    let dst_size = dst_luma_size
        .checked_add(dst_luma_size / 2)
        .ok_or_else(|| BackendError::InvalidInput("destination nv12 size overflow".to_string()))?;
    let mut out = vec![0_u8; dst_size];
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

fn validate_nv12_dimensions(width: usize, height: usize) -> Result<(), BackendError> {
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "nv12 dimensions must be positive".to_string(),
        ));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(format!(
            "nv12 dimensions must be even, got {width}x{height}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semiplanar_copy_rejects_short_source_without_panicking() {
        let error = copy_semiplanar_yuv420_to_nv12(&[0; 5], 2, 2, 2, 2)
            .expect_err("short source must be rejected");
        assert!(matches!(error, BackendError::InvalidInput(_)));
    }

    #[test]
    fn semiplanar_copy_preserves_stride_and_rows() {
        let source = [1, 2, 9, 9, 3, 4];
        let copied = copy_semiplanar_yuv420_to_nv12(&source, 2, 2, 2, 2)
            .expect("valid tightly packed NV12 should copy");
        assert_eq!(copied, source);
    }
}
