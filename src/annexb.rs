use std::mem;

use crate::{error::Result, Codec, VtBackendError};

#[derive(Debug, Clone)]
pub struct BitstreamPrepared {
    pub parameter_sets: Vec<Vec<u8>>,
    pub access_units: Vec<AccessUnit>,
}

#[derive(Debug, Clone)]
pub struct AccessUnit {
    pub nalus: Vec<Vec<u8>>,
    pub codec: Codec,
    pub pts_90k: Option<i64>,
    pub is_keyframe: bool,
}

#[derive(Debug, Clone)]
pub struct AnnexBStreamParser {
    codec: Codec,
    buffer: Vec<u8>,
    saw_aud: bool,
    current_nalus: Vec<Vec<u8>>,
    current_has_vcl: bool,
    current_has_key_vcl: bool,
    parameter_sets: ParameterSetCache,
}

#[derive(Debug, Clone, Default)]
struct ParameterSetCache {
    h264_sps: Option<Vec<u8>>,
    h264_pps: Option<Vec<u8>>,
    hevc_vps: Option<Vec<u8>>,
    hevc_sps: Option<Vec<u8>>,
    hevc_pps: Option<Vec<u8>>,
}

pub fn parse_annexb(data: &[u8], codec: Codec) -> Result<BitstreamPrepared> {
    let prepared = parse_annexb_for_stream(data, codec)?;
    if prepared.access_units.is_empty() {
        return Err(VtBackendError::EmptyAccessUnit);
    }
    Ok(prepared)
}

pub fn parse_annexb_for_stream(data: &[u8], codec: Codec) -> Result<BitstreamPrepared> {
    let mut parser = AnnexBStreamParser::new(codec);
    let mut access_units = parser.push_chunk(data)?;
    access_units.extend(parser.flush()?);
    Ok(BitstreamPrepared {
        parameter_sets: parser.parameter_sets(),
        access_units,
    })
}

impl AnnexBStreamParser {
    pub fn new(codec: Codec) -> Self {
        Self {
            codec,
            buffer: Vec::new(),
            saw_aud: false,
            current_nalus: Vec::new(),
            current_has_vcl: false,
            current_has_key_vcl: false,
            parameter_sets: ParameterSetCache::default(),
        }
    }

    pub fn parameter_sets(&self) -> Vec<Vec<u8>> {
        self.parameter_sets.snapshot(self.codec)
    }

    pub fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<AccessUnit>> {
        if !chunk.is_empty() {
            self.buffer.extend_from_slice(chunk);
        }
        let nalus = self.take_complete_nals(false);
        Ok(self.process_nals(nalus))
    }

    pub fn flush(&mut self) -> Result<Vec<AccessUnit>> {
        let nalus = self.take_complete_nals(true);
        let mut access_units = self.process_nals(nalus);
        if self.current_has_vcl && !self.current_nalus.is_empty() {
            access_units.push(self.finish_current_access_unit());
        }
        Ok(access_units)
    }

    fn process_nals(&mut self, nalus: Vec<Vec<u8>>) -> Vec<AccessUnit> {
        let mut out = Vec::new();

        for nal in nalus {
            self.parameter_sets.observe(self.codec, &nal);

            if is_aud(self.codec, &nal) {
                self.saw_aud = true;
                if self.current_has_vcl && !self.current_nalus.is_empty() {
                    out.push(self.finish_current_access_unit());
                } else {
                    self.current_nalus.clear();
                    self.current_has_vcl = false;
                    self.current_has_key_vcl = false;
                }
                continue;
            }

            if !self.saw_aud
                && is_vcl(self.codec, &nal)
                && self.current_has_vcl
                && !self.current_nalus.is_empty()
            {
                out.push(self.finish_current_access_unit());
            }

            let nal_is_vcl = is_vcl(self.codec, &nal);
            let nal_is_key = is_key_vcl(self.codec, &nal);
            self.current_nalus.push(nal);
            if nal_is_vcl {
                self.current_has_vcl = true;
                self.current_has_key_vcl = self.current_has_key_vcl || nal_is_key;
            }
        }

        out
    }

    fn finish_current_access_unit(&mut self) -> AccessUnit {
        let au = AccessUnit {
            nalus: mem::take(&mut self.current_nalus),
            codec: self.codec,
            pts_90k: None,
            is_keyframe: self.current_has_key_vcl,
        };
        self.current_has_vcl = false;
        self.current_has_key_vcl = false;
        au
    }

    fn take_complete_nals(&mut self, finalize: bool) -> Vec<Vec<u8>> {
        if self.buffer.is_empty() {
            return Vec::new();
        }

        let mut start_codes = find_start_codes(&self.buffer);
        if start_codes.is_empty() {
            if finalize {
                self.buffer.clear();
            }
            return Vec::new();
        }

        if start_codes[0].0 > 0 {
            let remainder = self.buffer.split_off(start_codes[0].0);
            self.buffer = remainder;
            start_codes = find_start_codes(&self.buffer);
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
                nalus.push(self.buffer[payload_start..end].to_vec());
            }
        }

        if finalize {
            if let Some((start, start_len)) = start_codes.last().copied() {
                let payload_start = start + start_len;
                if self.buffer.len() > payload_start {
                    nalus.push(self.buffer[payload_start..].to_vec());
                }
            }
            self.buffer.clear();
        } else if let Some((start, _)) = start_codes.last().copied() {
            let remainder = self.buffer.split_off(start);
            self.buffer = remainder;
        }

        nalus
    }
}

impl ParameterSetCache {
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

    fn snapshot(&self, codec: Codec) -> Vec<Vec<u8>> {
        match codec {
            Codec::H264 => {
                let mut out = Vec::new();
                if let Some(v) = &self.h264_sps {
                    out.push(v.clone());
                }
                if let Some(v) = &self.h264_pps {
                    out.push(v.clone());
                }
                out
            }
            Codec::Hevc => {
                let mut out = Vec::new();
                if let Some(v) = &self.hevc_vps {
                    out.push(v.clone());
                }
                if let Some(v) = &self.hevc_sps {
                    out.push(v.clone());
                }
                if let Some(v) = &self.hevc_pps {
                    out.push(v.clone());
                }
                out
            }
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
