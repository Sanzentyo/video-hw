use anyhow::{Result, bail};

use super::config::FrameSize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaFrame {
    data: Vec<u8>,
}

impl RgbaFrame {
    pub fn new(data: Vec<u8>, size: FrameSize) -> Result<Self> {
        let expected = packed_four_channel_len(size)?;
        if data.len() != expected {
            bail!(
                "invalid RGBA frame length: expected {}, got {}",
                expected,
                data.len()
            );
        }
        Ok(Self { data })
    }

    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.data
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgbFrame {
    data: Vec<u8>,
}

impl ArgbFrame {
    pub fn new(data: Vec<u8>, size: FrameSize) -> Result<Self> {
        let expected = packed_four_channel_len(size)?;
        if data.len() != expected {
            bail!(
                "invalid ARGB frame length: expected {}, got {}",
                expected,
                data.len()
            );
        }
        Ok(Self { data })
    }

    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.data
    }
}

fn packed_four_channel_len(size: FrameSize) -> Result<usize> {
    size.checked_pixel_count()
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("frame byte length overflows usize"))
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;

    #[test]
    fn packed_frame_constructors_reject_size_overflow() {
        let max = NonZeroU32::new(u32::MAX).expect("u32::MAX is non-zero");
        let size = FrameSize::new(max, max);

        let rgba_error = RgbaFrame::new(Vec::new(), size).unwrap_err();
        let argb_error = ArgbFrame::new(Vec::new(), size).unwrap_err();

        assert!(rgba_error.to_string().contains("overflows usize"));
        assert!(argb_error.to_string().contains("overflows usize"));
    }
}
