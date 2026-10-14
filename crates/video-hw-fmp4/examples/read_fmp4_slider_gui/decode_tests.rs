use std::num::NonZeroU32;

use anyhow::{Context, Result};
use video_hw::Dimensions;

use super::*;

#[test]
fn strict_backend_rejects_fallback() {
    assert!(
        enforce_strict_backend_result(
            true,
            Backend::Nvidia,
            BackendKind::Intel,
            true,
            Some("primary backend failed")
        )
        .is_err()
    );
    assert!(
        enforce_strict_backend_result(
            false,
            Backend::Nvidia,
            BackendKind::Intel,
            true,
            Some("primary backend failed")
        )
        .is_ok()
    );
}

#[test]
fn worker_drop_closes_a_full_command_channel_before_joining() {
    let request = DecodeWorkerRequest {
        request_id: 1,
        generation: 0,
        sample_index: 0,
        backend: Backend::Auto,
        require_hardware: false,
        strict_backend: false,
    };
    let (command_tx, command_rx) = mpsc::sync_channel(1);
    command_tx
        .try_send(DecodeWorkerCommand::Decode(request))
        .expect("empty command slot accepts decode");
    let (_event_tx, event_rx) = mpsc::channel();

    drop(DecodeWorker {
        command_tx: Some(command_tx),
        event_rx,
        join_handle: Some(std::thread::spawn(|| {})),
    });
    assert!(matches!(
        command_rx.recv(),
        Ok(DecodeWorkerCommand::Decode(received)) if received.request_id == 1
    ));
    assert!(command_rx.recv().is_err());
}

#[test]
fn decoded_rgb_frame_converts_to_rgba_preview() -> Result<()> {
    let preview = decoded_frame_to_preview(DecodedFrame::Rgb24 {
        dims: Dimensions {
            width: NonZeroU32::new(2).expect("non-zero width"),
            height: NonZeroU32::new(1).expect("non-zero height"),
        },
        pts_90k: None,
        data: vec![1, 2, 3, 4, 5, 6],
    })?
    .context("RGB frame should have a preview")?;
    assert_eq!((preview.width, preview.height), (2, 1));
    assert_eq!(preview.rgba.as_ref(), &[1, 2, 3, 255, 4, 5, 6, 255]);
    assert!(
        decoded_frame_to_preview(DecodedFrame::Metadata {
            dims: None,
            pts_90k: None,
            pixel_format: None,
            decode_info_flags: None,
            color: None,
        })?
        .is_none()
    );
    Ok(())
}
