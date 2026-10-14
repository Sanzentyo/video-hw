use crate::{
    BackendError, ColorConvertOptions, ColorMatrix, ColorRange, Dimensions, Nv12FramePayload,
    PixelBufferOwned, PixelOutputLayout,
};

impl Nv12FramePayload {
    pub fn from_argb8888(dims: Dimensions, argb: &[u8]) -> Result<Self, BackendError> {
        let width = dims.width.get() as usize;
        let height = dims.height.get() as usize;
        validate_even_dimensions(dims)?;
        let expected = pixel_count(dims)?
            .checked_mul(4)
            .ok_or_else(|| BackendError::InvalidInput("argb size overflow".to_string()))?;
        if argb.len() != expected {
            return Err(BackendError::InvalidInput(format!(
                "argb payload size mismatch: expected {expected}, got {}",
                argb.len()
            )));
        }

        let pitch = width;
        let luma_len = pitch
            .checked_mul(height)
            .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
        let chroma_len = pitch
            .checked_mul(height / 2)
            .ok_or_else(|| BackendError::InvalidInput("nv12 chroma size overflow".to_string()))?;
        let total_len = luma_len
            .checked_add(chroma_len)
            .ok_or_else(|| BackendError::InvalidInput("nv12 total size overflow".to_string()))?;
        let mut data = Vec::new();
        data.try_reserve_exact(total_len).map_err(|error| {
            BackendError::InvalidInput(format!(
                "cannot allocate {total_len} bytes for NV12 frame: {error}"
            ))
        })?;
        data.resize(total_len, 0);

        for y in (0..height).step_by(2) {
            let uv_row = luma_len + (y / 2) * pitch;
            for x in (0..width).step_by(2) {
                let mut u_acc = 0_i32;
                let mut v_acc = 0_i32;
                for dy in 0..2 {
                    let pixel_y = y + dy;
                    let y_row = pixel_y * pitch;
                    for dx in 0..2 {
                        let pixel_x = x + dx;
                        let source = (pixel_y * width + pixel_x) * 4;
                        let r = i32::from(argb[source + 1]);
                        let g = i32::from(argb[source + 2]);
                        let b = i32::from(argb[source + 3]);
                        let luma = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
                        let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                        let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                        data[y_row + pixel_x] = clamp_u8(luma);
                        u_acc += u;
                        v_acc += v;
                    }
                }
                data[uv_row + x] = clamp_u8(u_acc / 4);
                data[uv_row + x + 1] = clamp_u8(v_acc / 4);
            }
        }

        Ok(Self { pitch, data })
    }

    pub fn validate(&self, dims: Dimensions) -> Result<(), BackendError> {
        validate_nv12_payload(dims, self.pitch, &self.data).map(|_| ())
    }
}

pub(crate) fn rgb24_to_pixel_buffer(
    dims: Dimensions,
    data: &[u8],
    layout: PixelOutputLayout,
) -> Result<PixelBufferOwned, BackendError> {
    let pixel_count = pixel_count(dims)?;
    let expected_len = pixel_count
        .checked_mul(3)
        .ok_or_else(|| BackendError::InvalidInput("rgb24 payload size overflow".to_string()))?;
    if data.len() != expected_len {
        return Err(BackendError::InvalidInput(format!(
            "rgb24 payload size mismatch: expected {expected_len}, got {}",
            data.len()
        )));
    }

    let output_len = pixel_count
        .checked_mul(layout.bytes_per_pixel())
        .ok_or_else(|| BackendError::InvalidInput("pixel output size overflow".to_string()))?;
    let mut output = Vec::new();
    output.try_reserve_exact(output_len).map_err(|error| {
        BackendError::InvalidInput(format!(
            "cannot allocate {output_len} bytes for converted pixels: {error}"
        ))
    })?;

    match layout {
        PixelOutputLayout::Rgb24 => output.extend_from_slice(data),
        PixelOutputLayout::Rgba8888 => data.as_chunks::<3>().0.iter().for_each(|rgb| {
            output.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }),
        PixelOutputLayout::Argb8888 => data.as_chunks::<3>().0.iter().for_each(|rgb| {
            output.extend_from_slice(&[255, rgb[0], rgb[1], rgb[2]]);
        }),
        PixelOutputLayout::Bgra8888 => data.as_chunks::<3>().0.iter().for_each(|rgb| {
            output.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }),
    }

    Ok(PixelBufferOwned {
        layout,
        dims,
        data: output,
    })
}

pub(crate) fn nv12_to_pixel_buffer(
    dims: Dimensions,
    pitch: usize,
    data: &[u8],
    layout: PixelOutputLayout,
    options: ColorConvertOptions,
) -> Result<PixelBufferOwned, BackendError> {
    let width = dims.width.get() as usize;
    let height = dims.height.get() as usize;
    let luma_len = validate_nv12_payload(dims, pitch, data)?;

    let rgb_len = pixel_count(dims)?
        .checked_mul(3)
        .ok_or_else(|| BackendError::InvalidInput("rgb24 output size overflow".to_string()))?;
    let mut rgb = Vec::new();
    rgb.try_reserve_exact(rgb_len).map_err(|error| {
        BackendError::InvalidInput(format!(
            "cannot allocate {rgb_len} bytes for converted pixels: {error}"
        ))
    })?;

    for y in 0..height {
        for x in 0..width {
            let y_value = data[y * pitch + x];
            let uv_index = luma_len + (y / 2) * pitch + (x / 2) * 2;
            let (r, g, b) = yuv_to_rgb(y_value, data[uv_index], data[uv_index + 1], options);
            rgb.extend_from_slice(&[r, g, b]);
        }
    }
    rgb24_to_pixel_buffer(dims, &rgb, layout)
}

pub(crate) fn validate_nv12_ref(
    dims: Dimensions,
    pitch: usize,
    data: &[u8],
) -> Result<(), BackendError> {
    validate_nv12_payload(dims, pitch, data).map(|_| ())
}

impl PixelOutputLayout {
    const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb24 => 3,
            Self::Rgba8888 | Self::Argb8888 | Self::Bgra8888 => 4,
        }
    }
}

fn pixel_count(dims: Dimensions) -> Result<usize, BackendError> {
    (dims.width.get() as usize)
        .checked_mul(dims.height.get() as usize)
        .ok_or_else(|| BackendError::InvalidInput("pixel count overflow".to_string()))
}

fn validate_even_dimensions(dims: Dimensions) -> Result<(), BackendError> {
    let width = dims.width.get() as usize;
    let height = dims.height.get() as usize;
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(format!(
            "nv12 dimensions must be even, got {dims}"
        )));
    }
    Ok(())
}

fn validate_nv12_payload(
    dims: Dimensions,
    pitch: usize,
    data: &[u8],
) -> Result<usize, BackendError> {
    validate_even_dimensions(dims)?;
    let width = dims.width.get() as usize;
    let height = dims.height.get() as usize;
    if pitch < width {
        return Err(BackendError::InvalidInput(format!(
            "nv12 pitch is smaller than width: pitch={pitch}, width={width}"
        )));
    }
    let luma_len = pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    let chroma_len = pitch
        .checked_mul(height / 2)
        .ok_or_else(|| BackendError::InvalidInput("nv12 chroma size overflow".to_string()))?;
    let required_len = luma_len
        .checked_add(chroma_len)
        .ok_or_else(|| BackendError::InvalidInput("nv12 payload size overflow".to_string()))?;
    if data.len() < required_len {
        return Err(BackendError::InvalidInput(format!(
            "nv12 payload size mismatch: expected at least {required_len}, got {}",
            data.len()
        )));
    }
    Ok(luma_len)
}

fn yuv_to_rgb(y: u8, u: u8, v: u8, options: ColorConvertOptions) -> (u8, u8, u8) {
    let y = i32::from(y);
    let c = match options.range {
        ColorRange::Limited => (y - 16).max(0),
        ColorRange::Full => y,
    };
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    let (r, g, b) = match (options.matrix, options.range) {
        (ColorMatrix::Bt601, ColorRange::Limited) => (
            (298 * c + 409 * e + 128) >> 8,
            (298 * c - 100 * d - 208 * e + 128) >> 8,
            (298 * c + 516 * d + 128) >> 8,
        ),
        (ColorMatrix::Bt709, ColorRange::Limited) => (
            (298 * c + 459 * e + 128) >> 8,
            (298 * c - 55 * d - 136 * e + 128) >> 8,
            (298 * c + 541 * d + 128) >> 8,
        ),
        (ColorMatrix::Bt601, ColorRange::Full) => (
            y + ((1436 * e) >> 10),
            y - ((352 * d + 731 * e) >> 10),
            y + ((1815 * d) >> 10),
        ),
        (ColorMatrix::Bt709, ColorRange::Full) => (
            y + ((1613 * e) >> 10),
            y - ((192 * d + 479 * e) >> 10),
            y + ((1900 * d) >> 10),
        ),
    };
    (clamp_u8(r), clamp_u8(g), clamp_u8(b))
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::DecodedFrame;

    fn dimensions(width: u32, height: u32) -> Dimensions {
        Dimensions {
            width: NonZeroU32::new(width).expect("test width is non-zero"),
            height: NonZeroU32::new(height).expect("test height is non-zero"),
        }
    }

    #[test]
    fn rejects_odd_nv12_dimensions_instead_of_indexing_past_chroma_rows() {
        let frame = DecodedFrame::Nv12 {
            dims: dimensions(3, 2),
            pitch: 3,
            pts_90k: None,
            data: vec![0; 9],
        };

        let error = frame
            .to_pixel_buffer(PixelOutputLayout::Rgb24, ColorConvertOptions::default())
            .expect_err("odd NV12 width must be rejected");
        assert!(matches!(error, BackendError::InvalidInput(_)));
    }

    #[test]
    fn validates_nv12_pitch_before_reading_pixels() {
        let frame = DecodedFrame::Nv12 {
            dims: dimensions(4, 2),
            pitch: 3,
            pts_90k: None,
            data: vec![0; 9],
        };

        let error = frame
            .to_pixel_buffer(PixelOutputLayout::Rgb24, ColorConvertOptions::default())
            .expect_err("pitch smaller than width must be rejected");
        assert!(matches!(error, BackendError::InvalidInput(_)));
    }

    #[test]
    fn converts_valid_nv12_with_padded_rows() {
        let frame = DecodedFrame::Nv12 {
            dims: dimensions(2, 2),
            pitch: 4,
            pts_90k: None,
            data: vec![16, 16, 0, 0, 16, 16, 0, 0, 128, 128, 0, 0],
        };

        let output = frame
            .to_pixel_buffer(PixelOutputLayout::Rgba8888, ColorConvertOptions::default())
            .expect("valid padded NV12 frame converts");
        assert_eq!(output.data, [0, 0, 0, 255].repeat(4));
    }

    #[test]
    fn argb_to_nv12_produces_a_valid_typed_payload() {
        let dims = dimensions(2, 2);
        let argb = [255, 0, 0, 0].repeat(4);

        let nv12 =
            Nv12FramePayload::from_argb8888(dims, &argb).expect("even ARGB frame converts to NV12");

        assert_eq!(nv12.pitch, 2);
        assert_eq!(nv12.data.len(), 6);
        nv12.validate(dims).expect("converted NV12 validates");
    }
}
