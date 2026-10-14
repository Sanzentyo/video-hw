use std::{
    num::{NonZeroU32, NonZeroUsize},
    path::PathBuf,
};

use video_hw::{Backend, Codec, Timestamp90k, bitstream::ParameterSets};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Pts90k(u64);

impl Pts90k {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    /// Converts this non-negative timestamp to the signed timestamp used by
    /// `video-hw`, rejecting values that cannot be represented.
    pub fn try_into_timestamp(
        self,
    ) -> std::result::Result<Timestamp90k, std::num::TryFromIntError> {
        i64::try_from(self.0).map(Timestamp90k)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct SampleDuration90k(NonZeroU32);

impl SampleDuration90k {
    pub const fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

    pub fn checked_pixel_count(self) -> Option<usize> {
        usize::try_from(self.width.get())
            .ok()?
            .checked_mul(usize::try_from(self.height.get()).ok()?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
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
pub struct EncodedTrackConfig {
    pub output_path: PathBuf,
    pub frame_size: FrameSize,
    pub frame_rate: FrameRate,
    pub codec: Codec,
    pub fragment_frames: FragmentFrames,
    pub initial_parameter_sets: Option<ParameterSets>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Fmp4WriterStatus {
    pub output_path: PathBuf,
    pub segments_written: u64,
    pub packets_seen: u64,
    pub bytes_written: u64,
    pub fragment_frames: FragmentFrames,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Fmp4WriterSummary {
    pub output_path: PathBuf,
    pub segments_written: u64,
    pub packets_seen: u64,
    pub flush_packets: u64,
    pub bytes_written: u64,
    pub duration_90k: u64,
}
