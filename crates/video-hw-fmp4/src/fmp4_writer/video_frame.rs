use anyhow::{Result, bail};

use super::config::FrameSize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaFrame {
    data: Vec<u8>,
}

impl RgbaFrame {
    pub fn new(data: Vec<u8>, size: FrameSize) -> Result<Self> {
        let expected = size.pixel_count().saturating_mul(4);
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
        let expected = size.pixel_count().saturating_mul(4);
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
