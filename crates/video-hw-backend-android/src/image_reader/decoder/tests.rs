use super::*;

fn config(capacity: ImageLeaseCapacity) -> DecoderConfig {
    DecoderConfig::new(
        ImageReaderCodec::Hevc,
        4,
        4,
        "test.decoder",
        false,
        15,
        ColorRange::Full,
        capacity,
    )
    .unwrap()
}
fn decode(decoder: &mut ImageReaderDecoder, pts: u64) -> Result<DecodedImageLease, DecodeError> {
    decoder.decode_access_unit(&[1, 2, 3], TimestampMicros::new(pts).unwrap())
}
fn facts(gpu: &HardwareBufferLease) -> (usize, u64, [u32; 2], [u32; 3], ColorRange) {
    let d = gpu.descriptor();
    assert_eq!(gpu.crop().width(), 4);
    assert_eq!(gpu.crop().height(), 4);
    assert_eq!(gpu.crop().left(), 0);
    assert_eq!(gpu.crop().top(), 0);
    assert_eq!(d.layers(), 1);
    assert_eq!(d.usage(), IMAGE_USAGE);
    (
        gpu.as_ptr().as_ptr().addr(),
        gpu.timestamp().get(),
        gpu.image_extent(),
        [d.width(), d.height(), d.stride()],
        gpu.colorimetry().range(),
    )
}

#[test]
fn two_owned_images_keep_independent_facts_and_allow_ordered_decode() {
    let mut decoder = ImageReaderDecoder::open(config(ImageLeaseCapacity::Two)).unwrap();
    let mut first = decode(&mut decoder, 1).unwrap();
    let borrowed = first.hardware_buffer().unwrap();
    let first_pointer = borrowed.as_ptr().as_ptr().addr();
    assert_eq!(borrowed.timestamp().get(), 1);
    assert_eq!(borrowed.image_extent(), [4, 4]);
    assert_eq!(borrowed.descriptor().format(), 35);
    assert_eq!(borrowed.crop().width(), 4);
    assert_eq!(borrowed.colorimetry().range(), ColorRange::Full);
    let first = first.into_hardware_buffer().unwrap();
    let second = decode(&mut decoder, 2)
        .unwrap()
        .into_hardware_buffer()
        .unwrap();
    assert_eq!(
        facts(&first),
        (first_pointer, 1, [4, 4], [4, 4, 4], ColorRange::Full)
    );
    assert_ne!(first.as_ptr(), second.as_ptr());
    assert_eq!(facts(&second).1, 2);
    assert_eq!(decoder.anchor.images.get(), 2);
    assert!(matches!(
        decode(&mut decoder, 3),
        Err(DecodeError::Borrowed)
    ));
    assert!(matches!(decoder.try_close(), CloseOutcome::Borrowed));
    ffi::with_driver(|d| {
        assert_eq!(d.queued, [1, 2]);
        assert_eq!(d.live_images(), 2);
        assert_eq!(d.plane_reads, 0);
        assert_eq!(d.reader_max_images, [4]);
    });
    // Release newest first, then reuse only that capacity unit.
    drop(second);
    let third = decode(&mut decoder, 3)
        .unwrap()
        .into_hardware_buffer()
        .unwrap();
    assert_eq!(facts(&first).1, 1);
    assert_eq!(facts(&third).1, 3);
    drop(first);
    assert!(matches!(decoder.try_close(), CloseOutcome::Borrowed));
    drop(third);
    assert!(matches!(decoder.try_close(), CloseOutcome::Released));
    assert!(matches!(decoder.try_close(), CloseOutcome::Released));
    ffi::with_driver(|d| {
        assert_eq!(d.deleted_images.len(), 3);
        assert_eq!(d.live_images(), 0);
        assert_eq!((d.stopped, d.deleted_codecs, d.deleted_readers), (1, 1, 1));
    });
}

#[test]
fn one_image_capacity_blocks_without_queueing_and_resumes_after_return() {
    let mut decoder = ImageReaderDecoder::open(config(ImageLeaseCapacity::One)).unwrap();
    let first = decode(&mut decoder, 11).unwrap();
    assert!(matches!(
        decode(&mut decoder, 12),
        Err(DecodeError::Borrowed)
    ));
    ffi::with_driver(|d| assert_eq!(d.queued, [11]));
    drop(first);
    let next = decode(&mut decoder, 12).unwrap();
    assert!(decode(&mut decoder, 12).is_err());
    drop(next);
    assert!(matches!(decoder.try_close(), CloseOutcome::Released));
}

#[test]
fn decoder_drop_defers_close_until_last_owned_image_drop() {
    let mut decoder = ImageReaderDecoder::open(config(ImageLeaseCapacity::Two)).unwrap();
    let first = decode(&mut decoder, 1)
        .unwrap()
        .into_hardware_buffer()
        .unwrap();
    let second = decode(&mut decoder, 2)
        .unwrap()
        .into_hardware_buffer()
        .unwrap();
    drop(decoder);
    ffi::with_driver(|d| assert_eq!(d.stopped, 0));
    drop(first);
    ffi::with_driver(|d| assert_eq!(d.stopped, 0));
    drop(second);
    ffi::with_driver(|d| {
        assert_eq!(d.deleted_images.len(), 2);
        assert_eq!((d.stopped, d.deleted_codecs, d.deleted_readers), (1, 1, 1));
    });
    assert_eq!(ADMITTED.load(Ordering::Acquire), 0);
}

#[test]
fn held_gpu_lease_keeps_its_decoder_permit_after_decoder_handle_drops() {
    let mut first = ImageReaderDecoder::open(config(ImageLeaseCapacity::Two)).unwrap();
    let mut second = ImageReaderDecoder::open(config(ImageLeaseCapacity::One)).unwrap();
    let image = decode(&mut first, 1)
        .unwrap()
        .into_hardware_buffer()
        .unwrap();
    drop(first);
    assert_eq!(ADMITTED.load(Ordering::Acquire), 2);
    assert!(ImageReaderDecoder::open(config(ImageLeaseCapacity::One)).is_err());
    drop(image);
    let mut third = ImageReaderDecoder::open(config(ImageLeaseCapacity::One)).unwrap();
    assert!(matches!(second.try_close(), CloseOutcome::Released));
    assert!(matches!(third.try_close(), CloseOutcome::Released));
    assert_eq!(ADMITTED.load(Ordering::Acquire), 0);
}

#[test]
fn decode_failure_returns_only_new_image_while_previous_owner_survives() {
    let mut decoder = ImageReaderDecoder::open(config(ImageLeaseCapacity::Two)).unwrap();
    let first = decode(&mut decoder, 1)
        .unwrap()
        .into_hardware_buffer()
        .unwrap();
    ffi::with_driver(|d| d.invalid_extent = true);
    assert!(decode(&mut decoder, 2).is_err());
    assert_eq!(decoder.anchor.images.get(), 1);
    assert_eq!(facts(&first).1, 1);
    assert!(matches!(decoder.try_close(), CloseOutcome::Borrowed));
    ffi::with_driver(|d| {
        assert_eq!(d.live_images(), 1);
        assert_eq!(d.deleted_images.len(), 1);
        d.invalid_extent = false;
    });
    assert!(decode(&mut decoder, 3).is_err());
    ffi::with_driver(|d| assert_eq!(d.queued, [1, 2]));
    drop(first);
    assert!(matches!(decoder.try_close(), CloseOutcome::Released));
    ffi::with_driver(|d| assert_eq!(d.deleted_images.len(), 2));
}

#[test]
fn owned_conversion_validation_failure_uses_the_image_cleanup_once() {
    let mut decoder = ImageReaderDecoder::open(config(ImageLeaseCapacity::Two)).unwrap();
    let first = decode(&mut decoder, 1)
        .unwrap()
        .into_hardware_buffer()
        .unwrap();
    let mut second = decode(&mut decoder, 2).unwrap();
    ffi::with_driver(|d| d.null_buffer = true);
    assert!(second.hardware_buffer().is_err());
    ffi::with_driver(|d| assert!(d.deleted_images.is_empty()));
    assert!(second.into_hardware_buffer().is_err());
    assert_eq!(decoder.anchor.images.get(), 1);
    assert_eq!(facts(&first).1, 1);
    ffi::with_driver(|d| {
        assert_eq!(d.deleted_images.len(), 1);
        d.null_buffer = false;
    });
    drop(decode(&mut decoder, 3).unwrap());
    drop(first);
    assert!(matches!(decoder.try_close(), CloseOutcome::Released));
}

#[test]
fn failed_cpu_mapping_blocks_both_gpu_capabilities_and_owned_drop_returns_image() {
    let mut decoder = ImageReaderDecoder::open(config(ImageLeaseCapacity::One)).unwrap();
    let mut image = decode(&mut decoder, 1).unwrap();
    assert!(image.planes().is_err());
    assert!(image.hardware_buffer().is_err());
    assert!(image.into_hardware_buffer().is_err());
    assert_eq!(decoder.anchor.images.get(), 0);
    ffi::with_driver(|d| {
        assert_eq!(d.deleted_images.len(), 1);
        assert_eq!(d.plane_reads, 1);
    });
    assert!(matches!(decoder.try_close(), CloseOutcome::Released));
}

#[test]
fn acquire_fence_failure_transfers_fence_once_and_does_not_return_previous_image() {
    let mut decoder = ImageReaderDecoder::open(config(ImageLeaseCapacity::Two)).unwrap();
    let first = decode(&mut decoder, 1)
        .unwrap()
        .into_hardware_buffer()
        .unwrap();
    ffi::with_driver(|d| {
        d.acquire_fence = Some(17);
        d.fence_error = true;
    });
    assert!(decode(&mut decoder, 2).is_err());
    assert_eq!(decoder.anchor.images.get(), 1);
    ffi::with_driver(|d| {
        assert_eq!(d.async_returns.len(), 1);
        assert_eq!(d.async_returns[0].1, 17);
        assert_eq!(d.closed_fences, [17]);
        assert!(d.deleted_images.is_empty());
        assert_eq!(d.live_images(), 1);
    });
    drop(first);
    assert!(matches!(decoder.try_close(), CloseOutcome::Released));
}

#[test]
fn ready_acquire_fence_closes_once_before_owned_image_publication() {
    let mut decoder = ImageReaderDecoder::open(config(ImageLeaseCapacity::One)).unwrap();
    ffi::with_driver(|d| d.acquire_fence = Some(21));
    let image = decode(&mut decoder, 1)
        .unwrap()
        .into_hardware_buffer()
        .unwrap();
    ffi::with_driver(|d| {
        assert_eq!(d.closed_fences, [21]);
        assert!(d.async_returns.is_empty());
    });
    drop(image);
    assert!(matches!(decoder.try_close(), CloseOutcome::Released));
}

#[test]
fn queue_failure_releases_reservation_without_touching_a_held_image() {
    let mut decoder = ImageReaderDecoder::open(config(ImageLeaseCapacity::Two)).unwrap();
    let first = decode(&mut decoder, 1).unwrap();
    ffi::with_driver(|d| d.fail_queue = true);
    assert!(decode(&mut decoder, 2).is_err());
    assert_eq!(decoder.anchor.images.get(), 1);
    ffi::with_driver(|d| {
        assert_eq!(d.live_images(), 1);
        assert!(d.deleted_images.is_empty());
    });
    drop(first);
    assert!(matches!(decoder.try_close(), CloseOutcome::Released));
}

#[test]
fn incorrect_timestamp_returns_acquired_image_and_capacity_once() {
    let mut decoder = ImageReaderDecoder::open(config(ImageLeaseCapacity::Two)).unwrap();
    ffi::with_driver(|d| d.invalid_timestamp = true);
    assert!(decode(&mut decoder, 1).is_err());
    assert_eq!(decoder.anchor.images.get(), 0);
    ffi::with_driver(|d| {
        assert_eq!(d.deleted_images.len(), 1);
        assert_eq!(d.live_images(), 0);
    });
    assert!(matches!(decoder.try_close(), CloseOutcome::Released));
}

#[test]
fn failed_format_change_cannot_rewrite_previous_owned_image_colorimetry() {
    let mut decoder = ImageReaderDecoder::open(config(ImageLeaseCapacity::Two)).unwrap();
    let first = decode(&mut decoder, 1)
        .unwrap()
        .into_hardware_buffer()
        .unwrap();
    ffi::with_driver(|d| d.color_range = 2);
    assert!(matches!(
        decode(&mut decoder, 2),
        Err(DecodeError::OutputColorimetry { .. })
    ));
    assert_eq!(first.colorimetry().range(), ColorRange::Full);
    assert_eq!(decoder.anchor.images.get(), 1);
    drop(first);
    assert!(matches!(decoder.try_close(), CloseOutcome::Released));
}

#[test]
fn retained_close_keeps_both_decoder_permits_and_never_retries_stop() {
    let mut first = ImageReaderDecoder::open(config(ImageLeaseCapacity::Two)).unwrap();
    let mut second = ImageReaderDecoder::open(config(ImageLeaseCapacity::One)).unwrap();
    ffi::with_driver(|d| d.fail_stop = true);
    assert!(matches!(first.try_close(), CloseOutcome::Retained(_)));
    assert!(matches!(second.try_close(), CloseOutcome::Retained(_)));
    assert!(matches!(first.try_close(), CloseOutcome::Retained(_)));
    drop(first);
    drop(second);
    assert_eq!(ADMITTED.load(Ordering::Acquire), 2);
    assert!(ImageReaderDecoder::open(config(ImageLeaseCapacity::One)).is_err());
    ffi::with_driver(|d| assert_eq!((d.stopped, d.deleted_codecs, d.deleted_readers), (2, 0, 0)));
}

// A positive implementation of the asserted trait makes inference ambiguous.
// These checks compile the actual owner types, including their complete fields.
macro_rules! assert_not_impl {
    ($ty:ty, $bound:path) => {
        const _: fn() = || {
            trait Ambiguous<A> {
                fn check() {}
            }
            impl<T: ?Sized> Ambiguous<()> for T {}
            struct HasTrait;
            impl<T: ?Sized + $bound> Ambiguous<HasTrait> for T {}
            let _ = <$ty as Ambiguous<_>>::check;
        };
    };
}
assert_not_impl!(ImageReaderDecoder, Send);
assert_not_impl!(ImageReaderDecoder, Sync);
assert_not_impl!(DecodedImageLease, Send);
assert_not_impl!(DecodedImageLease, Sync);
assert_not_impl!(HardwareBufferLease, Send);
assert_not_impl!(HardwareBufferLease, Sync);
assert_not_impl!(HardwareBufferLease, Clone);
assert_not_impl!(ImageReservation, Clone);
