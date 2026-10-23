//! CPU-only NDK contract model. It runs the actual decoder/lease code without
//! loading platform libraries; it does not model codec timing or GPU completion.
//!
//! Safety of this test boundary: only the real decoder calls these functions.
//! Its output pointers name initialized, aligned, exclusive stack/Session fields
//! for the complete synchronous call. Opaque object pointers are map identities
//! and are never dereferenced. The input-buffer pointer names a fixed Vec whose
//! allocation stays in Codec until deletion; queueing never reallocates it.
use super::*;
use std::{cell::RefCell, collections::BTreeMap, ffi::CStr};

#[derive(Default)]
struct Codec {
    input: Vec<u8>,
    window: usize,
    pts: Option<u64>,
    announced: bool,
}
#[derive(Clone, Copy)]
struct Image {
    reader: usize,
    width: i32,
    height: i32,
    pts: u64,
}
#[derive(Default)]
pub(crate) struct Driver {
    next: usize,
    codecs: BTreeMap<usize, Codec>,
    readers: BTreeMap<usize, (i32, i32)>,
    pending: BTreeMap<usize, Image>,
    images: BTreeMap<usize, Image>,
    pub queued: Vec<u64>,
    pub deleted_images: Vec<usize>,
    pub async_returns: Vec<(usize, i32)>,
    pub deleted_readers: usize,
    pub stopped: usize,
    pub deleted_codecs: usize,
    pub closed_fences: Vec<i32>,
    pub reader_max_images: Vec<i32>,
    pub color_range: i32,
    pub invalid_extent: bool,
    pub null_buffer: bool,
    pub invalid_timestamp: bool,
    pub fail_queue: bool,
    pub fail_stop: bool,
    pub acquire_fence: Option<i32>,
    pub fence_error: bool,
    pub plane_reads: usize,
}
impl Driver {
    fn id(&mut self) -> usize {
        self.next += 1;
        self.next
    }
    pub(crate) fn live_images(&self) -> usize {
        self.images.len()
    }
}
thread_local! {
    static DRIVER: RefCell<Driver> = RefCell::new(Driver { color_range: 1, ..Driver::default() });
}
pub(crate) fn with_driver<T>(f: impl FnOnce(&mut Driver) -> T) -> T {
    DRIVER.with(|d| f(&mut d.borrow_mut()))
}
fn handle<T>(id: usize) -> *mut T {
    // Opaque test identities are never dereferenced as platform objects.
    ptr::without_provenance_mut(id)
}
use std::ptr;

pub(crate) struct OwnedFd(i32);
impl OwnedFd {
    pub(crate) unsafe fn from_raw_fd(fd: i32) -> Self {
        Self(fd)
    }
    pub(crate) fn as_raw_fd(&self) -> i32 {
        self.0
    }
    pub(crate) fn into_raw_fd(self) -> i32 {
        let fd = self.0;
        std::mem::forget(self);
        fd
    }
}
impl Drop for OwnedFd {
    fn drop(&mut self) {
        with_driver(|d| d.closed_fences.push(self.0));
    }
}
pub(crate) unsafe fn poll(fds: *mut PollFd, count: u32, timeout_ms: i32) -> i32 {
    assert_eq!(count, 1);
    assert!(timeout_ms > 0);
    // SAFETY: actual decoder supplies its initialized one-element poll storage.
    let fd = unsafe { &mut *fds };
    assert!(fd.fd >= 0);
    assert_eq!(fd.events, 1);
    fd.revents = with_driver(|d| if d.fence_error { 8 } else { 1 });
    1
}
pub(crate) unsafe fn AImageReader_newWithUsage(
    width: i32,
    height: i32,
    format: i32,
    usage: u64,
    max_images: i32,
    reader: *mut *mut AImageReader,
) -> i32 {
    assert_eq!(format, 35);
    assert_eq!(usage & 0x103, 0x103);
    let id = with_driver(|d| {
        let id = d.id();
        d.readers.insert(id, (width, height));
        d.reader_max_images.push(max_images);
        id
    });
    // SAFETY: output pointer comes from the decoder's live Session.
    unsafe { *reader = handle(id) };
    0
}
pub(crate) unsafe fn AImageReader_getWindow(
    reader: *mut AImageReader,
    window: *mut *mut ANativeWindow,
) -> i32 {
    with_driver(|d| assert!(d.readers.contains_key(&reader.addr())));
    unsafe { *window = reader.cast() };
    0
}
pub(crate) unsafe fn AImageReader_acquireNextImageAsync(
    reader: *mut AImageReader,
    image: *mut *mut AImage,
    fence: *mut i32,
) -> i32 {
    with_driver(|d| {
        let Some(value) = d.pending.remove(&reader.addr()) else {
            return -30001;
        };
        let id = d.id();
        d.images.insert(id, value);
        unsafe {
            *image = handle(id);
            *fence = d.acquire_fence.take().unwrap_or(-1);
        }
        0
    })
}
pub(crate) unsafe fn AImageReader_delete(reader: *mut AImageReader) {
    with_driver(|d| {
        assert!(d.images.values().all(|i| i.reader != reader.addr()));
        assert!(d.readers.remove(&reader.addr()).is_some());
        d.deleted_readers += 1;
    });
}
pub(crate) unsafe fn AImage_delete(image: *mut AImage) {
    with_driver(|d| {
        assert!(
            d.images.remove(&image.addr()).is_some(),
            "image returned twice"
        );
        d.deleted_images.push(image.addr());
    });
}
pub(crate) unsafe fn AImage_deleteAsync(image: *mut AImage, fence: i32) {
    with_driver(|d| {
        assert!(
            d.images.remove(&image.addr()).is_some(),
            "image returned twice"
        );
        d.async_returns.push((image.addr(), fence));
        if fence >= 0 {
            d.closed_fences.push(fence);
        }
    });
}
pub(crate) unsafe fn AImage_getHardwareBuffer(
    image: *const AImage,
    buffer: *mut *mut AHardwareBuffer,
) -> i32 {
    with_driver(|d| {
        assert!(d.images.contains_key(&image.addr()));
        unsafe {
            *buffer = if d.null_buffer {
                ptr::null_mut()
            } else {
                handle(image.addr())
            };
        }
    });
    0
}
pub(crate) unsafe fn AHardwareBuffer_describe(
    buffer: *const AHardwareBuffer,
    desc: *mut HardwareBufferDesc,
) {
    with_driver(|d| {
        let i = d.images[&buffer.addr()];
        unsafe {
            *desc = HardwareBufferDesc {
                width: i.width as u32,
                height: i.height as u32,
                layers: 1,
                format: 35,
                usage: 0x103,
                stride: i.width as u32,
                ..HardwareBufferDesc::default()
            };
        }
    });
}
pub(crate) unsafe fn AImage_getTimestamp(image: *const AImage, ns: *mut i64) -> i32 {
    with_driver(|d| unsafe {
        *ns = (d.images[&image.addr()].pts * 1000) as i64 + i64::from(d.invalid_timestamp);
    });
    0
}
pub(crate) unsafe fn AImage_getWidth(image: *const AImage, width: *mut i32) -> i32 {
    with_driver(|d| unsafe {
        *width = if d.invalid_extent {
            0
        } else {
            d.images[&image.addr()].width
        };
    });
    0
}
pub(crate) unsafe fn AImage_getHeight(image: *const AImage, height: *mut i32) -> i32 {
    with_driver(|d| unsafe {
        *height = d.images[&image.addr()].height;
    });
    0
}
pub(crate) unsafe fn AImage_getFormat(_image: *const AImage, format: *mut i32) -> i32 {
    unsafe { *format = 35 };
    0
}
pub(crate) unsafe fn AImage_getNumberOfPlanes(_image: *const AImage, planes: *mut i32) -> i32 {
    unsafe { *planes = 3 };
    0
}
pub(crate) unsafe fn AImage_getCropRect(image: *const AImage, crop: *mut Crop) -> i32 {
    with_driver(|d| {
        let i = d.images[&image.addr()];
        unsafe {
            *crop = Crop {
                left: 0,
                top: 0,
                right: i.width,
                bottom: i.height,
            };
        }
    });
    0
}
pub(crate) unsafe fn AImage_getPlaneData(
    _image: *const AImage,
    _plane: i32,
    _data: *mut *mut u8,
    _length: *mut i32,
) -> i32 {
    with_driver(|d| d.plane_reads += 1);
    -1 // Model failed mapping: the permanent CPU-access latch must still apply.
}
pub(crate) unsafe fn AImage_getPlaneRowStride(
    _image: *const AImage,
    _plane: i32,
    _stride: *mut i32,
) -> i32 {
    unreachable!()
}
pub(crate) unsafe fn AImage_getPlanePixelStride(
    _image: *const AImage,
    _plane: i32,
    _stride: *mut i32,
) -> i32 {
    unreachable!()
}
pub(crate) unsafe fn AMediaCodec_createCodecByName(_name: *const c_char) -> *mut AMediaCodec {
    with_driver(|d| {
        let id = d.id();
        d.codecs.insert(
            id,
            Codec {
                input: vec![0; 64],
                ..Codec::default()
            },
        );
        handle(id)
    })
}
pub(crate) unsafe fn AMediaCodec_configure(
    codec: *mut AMediaCodec,
    _format: *const AMediaFormat,
    window: *mut ANativeWindow,
    _crypto: *mut c_void,
    flags: u32,
) -> i32 {
    assert_eq!(flags, 0);
    with_driver(|d| d.codecs.get_mut(&codec.addr()).unwrap().window = window.addr());
    0
}
pub(crate) unsafe fn AMediaCodec_start(_codec: *mut AMediaCodec) -> i32 {
    0
}
pub(crate) unsafe fn AMediaCodec_stop(_codec: *mut AMediaCodec) -> i32 {
    with_driver(|d| {
        d.stopped += 1;
        if d.fail_stop { -1 } else { 0 }
    })
}
pub(crate) unsafe fn AMediaCodec_delete(codec: *mut AMediaCodec) -> i32 {
    with_driver(|d| {
        assert!(d.codecs.remove(&codec.addr()).is_some());
        d.deleted_codecs += 1;
    });
    0
}
pub(crate) unsafe fn AMediaCodec_dequeueInputBuffer(
    _codec: *mut AMediaCodec,
    _timeout: i64,
) -> isize {
    0
}
pub(crate) unsafe fn AMediaCodec_getInputBuffer(
    codec: *mut AMediaCodec,
    _index: usize,
    size: *mut usize,
) -> *mut u8 {
    with_driver(|d| {
        let c = d.codecs.get_mut(&codec.addr()).unwrap();
        unsafe {
            *size = c.input.len();
        }
        c.input.as_mut_ptr()
    })
}
pub(crate) unsafe fn AMediaCodec_queueInputBuffer(
    codec: *mut AMediaCodec,
    _index: usize,
    _offset: i64,
    _size: usize,
    pts: u64,
    _flags: u32,
) -> i32 {
    with_driver(|d| {
        d.queued.push(pts);
        if d.fail_queue {
            return -1;
        }
        let c = d.codecs.get_mut(&codec.addr()).unwrap();
        c.pts = Some(pts);
        c.announced = false;
        0
    })
}
pub(crate) unsafe fn AMediaCodec_dequeueOutputBuffer(
    codec: *mut AMediaCodec,
    info: *mut BufferInfo,
    _timeout: i64,
) -> isize {
    with_driver(|d| {
        let c = d.codecs.get_mut(&codec.addr()).unwrap();
        if !c.announced {
            c.announced = true;
            return -2;
        }
        let Some(pts) = c.pts else { return -1 };
        unsafe {
            *info = BufferInfo {
                presentation_time_us: pts as i64,
                ..BufferInfo::default()
            };
        }
        0
    })
}
pub(crate) unsafe fn AMediaCodec_releaseOutputBuffer(
    codec: *mut AMediaCodec,
    _index: usize,
    render: bool,
) -> i32 {
    with_driver(|d| {
        let c = d.codecs.get_mut(&codec.addr()).unwrap();
        let pts = c.pts.take().unwrap();
        if render {
            let (width, height) = d.readers[&c.window];
            assert!(
                d.pending
                    .insert(
                        c.window,
                        Image {
                            reader: c.window,
                            width,
                            height,
                            pts
                        }
                    )
                    .is_none()
            );
        }
    });
    0
}
pub(crate) unsafe fn AMediaCodec_getOutputFormat(_codec: *mut AMediaCodec) -> *mut AMediaFormat {
    with_driver(|d| handle(d.id()))
}
pub(crate) unsafe fn AMediaFormat_new() -> *mut AMediaFormat {
    with_driver(|d| handle(d.id()))
}
pub(crate) unsafe fn AMediaFormat_delete(_format: *mut AMediaFormat) -> i32 {
    0
}
pub(crate) unsafe fn AMediaFormat_setInt32(
    _format: *mut AMediaFormat,
    _name: *const c_char,
    _value: i32,
) {
}
pub(crate) unsafe fn AMediaFormat_setString(
    _format: *mut AMediaFormat,
    _name: *const c_char,
    _value: *const c_char,
) {
}
pub(crate) unsafe fn AMediaFormat_getInt32(
    _format: *mut AMediaFormat,
    name: *const c_char,
    value: *mut i32,
) -> bool {
    // SAFETY: decoder passes one of its static NUL-terminated metadata keys.
    match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"color-standard" => unsafe {
            *value = 1;
        },
        b"color-range" => unsafe {
            *value = with_driver(|d| d.color_range);
        },
        _ => return false,
    }
    true
}
