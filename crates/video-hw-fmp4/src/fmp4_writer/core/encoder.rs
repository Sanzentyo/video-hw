//! Backend selection and typed encoder-session dispatch.

use super::*;

pub(super) fn resolve_recording_backend_and_config(
    settings: &RecordingSettings,
    fps: i32,
) -> Result<(BackendKind, EncoderConfig)> {
    let mut config = EncoderConfig::new(
        settings.codec,
        fps,
        settings.require_hardware,
        EncodeInputFormat::Argb8888,
    );
    let resolved_backend = settings.backend.resolve_encoder(&config).with_context(|| {
        format!(
            "failed to resolve encoder backend (backend={}, codec={}, require_hardware={})",
            settings.backend, settings.codec, settings.require_hardware
        )
    })?;
    if backend_is_intel(resolved_backend) {
        config.backend_options = BackendEncoderOptions::Intel(IntelEncoderOptions {
            force_software: settings.intel_force_software,
            hevc_use_vpp: None,
            ..Default::default()
        });
    }
    Ok((resolved_backend, config))
}

pub(super) enum BackendEncoderSession {
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    Nvidia(Box<video_hw::EncodeSession<NvEncoderAdapter>>),
    #[cfg(all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    Intel(Box<video_hw::EncodeSession<IntelEncoderAdapter>>),
    #[cfg(all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    ))]
    Vulkan(Box<video_hw::EncodeSession<VulkanEncoderAdapter>>),
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    VideoToolbox(Box<video_hw::EncodeSession<VtEncoderAdapter>>),
}

#[cfg(any(
    all(feature = "backend-vt", target_os = "macos"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    )
))]
impl BackendEncoderSession {
    pub(super) fn new(backend: BackendKind, config: EncoderConfig) -> Result<Self> {
        let session = match backend {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Nvidia => Self::Nvidia(Box::new(video_hw::EncodeSession::<
                NvEncoderAdapter,
            >::new(config))),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Intel => Self::Intel(Box::new(video_hw::EncodeSession::<
                IntelEncoderAdapter,
            >::new(config))),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Vulkan => Self::Vulkan(Box::new(video_hw::EncodeSession::<
                VulkanEncoderAdapter,
            >::new(config))),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            BackendKind::VideoToolbox => Self::VideoToolbox(Box::new(video_hw::EncodeSession::<
                VtEncoderAdapter,
            >::new(config))),
            #[allow(unreachable_patterns)]
            other => {
                bail!("encoder backend {other} is not compiled into video-hw-fmp4")
            }
        };
        Ok(session)
    }

    pub(super) fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError> {
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.submit(frame),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.submit(frame),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.submit(frame),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.submit(frame),
        }
    }

    pub(super) fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.try_reap(),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.try_reap(),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.try_reap(),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.try_reap(),
        }
    }

    pub(super) fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.flush(),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.flush(),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.flush(),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.flush(),
        }
    }

    pub(super) fn requires_periodic_fragment_flush(&self) -> bool {
        true
    }
}

#[cfg(not(any(
    all(feature = "backend-vt", target_os = "macos"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    )
)))]
impl BackendEncoderSession {
    pub(super) fn new(_backend: BackendKind, _config: EncoderConfig) -> Result<Self> {
        bail!("no encoder backend compiled")
    }

    pub(super) fn submit(&mut self, _frame: EncodeFrame) -> Result<(), BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no encoder backend compiled".to_string(),
        ))
    }

    pub(super) fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no encoder backend compiled".to_string(),
        ))
    }

    pub(super) fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no encoder backend compiled".to_string(),
        ))
    }

    pub(super) fn requires_periodic_fragment_flush(&self) -> bool {
        false
    }
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
fn backend_is_intel(backend: BackendKind) -> bool {
    matches!(backend, BackendKind::Intel)
}

#[cfg(not(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
)))]
fn backend_is_intel(_backend: BackendKind) -> bool {
    false
}
