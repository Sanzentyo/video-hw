use std::mem;

use crate::{BackendError, Codec};

#[derive(Debug, Clone)]
pub struct AccessUnit {
    pub nalus: Vec<Vec<u8>>,
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    pub pts_90k: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct ParameterSetCache {
    h264_sps: Option<Vec<u8>>,
    h264_pps: Option<Vec<u8>>,
    hevc_vps: Option<Vec<u8>>,
    hevc_sps: Option<Vec<u8>>,
    hevc_pps: Option<Vec<u8>>,
}

#[derive(Debug, Default)]
pub struct StatefulBitstreamAssembler {
    codec: Option<Codec>,
    pending: Vec<u8>,
    saw_aud: bool,
    current_nalus: Vec<Vec<u8>>,
    current_has_vcl: bool,
    current_pts_90k: Option<i64>,
    parameter_sets: ParameterSetCache,
}

impl StatefulBitstreamAssembler {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_codec(codec: Codec) -> Self {
        Self {
            codec: Some(codec),
            ..Self::default()
        }
    }

    pub fn push_chunk(
        &mut self,
        chunk: &[u8],
        codec: Codec,
        pts_90k: Option<i64>,
    ) -> Result<(Vec<AccessUnit>, ParameterSetCache), BackendError> {
        self.codec = Some(codec);
        if codec == Codec::Av1 {
            if chunk.is_empty() {
                return Ok((Vec::new(), self.parameter_sets.clone()));
            }
            if pts_90k.is_some() {
                return Ok((
                    vec![AccessUnit {
                        nalus: vec![chunk.to_vec()],
                        #[cfg(all(
                            feature = "backend-nvidia",
                            any(target_os = "linux", target_os = "windows")
                        ))]
                        pts_90k,
                    }],
                    self.parameter_sets.clone(),
                ));
            }
            self.pending.extend_from_slice(chunk);
            let access_units = self.take_complete_av1_temporal_units(false, None);
            return Ok((access_units, self.parameter_sets.clone()));
        }
        if !chunk.is_empty() {
            self.pending.extend_from_slice(chunk);
        }

        let finalize_chunk = pts_90k.is_some();
        let nalus = self.take_complete_nals(finalize_chunk);
        let mut access_units = self.process_nals(codec, nalus, pts_90k);
        if finalize_chunk && self.current_has_vcl && !self.current_nalus.is_empty() {
            access_units.push(self.finish_current_access_unit(codec));
        }

        Ok((access_units, self.parameter_sets.clone()))
    }

    pub fn flush(&mut self) -> Result<(Vec<AccessUnit>, ParameterSetCache), BackendError> {
        let codec = self
            .codec
            .ok_or_else(|| BackendError::InvalidInput("codec is not set".to_string()))?;
        if codec == Codec::Av1 {
            return Ok((
                self.take_complete_av1_temporal_units(true, None),
                self.parameter_sets.clone(),
            ));
        }
        let nalus = self.take_complete_nals(true);
        let mut access_units = self.process_nals(codec, nalus, None);
        if self.current_has_vcl && !self.current_nalus.is_empty() {
            access_units.push(self.finish_current_access_unit(codec));
        }

        Ok((access_units, self.parameter_sets.clone()))
    }

    fn take_complete_av1_temporal_units(
        &mut self,
        finalize: bool,
        pts_90k: Option<i64>,
    ) -> Vec<AccessUnit> {
        let parsed = parse_av1_low_overhead_obus(&self.pending, finalize);
        let mut out = Vec::new();
        let mut current = Vec::new();
        let mut current_has_frame = false;
        let mut emitted_end = 0usize;

        for obu in parsed.obus {
            if obu.obu_type == AV1_OBU_TEMPORAL_DELIMITER && !current.is_empty() {
                if current_has_frame {
                    out.push(av1_access_unit(mem::take(&mut current), pts_90k));
                } else {
                    current.clear();
                }
                emitted_end = obu.start;
                current_has_frame = false;
            }
            current.extend_from_slice(obu.bytes);
            current_has_frame |= av1_obu_is_frame_payload(obu.obu_type);
        }

        if finalize {
            if !current.is_empty() {
                out.push(av1_access_unit(current, pts_90k));
            } else if emitted_end == 0 && !self.pending.is_empty() {
                out.push(av1_access_unit(mem::take(&mut self.pending), pts_90k));
                return out;
            }
            self.pending.clear();
        } else if emitted_end > 0 {
            self.pending.drain(..emitted_end);
        }

        out
    }

    fn process_nals(
        &mut self,
        codec: Codec,
        nalus: Vec<Vec<u8>>,
        pts_90k: Option<i64>,
    ) -> Vec<AccessUnit> {
        let mut out = Vec::new();

        for nal in nalus {
            self.parameter_sets.observe(codec, &nal);

            if is_aud(codec, &nal) {
                self.saw_aud = true;
                if self.current_has_vcl && !self.current_nalus.is_empty() {
                    out.push(self.finish_current_access_unit(codec));
                } else {
                    self.current_nalus.clear();
                    self.clear_current_flags();
                }
                continue;
            }

            if !self.saw_aud
                && is_vcl(codec, &nal)
                && self.current_has_vcl
                && !self.current_nalus.is_empty()
            {
                out.push(self.finish_current_access_unit(codec));
            }

            let nal_is_vcl = is_vcl(codec, &nal);
            if self.current_nalus.is_empty() {
                self.current_pts_90k = pts_90k;
            }
            self.current_nalus.push(nal);
            if nal_is_vcl {
                self.record_vcl();
            }
        }

        out
    }

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    fn finish_current_access_unit(&mut self, codec: Codec) -> AccessUnit {
        let _ = codec;
        let au = AccessUnit {
            nalus: mem::take(&mut self.current_nalus),
            pts_90k: self.current_pts_90k.take(),
        };
        self.clear_current_flags();
        au
    }

    #[cfg(not(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )))]
    fn finish_current_access_unit(&mut self, codec: Codec) -> AccessUnit {
        let _ = codec;
        let au = AccessUnit {
            nalus: mem::take(&mut self.current_nalus),
        };
        self.clear_current_flags();
        au
    }

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    fn record_vcl(&mut self) {
        self.current_has_vcl = true;
    }

    #[cfg(not(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )))]
    fn record_vcl(&mut self) {
        self.current_has_vcl = true;
    }

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    fn clear_current_flags(&mut self) {
        self.current_has_vcl = false;
        self.current_pts_90k = None;
    }

    #[cfg(not(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )))]
    fn clear_current_flags(&mut self) {
        self.current_has_vcl = false;
        self.current_pts_90k = None;
    }

    fn take_complete_nals(&mut self, finalize: bool) -> Vec<Vec<u8>> {
        if self.pending.is_empty() {
            return Vec::new();
        }

        let mut start_codes = find_start_codes(&self.pending);
        if start_codes.is_empty() {
            if finalize {
                self.pending.clear();
            }
            return Vec::new();
        }

        if start_codes[0].0 > 0 {
            let remainder = self.pending.split_off(start_codes[0].0);
            self.pending = remainder;
            start_codes = find_start_codes(&self.pending);
            if start_codes.is_empty() {
                return Vec::new();
            }
        }

        let mut nalus = Vec::new();
        for window in start_codes.windows(2) {
            let (start, start_len) = window[0];
            let end = window[1].0;
            let payload_start = start + start_len;
            if end > payload_start {
                nalus.push(self.pending[payload_start..end].to_vec());
            }
        }

        if finalize {
            if let Some((start, start_len)) = start_codes.last().copied() {
                let payload_start = start + start_len;
                if self.pending.len() > payload_start {
                    nalus.push(self.pending[payload_start..].to_vec());
                }
            }
            self.pending.clear();
        } else if let Some((start, _)) = start_codes.last().copied() {
            let remainder = self.pending.split_off(start);
            self.pending = remainder;
        }

        nalus
    }
}

#[cfg(test)]
const AV1_OBU_SEQUENCE_HEADER: u8 = 1;
const AV1_OBU_TEMPORAL_DELIMITER: u8 = 2;
const AV1_OBU_FRAME_HEADER: u8 = 3;
const AV1_OBU_TILE_GROUP: u8 = 4;
const AV1_OBU_FRAME: u8 = 6;

#[derive(Debug)]
struct Av1Obu<'a> {
    obu_type: u8,
    bytes: &'a [u8],
    start: usize,
}

#[derive(Debug, Default)]
struct Av1ParseResult<'a> {
    obus: Vec<Av1Obu<'a>>,
}

fn av1_access_unit(data: Vec<u8>, pts_90k: Option<i64>) -> AccessUnit {
    AccessUnit {
        nalus: vec![data],
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        pts_90k,
    }
}

fn av1_obu_is_frame_payload(obu_type: u8) -> bool {
    matches!(
        obu_type,
        AV1_OBU_FRAME_HEADER | AV1_OBU_TILE_GROUP | AV1_OBU_FRAME
    )
}

fn parse_av1_low_overhead_obus(data: &[u8], finalize: bool) -> Av1ParseResult<'_> {
    let mut offset = 0usize;
    let mut obus = Vec::new();

    while offset < data.len() {
        let Some((obu_type, end)) = parse_one_av1_obu(data, offset) else {
            if finalize {
                obus.push(Av1Obu {
                    obu_type: AV1_OBU_FRAME,
                    bytes: &data[offset..],
                    start: offset,
                });
            }
            break;
        };
        obus.push(Av1Obu {
            obu_type,
            bytes: &data[offset..end],
            start: offset,
        });
        offset = end;
    }

    Av1ParseResult { obus }
}

fn parse_one_av1_obu(data: &[u8], offset: usize) -> Option<(u8, usize)> {
    let header = *data.get(offset)?;
    let obu_type = (header >> 3) & 0x0f;
    let has_extension = (header & 0x04) != 0;
    let has_size_field = (header & 0x02) != 0;
    let mut cursor = offset.checked_add(1)?;
    if has_extension {
        cursor = cursor.checked_add(1)?;
        data.get(cursor - 1)?;
    }
    if !has_size_field {
        return None;
    }
    let (payload_size, leb_len) = read_leb128(&data[cursor..])?;
    cursor = cursor.checked_add(leb_len)?;
    let payload_size = usize::try_from(payload_size).ok()?;
    let end = cursor.checked_add(payload_size)?;
    if end > data.len() {
        return None;
    }
    Some((obu_type, end))
}

fn read_leb128(data: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, byte) in data.iter().copied().take(8).enumerate() {
        value |= u64::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

impl ParameterSetCache {
    #[cfg(any(test, all(target_os = "macos", feature = "backend-vt")))]
    pub fn required_for_codec(&self, codec: Codec) -> Option<Vec<Vec<u8>>> {
        match codec {
            Codec::H264 => Some(vec![self.h264_sps.clone()?, self.h264_pps.clone()?]),
            Codec::Hevc => Some(vec![
                self.hevc_vps.clone()?,
                self.hevc_sps.clone()?,
                self.hevc_pps.clone()?,
            ]),
            Codec::Av1 => Some(Vec::new()),
        }
    }

    fn observe(&mut self, codec: Codec, nal: &[u8]) {
        if nal.is_empty() {
            return;
        }

        match codec {
            Codec::H264 => match nal[0] & 0x1f {
                7 => self.h264_sps = Some(nal.to_vec()),
                8 => self.h264_pps = Some(nal.to_vec()),
                _ => {}
            },
            Codec::Hevc => match (nal[0] >> 1) & 0x3f {
                32 => self.hevc_vps = Some(nal.to_vec()),
                33 => self.hevc_sps = Some(nal.to_vec()),
                34 => self.hevc_pps = Some(nal.to_vec()),
                _ => {}
            },
            Codec::Av1 => {}
        }
    }
}

fn find_start_codes(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 3 <= data.len() {
        if i + 4 <= data.len()
            && data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && data[i + 3] == 1
        {
            out.push((i, 4));
            i += 4;
            continue;
        }
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            out.push((i, 3));
            i += 3;
            continue;
        }
        i += 1;
    }
    out
}

fn is_aud(codec: Codec, nal: &[u8]) -> bool {
    if nal.is_empty() {
        return false;
    }
    match codec {
        Codec::H264 => (nal[0] & 0x1f) == 9,
        Codec::Hevc => ((nal[0] >> 1) & 0x3f) == 35,
        Codec::Av1 => false,
    }
}

fn is_vcl(codec: Codec, nal: &[u8]) -> bool {
    if nal.is_empty() {
        return false;
    }
    match codec {
        Codec::H264 => matches!(nal[0] & 0x1f, 1 | 2 | 3 | 4 | 5 | 19),
        Codec::Hevc => ((nal[0] >> 1) & 0x3f) <= 31,
        Codec::Av1 => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h264_sample_annexb() -> Vec<u8> {
        let mut out = Vec::new();
        let mut push_nal = |nal: &[u8]| {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal);
        };

        push_nal(&[0x09, 0xF0]);
        push_nal(&[0x67, 0x42, 0x00, 0x1E]);
        push_nal(&[0x68, 0xCE, 0x06, 0xE2]);
        push_nal(&[0x65, 0x88, 0x84, 0x21]);
        push_nal(&[0x09, 0xF0]);
        push_nal(&[0x41, 0x9A, 0x22, 0x11]);

        out
    }

    fn av1_sample_obu_stream() -> Vec<u8> {
        let mut out = Vec::new();
        let mut push_obu = |obu_type: u8, payload: &[u8]| {
            out.push((obu_type << 3) | 0x02);
            out.push(payload.len() as u8);
            out.extend_from_slice(payload);
        };

        push_obu(AV1_OBU_TEMPORAL_DELIMITER, &[]);
        push_obu(AV1_OBU_SEQUENCE_HEADER, &[0x01, 0x02, 0x03]);
        push_obu(AV1_OBU_FRAME, &[0x10, 0x11]);
        push_obu(AV1_OBU_TEMPORAL_DELIMITER, &[]);
        push_obu(AV1_OBU_FRAME, &[0x20, 0x21]);

        out
    }

    #[test]
    fn chunked_parse_converges() {
        let data = h264_sample_annexb();
        let mut assembler = StatefulBitstreamAssembler::with_codec(Codec::H264);
        let mut emitted = Vec::new();

        for chunk in data.chunks(3) {
            let (aus, _) = assembler.push_chunk(chunk, Codec::H264, None).unwrap();
            emitted.extend(aus);
        }
        let (flush_aus, _) = assembler.flush().unwrap();
        emitted.extend(flush_aus);

        assert_eq!(emitted.len(), 2);
        assert!(!emitted[0].nalus.is_empty());
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            assert!(emitted[0].pts_90k.is_none());
        }
    }

    #[test]
    fn extracts_required_parameter_sets() {
        let data = h264_sample_annexb();
        let mut assembler = StatefulBitstreamAssembler::new();
        let _ = assembler.push_chunk(&data, Codec::H264, None).unwrap();
        let (_, cache) = assembler.flush().unwrap();

        let params = cache.required_for_codec(Codec::H264).unwrap();
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn av1_low_overhead_obu_stream_splits_temporal_units() {
        let data = av1_sample_obu_stream();
        let mut assembler = StatefulBitstreamAssembler::with_codec(Codec::Av1);
        let mut emitted = Vec::new();

        for chunk in data.chunks(2) {
            let (aus, _) = assembler.push_chunk(chunk, Codec::Av1, None).unwrap();
            emitted.extend(aus);
        }
        let (flush_aus, _) = assembler.flush().unwrap();
        emitted.extend(flush_aus);

        assert_eq!(emitted.len(), 2);
        assert!(emitted[0].nalus[0].starts_with(&[(AV1_OBU_TEMPORAL_DELIMITER << 3) | 0x02]));
        assert!(emitted[0].nalus[0].contains(&((AV1_OBU_SEQUENCE_HEADER << 3) | 0x02)));
        assert!(emitted[1].nalus[0].starts_with(&[(AV1_OBU_TEMPORAL_DELIMITER << 3) | 0x02]));
    }

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    #[test]
    fn av1_timestamped_chunk_stays_single_access_unit() {
        let data = av1_sample_obu_stream();
        let mut assembler = StatefulBitstreamAssembler::with_codec(Codec::Av1);

        let (aus, _) = assembler
            .push_chunk(&data, Codec::Av1, Some(9_000))
            .unwrap();

        assert_eq!(aus.len(), 1);
        assert_eq!(aus[0].nalus[0], data);
        assert_eq!(aus[0].pts_90k, Some(9_000));
    }

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    #[test]
    fn timestamped_chunks_emit_access_units_with_matching_pts() {
        let mut assembler = StatefulBitstreamAssembler::with_codec(Codec::H264);
        let first = [0, 0, 0, 1, 0x09, 0xF0, 0, 0, 0, 1, 0x65, 0x88];
        let second = [0, 0, 0, 1, 0x09, 0xF0, 0, 0, 0, 1, 0x41, 0x9A];

        let (first_aus, _) = assembler
            .push_chunk(&first, Codec::H264, Some(1_000))
            .unwrap();
        let (second_aus, _) = assembler
            .push_chunk(&second, Codec::H264, Some(2_000))
            .unwrap();

        assert_eq!(first_aus.len(), 1);
        assert_eq!(first_aus[0].pts_90k, Some(1_000));
        assert_eq!(second_aus.len(), 1);
        assert_eq!(second_aus[0].pts_90k, Some(2_000));
    }
}
