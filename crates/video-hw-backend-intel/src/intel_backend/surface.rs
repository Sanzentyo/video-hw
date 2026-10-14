//! oneVPL surface mapping and Intel decode/encode pixel conversion.
//!
//! Unsafe row copies remain beside the mapped oneVPL surfaces whose pitch and lifetime
//! invariants they validate.

use super::*;

pub(super) fn metadata_frame_from_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    fps: i32,
    next_pts_90k: &mut i64,
    cache: &mut Option<(usize, usize, Option<u32>)>,
) -> Frame {
    let (width, height, pixel_format) = if let Some(cached) = *cache {
        cached
    } else {
        let bounds = surface.bounds();
        let width = usize::from(if bounds.crop_width > 0 {
            bounds.crop_width
        } else {
            bounds.width
        });
        let height = usize::from(if bounds.crop_height > 0 {
            bounds.crop_height
        } else {
            bounds.height
        });
        let fourcc = surface.fourcc();
        let pixel_format = Some(fourcc.repr() as u32);
        *cache = Some((width, height, pixel_format));
        (width, height, pixel_format)
    };
    let pts_90k = Some(bump_decode_pts(next_pts_90k, fps));
    Frame {
        width,
        height,
        pixel_format,
        pts_90k,
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb: None,
        rgb24: None,
        nv12: None,
        force_keyframe: false,
    }
}

pub(super) fn surface_to_backend_frame(
    surface: &mut onevpl::FrameSurface<'_>,
    output_mode: DecodeOutputMode,
    fps: i32,
    next_pts_90k: &mut i64,
) -> Result<Frame, BackendError> {
    let bounds = surface.bounds();
    let width = usize::from(if bounds.crop_width > 0 {
        bounds.crop_width
    } else {
        bounds.width
    });
    let height = usize::from(if bounds.crop_height > 0 {
        bounds.crop_height
    } else {
        bounds.height
    });
    let fourcc = surface.fourcc();
    let pixel_format = Some(fourcc.repr() as u32);
    let pts_90k = Some(bump_decode_pts(next_pts_90k, fps));

    let argb = if matches!(output_mode, DecodeOutputMode::Metadata) {
        None
    } else {
        let data = read_surface_payload(surface, width, height, fourcc)?;
        Some(surface_payload_to_argb(&data, width, height, fourcc)?)
    };

    Ok(Frame {
        width,
        height,
        pixel_format,
        pts_90k,
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb,
        rgb24: None,
        nv12: None,
        force_keyframe: false,
    })
}

fn surface_payload_to_argb(
    data: &[u8],
    width: usize,
    height: usize,
    fourcc: FourCC,
) -> Result<Vec<u8>, BackendError> {
    match fourcc {
        FourCC::NV12 => nv12_to_argb(data, width, height),
        FourCC::IyuvOrI420 | FourCC::YV12 => i420_to_argb(data, width, height),
        FourCC::Rgb4OrBgra => bgra_to_argb(data, width, height),
        _ => Err(BackendError::UnsupportedConfig(format!(
            "unsupported decoded pixel format from Intel backend: {fourcc:?}"
        ))),
    }
}

fn read_surface_payload(
    surface: &mut onevpl::FrameSurface<'_>,
    width: usize,
    height: usize,
    fourcc: FourCC,
) -> Result<Vec<u8>, BackendError> {
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "decoded frame dimensions must be positive".to_string(),
        ));
    }
    surface
        .map(MemoryFlag::READ)
        .map_err(|status| map_onevpl_status(status, "FrameSurface::map"))?;
    let read_result = match fourcc {
        FourCC::NV12 => read_mapped_nv12_surface(surface, width, height),
        FourCC::IyuvOrI420 | FourCC::YV12 => read_mapped_i420_surface(surface, width, height),
        FourCC::Rgb4OrBgra => read_mapped_bgra_surface(surface, width, height),
        _ => Err(BackendError::UnsupportedConfig(format!(
            "unsupported decoded pixel format from Intel backend: {fourcc:?}"
        ))),
    };

    let unmap_result = surface
        .unmap()
        .map_err(|status| map_onevpl_status(status, "FrameSurface::unmap"));

    let data = read_result?;
    unmap_result?;
    Ok(data)
}

fn read_mapped_nv12_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, BackendError> {
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "NV12 decode currently requires even frame dimensions".to_string(),
        ));
    }
    let bounds = surface.bounds();
    let pitch = usize::from(bounds.pitch);
    if pitch < width {
        return Err(BackendError::Backend(format!(
            "unexpected NV12 surface pitch (smaller than width): pitch={}, width={width}",
            bounds.pitch
        )));
    }
    if usize::from(bounds.crop_width) < width || usize::from(bounds.crop_height) < height {
        return Err(BackendError::Backend(format!(
            "NV12 surface crop is smaller than output: crop={}x{}, output={width}x{height}",
            bounds.crop_width, bounds.crop_height
        )));
    }
    let y_plane = surface.y();
    let y_ptr = y_plane.as_ptr();
    let uv_ptr = surface.u().as_ptr();
    let y_size = width
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    let uv_size = y_size / 2;
    let total_size = y_size
        .checked_add(uv_size)
        .ok_or_else(|| BackendError::InvalidInput("nv12 frame size overflow".to_string()))?;
    let mut out = vec![0_u8; total_size];
    // SAFETY: both pointers come from a mapped NV12 surface whose crop and pitch
    // were validated above. oneVPL stores the interleaved UV plane at Data.U.
    unsafe {
        copy_pitched_rows(y_ptr, pitch, width, height, &mut out[..y_size])?;
        copy_pitched_rows(uv_ptr, pitch, width, height / 2, &mut out[y_size..])?;
    }
    Ok(out)
}

fn read_mapped_i420_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, BackendError> {
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "I420 decode currently requires even frame dimensions".to_string(),
        ));
    }
    let bounds = surface.bounds();
    let pitch = usize::from(bounds.pitch);
    let chroma_pitch = pitch / 2;
    let chroma_width = width / 2;
    if pitch < width || chroma_pitch < chroma_width {
        return Err(BackendError::Backend(format!(
            "unexpected I420 surface pitch: pitch={}, width={width}",
            bounds.pitch
        )));
    }
    if usize::from(bounds.crop_width) < width || usize::from(bounds.crop_height) < height {
        return Err(BackendError::Backend(format!(
            "I420 surface crop is smaller than output: crop={}x{}, output={width}x{height}",
            bounds.crop_width, bounds.crop_height
        )));
    }
    let y_plane = surface.y();
    let y_ptr = y_plane.as_ptr();
    let u_ptr = surface.u().as_ptr();
    let v_ptr = surface.v().as_ptr();
    let y_size = width
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("i420 luma size overflow".to_string()))?;
    let uv_size = y_size / 4;
    let chroma_size = uv_size
        .checked_mul(2)
        .ok_or_else(|| BackendError::InvalidInput("i420 chroma size overflow".to_string()))?;
    let total_size = y_size
        .checked_add(chroma_size)
        .ok_or_else(|| BackendError::InvalidInput("i420 frame size overflow".to_string()))?;
    let mut out = vec![0_u8; total_size];
    // SAFETY: each pointer comes from the corresponding mapped I420 plane and
    // the validated crop/pitch bounds cover every copied row.
    unsafe {
        copy_pitched_rows(y_ptr, pitch, width, height, &mut out[..y_size])?;
        copy_pitched_rows(
            u_ptr,
            chroma_pitch,
            chroma_width,
            height / 2,
            &mut out[y_size..(y_size + uv_size)],
        )?;
        copy_pitched_rows(
            v_ptr,
            chroma_pitch,
            chroma_width,
            height / 2,
            &mut out[(y_size + uv_size)..],
        )?;
    }
    Ok(out)
}

fn read_mapped_bgra_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, BackendError> {
    let bounds = surface.bounds();
    let pitch = usize::from(bounds.pitch);
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| BackendError::InvalidInput("BGRA row size overflow".to_string()))?;
    if pitch < row_bytes {
        return Err(BackendError::Backend(format!(
            "unexpected BGRA surface pitch (smaller than row): pitch={}, row={row_bytes}",
            bounds.pitch
        )));
    }
    if usize::from(bounds.crop_width) < width || usize::from(bounds.crop_height) < height {
        return Err(BackendError::Backend(format!(
            "BGRA surface crop is smaller than output: crop={}x{}, output={width}x{height}",
            bounds.crop_width, bounds.crop_height
        )));
    }
    let b_plane = surface.b();
    let output_len = row_bytes
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("BGRA frame size overflow".to_string()))?;
    let mut out = vec![0_u8; output_len];
    // SAFETY: `b_plane` covers the validated BGRA crop and pitch.
    unsafe { copy_pitched_rows(b_plane.as_ptr(), pitch, row_bytes, height, &mut out)? };
    Ok(out)
}

/// Copies rows from a mapped oneVPL plane into tightly packed storage.
///
/// # Safety
///
/// `src` must remain readable for `rows` rows of `source_pitch` bytes. Each
/// row must contain at least `row_bytes` initialized bytes.
unsafe fn copy_pitched_rows(
    src: *const u8,
    source_pitch: usize,
    row_bytes: usize,
    rows: usize,
    dst: &mut [u8],
) -> Result<(), BackendError> {
    if source_pitch < row_bytes {
        return Err(BackendError::Backend(format!(
            "surface pitch is smaller than row: pitch={source_pitch}, row={row_bytes}"
        )));
    }
    let required_dst = row_bytes
        .checked_mul(rows)
        .ok_or_else(|| BackendError::Backend("pitched copy size overflow".to_string()))?;
    if dst.len() < required_dst {
        return Err(BackendError::Backend(format!(
            "pitched copy destination is too small: required={required_dst}, actual={}",
            dst.len()
        )));
    }
    if required_dst != 0 && src.is_null() {
        return Err(BackendError::Backend(
            "mapped oneVPL plane pointer is null".to_string(),
        ));
    }
    for row in 0..rows {
        let source_offset = row
            .checked_mul(source_pitch)
            .ok_or_else(|| BackendError::Backend("pitched source offset overflow".to_string()))?;
        // SAFETY: guaranteed by this function's caller; offsets and lengths were
        // checked above and remain within the mapped plane.
        let source = unsafe { std::slice::from_raw_parts(src.add(source_offset), row_bytes) };
        let target = &mut dst[(row * row_bytes)..((row + 1) * row_bytes)];
        target.copy_from_slice(source);
    }
    Ok(())
}

fn bgra_to_argb(data: &[u8], width: usize, height: usize) -> Result<Vec<u8>, BackendError> {
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("BGRA size overflow".to_string()))?;
    if data.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "BGRA payload size mismatch: expected {}, got {}",
            expected,
            data.len()
        )));
    }
    let mut out = vec![0_u8; expected];
    for (src, dst) in data.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        dst[0] = src[3];
        dst[1] = src[2];
        dst[2] = src[1];
        dst[3] = src[0];
    }
    Ok(out)
}

pub(super) fn i420_to_argb(
    data: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<u8>, BackendError> {
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "decoded frame dimensions must be positive".to_string(),
        ));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "I420 decode currently requires even frame dimensions".to_string(),
        ));
    }
    let y_size = width
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("i420 luma size overflow".to_string()))?;
    let uv_size = y_size / 4;
    let expected = y_size
        .checked_add(uv_size.saturating_mul(2))
        .ok_or_else(|| BackendError::InvalidInput("i420 total size overflow".to_string()))?;
    if data.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "I420 payload size mismatch: expected {}, got {}",
            expected,
            data.len()
        )));
    }

    let (y_plane, uv_planes) = data.split_at(y_size);
    let (u_plane, v_plane) = uv_planes.split_at(uv_size);
    let mut out = vec![0_u8; y_size.saturating_mul(4)];
    for y in 0..height {
        for x in 0..width {
            let y_sample = y_plane[y * width + x] as f32;
            let uv_index = (y / 2) * (width / 2) + (x / 2);
            let u_sample = u_plane[uv_index] as f32;
            let v_sample = v_plane[uv_index] as f32;
            let (r, g, b) = yuv_to_rgb(y_sample, u_sample, v_sample);
            let dst = (y * width + x) * 4;
            out[dst] = 255;
            out[dst + 1] = r;
            out[dst + 2] = g;
            out[dst + 3] = b;
        }
    }
    Ok(out)
}

pub(super) fn nv12_to_argb(
    data: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<u8>, BackendError> {
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "decoded frame dimensions must be positive".to_string(),
        ));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "NV12 decode currently requires even frame dimensions".to_string(),
        ));
    }

    let y_size = width
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    let uv_size = y_size / 2;
    let expected = y_size
        .checked_add(uv_size)
        .ok_or_else(|| BackendError::InvalidInput("nv12 total size overflow".to_string()))?;
    if data.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "NV12 payload size mismatch: expected {}, got {}",
            expected,
            data.len()
        )));
    }

    let (y_plane, uv_plane) = data.split_at(y_size);
    let mut out = vec![0_u8; y_size.saturating_mul(4)];
    for y in 0..height {
        for x in 0..width {
            let y_sample = y_plane[y * width + x] as f32;
            let uv_base = (y / 2) * width + (x & !1);
            let u_sample = uv_plane[uv_base] as f32;
            let v_sample = uv_plane[uv_base + 1] as f32;
            let (r, g, b) = yuv_to_rgb(y_sample, u_sample, v_sample);
            let dst = (y * width + x) * 4;
            out[dst] = 255;
            out[dst + 1] = r;
            out[dst + 2] = g;
            out[dst + 3] = b;
        }
    }
    Ok(out)
}

fn yuv_to_rgb(y: f32, u: f32, v: f32) -> (u8, u8, u8) {
    let c = y - 16.0;
    let d = u - 128.0;
    let e = v - 128.0;
    let r = (1.164 * c + 1.596 * e).round().clamp(0.0, 255.0) as u8;
    let g = (1.164 * c - 0.392 * d - 0.813 * e)
        .round()
        .clamp(0.0, 255.0) as u8;
    let b = (1.164 * c + 2.017 * d).round().clamp(0.0, 255.0) as u8;
    (r, g, b)
}

pub(super) fn write_nv12_to_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    nv12: &Nv12FramePayload,
    width: usize,
    height: usize,
) -> Result<(), BackendError> {
    let dims = Dimensions::try_from((width, height))
        .map_err(|error| BackendError::InvalidInput(error.to_string()))?;
    nv12.validate(dims)?;
    copy_nv12_to_surface(surface, nv12.data.as_slice(), nv12.pitch, width, height)
}

/// Returns a mutable slice over the full interleaved NV12 UV plane
/// (`(height / 2) * pitch` bytes).
///
/// # Safety
///
/// For system-memory NV12 surfaces, `Data.U` is the start of a contiguous
/// interleaved UV plane of `(crop_height / 2) * pitch` bytes, where `V = U + 1`.
/// The onevpl `u()` accessor exposes only `(crop_height / 2) * (pitch / 2)` bytes
/// (half the plane) because the library models U and V as planar with half-pitch.
/// Extending the slice to `(height / 2) * pitch` is valid as long as `height <=
/// crop_height` (verified by the caller) and the surface is mapped for writing.
unsafe fn nv12_uv_plane_full_mut<'a>(
    surface: &'a mut onevpl::FrameSurface<'_>,
    height: usize,
    pitch: usize,
) -> &'a mut [u8] {
    let u_plane = surface.u();
    let full_len = (height / 2) * pitch;
    // SAFETY: see function-level doc. Caller guarantees height <= crop_height
    // and surface is mapped WRITE. The u_plane pointer is valid for full_len bytes.
    unsafe { std::slice::from_raw_parts_mut(u_plane.as_mut_ptr(), full_len) }
}

fn copy_nv12_to_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    nv12: &[u8],
    source_pitch: usize,
    width: usize,
    height: usize,
) -> Result<(), BackendError> {
    surface
        .map(MemoryFlag::WRITE)
        .map_err(|status| map_onevpl_status(status, "FrameSurface::map"))?;
    let conversion_result = (|| {
        let bounds = surface.bounds();
        let pitch = usize::from(bounds.pitch);
        if !pitch.is_multiple_of(2) {
            return Err(BackendError::Backend(format!(
                "unexpected NV12 surface pitch (not 2-byte aligned): {}",
                bounds.pitch
            )));
        }
        if pitch < width {
            return Err(BackendError::Backend(format!(
                "unexpected NV12 surface pitch (smaller than width): pitch={}, width={width}",
                bounds.pitch
            )));
        }
        if usize::from(bounds.crop_width) < width || usize::from(bounds.crop_height) < height {
            return Err(BackendError::Backend(format!(
                "NV12 surface crop is smaller than input: crop={}x{}, input={}x{}",
                bounds.crop_width, bounds.crop_height, width, height
            )));
        }

        let y_plane = surface.y();
        y_plane.fill(16);
        for row in 0..height {
            let src_start = row * source_pitch;
            let dst_start = row * pitch;
            y_plane[dst_start..(dst_start + width)]
                .copy_from_slice(&nv12[src_start..(src_start + width)]);
        }

        let src_uv_base = source_pitch
            .checked_mul(height)
            .ok_or_else(|| BackendError::InvalidInput("nv12 Y size overflow".to_string()))?;
        let uv_width = width / 2;
        // SAFETY: height <= crop_height, surface mapped WRITE.
        let uv_plane = unsafe { nv12_uv_plane_full_mut(surface, height, pitch) };
        uv_plane.fill(0x80);
        for row in 0..(height / 2) {
            let src_row = &nv12
                [(src_uv_base + row * source_pitch)..(src_uv_base + row * source_pitch + width)];
            let dst_row = &mut uv_plane[(row * pitch)..(row * pitch + width)];
            for col in 0..uv_width {
                let base = col * 2;
                dst_row[base] = src_row[base]; // U
                dst_row[base + 1] = src_row[base + 1]; // V
            }
        }

        Ok(())
    })();

    let unmap_result = surface
        .unmap()
        .map_err(|status| map_onevpl_status(status, "FrameSurface::unmap"));

    conversion_result?;
    unmap_result
}

pub(super) fn write_argb_to_nv12_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    argb: &[u8],
    width: usize,
    height: usize,
) -> Result<(), BackendError> {
    let dims = Dimensions::try_from((width, height))
        .map_err(|error| BackendError::InvalidInput(error.to_string()))?;
    let nv12 = Nv12FramePayload::from_argb8888(dims, argb)?;
    write_nv12_to_surface(surface, &nv12, width, height)
}

pub(super) fn argb_to_i420(
    argb: &[u8],
    width: usize,
    height: usize,
    aligned_width: usize,
    aligned_height: usize,
) -> Result<Vec<u8>, BackendError> {
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
    if aligned_width < width || aligned_height < height {
        return Err(BackendError::InvalidInput(
            "aligned dimensions cannot be smaller than input dimensions".to_string(),
        ));
    }
    if !aligned_width.is_multiple_of(2) || !aligned_height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "aligned dimensions must be even for I420".to_string(),
        ));
    }

    let y_size = aligned_width
        .checked_mul(aligned_height)
        .ok_or_else(|| BackendError::InvalidInput("i420 luma size overflow".to_string()))?;
    let uv_width = aligned_width / 2;
    let uv_height = aligned_height / 2;
    let uv_size = uv_width
        .checked_mul(uv_height)
        .ok_or_else(|| BackendError::InvalidInput("i420 chroma size overflow".to_string()))?;

    let mut y_plane = vec![16_u8; y_size];
    let mut u_acc = vec![0_u32; uv_size];
    let mut v_acc = vec![0_u32; uv_size];
    let mut uv_count = vec![0_u32; uv_size];

    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 4;
            let r = argb[src + 1] as f32;
            let g = argb[src + 2] as f32;
            let b = argb[src + 3] as f32;

            let yy = (0.257 * r + 0.504 * g + 0.098 * b + 16.0).round() as i32;
            let uu = (-0.148 * r - 0.291 * g + 0.439 * b + 128.0).round() as i32;
            let vv = (0.439 * r - 0.368 * g - 0.071 * b + 128.0).round() as i32;

            y_plane[y * aligned_width + x] = yy.clamp(0, 255) as u8;
            let uv_index = (y / 2) * uv_width + (x / 2);
            u_acc[uv_index] = u_acc[uv_index].saturating_add(uu.clamp(0, 255) as u32);
            v_acc[uv_index] = v_acc[uv_index].saturating_add(vv.clamp(0, 255) as u32);
            uv_count[uv_index] = uv_count[uv_index].saturating_add(1);
        }
    }

    let mut u_plane = vec![128_u8; uv_size];
    let mut v_plane = vec![128_u8; uv_size];
    for idx in 0..uv_size {
        if let Some(count) = std::num::NonZeroU32::new(uv_count[idx]) {
            u_plane[idx] = (u_acc[idx] / count.get()) as u8;
            v_plane[idx] = (v_acc[idx] / count.get()) as u8;
        }
    }

    let mut out = Vec::with_capacity(y_size + uv_size * 2);
    out.extend_from_slice(&y_plane);
    out.extend_from_slice(&u_plane);
    out.extend_from_slice(&v_plane);
    Ok(out)
}

pub(super) fn write_argb_to_bgra_surface(
    surface: &mut onevpl::FrameSurface<'_>,
    argb: &[u8],
    width: usize,
    height: usize,
) -> Result<(), BackendError> {
    surface
        .map(MemoryFlag::WRITE)
        .map_err(|status| map_onevpl_status(status, "FrameSurface::map"))?;

    let conversion_result = (|| {
        let bounds = surface.bounds();
        let pitch_bytes = usize::from(bounds.pitch);
        if !pitch_bytes.is_multiple_of(4) {
            return Err(BackendError::Backend(format!(
                "unexpected BGRA surface pitch (not 4-byte aligned): {}",
                bounds.pitch
            )));
        }

        let aligned_width = pitch_bytes / 4;
        let aligned_height = usize::from(bounds.crop_height);
        let out = surface.b();
        argb_to_bgra_inplace(argb, width, height, aligned_width, aligned_height, out)
    })();

    let unmap_result = surface
        .unmap()
        .map_err(|status| map_onevpl_status(status, "FrameSurface::unmap"));

    conversion_result?;
    unmap_result
}

fn argb_to_bgra_inplace(
    argb: &[u8],
    width: usize,
    height: usize,
    aligned_width: usize,
    aligned_height: usize,
    out: &mut [u8],
) -> Result<(), BackendError> {
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
    if aligned_width < width || aligned_height < height {
        return Err(BackendError::InvalidInput(
            "aligned dimensions cannot be smaller than input dimensions".to_string(),
        ));
    }

    let required_size = aligned_width
        .checked_mul(aligned_height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("BGRA buffer size overflow".to_string()))?;
    if out.len() != required_size {
        return Err(BackendError::InvalidInput(format!(
            "BGRA output buffer size mismatch: expected {}, got {}",
            required_size,
            out.len()
        )));
    }

    for y in 0..height {
        let src_row_start = y
            .checked_mul(width)
            .and_then(|offset| offset.checked_mul(4))
            .ok_or_else(|| BackendError::InvalidInput("ARGB row offset overflow".to_string()))?;
        let dst_row_start = y
            .checked_mul(aligned_width)
            .and_then(|offset| offset.checked_mul(4))
            .ok_or_else(|| BackendError::InvalidInput("BGRA row offset overflow".to_string()))?;
        let src_row_end = src_row_start
            .checked_add(width.saturating_mul(4))
            .ok_or_else(|| BackendError::InvalidInput("ARGB row end overflow".to_string()))?;
        let dst_row_end = dst_row_start
            .checked_add(width.saturating_mul(4))
            .ok_or_else(|| BackendError::InvalidInput("BGRA row end overflow".to_string()))?;
        let src_row = &argb[src_row_start..src_row_end];
        let dst_row = &mut out[dst_row_start..dst_row_end];
        for (src_px, dst_px) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
            let packed = u32::from_le_bytes([src_px[0], src_px[1], src_px[2], src_px[3]]);
            dst_px.copy_from_slice(&packed.swap_bytes().to_le_bytes());
        }

        let padded_row_end = dst_row_start
            .checked_add(aligned_width.saturating_mul(4))
            .ok_or_else(|| {
                BackendError::InvalidInput("BGRA padded row end overflow".to_string())
            })?;
        if dst_row_end < padded_row_end {
            out[dst_row_end..padded_row_end].fill(0);
        }
    }

    Ok(())
}

pub(super) fn argb_to_yv12(
    argb: &[u8],
    width: usize,
    height: usize,
    aligned_width: usize,
    aligned_height: usize,
) -> Result<Vec<u8>, BackendError> {
    let i420 = argb_to_i420(argb, width, height, aligned_width, aligned_height)?;
    i420_to_yv12(&i420, aligned_width, aligned_height)
}

fn i420_to_yv12(
    i420: &[u8],
    aligned_width: usize,
    aligned_height: usize,
) -> Result<Vec<u8>, BackendError> {
    if !aligned_width.is_multiple_of(2) || !aligned_height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "aligned dimensions must be even for YV12".to_string(),
        ));
    }
    let y_size = aligned_width
        .checked_mul(aligned_height)
        .ok_or_else(|| BackendError::InvalidInput("YV12 luma size overflow".to_string()))?;
    let uv_size = y_size / 4;
    let expected = y_size
        .checked_add(uv_size.saturating_mul(2))
        .ok_or_else(|| BackendError::InvalidInput("I420 size overflow".to_string()))?;
    if i420.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "I420 payload size mismatch for YV12 conversion: expected {}, got {}",
            expected,
            i420.len()
        )));
    }

    let (y_plane, uv_planes) = i420.split_at(y_size);
    let (u_plane, v_plane) = uv_planes.split_at(uv_size);
    // YV12 stores planes as Y + V + U (I420 is Y + U + V).
    let mut out = Vec::with_capacity(y_size.saturating_add(uv_size.saturating_mul(2)));
    out.extend_from_slice(y_plane);
    out.extend_from_slice(v_plane);
    out.extend_from_slice(u_plane);
    Ok(out)
}
