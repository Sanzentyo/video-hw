use std::collections::VecDeque;
use std::ffi::{c_int, c_longlong, c_ulong, c_void};
use std::ptr;
use std::sync::{Arc, Mutex};

use cudarc::driver::CudaContext;
use cudarc::driver::sys::CUresult;
use nvidia_video_codec_sdk::DecodeCodec;
use nvidia_video_codec_sdk::sys::cuviddec::{
    CUVIDDECODECAPS, CUVIDDECODECREATEINFO, CUVIDPICPARAMS, CUVIDRECONFIGUREDECODERINFO,
    CUvideodecoder, cudaVideoChromaFormat, cudaVideoCodec, cudaVideoCreateFlags,
    cudaVideoDeinterlaceMode, cudaVideoSurfaceFormat, cuvidCreateDecoder, cuvidDecodePicture,
    cuvidDestroyDecoder, cuvidGetDecoderCaps, cuvidReconfigureDecoder,
};
use nvidia_video_codec_sdk::sys::nvcuvid::{
    CUVIDEOFORMAT, CUVIDPARSERDISPINFO, CUVIDPARSERPARAMS, CUVIDSOURCEDATAPACKET,
    CUvideopacketflags, CUvideoparser, cuvidCreateVideoParser, cuvidDestroyVideoParser,
    cuvidParseVideoData,
};

use crate::{BackendError, Frame};

#[derive(Debug)]
pub struct NvMetaDecoder {
    ctx: Arc<CudaContext>,
    parser: CUvideoparser,
    bridge: Box<MetaCallbackBridge>,
}

impl NvMetaDecoder {
    pub fn new(ctx: Arc<CudaContext>, codec: DecodeCodec) -> Result<Self, BackendError> {
        ctx.bind_to_thread().map_err(map_cuda_error)?;
        check_decoder_caps(codec)?;

        let mut bridge = Box::new(MetaCallbackBridge {
            codec,
            state: Mutex::new(MetaDecoderState::default()),
        });
        let bridge_ptr = ptr::from_mut(bridge.as_mut()).cast::<c_void>();

        let mut parser_params = CUVIDPARSERPARAMS {
            CodecType: to_cuda_codec(codec),
            ulMaxNumDecodeSurfaces: 1,
            ulClockRate: 90_000,
            ulErrorThreshold: 0,
            ulMaxDisplayDelay: 0,
            pUserData: bridge_ptr,
            pfnSequenceCallback: Some(sequence_callback),
            pfnDecodePicture: Some(decode_callback),
            pfnDisplayPicture: Some(display_callback),
            ..Default::default()
        };

        let mut parser = ptr::null_mut();
        check_nvdec(
            unsafe { cuvidCreateVideoParser(&mut parser, &mut parser_params) },
            "cuvidCreateVideoParser",
        )?;

        Ok(Self {
            ctx,
            parser,
            bridge,
        })
    }

    pub fn push_access_unit(
        &mut self,
        access_unit: &[u8],
        timestamp_90k: i64,
    ) -> Result<Vec<Frame>, BackendError> {
        if access_unit.is_empty() {
            return Err(BackendError::InvalidInput(
                "access unit must not be empty".to_string(),
            ));
        }
        self.ctx.bind_to_thread().map_err(map_cuda_error)?;
        self.ensure_no_callback_error()?;

        let payload_size = c_ulong::try_from(access_unit.len()).map_err(|_| {
            BackendError::InvalidInput("access unit size does not fit into c_ulong".to_string())
        })?;
        let flags = (CUvideopacketflags::CUVID_PKT_TIMESTAMP as c_ulong)
            | (CUvideopacketflags::CUVID_PKT_ENDOFPICTURE as c_ulong);
        let mut packet = CUVIDSOURCEDATAPACKET {
            flags,
            payload_size,
            payload: access_unit.as_ptr(),
            timestamp: timestamp_90k as c_longlong,
        };
        check_nvdec(
            unsafe { cuvidParseVideoData(self.parser, &mut packet) },
            "cuvidParseVideoData",
        )?;
        self.ensure_no_callback_error()?;

        self.drain_display_queue()
    }

    pub fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        self.ctx.bind_to_thread().map_err(map_cuda_error)?;
        self.ensure_no_callback_error()?;

        let flags = (CUvideopacketflags::CUVID_PKT_ENDOFSTREAM as c_ulong)
            | (CUvideopacketflags::CUVID_PKT_NOTIFY_EOS as c_ulong);
        let mut packet = CUVIDSOURCEDATAPACKET {
            flags,
            payload_size: 0,
            payload: ptr::null(),
            timestamp: 0,
        };
        check_nvdec(
            unsafe { cuvidParseVideoData(self.parser, &mut packet) },
            "cuvidParseVideoData",
        )?;
        self.ensure_no_callback_error()?;

        self.drain_display_queue()
    }

    fn ensure_no_callback_error(&self) -> Result<(), BackendError> {
        let state = lock_state(&self.bridge.state);
        match &state.sticky_error {
            Some(err) => Err(BackendError::Backend(err.clone())),
            None => Ok(()),
        }
    }

    fn drain_display_queue(&mut self) -> Result<Vec<Frame>, BackendError> {
        self.ctx.bind_to_thread().map_err(map_cuda_error)?;
        let mut out = Vec::new();
        loop {
            let (entry, width, height) = {
                let mut state = lock_state(&self.bridge.state);
                if let Some(err) = &state.sticky_error {
                    return Err(BackendError::Backend(err.clone()));
                }
                let Some(entry) = state.display_queue.pop_front() else {
                    break;
                };
                let width = state.width;
                let height = state.height;
                (entry, width, height)
            };
            out.push(Frame {
                width: width as usize,
                height: height as usize,
                pixel_format: None,
                pts_90k: Some(entry.timestamp),
                argb: None,
                force_keyframe: false,
            });
        }
        self.ensure_no_callback_error()?;
        Ok(out)
    }
}

impl Drop for NvMetaDecoder {
    fn drop(&mut self) {
        let _ = self.ctx.bind_to_thread();
        if !self.parser.is_null() {
            let _ = unsafe { cuvidDestroyVideoParser(self.parser) };
            self.parser = ptr::null_mut();
        }

        let decoder = {
            let mut state = lock_state(&self.bridge.state);
            state.decoder.take()
        };
        if let Some(decoder) = decoder {
            let _ = unsafe { cuvidDestroyDecoder(decoder) };
        }
    }
}

#[derive(Debug)]
struct MetaCallbackBridge {
    codec: DecodeCodec,
    state: Mutex<MetaDecoderState>,
}

#[derive(Debug, Clone, Copy, Default)]
struct DisplayQueueEntry {
    timestamp: i64,
}

#[derive(Debug, Default)]
struct MetaDecoderState {
    decoder: Option<CUvideodecoder>,
    sticky_error: Option<String>,
    display_queue: VecDeque<DisplayQueueEntry>,
    width: u32,
    height: u32,
}

impl MetaDecoderState {
    fn set_error_once(&mut self, message: String) {
        if self.sticky_error.is_none() {
            self.sticky_error = Some(message);
        }
    }

    fn configure_decoder(
        &mut self,
        codec: DecodeCodec,
        format: &CUVIDEOFORMAT,
    ) -> Result<c_int, String> {
        if format.bit_depth_luma_minus8 != 0 || format.bit_depth_chroma_minus8 != 0 {
            return Err("only 8-bit decode is supported".to_string());
        }
        if format.chroma_format != cudaVideoChromaFormat::cudaVideoChromaFormat_420 {
            return Err("only 4:2:0 decode is supported".to_string());
        }
        if format.coded_width == 0 || format.coded_height == 0 {
            return Err("decoder reported zero dimensions".to_string());
        }

        let num_surfaces = u32::from(format.min_num_decode_surfaces.max(1));
        let rect = resolve_target_rect(format);
        let target_width = rect.2.saturating_sub(rect.0) as u32;
        let target_height = rect.3.saturating_sub(rect.1) as u32;

        if let Some(decoder) = self.decoder {
            let mut reconfigure = CUVIDRECONFIGUREDECODERINFO {
                ulWidth: format.coded_width,
                ulHeight: format.coded_height,
                ulTargetWidth: target_width,
                ulTargetHeight: target_height,
                ulNumDecodeSurfaces: num_surfaces,
                display_area: to_reconfigure_rect(rect),
                target_rect: to_reconfigure_target_rect(rect),
                ..Default::default()
            };
            check_nvdec(
                unsafe { cuvidReconfigureDecoder(decoder, &mut reconfigure) },
                "cuvidReconfigureDecoder",
            )
            .map_err(|e| e.to_string())?;
        } else {
            let mut create_info = CUVIDDECODECREATEINFO {
                ulWidth: format.coded_width as c_ulong,
                ulHeight: format.coded_height as c_ulong,
                ulNumDecodeSurfaces: num_surfaces as c_ulong,
                CodecType: to_cuda_codec(codec),
                ChromaFormat: format.chroma_format,
                ulCreationFlags: cudaVideoCreateFlags::cudaVideoCreate_PreferCUVID as c_ulong,
                bitDepthMinus8: format.bit_depth_luma_minus8 as c_ulong,
                ulIntraDecodeOnly: 0,
                ulMaxWidth: format.coded_width as c_ulong,
                ulMaxHeight: format.coded_height as c_ulong,
                display_area: to_create_rect(rect),
                OutputFormat: cudaVideoSurfaceFormat::cudaVideoSurfaceFormat_NV12,
                DeinterlaceMode: cudaVideoDeinterlaceMode::cudaVideoDeinterlaceMode_Weave,
                ulTargetWidth: target_width as c_ulong,
                ulTargetHeight: target_height as c_ulong,
                ulNumOutputSurfaces: 2,
                vidLock: ptr::null_mut(),
                target_rect: to_create_target_rect(rect),
                enableHistogram: 0,
                ..Default::default()
            };
            let mut decoder = ptr::null_mut();
            check_nvdec(
                unsafe { cuvidCreateDecoder(&mut decoder, &mut create_info) },
                "cuvidCreateDecoder",
            )
            .map_err(|e| e.to_string())?;
            self.decoder = Some(decoder);
        }

        self.width = target_width;
        self.height = target_height;
        Ok(num_surfaces as c_int)
    }
}

unsafe extern "C" fn sequence_callback(
    user_data: *mut c_void,
    format: *mut CUVIDEOFORMAT,
) -> c_int {
    let Some(bridge) = bridge_from_user_data(user_data) else {
        return 0;
    };
    if format.is_null() {
        let mut state = lock_state(&bridge.state);
        state.set_error_once("null CUVIDEOFORMAT in sequence callback".to_string());
        return 0;
    }

    let mut state = lock_state(&bridge.state);
    let result = state.configure_decoder(bridge.codec, unsafe { &*format });
    match result {
        Ok(surfaces) => surfaces,
        Err(message) => {
            state.set_error_once(message);
            0
        }
    }
}

unsafe extern "C" fn decode_callback(
    user_data: *mut c_void,
    pic_params: *mut CUVIDPICPARAMS,
) -> c_int {
    let Some(bridge) = bridge_from_user_data(user_data) else {
        return 0;
    };
    if pic_params.is_null() {
        let mut state = lock_state(&bridge.state);
        state.set_error_once("null CUVIDPICPARAMS in decode callback".to_string());
        return 0;
    }

    let mut state = lock_state(&bridge.state);
    let Some(decoder) = state.decoder else {
        state.set_error_once("decode callback before decoder init".to_string());
        return 0;
    };

    match check_nvdec(
        unsafe { cuvidDecodePicture(decoder, pic_params) },
        "cuvidDecodePicture",
    ) {
        Ok(()) => 1,
        Err(err) => {
            state.set_error_once(err.to_string());
            0
        }
    }
}

unsafe extern "C" fn display_callback(
    user_data: *mut c_void,
    display_info: *mut CUVIDPARSERDISPINFO,
) -> c_int {
    let Some(bridge) = bridge_from_user_data(user_data) else {
        return 0;
    };
    if display_info.is_null() {
        return 1;
    }
    let info = unsafe { &*display_info };
    let mut state = lock_state(&bridge.state);
    state.display_queue.push_back(DisplayQueueEntry {
        timestamp: info.timestamp,
    });
    1
}

fn check_decoder_caps(codec: DecodeCodec) -> Result<(), BackendError> {
    let mut caps = CUVIDDECODECAPS {
        eCodecType: to_cuda_codec(codec),
        eChromaFormat: cudaVideoChromaFormat::cudaVideoChromaFormat_420,
        nBitDepthMinus8: 0,
        ..Default::default()
    };
    check_nvdec(
        unsafe { cuvidGetDecoderCaps(&mut caps) },
        "cuvidGetDecoderCaps",
    )?;
    if caps.bIsSupported == 0 {
        return Err(BackendError::UnsupportedConfig(format!(
            "{codec:?} decoder is not supported by this GPU"
        )));
    }
    let nv12_mask = 1_u16 << (cudaVideoSurfaceFormat::cudaVideoSurfaceFormat_NV12 as u32);
    if (caps.nOutputFormatMask & nv12_mask) == 0 {
        return Err(BackendError::UnsupportedConfig(
            "NV12 output is not supported by NVDEC".to_string(),
        ));
    }
    Ok(())
}

fn check_nvdec(status: CUresult, operation: &'static str) -> Result<(), BackendError> {
    status
        .result()
        .map_err(|err| BackendError::Backend(format!("{operation} failed: {err:?}")))
}

fn map_cuda_error(err: cudarc::driver::DriverError) -> BackendError {
    BackendError::UnsupportedConfig(format!("failed to bind CUDA context: {err}"))
}

fn to_cuda_codec(codec: DecodeCodec) -> cudaVideoCodec {
    match codec {
        DecodeCodec::H264 => cudaVideoCodec::cudaVideoCodec_H264,
        DecodeCodec::H265 => cudaVideoCodec::cudaVideoCodec_HEVC,
        DecodeCodec::Av1 => cudaVideoCodec::cudaVideoCodec_AV1,
    }
}

fn resolve_target_rect(format: &CUVIDEOFORMAT) -> (i32, i32, i32, i32) {
    let left = format.display_area.left.max(0);
    let top = format.display_area.top.max(0);
    let mut right = format.display_area.right.max(0);
    let mut bottom = format.display_area.bottom.max(0);
    if right == 0 || right > format.coded_width as i32 {
        right = format.coded_width as i32;
    }
    if bottom == 0 || bottom > format.coded_height as i32 {
        bottom = format.coded_height as i32;
    }
    if right <= left || bottom <= top {
        return (0, 0, format.coded_width as i32, format.coded_height as i32);
    }
    (left, top, right, bottom)
}

fn to_create_rect(
    (left, top, right, bottom): (i32, i32, i32, i32),
) -> nvidia_video_codec_sdk::sys::cuviddec::_CUVIDDECODECREATEINFO__bindgen_ty_1 {
    nvidia_video_codec_sdk::sys::cuviddec::_CUVIDDECODECREATEINFO__bindgen_ty_1 {
        left: to_c_short(left),
        top: to_c_short(top),
        right: to_c_short(right),
        bottom: to_c_short(bottom),
    }
}

fn to_create_target_rect(
    (left, top, right, bottom): (i32, i32, i32, i32),
) -> nvidia_video_codec_sdk::sys::cuviddec::_CUVIDDECODECREATEINFO__bindgen_ty_2 {
    nvidia_video_codec_sdk::sys::cuviddec::_CUVIDDECODECREATEINFO__bindgen_ty_2 {
        left: to_c_short(left),
        top: to_c_short(top),
        right: to_c_short(right),
        bottom: to_c_short(bottom),
    }
}

fn to_reconfigure_rect(
    (left, top, right, bottom): (i32, i32, i32, i32),
) -> nvidia_video_codec_sdk::sys::cuviddec::_CUVIDRECONFIGUREDECODERINFO__bindgen_ty_1 {
    nvidia_video_codec_sdk::sys::cuviddec::_CUVIDRECONFIGUREDECODERINFO__bindgen_ty_1 {
        left: to_c_short(left),
        top: to_c_short(top),
        right: to_c_short(right),
        bottom: to_c_short(bottom),
    }
}

fn to_reconfigure_target_rect(
    (left, top, right, bottom): (i32, i32, i32, i32),
) -> nvidia_video_codec_sdk::sys::cuviddec::_CUVIDRECONFIGUREDECODERINFO__bindgen_ty_2 {
    nvidia_video_codec_sdk::sys::cuviddec::_CUVIDRECONFIGUREDECODERINFO__bindgen_ty_2 {
        left: to_c_short(left),
        top: to_c_short(top),
        right: to_c_short(right),
        bottom: to_c_short(bottom),
    }
}

fn to_c_short(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn bridge_from_user_data(user_data: *mut c_void) -> Option<&'static MetaCallbackBridge> {
    if user_data.is_null() {
        None
    } else {
        Some(unsafe { &*user_data.cast::<MetaCallbackBridge>() })
    }
}

fn lock_state<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
