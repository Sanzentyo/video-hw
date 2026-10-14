//! Intel oneVPL encoder adapter, validation, and packet collection.

use super::*;

pub struct IntelEncoderAdapter {
    codec: Codec,
    fps: i32,
    require_hardware: bool,
    input_format: EncodeInputFormat,
    options: IntelEncoderOptions,
    pub(super) pending_frames: Vec<Frame>,
    pub(super) width: Option<usize>,
    pub(super) height: Option<usize>,
}

impl IntelEncoderAdapter {
    pub fn new(config: EncoderConfig) -> Self {
        let EncoderConfig {
            codec,
            fps,
            require_hardware,
            input_format,
            backend_options,
        } = config;
        let options = match backend_options {
            BackendEncoderOptions::Intel(options) => options,
            BackendEncoderOptions::Default
            | BackendEncoderOptions::VideoToolbox(_)
            | BackendEncoderOptions::Nvidia(_)
            | BackendEncoderOptions::Vulkan(_)
            | BackendEncoderOptions::Android(_) => IntelEncoderOptions::default(),
        };
        Self {
            codec,
            fps,
            require_hardware,
            input_format,
            options,
            pending_frames: Vec::new(),
            width: None,
            height: None,
        }
    }

    fn encode_pending_frames(
        &self,
        pending_frames: &[Frame],
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        let runtime = RuntimeBuilder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|err| {
                BackendError::UnsupportedConfig(format!("failed to create Tokio runtime: {err}"))
            })?;

        let width = self.width.ok_or_else(|| {
            BackendError::Backend("pending Intel frames have no recorded width".to_string())
        })?;
        let height = self.height.ok_or_else(|| {
            BackendError::Backend("pending Intel frames have no recorded height".to_string())
        })?;
        let codec = self.codec;
        let fps = self.fps;
        let input_format = self.input_format;
        let options = self.options.clone();
        let require_hardware = self.require_hardware;

        runtime.block_on(async move {
            let attempt = |use_hardware| OneVplEncodeAttempt {
                width,
                height,
                codec,
                fps,
                input_format,
                options: &options,
                use_hardware,
            };
            if require_hardware && options.force_software {
                return Err(BackendError::UnsupportedConfig(
                    "Intel encode cannot use require_hardware=true together with IntelEncoderOptions.force_software=true".to_string(),
                ));
            }
            if options.force_software {
                return encode_with_onevpl(pending_frames, attempt(false)).await;
            }
            let hardware_attempt = encode_with_onevpl(pending_frames, attempt(true)).await;
            if require_hardware {
                return hardware_attempt;
            }
            match hardware_attempt {
                Ok(packets) => Ok(packets),
                Err(_) => {
                    encode_with_onevpl(pending_frames, attempt(false)).await
                }
            }
        })
    }
}

impl VideoEncoder for IntelEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(intel_capability_report(codec))
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        validate_intel_encode_frame(&frame, self.input_format)?;
        if let Some(width) = self.width {
            if width != frame.width {
                return Err(BackendError::InvalidInput(
                    "all frames in one flush cycle must have the same width".to_string(),
                ));
            }
        } else {
            self.width = Some(frame.width);
        }
        if let Some(height) = self.height {
            if height != frame.height {
                return Err(BackendError::InvalidInput(
                    "all frames in one flush cycle must have the same height".to_string(),
                ));
            }
        } else {
            self.height = Some(frame.height);
        }
        self.pending_frames.push(frame);
        Ok(Vec::new())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        if self.pending_frames.is_empty() {
            return Ok(Vec::new());
        }
        let pending_frames = std::mem::take(&mut self.pending_frames);
        match self.encode_pending_frames(&pending_frames) {
            Ok(packets) => {
                self.width = None;
                self.height = None;
                Ok(packets)
            }
            Err(err) => {
                self.pending_frames = pending_frames;
                Err(err)
            }
        }
    }

    fn try_reap(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        Ok(Vec::new())
    }

    fn request_session_switch(
        &mut self,
        _request: SessionSwitchRequest,
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "session switching is not supported by Intel backend".to_string(),
        ))
    }
}

fn validate_intel_encode_frame(
    frame: &Frame,
    input_format: EncodeInputFormat,
) -> Result<(), BackendError> {
    if frame.width == 0 || frame.height == 0 {
        return Err(BackendError::InvalidInput(
            "frame dimensions must be positive".to_string(),
        ));
    }
    if u16::try_from(frame.width).is_err() || u16::try_from(frame.height).is_err() {
        return Err(BackendError::InvalidInput(format!(
            "frame dimensions exceed oneVPL limits: {}x{}",
            frame.width, frame.height
        )));
    }
    if !frame.width.is_multiple_of(2) || !frame.height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(format!(
            "Intel encode requires even dimensions, got {}x{}",
            frame.width, frame.height
        )));
    }

    match input_format {
        EncodeInputFormat::Argb8888 => {
            let argb = frame.argb.as_deref().ok_or_else(|| {
                BackendError::InvalidInput(
                    "Intel backend requires ARGB payload for the configured input format"
                        .to_string(),
                )
            })?;
            let expected = frame
                .width
                .checked_mul(frame.height)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| {
                    BackendError::InvalidInput("ARGB frame size overflows usize".to_string())
                })?;
            if argb.len() != expected {
                return Err(BackendError::InvalidInput(format!(
                    "argb payload size mismatch: expected {expected}, got {}",
                    argb.len()
                )));
            }
        }
        EncodeInputFormat::Nv12 => {
            let nv12 = frame.nv12.as_ref().ok_or_else(|| {
                BackendError::InvalidInput(
                    "Intel backend requires NV12 payload for the configured input format"
                        .to_string(),
                )
            })?;
            let dims = Dimensions::try_from((frame.width, frame.height))
                .map_err(|error| BackendError::InvalidInput(error.to_string()))?;
            nv12.validate(dims)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct OneVplEncodeAttempt<'a> {
    width: usize,
    height: usize,
    codec: Codec,
    fps: i32,
    input_format: EncodeInputFormat,
    options: &'a IntelEncoderOptions,
    use_hardware: bool,
}

async fn encode_with_onevpl(
    pending_frames: &[Frame],
    attempt: OneVplEncodeAttempt<'_>,
) -> Result<Vec<EncodedPacket>, BackendError> {
    let OneVplEncodeAttempt {
        width,
        height,
        codec,
        fps,
        input_format,
        options,
        use_hardware,
    } = attempt;
    let width_u16 = u16::try_from(width).map_err(|_| {
        BackendError::InvalidInput(format!("frame width {} exceeds oneVPL limits", width))
    })?;
    let height_u16 = u16::try_from(height).map_err(|_| {
        BackendError::InvalidInput(format!("frame height {} exceeds oneVPL limits", height))
    })?;
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "frame dimensions must be positive".to_string(),
        ));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(
            "Intel backend currently requires even frame dimensions".to_string(),
        ));
    }
    let use_nv12_input = input_format == EncodeInputFormat::Nv12;

    let mut loader = configured_onevpl_loader(codec, OneVplOperation::Encode, use_hardware)?;

    let session = loader
        .new_session(0)
        .map_err(|status| map_onevpl_status(status, "Loader::new_session"))?;

    let fps_numerator = u32::try_from(fps.max(1)).unwrap_or(30);
    let target_kbps = options
        .target_kbps
        .unwrap_or_else(|| default_target_kbps(width, height, fps_numerator));
    let input_pic_struct = PicStruct::Progressive;
    let default_rate_control_method = default_rate_control_method(codec);
    let rate_control_method = std::env::var("VIDEO_HW_INTEL_RATE_CONTROL")
        .ok()
        .and_then(|raw| parse_rate_control_method(&raw))
        .unwrap_or(default_rate_control_method);
    let hevc_low_power =
        codec == Codec::Hevc && env_bool("VIDEO_HW_INTEL_HEVC_LOW_POWER").unwrap_or(true);
    let async_depth = std::env::var("VIDEO_HW_INTEL_ASYNC_DEPTH")
        .ok()
        .and_then(|raw| raw.parse::<u16>().ok())
        .filter(|depth| (1..=16).contains(depth))
        .unwrap_or(10);
    let hw_width = onevpl::utils::hw_align_width(width_u16);
    let hw_height = onevpl::utils::hw_align_height(height_u16, input_pic_struct);
    let mut encode_params = onevpl::MfxVideoParams::default();
    encode_params.set_codec(to_onevpl_codec(codec));
    encode_params.set_target_usage(TargetUsage::Level7);
    encode_params.set_rate_control_method(rate_control_method);
    encode_params.set_target_kbps(target_kbps);
    encode_params.set_framerate(fps_numerator, 1);
    encode_params.set_async_depth(async_depth);
    encode_params.set_gop_ref_dist(1);
    encode_params.set_num_ref_frame(1);
    encode_params.set_chroma_format(ChromaFormat::YUV420);
    if matches!(rate_control_method, RateControlMethod::CQP) {
        let qp = std::env::var("VIDEO_HW_INTEL_CQP")
            .ok()
            .and_then(|raw| raw.parse::<u16>().ok())
            .map(|value| value.clamp(1, 51))
            .unwrap_or(24);
        encode_params.set_qpi(qp);
        encode_params.set_qpp(qp);
    }
    {
        // Some runtimes reject AVC init unless FrameInfo.PicStruct is explicitly set.
        let raw = &mut **encode_params;
        raw.__bindgen_anon_1.mfx.FrameInfo.PicStruct = input_pic_struct.repr() as u16;
        if codec == Codec::Hevc {
            raw.__bindgen_anon_1.mfx.LowPower = if hevc_low_power {
                CodingOptionValue::On.repr() as u16
            } else {
                CodingOptionValue::Off.repr() as u16
            };
        }
    }
    if let Some(gop_length) = options.gop_length {
        encode_params.set_gop_pic_size(gop_length);
    }

    let mut vpp = None;
    let mut vpp_input_fourcc = None;
    let mut hw_direct_rgb4 = false;
    let mut hevc_cpu_nv12 = false;
    if use_hardware {
        let hevc_direct_rgb4 =
            codec == Codec::Hevc && env_bool("VIDEO_HW_INTEL_HEVC_DIRECT_RGB4").unwrap_or(false);
        let hevc_force_vpp = codec == Codec::Hevc
            && options
                .hevc_use_vpp
                .unwrap_or_else(|| env_bool("VIDEO_HW_INTEL_HEVC_USE_VPP").unwrap_or(false));
        if use_nv12_input || (codec == Codec::Hevc && !hevc_direct_rgb4 && !hevc_force_vpp) {
            encode_params.set_fourcc(FourCC::NV12);
            encode_params.set_io_pattern(IoPattern::IN_SYSTEM_MEMORY);
            encode_params.set_height(hw_height);
            encode_params.set_width(hw_width);
            encode_params.set_crop(width_u16, height_u16);
            hevc_cpu_nv12 = codec == Codec::Hevc && !use_nv12_input;
        } else if codec == Codec::H264 || hevc_direct_rgb4 {
            encode_params.set_fourcc(FourCC::Rgb4OrBgra);
            encode_params.set_io_pattern(IoPattern::IN_SYSTEM_MEMORY);
            encode_params.set_height(hw_height);
            encode_params.set_width(hw_width);
            encode_params.set_crop(width_u16, height_u16);
            hw_direct_rgb4 = true;
        } else {
            encode_params.set_fourcc(FourCC::NV12);
            encode_params.set_io_pattern(IoPattern::IN_VIDEO_MEMORY);
            encode_params.set_height(hw_height);
            encode_params.set_width(hw_width);
            encode_params.set_crop(width_u16, height_u16);

            let build_vpp_params = |in_fourcc: FourCC| {
                let mut vpp_params = VppVideoParams::default();
                vpp_params
                    .set_io_pattern(IoPattern::IN_SYSTEM_MEMORY | IoPattern::OUT_VIDEO_MEMORY);
                vpp_params.set_in_fourcc(in_fourcc);
                vpp_params.set_in_picstruct(input_pic_struct);
                vpp_params.set_in_height(hw_height);
                vpp_params.set_in_width(hw_width);
                vpp_params.set_in_crop(0, 0, width_u16, height_u16);
                vpp_params.set_in_framerate(fps_numerator, 1);
                vpp_params.set_out_fourcc(FourCC::NV12);
                vpp_params.set_out_picstruct(PicStruct::Progressive);
                vpp_params.set_out_height(hw_height);
                vpp_params.set_out_width(hw_width);
                vpp_params.set_out_crop(0, 0, width_u16, height_u16);
                vpp_params.set_out_framerate(fps_numerator, 1);
                vpp_params
            };

            let mut bgra_vpp_params = build_vpp_params(FourCC::Rgb4OrBgra);
            match session.video_processor(&mut bgra_vpp_params) {
                Ok(processor) => {
                    vpp = Some(processor);
                    vpp_input_fourcc = Some(FourCC::Rgb4OrBgra);
                }
                Err(
                    MfxStatus::Unsupported
                    | MfxStatus::NotImplemented
                    | MfxStatus::IncompatibleVideoParam,
                ) => {
                    let mut yv12_vpp_params = build_vpp_params(FourCC::YV12);
                    match session.video_processor(&mut yv12_vpp_params) {
                        Ok(processor) => {
                            vpp = Some(processor);
                            vpp_input_fourcc = Some(FourCC::YV12);
                        }
                        Err(
                            MfxStatus::Unsupported
                            | MfxStatus::NotImplemented
                            | MfxStatus::IncompatibleVideoParam,
                        ) => {
                            return Err(BackendError::UnsupportedConfig(
                                "Intel hardware encode requires oneVPL VPP (BGRA/YV12 -> NV12), but this runtime does not expose a compatible VPP path".to_string(),
                            ));
                        }
                        Err(status) => {
                            return Err(map_onevpl_status(
                                status,
                                "Session::video_processor (YV12 fallback)",
                            ));
                        }
                    }
                }
                Err(status) => {
                    return Err(map_onevpl_status(status, "Session::video_processor"));
                }
            }
        }
    } else {
        if use_nv12_input {
            encode_params.set_fourcc(FourCC::NV12);
        } else {
            encode_params.set_fourcc(FourCC::IyuvOrI420);
        }
        encode_params.set_io_pattern(IoPattern::IN_SYSTEM_MEMORY);
        encode_params.set_height(height_u16);
        encode_params.set_width(width_u16);
        encode_params.set_crop(width_u16, height_u16);
    }

    let mut encoder = match session.encoder(encode_params) {
        Ok(encoder) => encoder,
        Err(MfxStatus::InvalidVideoParam) if use_hardware => {
            return Err(BackendError::UnsupportedConfig(format!(
                "Intel hardware encoder rejected {} parameters (Session::encoder: InvalidVideoParam); this runtime may not expose hardware encode for the requested codec/profile",
                codec
            )));
        }
        Err(status) => return Err(map_onevpl_status(status, "Session::encoder")),
    };
    let encoder_params = encoder
        .params()
        .map_err(|status| map_onevpl_status(status, "Encoder::params"))?;

    let min_bitstream_capacity = if codec == Codec::Hevc {
        2 * 1024 * 1024
    } else {
        512 * 1024
    };
    let buffer_size = encoder_params
        .suggested_buffer_size()
        .max(min_bitstream_capacity);
    let mut backing_buffer = vec![0_u8; buffer_size];
    let mut bitstream = Bitstream::with_codec(&mut backing_buffer, to_onevpl_codec(codec));
    let mut packets = Vec::new();
    let mut pending_pts = VecDeque::new();
    let aligned_width = usize::from(hw_width);
    let aligned_height = usize::from(hw_height);

    for frame in pending_frames {
        let mut ctrl = EncodeCtrl::new();
        if frame.force_keyframe {
            ctrl.set_frame_type(FrameType::I | FrameType::IDR | FrameType::REF);
        }

        pending_pts.push_back(frame.pts_90k);
        if use_nv12_input {
            let nv12 = frame.nv12.as_ref().ok_or_else(|| {
                BackendError::InvalidInput("Intel backend requires NV12 frame payload".to_string())
            })?;
            let mut input_surface = encoder
                .get_surface()
                .map_err(|status| map_onevpl_status(status, "Encoder::get_surface"))?;
            write_nv12_to_surface(&mut input_surface, nv12, width, height)?;
            match encoder
                .encode(&mut ctrl, Some(input_surface), &mut bitstream, None)
                .await
            {
                Ok(_) => collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?,
                Err(MfxStatus::MoreData) => {}
                Err(status) => {
                    return Err(map_onevpl_status(status, "Encoder::encode (nv12-input)"));
                }
            }
            continue;
        }
        if hevc_cpu_nv12 {
            let argb = frame.argb.as_deref().ok_or_else(|| {
                BackendError::InvalidInput("Intel backend requires ARGB frame payload".to_string())
            })?;
            let mut input_surface = encoder
                .get_surface()
                .map_err(|status| map_onevpl_status(status, "Encoder::get_surface"))?;
            write_argb_to_nv12_surface(&mut input_surface, argb, width, height)?;
            match encoder
                .encode(&mut ctrl, Some(input_surface), &mut bitstream, None)
                .await
            {
                Ok(_) => collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?,
                Err(MfxStatus::MoreData) => {}
                Err(status) => {
                    return Err(map_onevpl_status(
                        status,
                        "Encoder::encode (hardware hevc-nv12)",
                    ));
                }
            }
            continue;
        }
        let argb = frame.argb.as_deref().ok_or_else(|| {
            BackendError::InvalidInput("Intel backend requires ARGB frame payload".to_string())
        })?;
        if hw_direct_rgb4 {
            let mut input_surface = encoder
                .get_surface()
                .map_err(|status| map_onevpl_status(status, "Encoder::get_surface"))?;
            write_argb_to_bgra_surface(&mut input_surface, argb, width, height)?;
            match encoder
                .encode(&mut ctrl, Some(input_surface), &mut bitstream, None)
                .await
            {
                Ok(_) => collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?,
                Err(MfxStatus::MoreData) => {}
                Err(status) => {
                    return Err(map_onevpl_status(
                        status,
                        "Encoder::encode (hardware direct-rgb4)",
                    ));
                }
            }
            continue;
        }
        match vpp_input_fourcc {
            Some(FourCC::Rgb4OrBgra) => {
                let mut input_surface = vpp
                    .as_mut()
                    .ok_or_else(|| BackendError::Backend("VPP should be initialized".to_string()))?
                    .get_surface_input()
                    .map_err(|status| map_onevpl_status(status, "VPP::get_surface_input"))?;
                write_argb_to_bgra_surface(&mut input_surface, argb, width, height)?;
                let vpp_frame = vpp
                    .as_ref()
                    .ok_or_else(|| BackendError::Backend("VPP should be initialized".to_string()))?
                    .process(Some(&mut input_surface), None)
                    .await
                    .map_err(|status| map_onevpl_status(status, "VPP::process"))?;
                match encoder
                    .encode(&mut ctrl, Some(vpp_frame), &mut bitstream, None)
                    .await
                {
                    Ok(_) => {
                        collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?
                    }
                    Err(MfxStatus::MoreData) => {}
                    Err(status) => {
                        return Err(map_onevpl_status(status, "Encoder::encode (hardware)"));
                    }
                }
            }
            Some(FourCC::YV12) => {
                let converted = argb_to_yv12(argb, width, height, aligned_width, aligned_height)?;
                let mut input_surface = vpp
                    .as_mut()
                    .ok_or_else(|| BackendError::Backend("VPP should be initialized".to_string()))?
                    .get_surface_input()
                    .map_err(|status| map_onevpl_status(status, "VPP::get_surface_input"))?;
                let mut input_cursor = Cursor::new(converted.as_slice());
                input_surface
                    .read_raw_frame(&mut input_cursor, FourCC::YV12)
                    .await
                    .map_err(|status| map_onevpl_status(status, "FrameSurface::read_raw_frame"))?;
                let vpp_frame = vpp
                    .as_ref()
                    .ok_or_else(|| BackendError::Backend("VPP should be initialized".to_string()))?
                    .process(Some(&mut input_surface), None)
                    .await
                    .map_err(|status| map_onevpl_status(status, "VPP::process"))?;
                match encoder
                    .encode(&mut ctrl, Some(vpp_frame), &mut bitstream, None)
                    .await
                {
                    Ok(_) => {
                        collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?
                    }
                    Err(MfxStatus::MoreData) => {}
                    Err(status) => {
                        return Err(map_onevpl_status(status, "Encoder::encode (hardware)"));
                    }
                }
            }
            Some(other) => {
                return Err(BackendError::UnsupportedConfig(format!(
                    "unsupported VPP input fourcc configured for Intel encoder: {other:?}"
                )));
            }
            None => {
                let converted = argb_to_i420(argb, width, height, width, height)?;
                let mut input_surface = encoder
                    .get_surface()
                    .map_err(|status| map_onevpl_status(status, "Encoder::get_surface"))?;
                let mut input_cursor = Cursor::new(converted.as_slice());
                input_surface
                    .read_raw_frame(&mut input_cursor, FourCC::IyuvOrI420)
                    .await
                    .map_err(|status| map_onevpl_status(status, "FrameSurface::read_raw_frame"))?;
                match encoder
                    .encode(&mut ctrl, Some(input_surface), &mut bitstream, None)
                    .await
                {
                    Ok(_) => {
                        collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?
                    }
                    Err(MfxStatus::MoreData) => {}
                    Err(status) => {
                        return Err(map_onevpl_status(status, "Encoder::encode (software)"));
                    }
                }
            }
        }
    }

    loop {
        let mut ctrl = EncodeCtrl::new();
        match encoder.encode(&mut ctrl, None, &mut bitstream, None).await {
            Ok(_) => collect_packets(&mut bitstream, codec, &mut pending_pts, &mut packets)?,
            Err(MfxStatus::MoreData) => break,
            Err(status) => return Err(map_onevpl_status(status, "Encoder::encode drain")),
        }
    }

    Ok(packets)
}

fn collect_packets(
    bitstream: &mut Bitstream<'_>,
    codec: Codec,
    pending_pts: &mut VecDeque<Option<i64>>,
    packets: &mut Vec<EncodedPacket>,
) -> Result<(), BackendError> {
    let data_len = bitstream.size() as usize;
    if data_len == 0 {
        return Ok(());
    }

    if bitstream.offset() > 0 {
        bitstream.write_all(&[]).map_err(|err| {
            BackendError::Backend(format!(
                "failed to normalize encoded bitstream offset before read: {err}"
            ))
        })?;
    }
    let mut data = vec![0_u8; data_len];
    bitstream.read_exact(&mut data).map_err(|err| {
        BackendError::Backend(format!("failed to read encoded bitstream payload: {err}"))
    })?;
    let frame_type = bitstream.frame_type();
    let key_flags = FrameType::I | FrameType::IDR | FrameType::XI | FrameType::XIDR;
    let fallback_pts = pending_pts.pop_front().flatten();
    let pts_90k = i64::try_from(bitstream.timestamp()).ok().or(fallback_pts);
    packets.push(EncodedPacket {
        codec,
        layout: intel_encoded_layout(codec),
        kind: EncodedContentKind::Frame,
        data,
        pts_90k,
        is_keyframe: frame_type.intersects(key_flags),
    });
    Ok(())
}
