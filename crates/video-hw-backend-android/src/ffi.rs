use std::ffi::{c_char, c_void};
use std::ptr::NonNull;

use video_hw_core::BackendError;

pub(crate) mod codec {
    use super::*;

    pub(crate) const OK: i32 = 0;
    pub(crate) const TRY_AGAIN_LATER: isize = -1;
    pub(crate) const OUTPUT_FORMAT_CHANGED: isize = -2;
    pub(crate) const BUFFER_FLAG_KEY_FRAME: u32 = 1;
    pub(crate) const BUFFER_FLAG_CODEC_CONFIG: u32 = 2;
    pub(crate) const BUFFER_FLAG_END_OF_STREAM: u32 = 4;
    pub(crate) const CONFIGURE_FLAG_ENCODE: u32 = 1;

    const COLOR_FORMAT_YUV420_SEMIPLANAR: i32 = 21;

    #[repr(C)]
    pub(crate) struct AMediaCodec {
        _private: [u8; 0],
    }

    #[repr(C)]
    pub(crate) struct AMediaFormat {
        _private: [u8; 0],
    }

    #[repr(C)]
    pub(crate) struct AMediaCodecBufferInfo {
        pub(crate) offset: i32,
        pub(crate) size: i32,
        pub(crate) presentation_time_us: i64,
        pub(crate) flags: u32,
    }

    #[link(name = "mediandk")]
    unsafe extern "C" {
        fn AMediaCodec_createDecoderByType(mime_type: *const c_char) -> *mut AMediaCodec;
        fn AMediaCodec_createEncoderByType(mime_type: *const c_char) -> *mut AMediaCodec;
        fn AMediaCodec_createCodecByName(name: *const c_char) -> *mut AMediaCodec;
        fn AMediaCodec_delete(codec: *mut AMediaCodec) -> i32;
        fn AMediaCodec_configure(
            codec: *mut AMediaCodec,
            format: *const AMediaFormat,
            surface: *mut c_void,
            crypto: *mut c_void,
            flags: u32,
        ) -> i32;
        fn AMediaCodec_start(codec: *mut AMediaCodec) -> i32;
        fn AMediaCodec_stop(codec: *mut AMediaCodec) -> i32;
        fn AMediaCodec_dequeueInputBuffer(codec: *mut AMediaCodec, timeout_us: i64) -> isize;
        fn AMediaCodec_getInputBuffer(
            codec: *mut AMediaCodec,
            idx: usize,
            out_size: *mut usize,
        ) -> *mut u8;
        fn AMediaCodec_queueInputBuffer(
            codec: *mut AMediaCodec,
            idx: usize,
            offset: usize,
            size: usize,
            time: i64,
            flags: u32,
        ) -> i32;
        fn AMediaCodec_dequeueOutputBuffer(
            codec: *mut AMediaCodec,
            info: *mut AMediaCodecBufferInfo,
            timeout_us: i64,
        ) -> isize;
        fn AMediaCodec_getOutputBuffer(
            codec: *mut AMediaCodec,
            idx: usize,
            out_size: *mut usize,
        ) -> *mut u8;
        fn AMediaCodec_releaseOutputBuffer(
            codec: *mut AMediaCodec,
            idx: usize,
            render: bool,
        ) -> i32;
        fn AMediaCodec_getOutputFormat(codec: *mut AMediaCodec) -> *mut AMediaFormat;

        fn AMediaFormat_new() -> *mut AMediaFormat;
        fn AMediaFormat_delete(format: *mut AMediaFormat) -> i32;
        fn AMediaFormat_setString(
            format: *mut AMediaFormat,
            name: *const c_char,
            value: *const c_char,
        );
        fn AMediaFormat_setInt32(format: *mut AMediaFormat, name: *const c_char, value: i32);
        fn AMediaFormat_getInt32(
            format: *mut AMediaFormat,
            name: *const c_char,
            out: *mut i32,
        ) -> bool;
        fn AMediaFormat_getBuffer(
            format: *mut AMediaFormat,
            name: *const c_char,
            out: *mut *mut c_void,
            size: *mut usize,
        ) -> bool;
    }

    #[derive(Debug)]
    pub(crate) struct MediaCodec {
        ptr: NonNull<AMediaCodec>,
    }

    impl MediaCodec {
        pub(crate) fn decoder_by_type(mime: &str) -> Result<Self, BackendError> {
            let mime = nul_terminated(mime)?;
            let ptr = unsafe { AMediaCodec_createDecoderByType(mime.as_ptr().cast()) };
            Self::from_raw(ptr, "AMediaCodec_createDecoderByType")
        }

        pub(crate) fn encoder_by_type(mime: &str) -> Result<Self, BackendError> {
            let mime = nul_terminated(mime)?;
            let ptr = unsafe { AMediaCodec_createEncoderByType(mime.as_ptr().cast()) };
            Self::from_raw(ptr, "AMediaCodec_createEncoderByType")
        }

        pub(crate) fn codec_by_name(name: &str) -> Result<Self, BackendError> {
            let name = nul_terminated(name)?;
            let ptr = unsafe { AMediaCodec_createCodecByName(name.as_ptr().cast()) };
            Self::from_raw(ptr, "AMediaCodec_createCodecByName")
        }

        fn from_raw(ptr: *mut AMediaCodec, api: &str) -> Result<Self, BackendError> {
            NonNull::new(ptr)
                .map(|ptr| Self { ptr })
                .ok_or_else(|| BackendError::Backend(format!("{api} returned null")))
        }

        pub(crate) fn configure(
            &mut self,
            format: &MediaFormat,
            encode: bool,
        ) -> Result<(), BackendError> {
            let flags = if encode { CONFIGURE_FLAG_ENCODE } else { 0 };
            status_to_result(
                unsafe {
                    AMediaCodec_configure(
                        self.ptr.as_ptr(),
                        format.ptr.as_ptr(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        flags,
                    )
                },
                "AMediaCodec_configure",
            )
        }

        pub(crate) fn start(&mut self) -> Result<(), BackendError> {
            status_to_result(
                unsafe { AMediaCodec_start(self.ptr.as_ptr()) },
                "AMediaCodec_start",
            )
        }

        pub(crate) fn stop(&mut self) {
            let _ = unsafe { AMediaCodec_stop(self.ptr.as_ptr()) };
        }

        pub(crate) fn dequeue_input(&mut self, timeout_us: i64) -> Option<usize> {
            let index = unsafe { AMediaCodec_dequeueInputBuffer(self.ptr.as_ptr(), timeout_us) };
            (index >= 0).then_some(index as usize)
        }

        pub(crate) fn input_buffer(&mut self, index: usize) -> Result<&mut [u8], BackendError> {
            let mut size = 0_usize;
            let ptr = unsafe { AMediaCodec_getInputBuffer(self.ptr.as_ptr(), index, &mut size) };
            if ptr.is_null() {
                return Err(BackendError::Backend(
                    "AMediaCodec_getInputBuffer returned null".to_string(),
                ));
            }
            Ok(unsafe { std::slice::from_raw_parts_mut(ptr, size) })
        }

        pub(crate) fn queue_input(
            &mut self,
            index: usize,
            size: usize,
            pts_us: i64,
            flags: u32,
        ) -> Result<(), BackendError> {
            status_to_result(
                unsafe {
                    AMediaCodec_queueInputBuffer(self.ptr.as_ptr(), index, 0, size, pts_us, flags)
                },
                "AMediaCodec_queueInputBuffer",
            )
        }

        pub(crate) fn dequeue_output(
            &mut self,
            timeout_us: i64,
        ) -> Result<OutputEvent, BackendError> {
            let mut info = AMediaCodecBufferInfo {
                offset: 0,
                size: 0,
                presentation_time_us: 0,
                flags: 0,
            };
            let index = unsafe {
                AMediaCodec_dequeueOutputBuffer(self.ptr.as_ptr(), &mut info, timeout_us)
            };
            match index {
                i if i >= 0 => Ok(OutputEvent::Buffer {
                    index: i as usize,
                    info,
                }),
                OUTPUT_FORMAT_CHANGED => Ok(OutputEvent::FormatChanged),
                TRY_AGAIN_LATER => Ok(OutputEvent::TryAgainLater),
                other => Err(BackendError::Backend(format!(
                    "AMediaCodec_dequeueOutputBuffer returned {other}"
                ))),
            }
        }

        pub(crate) fn output_buffer(&mut self, index: usize) -> Result<&[u8], BackendError> {
            let mut size = 0_usize;
            let ptr = unsafe { AMediaCodec_getOutputBuffer(self.ptr.as_ptr(), index, &mut size) };
            if ptr.is_null() {
                return Err(BackendError::Backend(
                    "AMediaCodec_getOutputBuffer returned null".to_string(),
                ));
            }
            Ok(unsafe { std::slice::from_raw_parts(ptr, size) })
        }

        pub(crate) fn release_output(&mut self, index: usize) {
            let _ = unsafe { AMediaCodec_releaseOutputBuffer(self.ptr.as_ptr(), index, false) };
        }

        pub(crate) fn output_format(&mut self) -> Option<MediaFormat> {
            let ptr = unsafe { AMediaCodec_getOutputFormat(self.ptr.as_ptr()) };
            NonNull::new(ptr).map(|ptr| MediaFormat { ptr })
        }
    }

    impl Drop for MediaCodec {
        fn drop(&mut self) {
            self.stop();
            let _ = unsafe { AMediaCodec_delete(self.ptr.as_ptr()) };
        }
    }

    #[derive(Debug)]
    pub(crate) struct MediaFormat {
        ptr: NonNull<AMediaFormat>,
    }

    impl MediaFormat {
        pub(crate) fn video(
            mime: &str,
            width: usize,
            height: usize,
            fps: i32,
            bitrate: Option<u32>,
            i_frame_interval_sec: Option<i32>,
            color_format: bool,
        ) -> Result<Self, BackendError> {
            let ptr = unsafe { AMediaFormat_new() };
            let mut format = NonNull::new(ptr).map(|ptr| Self { ptr }).ok_or_else(|| {
                BackendError::Backend("AMediaFormat_new returned null".to_string())
            })?;
            format.set_string("mime", mime)?;
            format.set_i32("width", checked_i32(width, "width")?);
            format.set_i32("height", checked_i32(height, "height")?);
            format.set_i32("frame-rate", fps);
            if let Some(bitrate) = bitrate {
                format.set_i32("bitrate", checked_i32(bitrate, "bitrate")?);
            }
            if let Some(interval) = i_frame_interval_sec {
                format.set_i32("i-frame-interval", interval);
            }
            if color_format {
                format.set_i32("color-format", COLOR_FORMAT_YUV420_SEMIPLANAR);
            }
            Ok(format)
        }

        pub(crate) fn set_string(&mut self, key: &str, value: &str) -> Result<(), BackendError> {
            let key = nul_terminated(key)?;
            let value = nul_terminated(value)?;
            unsafe {
                AMediaFormat_setString(
                    self.ptr.as_ptr(),
                    key.as_ptr().cast(),
                    value.as_ptr().cast(),
                );
            }
            Ok(())
        }

        pub(crate) fn set_i32(&mut self, key: &str, value: i32) {
            if let Ok(key) = nul_terminated(key) {
                unsafe { AMediaFormat_setInt32(self.ptr.as_ptr(), key.as_ptr().cast(), value) };
            }
        }

        pub(crate) fn get_i32(&mut self, key: &str) -> Option<i32> {
            let key = nul_terminated(key).ok()?;
            let mut out = 0_i32;
            unsafe { AMediaFormat_getInt32(self.ptr.as_ptr(), key.as_ptr().cast(), &mut out) }
                .then_some(out)
        }

        pub(crate) fn get_buffer(&mut self, key: &str) -> Option<Vec<u8>> {
            let key = nul_terminated(key).ok()?;
            let mut data = std::ptr::null_mut::<c_void>();
            let mut size = 0_usize;
            let ok = unsafe {
                AMediaFormat_getBuffer(self.ptr.as_ptr(), key.as_ptr().cast(), &mut data, &mut size)
            };
            if !ok || data.is_null() || size == 0 {
                return None;
            }
            Some(unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size).to_vec() })
        }
    }

    impl Drop for MediaFormat {
        fn drop(&mut self) {
            let _ = unsafe { AMediaFormat_delete(self.ptr.as_ptr()) };
        }
    }

    pub(crate) enum OutputEvent {
        Buffer {
            index: usize,
            info: AMediaCodecBufferInfo,
        },
        FormatChanged,
        TryAgainLater,
    }

    pub(crate) fn can_create_decoder(mime: &str) -> bool {
        MediaCodec::decoder_by_type(mime).is_ok()
    }

    pub(crate) fn can_create_encoder(mime: &str) -> bool {
        MediaCodec::encoder_by_type(mime).is_ok()
    }

    fn status_to_result(status: i32, api: &str) -> Result<(), BackendError> {
        if status == OK {
            Ok(())
        } else {
            Err(BackendError::Backend(format!("{api} returned {status}")))
        }
    }

    fn nul_terminated(value: &str) -> Result<Vec<u8>, BackendError> {
        if value.as_bytes().contains(&0) {
            return Err(BackendError::InvalidInput(
                "string contains interior NUL".to_string(),
            ));
        }
        let mut out = Vec::with_capacity(value.len() + 1);
        out.extend_from_slice(value.as_bytes());
        out.push(0);
        Ok(out)
    }

    fn checked_i32<T>(value: T, name: &str) -> Result<i32, BackendError>
    where
        T: TryInto<i32> + Copy + std::fmt::Display,
    {
        value
            .try_into()
            .map_err(|_| BackendError::InvalidInput(format!("{name} out of range: {value}")))
    }
}
