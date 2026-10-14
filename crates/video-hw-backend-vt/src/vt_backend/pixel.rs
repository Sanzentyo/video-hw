//! CoreVideo pixel-buffer creation, copying, and color metadata extraction.
//!
//! Unsafe base-address access stays beside the lock/unlock checks that establish its lifetime.

use super::*;

pub(super) fn make_bgra_frame(
    width: usize,
    height: usize,
    argb: &[u8],
) -> Result<CVPixelBuffer, BackendError> {
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| BackendError::InvalidInput("ARGB row size overflows usize".to_string()))?;
    let expected = row_bytes
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("ARGB frame size overflows usize".to_string()))?;
    if argb.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "argb payload size mismatch: expected {expected}, got {}",
            argb.len()
        )));
    }
    let pixel_buffer = CVPixelBuffer::new(kCVPixelFormatType_32BGRA, width, height, None)
        .map_err(|status| cv_error("CVPixelBuffer::new", status))?;

    let lock_status = pixel_buffer.lock_base_address(0);
    if lock_status != 0 {
        return Err(cv_error("CVPixelBuffer::lock_base_address", lock_status));
    }

    let bytes_per_row = pixel_buffer.get_bytes_per_row();
    // SAFETY: the successful lock above makes the CoreVideo base address available until the
    // matching unlock below; the pointer is checked before it is dereferenced.
    let base_ptr = unsafe { pixel_buffer.get_base_address() } as *mut u8;

    let write_result = (|| {
        let total = bytes_per_row.checked_mul(height).ok_or_else(|| {
            BackendError::Backend("CVPixelBuffer byte size overflows usize".to_string())
        })?;
        if bytes_per_row < row_bytes {
            return Err(BackendError::Backend(format!(
                "CVPixelBuffer row is too small: stride={bytes_per_row}, required={row_bytes}"
            )));
        }
        if total != 0 && base_ptr.is_null() {
            return Err(BackendError::Backend(
                "CVPixelBuffer base address is null".to_string(),
            ));
        }
        // SAFETY: the base address is non-null for a locked buffer and CoreVideo
        // reports exactly `bytes_per_row * height` writable bytes.
        let buffer = unsafe { std::slice::from_raw_parts_mut(base_ptr, total) };
        for (source_row, target_row) in argb
            .chunks_exact(row_bytes)
            .zip(buffer.chunks_exact_mut(bytes_per_row))
        {
            for (source, target) in source_row
                .chunks_exact(4)
                .zip(target_row[..row_bytes].chunks_exact_mut(4))
            {
                target.copy_from_slice(&[source[3], source[2], source[1], source[0]]);
            }
        }
        Ok(())
    })();

    let unlock_status = pixel_buffer.unlock_base_address(0);
    write_result?;
    if unlock_status != 0 {
        return Err(cv_error(
            "CVPixelBuffer::unlock_base_address",
            unlock_status,
        ));
    }

    Ok(pixel_buffer)
}

pub(super) fn copy_argb_payload_from_pixel_buffer(
    pixel_buffer: &CVPixelBuffer,
    pixel_format: u32,
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    if pixel_format != kCVPixelFormatType_32BGRA {
        return None;
    }

    let lock_status = pixel_buffer.lock_base_address(0);
    if lock_status != 0 {
        return None;
    }

    let bytes_per_row = pixel_buffer.get_bytes_per_row();
    // SAFETY: the successful lock above makes the CoreVideo base address available until the
    // matching unlock below; the pointer is checked before it is dereferenced.
    let base_ptr = unsafe { pixel_buffer.get_base_address() } as *const u8;
    let out = (|| {
        let row_bytes = width.checked_mul(4)?;
        if bytes_per_row < row_bytes {
            return None;
        }
        let total = bytes_per_row.checked_mul(height)?;
        let output_len = row_bytes.checked_mul(height)?;
        if total != 0 && base_ptr.is_null() {
            return None;
        }
        // SAFETY: CoreVideo supplied this non-null base address for the currently
        // locked pixel buffer and reports `bytes_per_row * height` readable bytes.
        let buffer = unsafe { std::slice::from_raw_parts(base_ptr, total) };
        let mut argb = vec![0_u8; output_len];
        for (source_row, target_row) in buffer
            .chunks_exact(bytes_per_row)
            .zip(argb.chunks_exact_mut(row_bytes))
        {
            for (source, target) in source_row[..row_bytes]
                .chunks_exact(4)
                .zip(target_row.chunks_exact_mut(4))
            {
                target.copy_from_slice(&[source[3], source[2], source[1], source[0]]);
            }
        }
        Some(argb)
    })();

    if pixel_buffer.unlock_base_address(0) == 0 {
        out
    } else {
        None
    }
}

pub(super) fn cm_time_to_90k(time: CMTime) -> Option<i64> {
    if time.timescale <= 0 {
        return None;
    }
    let numerator = i128::from(time.value).saturating_mul(90_000);
    let denominator = i128::from(time.timescale);
    let scaled = numerator.checked_div(denominator)?;
    i64::try_from(scaled).ok()
}

pub(super) fn extract_color_metadata(pixel_buffer: &CVPixelBuffer) -> ColorMetadata {
    ColorMetadata {
        color_primaries: copy_color_primaries(pixel_buffer),
        transfer_function: copy_transfer_function(pixel_buffer),
        ycbcr_matrix: copy_ycbcr_matrix(pixel_buffer),
    }
}

fn copy_color_primaries(pixel_buffer: &CVPixelBuffer) -> Option<i32> {
    let value = copy_attachment_cfstring(pixel_buffer, CVImageBufferKeys::ColorPrimaries)?;
    // SAFETY: `value` owns a valid CFString for the duration of this read-only CoreVideo call.
    Some(unsafe { CVColorPrimariesGetIntegerCodePointForString(value.as_concrete_TypeRef()) })
}

fn copy_transfer_function(pixel_buffer: &CVPixelBuffer) -> Option<i32> {
    let value = copy_attachment_cfstring(pixel_buffer, CVImageBufferKeys::TransferFunction)?;
    // SAFETY: `value` owns a valid CFString for the duration of this read-only CoreVideo call.
    Some(unsafe { CVTransferFunctionGetIntegerCodePointForString(value.as_concrete_TypeRef()) })
}

fn copy_ycbcr_matrix(pixel_buffer: &CVPixelBuffer) -> Option<i32> {
    let value = copy_attachment_cfstring(pixel_buffer, CVImageBufferKeys::YCbCrMatrix)?;
    // SAFETY: `value` owns a valid CFString for the duration of this read-only CoreVideo call.
    Some(unsafe { CVYCbCrMatrixGetIntegerCodePointForString(value.as_concrete_TypeRef()) })
}

fn copy_attachment_cfstring(
    pixel_buffer: &CVPixelBuffer,
    key: CVImageBufferKeys,
) -> Option<CFString> {
    let value = pixel_buffer
        .as_buffer()
        .copy_attachment(&CFString::from(key), None)?;
    value.downcast::<CFString>()
}
