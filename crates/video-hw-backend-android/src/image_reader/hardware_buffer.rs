use super::{CropRect, DecodeError};
use std::{cell::Cell, io, time::Duration};

pub(super) const IMAGE_USAGE: u64 = 3 | 0x100; // CPU_READ_OFTEN | GPU_SAMPLED_IMAGE
pub(super) const DECODE_TIMEOUT: Duration = Duration::from_secs(5);

/// Actual allocation description. Stride is observational; it is not a CPU plane pitch.
#[derive(Debug, Clone, Copy)]
pub struct HardwareBufferDescriptor {
    width: u32,
    height: u32,
    layers: u32,
    format: u32,
    usage: u64,
    stride: u32,
}
impl HardwareBufferDescriptor {
    pub(super) fn new(
        extent: [u32; 2],
        layers: u32,
        format: u32,
        usage: u64,
        stride: u32,
        image_extent: [u32; 2],
    ) -> Result<Self, DecodeError> {
        if extent[0] < image_extent[0]
            || extent[1] < image_extent[1]
            || extent.contains(&0)
            || extent.iter().any(|v| *v > i32::MAX as u32)
            || layers != 1
            || usage & IMAGE_USAGE != IMAGE_USAGE
        {
            return Err(DecodeError::HardwareBufferDescriptor {
                extent,
                image_extent,
                layers,
                format,
                usage,
                stride,
            });
        }
        Ok(Self {
            width: extent[0],
            height: extent[1],
            layers,
            format,
            usage,
            stride,
        })
    }
    pub const fn width(self) -> u32 {
        self.width
    }
    pub const fn height(self) -> u32 {
        self.height
    }
    pub const fn layers(self) -> u32 {
        self.layers
    }
    /// Opaque allocation format; it can differ from the image's logical format.
    pub const fn format(self) -> u32 {
        self.format
    }
    pub const fn usage(self) -> u64 {
        self.usage
    }
    pub const fn stride(self) -> u32 {
        self.stride
    }
}

pub(super) fn validate_metadata(
    extent: [i32; 2],
    format: i32,
    planes: i32,
    crop: [i32; 4],
    expected: [u32; 2],
) -> Result<CropRect, DecodeError> {
    if format != 35 || planes != 3 {
        return Err(DecodeError::Contract(
            "image is not three-plane YUV420Flexible",
        ));
    }
    let crop = CropRect::new(crop[0], crop[1], crop[2], crop[3], extent[0], extent[1])?;
    if crop.width() != expected[0] as usize || crop.height() != expected[1] as usize {
        return Err(DecodeError::Contract("image crop dimensions differ"));
    }
    Ok(crop)
}

/// CPU mapping persists in AImage after a Rust plane view ends.
#[derive(Default)]
pub(super) struct ImageAccess {
    cpu_mapped: Cell<bool>,
}
impl ImageAccess {
    pub(super) fn hardware(&self) -> Result<(), DecodeError> {
        if self.cpu_mapped.get() {
            return Err(DecodeError::Contract(
                "CPU-mapped image cannot be borrowed by GPU",
            ));
        }
        Ok(())
    }
    pub(super) fn begin_cpu(&self) {
        self.cpu_mapped.set(true);
    }
}

pub(super) fn poll_timeout(remaining: Duration) -> Result<i32, DecodeError> {
    if remaining.is_zero() {
        return Err(DecodeError::Deadline);
    }
    // poll uses whole milliseconds; round up, then re-observe the absolute clock.
    Ok(remaining
        .as_nanos()
        .div_ceil(1_000_000)
        .min(i32::MAX as u128) as i32)
}

pub(super) fn wait_acquire_fence(
    mut elapsed: impl FnMut() -> Duration,
    mut poll: impl FnMut(i32) -> io::Result<Option<i16>>,
) -> Result<(), DecodeError> {
    loop {
        let timeout = poll_timeout(DECODE_TIMEOUT.saturating_sub(elapsed()))?;
        let observation = poll(timeout);
        if elapsed() >= DECODE_TIMEOUT {
            return Err(DecodeError::Deadline);
        }
        match observation {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(source) => {
                return Err(DecodeError::System {
                    operation: "acquire fence poll",
                    source,
                });
            }
            Ok(None) => continue,
            Ok(Some(events)) if events & (8 | 16 | 32) == 0 && events & 1 != 0 => return Ok(()),
            Ok(Some(_)) => {
                return Err(DecodeError::Contract(
                    "acquire fence did not signal readiness",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_preserves_actual_extent_usage_and_observational_stride() {
        let d = HardwareBufferDescriptor::new(
            [1088, 1088],
            1,
            0x7fa3_0c04,
            IMAGE_USAGE | 0x800,
            0,
            [1080, 1080],
        )
        .unwrap();
        assert_eq!(
            (
                d.width(),
                d.height(),
                d.layers(),
                d.format(),
                d.usage(),
                d.stride()
            ),
            (1088, 1088, 1, 0x7fa3_0c04, IMAGE_USAGE | 0x800, 0)
        );
        for (extent, layers, usage) in [
            ([1079, 1088], 1, IMAGE_USAGE),
            ([1088, 0], 1, IMAGE_USAGE),
            ([1088, 1088], 2, IMAGE_USAGE),
            ([1088, 1088], 1, 3),
            ([u32::MAX, 1088], 1, IMAGE_USAGE),
        ] {
            assert!(
                HardwareBufferDescriptor::new(extent, layers, 35, usage, 0, [1080, 1080]).is_err()
            );
        }
    }

    #[test]
    fn metadata_retains_odd_crop_without_mapping_planes() {
        let crop = validate_metadata([12, 10], 35, 3, [1, 1, 9, 7], [8, 6]).unwrap();
        assert_eq!(
            (crop.left(), crop.top(), crop.width(), crop.height()),
            (1, 1, 8, 6)
        );
        for (format, planes, crop, expected) in [
            (1, 3, [1, 1, 9, 7], [8, 6]),
            (35, 2, [1, 1, 9, 7], [8, 6]),
            (35, 3, [1, 1, 13, 7], [12, 6]),
            (35, 3, [1, 1, 9, 7], [10, 6]),
        ] {
            assert!(validate_metadata([12, 10], format, planes, crop, expected).is_err());
        }
    }

    #[test]
    fn explicit_cpu_view_latches_gpu_exclusion_for_image_lifetime() {
        let access = ImageAccess::default();
        assert!(access.hardware().is_ok());
        assert!(access.hardware().is_ok());
        access.begin_cpu();
        assert!(access.hardware().is_err());
        access.begin_cpu();
        assert!(access.hardware().is_err());
        assert!(ImageAccess::default().hardware().is_ok());
    }

    #[test]
    fn fence_timeout_uses_remaining_absolute_budget() {
        assert!(poll_timeout(Duration::ZERO).is_err());
        assert_eq!(poll_timeout(Duration::from_nanos(1)).unwrap(), 1);
        assert_eq!(poll_timeout(Duration::from_millis(1)).unwrap(), 1);
        assert_eq!(poll_timeout(Duration::from_micros(1001)).unwrap(), 2);
        assert_eq!(poll_timeout(DECODE_TIMEOUT).unwrap(), 5000);
    }

    #[test]
    fn interrupted_and_empty_polls_do_not_reset_absolute_deadline() {
        let mut times = [0, 1000, 1000, 3000, 3000, 5000].into_iter();
        let mut calls = 0;
        let result = wait_acquire_fence(
            || Duration::from_millis(times.next().unwrap()),
            |timeout| {
                calls += 1;
                assert_eq!(timeout, [5000, 4000, 2000][calls - 1]);
                match calls {
                    1 => Err(io::ErrorKind::Interrupted.into()),
                    2 => Ok(None),
                    _ => Ok(Some(1)),
                }
            },
        );
        assert!(matches!(result, Err(DecodeError::Deadline)));
        assert_eq!(calls, 3);
    }

    #[test]
    fn only_a_signaled_non_error_fence_is_ready() {
        for events in [0, 8, 16, 32, 1 | 8, 1 | 16, 1 | 32] {
            assert!(wait_acquire_fence(|| Duration::ZERO, |_| Ok(Some(events))).is_err());
        }
        assert!(wait_acquire_fence(|| Duration::ZERO, |_| Ok(Some(1))).is_ok());
        let failure = wait_acquire_fence(
            || Duration::ZERO,
            |_| Err(io::ErrorKind::PermissionDenied.into()),
        )
        .unwrap_err();
        match failure {
            DecodeError::System { operation, source } => {
                assert_eq!(operation, "acquire fence poll");
                assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
