//! JNI validation, buffer reclamation, and MediaCodec/Surface cleanup paths.

use super::*;

pub(super) fn checked_i32<T>(value: T, name: &str) -> Result<i32, BackendError>
where
    T: TryInto<i32> + Copy + std::fmt::Display,
{
    value
        .try_into()
        .map_err(|_| BackendError::InvalidInput(format!("{name} out of range: {value}")))
}

pub(super) fn validate_surface_config(
    width: u32,
    height: u32,
    fps: u32,
    role: &str,
) -> Result<(), BackendError> {
    if width < 16 || height < 16 {
        return Err(BackendError::InvalidInput(format!(
            "Android surface {role} dimensions must be at least 16x16, got {width}x{height}"
        )));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(format!(
            "Android surface {role} dimensions must be even, got {width}x{height}"
        )));
    }
    if fps == 0 {
        return Err(BackendError::InvalidInput(format!(
            "Android surface {role} fps must be non-zero"
        )));
    }
    Ok(())
}

pub(super) fn validate_timeout(timeout_us: i64, role: &str) -> Result<(), BackendError> {
    if timeout_us < 0 {
        return Err(BackendError::InvalidInput(format!(
            "Android surface {role} timeout must be non-negative, got {timeout_us}"
        )));
    }
    Ok(())
}

pub(super) fn bounded_blocking_timeout_us(timeout_us: i64) -> i64 {
    timeout_us.clamp(0, MAX_BLOCKING_DEQUEUE_TIMEOUT_US)
}

pub(super) fn queue_empty_input_buffer(
    env: &mut JNIEnv<'_>,
    codec: &JObject<'_>,
    index: i32,
    presentation_time_us: i64,
) -> Result<(), BackendError> {
    env.call_method(
        codec,
        "queueInputBuffer",
        "(IIIJI)V",
        &[
            JValue::Int(index),
            JValue::Int(0),
            JValue::Int(0),
            JValue::Long(presentation_time_us),
            JValue::Int(0),
        ],
    )
    .map_err(jni_error)?;
    Ok(())
}

pub(super) fn input_error_after_reclaim(
    error: BackendError,
    reclaim: Result<(), BackendError>,
) -> BackendError {
    match reclaim {
        Ok(()) => error,
        Err(reclaim_error) => BackendError::Backend(format!(
            "{error}; additionally failed to return dequeued input buffer: {reclaim_error}"
        )),
    }
}

pub(super) fn release_output_buffer(
    env: &mut JNIEnv<'_>,
    codec: &JObject<'_>,
    index: i32,
    render: bool,
) -> Result<(), BackendError> {
    env.call_method(
        codec,
        "releaseOutputBuffer",
        "(IZ)V",
        &[
            JValue::Int(index),
            JValue::Bool(if render { JNI_TRUE } else { JNI_FALSE }),
        ],
    )
    .map_err(jni_error)?;
    Ok(())
}

pub(super) fn output_after_release<T>(
    output: Result<T, BackendError>,
    release: Result<(), BackendError>,
) -> Result<T, BackendError> {
    match (output, release) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(release_error)) => Err(BackendError::Backend(format!(
            "{error}; additionally failed to release output buffer: {release_error}"
        ))),
    }
}

pub(super) fn release_media_codec(
    env: &mut JNIEnv<'_>,
    codec: &GlobalRef,
) -> Result<(), BackendError> {
    let stop_result = call_cleanup_method(env, codec.as_obj(), "stop");
    let release_result = call_cleanup_method(env, codec.as_obj(), "release");
    cleanup_results(stop_result, release_result)
}

pub(super) fn release_surface(
    env: &mut JNIEnv<'_>,
    surface: &GlobalRef,
) -> Result<(), BackendError> {
    call_cleanup_method(env, surface.as_obj(), "release")
}

pub(super) fn call_cleanup_method(
    env: &mut JNIEnv<'_>,
    object: &JObject<'_>,
    method: &str,
) -> Result<(), BackendError> {
    match env.call_method(object, method, "()V", &[]) {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = env.exception_clear();
            Err(jni_error(error))
        }
    }
}

pub(super) fn cleanup_results(
    first: Result<(), BackendError>,
    second: Result<(), BackendError>,
) -> Result<(), BackendError> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(BackendError::Backend(format!(
            "cleanup failed: {first}; additional cleanup failure: {second}"
        ))),
    }
}

pub(super) fn jni_error(error: jni::errors::Error) -> BackendError {
    BackendError::Backend(format!("JNI error: {error}"))
}
