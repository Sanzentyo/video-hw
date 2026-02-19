#![allow(dead_code)]

use std::mem;

use crate::{BackendError, Codec};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AccessUnit {
    pub nalus: Vec<Vec<u8>>,
    pub codec: Codec,
    pub pts_90k: Option<i64>,
    pub is_keyframe: bool,
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
    current_has_key_vcl: bool,
    parameter_sets: ParameterSetCache,
}

impl StatefulBitstreamAssembler {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_codec(codec: Codec) -> Self {
        let mut this = Self::default();
        this.codec = Some(codec);
        this
    }

    pub fn push_chunk(
        &mut self,
        chunk: &[u8],
        codec: Codec,
        _pts_90k: Option<i64>,
    ) -> Result<(Vec<AccessUnit>, ParameterSetCache), BackendError> {
        self.codec = Some(codec);
        if !chunk.is_empty() {
            self.pending.extend_from_slice(chunk);
        }

        let nalus = self.take_complete_nals(false);
        let access_units = self.process_nals(codec, nalus);

        Ok((access_units, self.parameter_sets.clone()))
    }

    pub fn flush(&mut self) -> Result<(Vec<AccessUnit>, ParameterSetCache), BackendError> {
        let codec = self
            .codec
            .ok_or_else(|| BackendError::InvalidInput("codec is not set".to_string()))?;
        let nalus = self.take_complete_nals(true);
        let mut access_units = self.process_nals(codec, nalus);
        if self.current_has_vcl && !self.current_nalus.is_empty() {
            access_units.push(self.finish_current_access_unit(codec));
        }

        Ok((access_units, self.parameter_sets.clone()))
    }

    fn process_nals(&mut self, codec: Codec, nalus: Vec<Vec<u8>>) -> Vec<AccessUnit> {
        let mut out = Vec::new();

        for nal in nalus {
            self.parameter_sets.observe(codec, &nal);

            if is_aud(codec, &nal) {
                self.saw_aud = true;
                if self.current_has_vcl && !self.current_nalus.is_empty() {
                    out.push(self.finish_current_access_unit(codec));
                } else {
                    self.current_nalus.clear();
                    self.current_has_vcl = false;
                    self.current_has_key_vcl = false;
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
            let nal_is_key = is_key_vcl(codec, &nal);
            self.current_nalus.push(nal);
            if nal_is_vcl {
                self.current_has_vcl = true;
                self.current_has_key_vcl = self.current_has_key_vcl || nal_is_key;
            }
        }

        out
    }

    fn finish_current_access_unit(&mut self, codec: Codec) -> AccessUnit {
        let au = AccessUnit {
            nalus: mem::take(&mut self.current_nalus),
            codec,
            pts_90k: None,
            is_keyframe: self.current_has_key_vcl,
        };
        self.current_has_vcl = false;
        self.current_has_key_vcl = false;
        au
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

impl ParameterSetCache {
    pub fn required_for_codec(&self, codec: Codec) -> Option<Vec<Vec<u8>>> {
        match codec {
            Codec::H264 => Some(vec![self.h264_sps.clone()?, self.h264_pps.clone()?]),
            Codec::Hevc => Some(vec![
                self.hevc_vps.clone()?,
                self.hevc_sps.clone()?,
                self.hevc_pps.clone()?,
            ]),
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
    }
}

fn is_vcl(codec: Codec, nal: &[u8]) -> bool {
    if nal.is_empty() {
        return false;
    }
    match codec {
        Codec::H264 => matches!(nal[0] & 0x1f, 1 | 2 | 3 | 4 | 5 | 19),
        Codec::Hevc => ((nal[0] >> 1) & 0x3f) <= 31,
    }
}

fn is_key_vcl(codec: Codec, nal: &[u8]) -> bool {
    if nal.is_empty() {
        return false;
    }
    match codec {
        Codec::H264 => (nal[0] & 0x1f) == 5,
        Codec::Hevc => matches!((nal[0] >> 1) & 0x3f, 16 | 17 | 18 | 19 | 20 | 21),
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

    #[test]
    fn chunked_parse_converges() {
        let data = h264_sample_annexb();
        let mut assembler = StatefulBitstreamAssembler::new();
        let mut emitted = Vec::new();

        for chunk in data.chunks(3) {
            let (aus, _) = assembler.push_chunk(chunk, Codec::H264, None).unwrap();
            emitted.extend(aus);
        }
        let (flush_aus, _) = assembler.flush().unwrap();
        emitted.extend(flush_aus);

        assert_eq!(emitted.len(), 2);
        assert!(emitted[0].is_keyframe);
        assert!(!emitted[1].is_keyframe);
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
}
