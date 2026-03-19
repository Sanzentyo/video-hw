use std::{
    num::{NonZeroU32, NonZeroUsize},
    path::PathBuf,
};

use anyhow::Result;
use video_hw::{Backend, Codec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pts90k(u64);

impl Pts90k {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameSize {
    width: NonZeroU32,
    height: NonZeroU32,
}

impl FrameSize {
    pub const fn new(width: NonZeroU32, height: NonZeroU32) -> Self {
        Self { width, height }
    }

    pub const fn width(self) -> NonZeroU32 {
        self.width
    }

    pub const fn height(self) -> NonZeroU32 {
        self.height
    }

    pub fn pixel_count(self) -> usize {
        (self.width.get() as usize).saturating_mul(self.height.get() as usize)
    }

    pub fn dims(self) -> Result<video_hw::Dimensions> {
        Ok(video_hw::Dimensions {
            width: self.width,
            height: self.height,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameRate(NonZeroU32);

impl FrameRate {
    pub const fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> NonZeroU32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FragmentFrames(NonZeroUsize);

impl FragmentFrames {
    pub const fn new(value: NonZeroUsize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> NonZeroUsize {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct Fmp4WriterConfig {
    pub output_path: PathBuf,
    pub frame_size: FrameSize,
    pub frame_rate: FrameRate,
    pub backend: Backend,
    pub codec: Codec,
    pub require_hardware: bool,
    pub intel_force_software: bool,
    pub fragment_frames: FragmentFrames,
}

#[derive(Debug, Clone)]
pub struct Fmp4WriterStatus {
    pub output_path: PathBuf,
    pub segments_written: u64,
    pub packets_seen: u64,
    pub bytes_written: u64,
    pub fragment_frames: FragmentFrames,
}

#[derive(Debug, Clone)]
pub struct Fmp4WriterSummary {
    pub output_path: PathBuf,
    pub segments_written: u64,
    pub packets_seen: u64,
    pub flush_packets: u64,
    pub bytes_written: u64,
    pub duration_90k: u64,
}
