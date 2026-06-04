//! Minimal handwritten FFI skeleton for Android NDK MediaCodec.
//!
//! Prefer keeping this narrow for the MVP. Add functions only when used.

use core::ffi::{c_char, c_void};

#[repr(C)]
pub struct AMediaCodec {
    _private: [u8; 0],
}

#[repr(C)]
pub struct AMediaFormat {
    _private: [u8; 0],
}

#[repr(C)]
pub struct ANativeWindow {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AMediaCodecBufferInfo {
    pub offset: i32,
    pub size: i32,
    pub presentationTimeUs: i64,
    pub flags: u32,
}

pub type media_status_t = i32;

pub const AMEDIA_OK: media_status_t = 0;
pub const AMEDIACODEC_CONFIGURE_FLAG_ENCODE: u32 = 1;
pub const AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM: u32 = 4;
pub const AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED: isize = -2;
pub const AMEDIACODEC_INFO_TRY_AGAIN_LATER: isize = -1;

#[link(name = "mediandk")]
unsafe extern "C" {
    pub fn AMediaCodec_createCodecByName(name: *const c_char) -> *mut AMediaCodec;
    pub fn AMediaCodec_createDecoderByType(mime_type: *const c_char) -> *mut AMediaCodec;
    pub fn AMediaCodec_createEncoderByType(mime_type: *const c_char) -> *mut AMediaCodec;

    pub fn AMediaCodec_configure(
        codec: *mut AMediaCodec,
        format: *const AMediaFormat,
        surface: *mut ANativeWindow,
        crypto: *mut c_void,
        flags: u32,
    ) -> media_status_t;

    pub fn AMediaCodec_start(codec: *mut AMediaCodec) -> media_status_t;
    pub fn AMediaCodec_stop(codec: *mut AMediaCodec) -> media_status_t;
    pub fn AMediaCodec_flush(codec: *mut AMediaCodec) -> media_status_t;
    pub fn AMediaCodec_delete(codec: *mut AMediaCodec) -> media_status_t;

    pub fn AMediaCodec_dequeueInputBuffer(codec: *mut AMediaCodec, timeoutUs: i64) -> isize;
    pub fn AMediaCodec_getInputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        out_size: *mut usize,
    ) -> *mut u8;
    pub fn AMediaCodec_queueInputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        offset: usize,
        size: usize,
        time: u64,
        flags: u32,
    ) -> media_status_t;

    pub fn AMediaCodec_dequeueOutputBuffer(
        codec: *mut AMediaCodec,
        info: *mut AMediaCodecBufferInfo,
        timeoutUs: i64,
    ) -> isize;
    pub fn AMediaCodec_getOutputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        out_size: *mut usize,
    ) -> *mut u8;
    pub fn AMediaCodec_releaseOutputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        render: bool,
    ) -> media_status_t;

    pub fn AMediaCodec_createInputSurface(
        codec: *mut AMediaCodec,
        surface: *mut *mut ANativeWindow,
    ) -> media_status_t;

    pub fn AMediaFormat_new() -> *mut AMediaFormat;
    pub fn AMediaFormat_delete(format: *mut AMediaFormat) -> media_status_t;
    pub fn AMediaFormat_setString(format: *mut AMediaFormat, name: *const c_char, value: *const c_char);
    pub fn AMediaFormat_setInt32(format: *mut AMediaFormat, name: *const c_char, value: i32);
    pub fn AMediaFormat_setInt64(format: *mut AMediaFormat, name: *const c_char, value: i64);
    pub fn AMediaFormat_setBuffer(
        format: *mut AMediaFormat,
        name: *const c_char,
        data: *const c_void,
        size: usize,
    );
}

#[link(name = "android")]
unsafe extern "C" {
    pub fn ANativeWindow_release(window: *mut ANativeWindow);
}
