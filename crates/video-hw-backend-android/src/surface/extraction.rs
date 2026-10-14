//! MediaFormat and ByteBuffer extraction, packet framing, and timestamp normalization.

use super::*;

#[derive(Debug)]
pub(super) struct OutputBufferInfo {
    pub(super) offset: i32,
    pub(super) size: i32,
    pub(super) presentation_time_us: i64,
    pub(super) flags: i32,
}

impl OutputBufferInfo {
    pub(super) fn read(
        env: &mut JNIEnv<'_>,
        buffer_info: &JObject<'_>,
    ) -> Result<Self, BackendError> {
        Ok(Self {
            offset: env
                .get_field(buffer_info, "offset", "I")
                .map_err(jni_error)?
                .i()
                .map_err(jni_error)?,
            size: env
                .get_field(buffer_info, "size", "I")
                .map_err(jni_error)?
                .i()
                .map_err(jni_error)?,
            presentation_time_us: env
                .get_field(buffer_info, "presentationTimeUs", "J")
                .map_err(jni_error)?
                .j()
                .map_err(jni_error)?,
            flags: env
                .get_field(buffer_info, "flags", "I")
                .map_err(jni_error)?
                .i()
                .map_err(jni_error)?,
        })
    }
}

pub(super) fn set_media_format_i32(
    env: &mut JNIEnv<'_>,
    format: &JObject<'_>,
    key: &str,
    value: i32,
) -> Result<(), BackendError> {
    let key = env.new_string(key).map_err(jni_error)?;
    env.call_method(
        format,
        "setInteger",
        "(Ljava/lang/String;I)V",
        &[JValue::Object(&JObject::from(key)), JValue::Int(value)],
    )
    .map_err(jni_error)?;
    Ok(())
}

pub(super) fn media_format_buffer(
    env: &mut JNIEnv<'_>,
    format: &JObject<'_>,
    key: &str,
) -> Result<Option<Vec<u8>>, BackendError> {
    let key_string = env.new_string(key).map_err(jni_error)?;
    let has_key = env
        .call_method(
            format,
            "containsKey",
            "(Ljava/lang/String;)Z",
            &[JValue::Object(&JObject::from(key_string))],
        )
        .map_err(jni_error)?
        .z()
        .map_err(jni_error)?;
    if !has_key {
        return Ok(None);
    }

    let key_string = env.new_string(key).map_err(jni_error)?;
    let buffer = env
        .call_method(
            format,
            "getByteBuffer",
            "(Ljava/lang/String;)Ljava/nio/ByteBuffer;",
            &[JValue::Object(&JObject::from(key_string))],
        )
        .map_err(jni_error)?
        .l()
        .map_err(jni_error)?;
    if buffer.is_null() {
        return Ok(None);
    }
    let data = byte_buffer_remaining(env, &buffer)?;
    Ok((!data.is_empty()).then_some(data))
}

fn byte_buffer_remaining(
    env: &mut JNIEnv<'_>,
    buffer: &JObject<'_>,
) -> Result<Vec<u8>, BackendError> {
    let duplicate = env
        .call_method(buffer, "duplicate", "()Ljava/nio/ByteBuffer;", &[])
        .map_err(jni_error)?
        .l()
        .map_err(jni_error)?;
    let len = env
        .call_method(&duplicate, "remaining", "()I", &[])
        .map_err(jni_error)?
        .i()
        .map_err(jni_error)?;
    if len <= 0 {
        return Ok(Vec::new());
    }
    let array = env.new_byte_array(len).map_err(jni_error)?;
    let array_object: &JObject<'_> = array.as_ref();
    env.call_method(
        &duplicate,
        "get",
        "([B)Ljava/nio/ByteBuffer;",
        &[JValue::Object(array_object)],
    )
    .map_err(jni_error)?;
    env.convert_byte_array(&array).map_err(jni_error)
}

pub(super) fn encoder_config_buffer_payload(codec: Codec, buffer: Vec<u8>) -> Vec<u8> {
    match codec {
        Codec::H264 | Codec::Hevc => annexb_prefixed(buffer),
        Codec::Av1 => buffer,
    }
}

pub(super) fn encoded_layout(codec: Codec) -> EncodedLayout {
    match codec {
        Codec::H264 | Codec::Hevc => EncodedLayout::AnnexB,
        Codec::Av1 => EncodedLayout::Av1,
    }
}

fn annexb_prefixed(buffer: Vec<u8>) -> Vec<u8> {
    if buffer.starts_with(&[0, 0, 1]) || buffer.starts_with(&[0, 0, 0, 1]) {
        buffer
    } else {
        let mut out = Vec::with_capacity(buffer.len() + 4);
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&buffer);
        out
    }
}

fn us_to_pts_90k(value: i64) -> i64 {
    value.saturating_mul(90_000) / 1_000_000
}

pub(super) fn normalize_encoder_pts_90k(
    origin_us: &mut Option<i64>,
    last_pts_90k: &mut Option<i64>,
    presentation_time_us: i64,
) -> i64 {
    let origin = *origin_us.get_or_insert(presentation_time_us);
    let delta_us = presentation_time_us.saturating_sub(origin).max(0);
    let mut pts = us_to_pts_90k(delta_us);
    if let Some(last) = *last_pts_90k
        && pts <= last
    {
        pts = last.saturating_add(1);
    }
    *last_pts_90k = Some(pts);
    pts
}
