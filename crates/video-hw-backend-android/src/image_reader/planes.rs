use super::DecodeError;

const MAX_PLANE_BYTES: usize = 64 * 1024 * 1024;

/// Exclusive-right/bottom crop, validated against the actual image dimensions.
#[derive(Debug, Clone, Copy)]
pub struct CropRect {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
}
impl CropRect {
    pub fn new(
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        image_width: i32,
        image_height: i32,
    ) -> Result<Self, DecodeError> {
        if left < 0
            || top < 0
            || right <= left
            || bottom <= top
            || right > image_width
            || bottom > image_height
        {
            return Err(DecodeError::Contract("crop outside actual image"));
        }
        Ok(Self {
            left: usize::try_from(left).map_err(|_| DecodeError::Contract("crop conversion"))?,
            top: usize::try_from(top).map_err(|_| DecodeError::Contract("crop conversion"))?,
            width: usize::try_from(right - left)
                .map_err(|_| DecodeError::Contract("crop conversion"))?,
            height: usize::try_from(bottom - top)
                .map_err(|_| DecodeError::Contract("crop conversion"))?,
        })
    }
}

/// Immutable bytes tied to a borrowed image, with explicit positive strides.
#[derive(Debug, Clone, Copy)]
pub struct PlaneView<'image> {
    bytes: &'image [u8],
    row: usize,
    pixel: usize,
}
impl<'image> PlaneView<'image> {
    pub fn new(
        bytes: &'image [u8],
        row_stride: i32,
        pixel_stride: i32,
    ) -> Result<Self, DecodeError> {
        if bytes.is_empty() || bytes.len() > MAX_PLANE_BYTES || row_stride <= 0 || pixel_stride <= 0
        {
            return Err(DecodeError::Contract("plane span/strides out of bounds"));
        }
        Ok(Self {
            bytes,
            row: usize::try_from(row_stride)
                .map_err(|_| DecodeError::Contract("row stride conversion"))?,
            pixel: usize::try_from(pixel_stride)
                .map_err(|_| DecodeError::Contract("pixel stride conversion"))?,
        })
    }
    fn validate_rectangle(
        self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    ) -> Result<(), DecodeError> {
        let last_x = x
            .checked_add(width)
            .and_then(|v| v.checked_sub(1))
            .ok_or(DecodeError::Contract("plane rectangle overflow"))?;
        let last_y = y
            .checked_add(height)
            .and_then(|v| v.checked_sub(1))
            .ok_or(DecodeError::Contract("plane rectangle overflow"))?;
        let last = last_y
            .checked_mul(self.row)
            .and_then(|v| {
                last_x
                    .checked_mul(self.pixel)
                    .and_then(|x| v.checked_add(x))
            })
            .ok_or(DecodeError::Contract("plane address overflow"))?;
        if width == 0 || height == 0 || last >= self.bytes.len() {
            return Err(DecodeError::Contract(
                "plane rectangle exceeds available samples",
            ));
        }
        Ok(())
    }
    fn sample(self, x: usize, y: usize) -> u8 {
        self.bytes[y * self.row + x * self.pixel]
    }
}

/// Fully validated Y/U/V rectangles, permitting immutable aliased chroma planes.
#[derive(Debug)]
pub struct YuvImageView<'image> {
    planes: [PlaneView<'image>; 3],
    crop: CropRect,
    uv_width: usize,
    uv_height: usize,
}
impl<'image> YuvImageView<'image> {
    pub fn new(
        planes: [PlaneView<'image>; 3],
        crop: CropRect,
        expected_width: u32,
        expected_height: u32,
    ) -> Result<Self, DecodeError> {
        let width = usize::try_from(expected_width)
            .map_err(|_| DecodeError::Contract("width conversion"))?;
        let height = usize::try_from(expected_height)
            .map_err(|_| DecodeError::Contract("height conversion"))?;
        if crop.width != width
            || crop.height != height
            || width == 0
            || height == 0
            || width > 4096
            || height > 4096
        {
            return Err(DecodeError::Contract(
                "crop differs from immutable decoder dimensions",
            ));
        }
        let uv_width = (width + (crop.left & 1)).div_ceil(2);
        let uv_height = (height + (crop.top & 1)).div_ceil(2);
        planes[0].validate_rectangle(crop.left, crop.top, width, height)?;
        for plane in &planes[1..] {
            plane.validate_rectangle(crop.left / 2, crop.top / 2, uv_width, uv_height)?;
        }
        Ok(Self {
            planes,
            crop,
            uv_width,
            uv_height,
        })
    }
    pub fn copy_compact(&self) -> CompactYuv {
        let mut output = CompactYuv {
            width: 0,
            height: 0,
            uv_width: 0,
            uv_height: 0,
            phase_x: 0,
            phase_y: 0,
            y: Vec::new(),
            uv: Vec::new(),
        };
        self.copy_compact_into(&mut output);
        output
    }
    /// Reuses owned buffers after the caller has released their previous publication.
    pub fn copy_compact_into(&self, output: &mut CompactYuv) {
        let crop = self.crop;
        output.y.resize(crop.width * crop.height, 0);
        output.uv.resize(self.uv_width * self.uv_height * 2, 0);
        let y = &mut output.y;
        let uv = &mut output.uv;
        for row in 0..crop.height {
            if self.planes[0].pixel == 1 {
                let begin = (crop.top + row) * self.planes[0].row + crop.left;
                y[row * crop.width..(row + 1) * crop.width]
                    .copy_from_slice(&self.planes[0].bytes[begin..begin + crop.width]);
                continue;
            }
            for column in 0..crop.width {
                y[row * crop.width + column] =
                    self.planes[0].sample(crop.left + column, crop.top + row);
            }
        }
        for row in 0..self.uv_height {
            for column in 0..self.uv_width {
                let offset = (row * self.uv_width + column) * 2;
                uv[offset] = self.planes[1].sample(crop.left / 2 + column, crop.top / 2 + row);
                uv[offset + 1] = self.planes[2].sample(crop.left / 2 + column, crop.top / 2 + row);
            }
        }
        output.width = crop.width as u32;
        output.height = crop.height as u32;
        output.uv_width = self.uv_width as u32;
        output.uv_height = self.uv_height as u32;
        output.phase_x = (crop.left & 1) as u32;
        output.phase_y = (crop.top & 1) as u32;
    }
    /// Samples only inside the already validated cropped image rectangle.
    pub fn sample(&self, x: u32, y: u32) -> Result<[u8; 3], DecodeError> {
        let x = usize::try_from(x).map_err(|_| DecodeError::Contract("sample x conversion"))?;
        let y = usize::try_from(y).map_err(|_| DecodeError::Contract("sample y conversion"))?;
        if x >= self.crop.width || y >= self.crop.height {
            return Err(DecodeError::Contract("sample outside crop"));
        }
        let x = self.crop.left + x;
        let y = self.crop.top + y;
        Ok([
            self.planes[0].sample(x, y),
            self.planes[1].sample(x / 2, y / 2),
            self.planes[2].sample(x / 2, y / 2),
        ])
    }
}

/// Owned tight Y and interleaved U,V data; this is a CPU copy, not an AImage lease.
#[derive(Debug)]
pub struct CompactYuv {
    width: u32,
    height: u32,
    uv_width: u32,
    uv_height: u32,
    phase_x: u32,
    phase_y: u32,
    y: Vec<u8>,
    uv: Vec<u8>,
}
impl CompactYuv {
    pub const fn width(&self) -> u32 {
        self.width
    }
    pub const fn height(&self) -> u32 {
        self.height
    }
    pub const fn uv_width(&self) -> u32 {
        self.uv_width
    }
    pub const fn uv_height(&self) -> u32 {
        self.uv_height
    }
    pub const fn phase_x(&self) -> u32 {
        self.phase_x
    }
    pub const fn phase_y(&self) -> u32 {
        self.phase_y
    }
    pub fn y(&self) -> &[u8] {
        &self.y
    }
    pub fn uv(&self) -> &[u8] {
        &self.uv
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_crop_phases_match_independent_planar_sampling() {
        for phase_x in 0..=1 {
            for phase_y in 0..=1 {
                let y: Vec<u8> = (0..8 * 8).map(|v| (v * 7) as u8).collect();
                let uv: Vec<u8> = (0..4 * 4 * 2).map(|v| (v * 13) as u8).collect();
                let crop =
                    CropRect::new(phase_x, phase_y, phase_x + 4, phase_y + 4, 8, 8).expect("crop");
                let views = [
                    PlaneView::new(&y, 8, 1).expect("y"),
                    PlaneView::new(&uv, 8, 2).expect("u"),
                    PlaneView::new(&uv[1..], 8, 2).expect("v"),
                ];
                let image = YuvImageView::new(views, crop, 4, 4).expect("image");
                let compact = image.copy_compact();
                for row in 0..4 {
                    for column in 0..4 {
                        assert_eq!(
                            compact.y()[(row * 4 + column) as usize],
                            y[((row + phase_y) * 8 + column + phase_x) as usize]
                        );
                    }
                }
                for row in 0..compact.uv_height() {
                    for column in 0..compact.uv_width() {
                        let target = ((row * compact.uv_width() + column) * 2) as usize;
                        let source = (row * 8 + column * 2) as usize;
                        assert_eq!(&compact.uv()[target..target + 2], &uv[source..source + 2]);
                    }
                }
            }
        }
    }
    #[test]
    fn exact_last_sample_is_valid_and_one_byte_short_is_rejected() {
        let y = vec![0; 3 * 8 + 4];
        let uv = [0; 8 + 3];
        let crop = CropRect::new(0, 0, 4, 4, 4, 4).expect("crop");
        let image = |y_len, uv_len| {
            YuvImageView::new(
                [
                    PlaneView::new(&y[..y_len], 8, 1).expect("y"),
                    PlaneView::new(&uv[..uv_len], 8, 2).expect("u"),
                    PlaneView::new(&uv[..uv_len], 8, 2).expect("v"),
                ],
                crop,
                4,
                4,
            )
        };
        assert!(image(y.len(), uv.len()).is_ok());
        assert!(image(y.len() - 1, uv.len()).is_err());
        assert!(image(y.len(), uv.len() - 1).is_err());
        assert!(PlaneView::new(&y, 0, 1).is_err());
        assert!(PlaneView::new(&y, 8, -1).is_err());
        assert!(CropRect::new(-1, 0, 4, 4, 4, 4).is_err());
        assert!(CropRect::new(0, 0, 5, 4, 4, 4).is_err());
    }
    #[test]
    fn timestamp_never_saturates_or_rounds() {
        assert!(super::super::TimestampMicros::new(0).is_err());
        assert!(super::super::TimestampMicros::from_image_nanoseconds(1001).is_err());
        assert!(super::super::TimestampMicros::new(i64::MAX.unsigned_abs() / 1000 + 1).is_err());
        assert_eq!(
            super::super::TimestampMicros::from_image_nanoseconds(42000)
                .expect("exact")
                .get(),
            42
        );
    }
    #[test]
    fn copying_another_frame_reuses_publication_buffers() {
        let y = vec![42; 64];
        let uv = vec![128; 32];
        let crop = CropRect::new(1, 1, 5, 5, 8, 8).expect("crop");
        let image = YuvImageView::new(
            [
                PlaneView::new(&y, 8, 1).expect("y"),
                PlaneView::new(&uv, 8, 2).expect("u"),
                PlaneView::new(&uv[1..], 8, 2).expect("v"),
            ],
            crop,
            4,
            4,
        )
        .expect("image");
        let mut output = image.copy_compact();
        let y_pointer = output.y().as_ptr();
        let uv_pointer = output.uv().as_ptr();
        image.copy_compact_into(&mut output);
        assert_eq!(output.y().as_ptr(), y_pointer);
        assert_eq!(output.uv().as_ptr(), uv_pointer);
        assert_eq!(output.y(), &[42; 16]);
        assert_eq!(output.uv(), &[128; 18]);
    }
    #[test]
    fn nonzero_crop_and_independent_strides_survive_smaller_reuse() {
        let y: Vec<u8> = (0..12 * 40).map(|i| (i * 7) as u8).collect();
        let u: Vec<u8> = (0..6 * 17).map(|i| (i * 11) as u8).collect();
        let v: Vec<u8> = (0..6 * 29).map(|i| (i * 13) as u8).collect();
        let image = |left, top, width, height| {
            YuvImageView::new(
                [
                    PlaneView::new(&y, 40, 2).expect("padded y"),
                    PlaneView::new(&u, 17, 1).expect("independent u"),
                    PlaneView::new(&v, 29, 3).expect("independent v"),
                ],
                CropRect::new(left, top, left + width, top + height, 16, 12).expect("crop"),
                width as u32,
                height as u32,
            )
            .expect("image")
        };
        let mut compact = image(3, 5, 7, 5).copy_compact();
        let pointers = (compact.y().as_ptr(), compact.uv().as_ptr());
        for (left, top, width, height) in [(3, 5, 7, 5), (4, 4, 4, 4)] {
            image(left, top, width, height).copy_compact_into(&mut compact);
            assert_eq!(compact.y().len(), (width * height) as usize);
            assert_eq!(
                compact.uv().len(),
                (compact.uv_width() * compact.uv_height() * 2) as usize
            );
            assert_eq!((compact.y().as_ptr(), compact.uv().as_ptr()), pointers);
            assert_eq!(
                (compact.phase_x(), compact.phase_y()),
                (left as u32 & 1, top as u32 & 1)
            );
            for row in 0..height {
                for column in 0..width {
                    let x = (column + left) as usize;
                    let z = (row + top) as usize;
                    let y_index = (row * width + column) as usize;
                    let uv_index = (((row as u32 + compact.phase_y()) / 2 * compact.uv_width()
                        + (column as u32 + compact.phase_x()) / 2)
                        * 2) as usize;
                    assert_eq!(compact.y()[y_index], y[z * 40 + x * 2]);
                    assert_eq!(compact.uv()[uv_index], u[z / 2 * 17 + x / 2]);
                    assert_eq!(compact.uv()[uv_index + 1], v[z / 2 * 29 + x / 2 * 3]);
                }
            }
        }
    }
}
