use super::hardware_buffer::{
    DECODE_TIMEOUT, HardwareBufferDescriptor, IMAGE_USAGE, ImageAccess, validate_metadata,
    wait_acquire_fence,
};
use super::{
    CropRect, DecodeError, ImageReaderCodec, PlaneView, TimestampMicros, YuvImageView, ffi,
};
use std::{
    cell::RefCell,
    ffi::CString,
    mem::ManuallyDrop,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd},
    ptr::{self, NonNull},
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

const MAX_PACKET: usize = 8 * 1024 * 1024;
static ADMITTED: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn acquire() -> Result<Self, DecodeError> {
        ADMITTED
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 2).then_some(count + 1)
            })
            .map(|_| Self)
            .map_err(|_| DecodeError::Contract("ImageReader decoder capacity full"))
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ADMITTED.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Immutable coded image and an explicit hardware codec policy.
#[derive(Debug)]
pub struct DecoderConfig {
    codec: ImageReaderCodec,
    width: u32,
    height: u32,
    name: CString,
    allow_frame_drop: bool,
}
impl DecoderConfig {
    pub fn new(
        codec: ImageReaderCodec,
        width: u32,
        height: u32,
        codec_name: &str,
        allow_frame_drop: bool,
    ) -> Result<Self, DecodeError> {
        if width == 0
            || height == 0
            || !width.is_multiple_of(2)
            || !height.is_multiple_of(2)
            || width > 4096
            || height > 4096
        {
            return Err(DecodeError::Contract(
                "coded NV12 dimensions must be even and bounded",
            ));
        }
        let name = CString::new(codec_name)
            .map_err(|_| DecodeError::Contract("codec name contains NUL"))?;
        if name.as_bytes().is_empty() {
            return Err(DecodeError::Contract("empty codec name"));
        }
        Ok(Self {
            codec,
            width,
            height,
            name,
            allow_frame_drop,
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodecPhase {
    Created,
    StartAttempted,
    Started,
    Stopped,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecodePhase {
    Ready,
    Failed,
    Retained,
}
struct Session {
    config: DecoderConfig,
    _permit: Permit,
    reader: *mut ffi::AImageReader,
    codec: *mut ffi::AMediaCodec,
    codec_phase: CodecPhase,
    decode_phase: DecodePhase,
    last_pts: u64,
    color_valid: bool,
}
struct Anchor {
    session: RefCell<Option<Box<Session>>>,
}
thread_local! {
    // The initializer is const; Clippy also inspects the Android TLS fallback.
    #[expect(clippy::missing_const_for_thread_local, reason = "Android TLS macro expansion reports an already const initializer")]
    static RETAINED: RefCell<Vec<ManuallyDrop<Box<Session>>>> = const {RefCell::new(Vec::new())};
}
fn retain(session: Box<Session>) {
    let session = ManuallyDrop::new(session);
    let _ = RETAINED.try_with(|items| {
        if let Ok(mut items) = items.try_borrow_mut() {
            items.push(session);
        }
    });
}
impl Drop for Anchor {
    fn drop(&mut self) {
        if let Some(mut session) = self.session.get_mut().take()
            && session.close().is_err()
        {
            retain(session);
        }
    }
}
/// Distinguishes physical retirement from an outstanding lease or cleanup failure.
#[derive(Debug)]
pub enum CloseOutcome {
    Released,
    Borrowed,
    Retained(DecodeError),
}
/// Worker-affine decoder. No CUDA, JNI, timestamp rescaling, or fallback codec.
pub struct ImageReaderDecoder {
    anchor: Rc<Anchor>,
}
impl std::fmt::Debug for ImageReaderDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageReaderDecoder")
            .field(
                "image_lease_outstanding",
                &(Rc::strong_count(&self.anchor) > 1),
            )
            .finish_non_exhaustive()
    }
}
impl ImageReaderDecoder {
    pub fn open(config: DecoderConfig) -> Result<Self, DecodeError> {
        let permit = Permit::acquire()?;
        let decoder = Self {
            anchor: Rc::new(Anchor {
                session: RefCell::new(Some(Box::new(Session {
                    config,
                    _permit: permit,
                    reader: ptr::null_mut(),
                    codec: ptr::null_mut(),
                    codec_phase: CodecPhase::Created,
                    decode_phase: DecodePhase::Ready,
                    last_pts: 0,
                    color_valid: false,
                }))),
            }),
        };
        decoder
            .anchor
            .session
            .borrow_mut()
            .as_mut()
            .ok_or(DecodeError::Contract("missing session"))?
            .initialize()?;
        Ok(decoder)
    }
    pub fn decode_access_unit(
        &mut self,
        packet: &[u8],
        pts: TimestampMicros,
    ) -> Result<DecodedImageLease, DecodeError> {
        if Rc::strong_count(&self.anchor) != 1 {
            return Err(DecodeError::Borrowed);
        }
        let mut session = self.anchor.session.borrow_mut();
        let session = session
            .as_mut()
            .ok_or(DecodeError::Contract("decoder is closed"))?;
        if session.decode_phase != DecodePhase::Ready {
            return Err(DecodeError::Contract("failed decoder requires recreation"));
        }
        if packet.is_empty() || packet.len() > MAX_PACKET || pts.get() <= session.last_pts {
            return Err(DecodeError::Contract(
                "access unit size or increasing exact PTS differs",
            ));
        }
        session.decode_phase = DecodePhase::Failed;
        let start = Instant::now();
        let raw = session.decode(packet, pts, start)?;
        let mut lease = DecodedImageLease {
            raw,
            _anchor: Rc::clone(&self.anchor),
            timestamp: pts,
            width: session.config.width,
            height: session.config.height,
            access: ImageAccess::default(),
        };
        // Metadata and allocation capability only: getPlaneData would CPU-lock
        // the image until its return, preventing subsequent GPU access.
        lease.hardware_buffer()?;
        if start.elapsed() >= DECODE_TIMEOUT {
            return Err(DecodeError::Deadline);
        }
        session.last_pts = pts.get();
        session.decode_phase = DecodePhase::Ready;
        Ok(lease)
    }
    pub fn try_close(&mut self) -> CloseOutcome {
        if Rc::strong_count(&self.anchor) != 1 {
            return CloseOutcome::Borrowed;
        }
        let mut slot = self.anchor.session.borrow_mut();
        if let Some(session) = slot.as_mut()
            && let Err(error) = session.close()
        {
            return CloseOutcome::Retained(error);
        }
        *slot = None;
        CloseOutcome::Released
    }
}

/// A move-only acquired AImage. CPU views cannot outlive it; reader/session stay alive.
pub struct DecodedImageLease {
    raw: NonNull<ffi::AImage>,
    _anchor: Rc<Anchor>,
    timestamp: TimestampMicros,
    width: u32,
    height: u32,
    access: ImageAccess,
}
/// Exclusive, worker-affine borrow of an acquire-ready decoded hardware buffer.
///
/// Dropping this value does not wait for GPU work. A caller importing the raw
/// pointer must finish all external access before ending this borrow. Keeping an
/// allocation reference does not protect its contents after the image is returned.
pub struct HardwareBufferBorrow<'image> {
    image: &'image mut DecodedImageLease,
    buffer: NonNull<ffi::AHardwareBuffer>,
    descriptor: HardwareBufferDescriptor,
    image_extent: [u32; 2],
    crop: CropRect,
}
impl HardwareBufferBorrow<'_> {
    /// Borrowed AHardwareBuffer pointer, never transferred or separately released.
    /// Importing or accessing it through an external API requires that API's
    /// synchronization contract; no access may outlive this exclusive borrow.
    pub fn as_ptr(&self) -> NonNull<std::ffi::c_void> {
        self.buffer.cast()
    }
    pub const fn descriptor(&self) -> HardwareBufferDescriptor {
        self.descriptor
    }
    /// Full extent observed from AImage metadata, before applying its crop.
    pub const fn image_extent(&self) -> [u32; 2] {
        self.image_extent
    }
    pub const fn crop(&self) -> CropRect {
        self.crop
    }
    pub const fn timestamp(&self) -> TimestampMicros {
        self.image.timestamp
    }
}
impl std::fmt::Debug for DecodedImageLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedImageLease")
            .field("timestamp", &self.timestamp)
            .finish_non_exhaustive()
    }
}
impl Drop for DecodedImageLease {
    fn drop(&mut self) {
        // SAFETY: uniquely acquired image, no Rust views remain during Drop. Its anchor still owns reader/session.
        unsafe {
            ffi::AImage_delete(self.raw.as_ptr());
        }
    }
}
impl DecodedImageLease {
    pub const fn timestamp(&self) -> TimestampMicros {
        self.timestamp
    }
    fn metadata(&self) -> Result<(CropRect, [u32; 2]), DecodeError> {
        let image = self.raw.as_ptr();
        let mut width = 0;
        let mut height = 0;
        let mut format = 0;
        let mut count = 0;
        let mut crop = ffi::Crop::default();
        // SAFETY: live exclusively held AImage, initialized writable scalar/struct outputs.
        unsafe {
            check("image width", ffi::AImage_getWidth(image, &raw mut width))?;
            check(
                "image height",
                ffi::AImage_getHeight(image, &raw mut height),
            )?;
            check(
                "image format",
                ffi::AImage_getFormat(image, &raw mut format),
            )?;
            check(
                "image plane count",
                ffi::AImage_getNumberOfPlanes(image, &raw mut count),
            )?;
            check("image crop", ffi::AImage_getCropRect(image, &raw mut crop))?;
        }
        let crop = validate_metadata(
            [width, height],
            format,
            count,
            [crop.left, crop.top, crop.right, crop.bottom],
            [self.width, self.height],
        )?;
        Ok((crop, [width as u32, height as u32]))
    }
    /// Borrow an acquire-ready allocation without mapping its CPU planes.
    ///
    /// The exclusive borrow prevents simultaneous CPU access:
    /// ```compile_fail,E0502
    /// use video_hw_backend_android::image_reader::DecodedImageLease;
    /// fn overlapping(image: &mut DecodedImageLease) {
    ///     let gpu = image.hardware_buffer().unwrap();
    ///     let cpu = image.planes().unwrap();
    ///     std::hint::black_box((gpu, cpu));
    /// }
    /// ```
    /// The image owner cannot be dropped while its capability is in use:
    /// ```compile_fail,E0505
    /// use video_hw_backend_android::image_reader::DecodedImageLease;
    /// fn early_return(mut image: DecodedImageLease) {
    ///     let gpu = image.hardware_buffer().unwrap();
    ///     drop(image);
    ///     std::hint::black_box(gpu);
    /// }
    /// ```
    pub fn hardware_buffer(&mut self) -> Result<HardwareBufferBorrow<'_>, DecodeError> {
        self.access.hardware()?;
        let (crop, extent) = self.metadata()?;
        let mut buffer = ptr::null_mut();
        // SAFETY: acquire-ready live image, initialized output; NDK lends the
        // allocation without changing its reference count.
        unsafe {
            check(
                "image hardware buffer",
                ffi::AImage_getHardwareBuffer(self.raw.as_ptr(), &raw mut buffer),
            )?;
        }
        let buffer = NonNull::new(buffer).ok_or(DecodeError::Contract("null hardware buffer"))?;
        let mut desc = ffi::HardwareBufferDesc::default();
        // SAFETY: buffer remains owned by this exclusively borrowed live image.
        unsafe {
            ffi::AHardwareBuffer_describe(buffer.as_ptr(), &raw mut desc);
        }
        let descriptor = HardwareBufferDescriptor::new(
            [desc.width, desc.height],
            desc.layers,
            desc.format,
            desc.usage,
            desc.stride,
            extent,
        )?;
        Ok(HardwareBufferBorrow {
            image: self,
            buffer,
            descriptor,
            image_extent: extent,
            crop,
        })
    }
    /// CPU mapping stays active in AImage until image return. GPU borrowing is
    /// forbidden after this call, even if validation fails or this view is dropped.
    pub fn planes(&self) -> Result<YuvImageView<'_>, DecodeError> {
        let (crop, _) = self.metadata()?;
        self.access.begin_cpu();
        let image = self.raw.as_ptr();
        let mut planes = Vec::with_capacity(3);
        for index in 0..3 {
            let mut data = ptr::null_mut();
            let mut length = 0;
            let mut row = 0;
            let mut pixel = 0;
            // SAFETY: plane indices are proved 0..3, outputs stay live through each call.
            unsafe {
                check(
                    "plane data",
                    ffi::AImage_getPlaneData(image, index, &raw mut data, &raw mut length),
                )?;
                check(
                    "plane row stride",
                    ffi::AImage_getPlaneRowStride(image, index, &raw mut row),
                )?;
                check(
                    "plane pixel stride",
                    ffi::AImage_getPlanePixelStride(image, index, &raw mut pixel),
                )?;
            }
            if data.is_null() || length <= 0 || length > 64 * 1024 * 1024 {
                return Err(DecodeError::Contract("plane data span out of bounds"));
            }
            let length = usize::try_from(length)
                .map_err(|_| DecodeError::Contract("plane length conversion"))?;
            // SAFETY: NDK returns readable plane memory for this acquired image;
            // span is bounded and immutable slices borrow self, including its anchor.
            let bytes = unsafe { std::slice::from_raw_parts(data, length) };
            planes.push(PlaneView::new(bytes, row, pixel)?);
        }
        let planes = planes
            .try_into()
            .map_err(|_| DecodeError::Contract("plane count differs"))?;
        YuvImageView::new(planes, crop, self.width, self.height)
    }
}

fn check(operation: &'static str, status: i32) -> Result<(), DecodeError> {
    if status == 0 {
        Ok(())
    } else {
        Err(DecodeError::Media { operation, status })
    }
}

/// Owns both acquisition outputs before any fallible observation. Returning an
/// unready image transfers its original acquire fence as the release fence; no
/// synchronous image drop or unbounded wait is hidden in this destructor.
struct PendingImage {
    image: Option<NonNull<ffi::AImage>>,
    fence: Option<OwnedFd>,
}
impl PendingImage {
    fn new(image: *mut ffi::AImage, fence: i32) -> Self {
        Self {
            image: NonNull::new(image),
            fence: if fence >= 0 {
                // SAFETY: acquireNextImageAsync transfers this unique fd to us.
                Some(unsafe { OwnedFd::from_raw_fd(fence) })
            } else {
                None
            },
        }
    }
    fn ready(mut self, start: Instant) -> Result<NonNull<ffi::AImage>, DecodeError> {
        if self.image.is_none() {
            return Err(DecodeError::Contract("null acquired image"));
        }
        if let Some(fence) = &self.fence {
            wait_acquire_fence(
                || start.elapsed(),
                |timeout| {
                    let mut fd = ffi::PollFd {
                        fd: fence.as_raw_fd(),
                        events: 1,
                        revents: 0,
                    };
                    // SAFETY: one initialized pollfd with a uniquely owned live fd;
                    // timeout is finite and bounded by the decode's absolute budget.
                    let result = unsafe { ffi::poll(&raw mut fd, 1, timeout) };
                    if result < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    match result {
                        0 => Ok(None),
                        1 => Ok(Some(fd.revents)),
                        _ => Err(std::io::Error::other("unexpected poll count")),
                    }
                },
            )?;
        }
        if start.elapsed() >= DECODE_TIMEOUT {
            return Err(DecodeError::Deadline);
        }
        // The ready fence is no longer needed; its close occurs exactly once.
        drop(self.fence.take());
        self.image
            .take()
            .ok_or(DecodeError::Contract("missing acquired image"))
    }
}
impl Drop for PendingImage {
    fn drop(&mut self) {
        if let Some(image) = self.image.take() {
            let release_fence = self.fence.take().map_or(-1, IntoRawFd::into_raw_fd);
            // SAFETY: image has never escaped or been CPU/GPU accessed. NDK
            // consumes it and the transferred fd exactly once, waiting before
            // making its allocation reusable by the producer.
            unsafe {
                ffi::AImage_deleteAsync(image.as_ptr(), release_fence);
            }
        }
    }
}
struct Format(NonNull<ffi::AMediaFormat>);
impl Drop for Format {
    fn drop(&mut self) {
        // SAFETY: uniquely owned format; NDK deletion consumes it once.
        unsafe {
            ffi::AMediaFormat_delete(self.0.as_ptr());
        }
    }
}
impl Session {
    fn initialize(&mut self) -> Result<(), DecodeError> {
        // SAFETY: bounded dimensions, CPU-readable usage, initialized pointer outputs. Reader owns window.
        unsafe {
            check(
                "create reader",
                ffi::AImageReader_newWithUsage(
                    self.config.width as i32,
                    self.config.height as i32,
                    35,
                    IMAGE_USAGE,
                    4,
                    &raw mut self.reader,
                ),
            )?;
        }
        if self.reader.is_null() {
            return Err(DecodeError::Contract("null image reader"));
        }
        let mut window = ptr::null_mut();
        // SAFETY: live reader; returned borrowed window is never separately released.
        unsafe {
            check(
                "reader window",
                ffi::AImageReader_getWindow(self.reader, &raw mut window),
            )?;
            self.codec = ffi::AMediaCodec_createCodecByName(self.config.name.as_ptr());
        }
        if window.is_null() || self.codec.is_null() {
            return Err(DecodeError::Contract("null reader window or codec"));
        }
        // SAFETY: no inputs; newly owned format wrapped immediately.
        let format = Format(
            NonNull::new(unsafe { ffi::AMediaFormat_new() })
                .ok_or(DecodeError::Contract("null media format"))?,
        );
        // SAFETY: live format/codec/reader-owned window, static NUL-terminated keys. Coded config copied by codec.
        unsafe {
            ffi::AMediaFormat_setString(
                format.0.as_ptr(),
                c"mime".as_ptr(),
                self.config.codec.mime().as_ptr(),
            );
            for (key, value) in [
                (c"width", self.config.width as i32),
                (c"height", self.config.height as i32),
                (c"max-input-size", MAX_PACKET as i32),
                (c"low-latency", 1),
                (c"color-format", 0x7f420888),
            ] {
                ffi::AMediaFormat_setInt32(format.0.as_ptr(), key.as_ptr(), value);
            }
            if !self.config.allow_frame_drop {
                ffi::AMediaFormat_setInt32(format.0.as_ptr(), c"allow-frame-drop".as_ptr(), 0);
            }
            check(
                "configure codec",
                ffi::AMediaCodec_configure(
                    self.codec,
                    format.0.as_ptr(),
                    window,
                    ptr::null_mut(),
                    0,
                ),
            )?;
            self.codec_phase = CodecPhase::StartAttempted;
            check("start codec", ffi::AMediaCodec_start(self.codec))?;
        }
        self.codec_phase = CodecPhase::Started;
        Ok(())
    }
    fn decode(
        &mut self,
        packet: &[u8],
        pts: TimestampMicros,
        start: Instant,
    ) -> Result<NonNull<ffi::AImage>, DecodeError> {
        let mut queued = false;
        let mut rendered = false;
        while start.elapsed() < DECODE_TIMEOUT {
            if !queued {
                // SAFETY: started codec on its owning worker, no outstanding input index.
                let index = unsafe { ffi::AMediaCodec_dequeueInputBuffer(self.codec, 0) };
                if index >= 0 {
                    let mut capacity = 0;
                    // SAFETY: index just dequeued from this codec, writable capacity output.
                    let buffer = unsafe {
                        ffi::AMediaCodec_getInputBuffer(
                            self.codec,
                            index as usize,
                            &raw mut capacity,
                        )
                    };
                    if buffer.is_null() || packet.len() > capacity {
                        return Err(DecodeError::Contract("AU exceeds codec input allocation"));
                    }
                    // SAFETY: input buffer is exclusive and capacity-checked; copy once then queue once with exact unsigned microseconds.
                    unsafe {
                        ptr::copy_nonoverlapping(packet.as_ptr(), buffer, packet.len());
                        check(
                            "queue AU",
                            ffi::AMediaCodec_queueInputBuffer(
                                self.codec,
                                index as usize,
                                0,
                                packet.len(),
                                pts.get(),
                                0,
                            ),
                        )?;
                    }
                    queued = true;
                } else if index != -1 {
                    return Err(DecodeError::Contract("input dequeue failed"));
                }
            }
            let mut info = ffi::BufferInfo::default();
            // SAFETY: initialized BufferInfo matches ARM64 ABI; one synchronous poll.
            let index =
                unsafe { ffi::AMediaCodec_dequeueOutputBuffer(self.codec, &raw mut info, 1000) };
            if index >= 0 {
                if info.presentation_time_us < 0
                    || info.presentation_time_us as u64 > pts.get()
                    || info.flags & 4 != 0
                {
                    return Err(DecodeError::Contract("future/negative/EOS output PTS"));
                }
                let current = info.presentation_time_us as u64 == pts.get();
                if current && (!queued || rendered) {
                    return Err(DecodeError::Contract("duplicate/unqueued output"));
                }
                // SAFETY: this newly dequeued index is released exactly once. Any error exits into Failed; no release retry.
                unsafe {
                    check(
                        "render output",
                        ffi::AMediaCodec_releaseOutputBuffer(self.codec, index as usize, current),
                    )?;
                }
                rendered |= current;
            } else if index == -2 {
                self.validate_format()?;
            } else if index != -1 && index != -3 {
                return Err(DecodeError::Contract("output dequeue failed"));
            }
            let mut image = ptr::null_mut();
            let mut acquire_fence = -1;
            // SAFETY: live reader, no previous lease outstanding; writable output.
            let status = unsafe {
                ffi::AImageReader_acquireNextImageAsync(
                    self.reader,
                    &raw mut image,
                    &raw mut acquire_fence,
                )
            };
            // Claim outputs before any fallible status/metadata observation.
            let pending = PendingImage::new(image, acquire_fence);
            if status == -30001 {
                continue;
            }
            check("acquire image", status)?;
            let image = pending.ready(start)?;
            let mut ns = 0;
            // SAFETY: uniquely acquired live image; timestamp output initialized.
            let outcome = check("image timestamp", unsafe {
                ffi::AImage_getTimestamp(image.as_ptr(), &raw mut ns)
            })
            .and_then(|()| TimestampMicros::from_image_nanoseconds(ns));
            match outcome {
                Ok(actual) if actual.get() < pts.get() => {
                    /* SAFETY: proved stale uniquely owned image has no exported view. */
                    unsafe {
                        ffi::AImage_delete(image.as_ptr());
                    }
                }
                Ok(actual) if actual == pts && queued && rendered && self.color_valid => {
                    return Ok(image);
                }
                _ => {
                    // SAFETY: image never published; return it once before terminal failure.
                    unsafe {
                        ffi::AImage_delete(image.as_ptr());
                    }
                    return Err(DecodeError::Contract("exact image PTS/color state differs"));
                }
            }
        }
        Err(DecodeError::Deadline)
    }
    fn validate_format(&mut self) -> Result<(), DecodeError> {
        // SAFETY: NDK returns a newly owned output format for this codec.
        let format = Format(
            NonNull::new(unsafe { ffi::AMediaCodec_getOutputFormat(self.codec) })
                .ok_or(DecodeError::Contract("missing output format"))?,
        );
        let mut standard = 0;
        let mut range = 0;
        // SAFETY: live owned format, NUL-terminated keys and writable outputs.
        let valid = unsafe {
            ffi::AMediaFormat_getInt32(
                format.0.as_ptr(),
                c"color-standard".as_ptr(),
                &raw mut standard,
            ) && ffi::AMediaFormat_getInt32(
                format.0.as_ptr(),
                c"color-range".as_ptr(),
                &raw mut range,
            )
        };
        if !valid || standard != 1 || range != 2 {
            return Err(DecodeError::Contract("expected BT709 limited output"));
        }
        self.color_valid = true;
        Ok(())
    }
    fn close(&mut self) -> Result<(), DecodeError> {
        if self.decode_phase == DecodePhase::Retained {
            return Err(DecodeError::Contract(
                "failed cleanup is retained without retry",
            ));
        }
        // Latch retirement before fallible driver calls so neither Drop nor a
        // repeated explicit close retries a call whose outcome is uncertain.
        self.decode_phase = DecodePhase::Retained;
        if !self.codec.is_null() {
            if matches!(
                self.codec_phase,
                CodecPhase::Started | CodecPhase::StartAttempted
            ) {
                // SAFETY: known created codec; reader remains alive until successful stop/delete.
                unsafe {
                    check("stop codec", ffi::AMediaCodec_stop(self.codec))?;
                }
                self.codec_phase = CodecPhase::Stopped;
            }
            // SAFETY: known owned codec, no image lease; delete attempted once.
            unsafe {
                check("delete codec", ffi::AMediaCodec_delete(self.codec))?;
            }
            self.codec = ptr::null_mut();
        }
        if !self.reader.is_null() {
            // SAFETY: no images remain and codec was deleted; borrowed window no longer used.
            unsafe {
                ffi::AImageReader_delete(self.reader);
            }
            self.reader = ptr::null_mut();
        }
        Ok(())
    }
}
