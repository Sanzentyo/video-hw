//! Narrow ARM64/API32 ABI, reviewed against NDK r27c media headers.
#![allow(non_snake_case)]
use std::ffi::{c_char, c_void};

#[repr(C)]
pub(super) struct AImage {
    _private: [u8; 0],
}
#[repr(C)]
pub(super) struct AImageReader {
    _private: [u8; 0],
}
#[repr(C)]
pub(super) struct AMediaCodec {
    _private: [u8; 0],
}
#[repr(C)]
pub(super) struct AMediaFormat {
    _private: [u8; 0],
}
#[repr(C)]
pub(super) struct ANativeWindow {
    _private: [u8; 0],
}
#[repr(C)]
#[derive(Default)]
pub(super) struct BufferInfo {
    pub offset: i32,
    pub size: i32,
    pub presentation_time_us: i64,
    pub flags: u32,
}
#[repr(C)]
#[derive(Default)]
pub(super) struct Crop {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
const _: () = {
    assert!(std::mem::size_of::<BufferInfo>() == 24);
    assert!(std::mem::offset_of!(BufferInfo, presentation_time_us) == 8);
    assert!(std::mem::size_of::<Crop>() == 16);
};

// SAFETY CONTRACT: callers own each live pointer, initialize output storage,
// retain reader-owned windows and image memory, and invoke only on their worker.
#[link(name = "mediandk")]
unsafe extern "C" {
    pub(super) fn AImageReader_newWithUsage(
        width: i32,
        height: i32,
        format: i32,
        usage: u64,
        max_images: i32,
        reader: *mut *mut AImageReader,
    ) -> i32;
    pub(super) fn AImageReader_getWindow(
        reader: *mut AImageReader,
        window: *mut *mut ANativeWindow,
    ) -> i32;
    pub(super) fn AImageReader_acquireNextImage(
        reader: *mut AImageReader,
        image: *mut *mut AImage,
    ) -> i32;
    pub(super) fn AImageReader_delete(reader: *mut AImageReader);
    pub(super) fn AImage_delete(image: *mut AImage);
    pub(super) fn AImage_getTimestamp(image: *const AImage, ns: *mut i64) -> i32;
    pub(super) fn AImage_getWidth(image: *const AImage, width: *mut i32) -> i32;
    pub(super) fn AImage_getHeight(image: *const AImage, height: *mut i32) -> i32;
    pub(super) fn AImage_getFormat(image: *const AImage, format: *mut i32) -> i32;
    pub(super) fn AImage_getNumberOfPlanes(image: *const AImage, planes: *mut i32) -> i32;
    pub(super) fn AImage_getCropRect(image: *const AImage, crop: *mut Crop) -> i32;
    pub(super) fn AImage_getPlaneData(
        image: *const AImage,
        plane: i32,
        data: *mut *mut u8,
        length: *mut i32,
    ) -> i32;
    pub(super) fn AImage_getPlaneRowStride(
        image: *const AImage,
        plane: i32,
        stride: *mut i32,
    ) -> i32;
    pub(super) fn AImage_getPlanePixelStride(
        image: *const AImage,
        plane: i32,
        stride: *mut i32,
    ) -> i32;
    pub(super) fn AMediaCodec_createCodecByName(name: *const c_char) -> *mut AMediaCodec;
    pub(super) fn AMediaCodec_configure(
        codec: *mut AMediaCodec,
        format: *const AMediaFormat,
        window: *mut ANativeWindow,
        crypto: *mut c_void,
        flags: u32,
    ) -> i32;
    pub(super) fn AMediaCodec_start(codec: *mut AMediaCodec) -> i32;
    pub(super) fn AMediaCodec_stop(codec: *mut AMediaCodec) -> i32;
    pub(super) fn AMediaCodec_delete(codec: *mut AMediaCodec) -> i32;
    pub(super) fn AMediaCodec_dequeueInputBuffer(codec: *mut AMediaCodec, timeout_us: i64)
    -> isize;
    pub(super) fn AMediaCodec_getInputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        size: *mut usize,
    ) -> *mut u8;
    pub(super) fn AMediaCodec_queueInputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        offset: i64,
        size: usize,
        time: u64,
        flags: u32,
    ) -> i32;
    pub(super) fn AMediaCodec_dequeueOutputBuffer(
        codec: *mut AMediaCodec,
        info: *mut BufferInfo,
        timeout_us: i64,
    ) -> isize;
    pub(super) fn AMediaCodec_releaseOutputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        render: bool,
    ) -> i32;
    pub(super) fn AMediaCodec_getOutputFormat(codec: *mut AMediaCodec) -> *mut AMediaFormat;
    pub(super) fn AMediaFormat_new() -> *mut AMediaFormat;
    pub(super) fn AMediaFormat_delete(format: *mut AMediaFormat) -> i32;
    pub(super) fn AMediaFormat_setInt32(format: *mut AMediaFormat, name: *const c_char, value: i32);
    pub(super) fn AMediaFormat_setString(
        format: *mut AMediaFormat,
        name: *const c_char,
        value: *const c_char,
    );
    pub(super) fn AMediaFormat_getInt32(
        format: *mut AMediaFormat,
        name: *const c_char,
        value: *mut i32,
    ) -> bool;
}
