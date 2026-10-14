//! VideoToolbox callback boundary and native framework error adaptation.
//!
//! Callback pointer dereferences remain with the session-lifetime invariant they rely on.

use super::*;

pub(super) fn record_vt_encode_callback_error(error: &Mutex<Option<String>>, message: String) {
    if let Ok(mut slot) = error.lock()
        && slot.is_none()
    {
        *slot = Some(message);
    }
}

pub(super) fn vt_error(context: &str, status: i32) -> BackendError {
    BackendError::Backend(format!("videotoolbox({context}): {status}"))
}

pub(super) fn cm_error(context: &str, status: i32) -> BackendError {
    BackendError::Backend(format!("coremedia({context}): {status}"))
}

pub(super) fn cv_error(context: &str, status: i32) -> BackendError {
    BackendError::Backend(format!("corevideo({context}): {status}"))
}

pub(super) extern "C" fn vt_decode_output_callback(
    decompression_output_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: i32,
    info_flags: video_toolbox::errors::VTDecodeInfoFlags,
    image_buffer: core_video::image_buffer::CVImageBufferRef,
    presentation_time_stamp: CMTime,
    _presentation_duration: CMTime,
) {
    if decompression_output_ref_con.is_null() {
        return;
    }

    // SAFETY: VtDecoderSession owns the boxed mutex for at least as long as its
    // decompression session can invoke this callback.
    let state = unsafe { &*(decompression_output_ref_con as *const Mutex<DecodeOutputState>) };
    if status != 0 || image_buffer.is_null() {
        if let Ok(mut state) = state.lock()
            && state.callback_error.is_none()
        {
            state.callback_error = Some(if status != 0 {
                format!("VideoToolbox decode callback failed with status {status}")
            } else {
                "VideoToolbox decode callback returned a null image buffer".to_string()
            });
        }
        return;
    }
    // SAFETY: VideoToolbox guarantees a valid image buffer for the duration of
    // a successful callback; the wrapper retains it under CoreFoundation rules.
    let pixel_buffer = unsafe { CVPixelBuffer::wrap_under_get_rule(image_buffer) };

    if let Ok(mut s) = state.lock() {
        let width = pixel_buffer.get_width();
        let height = pixel_buffer.get_height();
        let pixel_format = pixel_buffer.get_pixel_format();
        let color = extract_color_metadata(&pixel_buffer);
        let argb = if s.capture_argb {
            copy_argb_payload_from_pixel_buffer(&pixel_buffer, pixel_format, width, height)
        } else {
            None
        };
        if s.capture_argb && argb.is_none() {
            if s.callback_error.is_none() {
                s.callback_error = Some(format!(
                    "failed to copy BGRA decode output: format={pixel_format:#010x}, size={width}x{height}"
                ));
            }
            return;
        }
        let frame = Frame {
            width,
            height,
            pixel_format: Some(pixel_format),
            pts_90k: cm_time_to_90k(presentation_time_stamp),
            decode_info_flags: Some(info_flags.bits()),
            color_primaries: color.color_primaries,
            transfer_function: color.transfer_function,
            ycbcr_matrix: color.ycbcr_matrix,
            argb,
            rgb24: None,
            nv12: None,
            force_keyframe: false,
        };
        s.decoded_frames = s.decoded_frames.saturating_add(1);
        if s.width.is_none() {
            s.width = Some(width);
        }
        if s.height.is_none() {
            s.height = Some(height);
        }
        if s.pixel_format.is_none() {
            s.pixel_format = Some(pixel_format);
        }
        s.pending_frames.push_back(frame);
    }
}
