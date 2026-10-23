//! Compile the Android-only API through the public facade without opening a codec.
#![cfg(all(target_os = "android", feature = "backend-android-imagereader"))]

use video_hw::{
    ColorRange,
    image_reader::{
        DecodedImageLease, DecoderConfig, HardwareBufferLease, ImageLeaseCapacity, ImageReaderCodec,
    },
};

#[test]
fn configuration_requires_capacity_and_owned_transfer_has_one_owner() {
    for capacity in [ImageLeaseCapacity::One, ImageLeaseCapacity::Two] {
        let _config = DecoderConfig::new(
            ImageReaderCodec::Hevc,
            1920,
            1080,
            "named.hardware.decoder",
            false,
            30,
            ColorRange::Full,
            capacity,
        )
        .unwrap();
    }
    // Type-check both supported access forms through video_hw's re-export.
    fn transfer(mut image: DecodedImageLease) -> HardwareBufferLease {
        {
            let borrowed = image.hardware_buffer().unwrap();
            let _ = (
                borrowed.as_ptr(),
                borrowed.timestamp(),
                borrowed.colorimetry(),
            );
        }
        image.into_hardware_buffer().unwrap()
    }
    let _ = transfer;
}
