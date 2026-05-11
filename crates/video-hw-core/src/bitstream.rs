use std::marker::PhantomData;

use crate::{Codec, EncodedLayout, Timestamp90k};

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum BitstreamError {
    #[error("invalid bitstream: {0}")]
    Invalid(String),
    #[error("unsupported bitstream layout: codec={codec}, layout={layout}")]
    UnsupportedLayout { codec: Codec, layout: EncodedLayout },
    #[error("opaque encoded payload cannot be converted")]
    OpaquePayload,
    #[error("access unit is empty")]
    EmptyAccessUnit,
    #[error("access unit exceeds configured byte limit: {actual} > {limit}")]
    AccessUnitTooLarge { actual: usize, limit: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationLevel {
    Full,
    StructuralOnly,
    TrustCaller,
}

impl Default for ValidationLevel {
    fn default() -> Self {
        Self::StructuralOnly
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyPolicy {
    BorrowWhenPossible,
    AlwaysOwned,
}

impl Default for CopyPolicy {
    fn default() -> Self {
        Self::BorrowWhenPossible
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BitstreamParseOptions {
    pub validation: ValidationLevel,
    pub copy_policy: CopyPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnexBAccessUnit(Vec<u8>);

impl AnnexBAccessUnit {
    pub fn new(data: Vec<u8>) -> Self {
        Self(data)
    }

    pub fn as_ref(&self) -> AnnexBAccessUnitRef<'_> {
        AnnexBAccessUnitRef(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnexBAccessUnitRef<'a>(&'a [u8]);

impl<'a> AnnexBAccessUnitRef<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self(data)
    }

    pub fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    pub fn to_owned_access_unit(self) -> AnnexBAccessUnit {
        AnnexBAccessUnit(self.0.to_vec())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NalUnit(Vec<u8>);

impl NalUnit {
    pub fn new(data: Vec<u8>) -> Self {
        Self(strip_start_code(&data).to_vec())
    }

    pub fn as_ref(&self) -> NalUnitRef<'_> {
        NalUnitRef(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NalUnitRef<'a>(&'a [u8]);

impl<'a> NalUnitRef<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self(strip_start_code(data))
    }

    pub fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    pub fn to_owned_nal(self) -> NalUnit {
        NalUnit(self.0.to_vec())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LengthPrefixedSample(Vec<u8>);

impl LengthPrefixedSample {
    pub fn new(data: Vec<u8>) -> Self {
        Self(data)
    }

    pub fn as_ref(&self) -> LengthPrefixedSampleRef<'_> {
        LengthPrefixedSampleRef(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LengthPrefixedSampleRef<'a>(&'a [u8]);

impl<'a> LengthPrefixedSampleRef<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self(data)
    }

    pub fn as_bytes(self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct H264ParameterSets {
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HevcParameterSets {
    pub vps: Vec<Vec<u8>>,
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Av1ConfigObus {
    pub config_obus: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterSets {
    H264(H264ParameterSets),
    Hevc(HevcParameterSets),
    Av1(Av1ConfigObus),
}

impl ParameterSets {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::H264(sets) => sets.sps.is_empty() && sets.pps.is_empty(),
            Self::Hevc(sets) => sets.vps.is_empty() && sets.sps.is_empty() && sets.pps.is_empty(),
            Self::Av1(sets) => sets.config_obus.is_empty(),
        }
    }

    pub fn all(&self) -> Vec<&[u8]> {
        match self {
            Self::H264(sets) => sets
                .sps
                .iter()
                .chain(sets.pps.iter())
                .map(Vec::as_slice)
                .collect(),
            Self::Hevc(sets) => sets
                .vps
                .iter()
                .chain(sets.sps.iter())
                .chain(sets.pps.iter())
                .map(Vec::as_slice)
                .collect(),
            Self::Av1(sets) => sets.config_obus.iter().map(Vec::as_slice).collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NalLengthSize {
    One,
    Two,
    Four,
}

impl NalLengthSize {
    pub fn bytes(self) -> usize {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Four => 4,
        }
    }

    pub fn from_bytes(value: usize) -> Result<Self, BitstreamError> {
        match value {
            1 => Ok(Self::One),
            2 => Ok(Self::Two),
            4 => Ok(Self::Four),
            _ => Err(BitstreamError::Invalid(format!(
                "unsupported NAL length size: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedPayloadRef<'a> {
    pub codec: Codec,
    pub layout: EncodedLayout,
    pub data: &'a [u8],
    pub is_keyframe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodePayload(Vec<u8>);

impl DecodePayload {
    pub fn new(data: Vec<u8>) -> Self {
        Self(data)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccessUnitAssemblerOptions {
    pub preallocate_bytes: Option<usize>,
    pub max_access_unit_bytes: Option<usize>,
    pub copy_policy: CopyPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Idle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Collecting;

#[derive(Debug)]
pub struct AccessUnitAssembler<State> {
    codec: Codec,
    options: AccessUnitAssemblerOptions,
    pts_90k: Option<Timestamp90k>,
    data: Vec<u8>,
    _state: PhantomData<State>,
}

impl AccessUnitAssembler<Idle> {
    pub fn new(codec: Codec, options: AccessUnitAssemblerOptions) -> Self {
        Self {
            codec,
            options,
            pts_90k: None,
            data: Vec::with_capacity(options.preallocate_bytes.unwrap_or(0)),
            _state: PhantomData,
        }
    }

    pub fn push_chunk(
        mut self,
        chunk: NalUnitRef<'_>,
        pts_90k: Option<Timestamp90k>,
    ) -> AccessUnitAssembler<Collecting> {
        append_annexb_nalu(&mut self.data, chunk.as_bytes());
        AccessUnitAssembler {
            codec: self.codec,
            options: self.options,
            pts_90k,
            data: self.data,
            _state: PhantomData,
        }
    }
}

impl AccessUnitAssembler<Collecting> {
    pub fn push_chunk(&mut self, chunk: NalUnitRef<'_>) -> Result<(), BitstreamError> {
        append_annexb_nalu(&mut self.data, chunk.as_bytes());
        self.validate_size()
    }

    pub fn finish(self) -> Result<(AnnexBAccessUnit, AccessUnitAssembler<Idle>), BitstreamError> {
        if self.data.is_empty() {
            return Err(BitstreamError::EmptyAccessUnit);
        }
        self.validate_size()?;
        let idle = AccessUnitAssembler {
            codec: self.codec,
            options: self.options,
            pts_90k: None,
            data: Vec::with_capacity(self.options.preallocate_bytes.unwrap_or(0)),
            _state: PhantomData,
        };
        Ok((AnnexBAccessUnit(self.data), idle))
    }

    pub fn discard(self) -> AccessUnitAssembler<Idle> {
        AccessUnitAssembler {
            codec: self.codec,
            options: self.options,
            pts_90k: None,
            data: Vec::with_capacity(self.options.preallocate_bytes.unwrap_or(0)),
            _state: PhantomData,
        }
    }

    pub fn pts_90k(&self) -> Option<Timestamp90k> {
        self.pts_90k
    }

    fn validate_size(&self) -> Result<(), BitstreamError> {
        if let Some(limit) = self.options.max_access_unit_bytes
            && self.data.len() > limit
        {
            return Err(BitstreamError::AccessUnitTooLarge {
                actual: self.data.len(),
                limit,
            });
        }
        Ok(())
    }
}

pub fn split_annexb_nalus(
    data: AnnexBAccessUnitRef<'_>,
) -> Result<Vec<NalUnitRef<'_>>, BitstreamError> {
    let bytes = data.as_bytes();
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 3 <= bytes.len() {
        if i + 4 <= bytes.len() && bytes[i..i + 4] == [0, 0, 0, 1] {
            starts.push((i, 4));
            i += 4;
        } else if bytes[i..i + 3] == [0, 0, 1] {
            starts.push((i, 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    if starts.is_empty() {
        return Err(BitstreamError::Invalid(
            "Annex-B payload does not contain a start code".to_string(),
        ));
    }

    let mut nalus = Vec::new();
    for (index, (start, prefix_len)) in starts.iter().copied().enumerate() {
        let payload_start = start + prefix_len;
        let end = starts
            .get(index + 1)
            .map(|(next, _)| *next)
            .unwrap_or(bytes.len());
        if payload_start < end {
            nalus.push(NalUnitRef(&bytes[payload_start..end]));
        }
    }
    Ok(nalus)
}

pub fn annexb_to_length_prefixed(
    data: AnnexBAccessUnitRef<'_>,
    nal_length_size: NalLengthSize,
) -> Result<LengthPrefixedSample, BitstreamError> {
    let nalus = split_annexb_nalus(data)?;
    let mut out = Vec::new();
    for nalu in nalus {
        write_be_nal_length(&mut out, nalu.as_bytes().len(), nal_length_size)?;
        out.extend_from_slice(nalu.as_bytes());
    }
    Ok(LengthPrefixedSample(out))
}

pub fn length_prefixed_to_annexb(
    data: LengthPrefixedSampleRef<'_>,
    nal_length_size: NalLengthSize,
) -> Result<AnnexBAccessUnit, BitstreamError> {
    let mut out = Vec::new();
    let mut payload = data.as_bytes();
    let len_size = nal_length_size.bytes();
    while payload.len() >= len_size {
        let nal_len = read_be_nal_length(&payload[..len_size]);
        payload = &payload[len_size..];
        if nal_len == 0 || payload.len() < nal_len {
            return Err(BitstreamError::Invalid(
                "invalid length-prefixed sample payload".to_string(),
            ));
        }
        append_annexb_nalu(&mut out, &payload[..nal_len]);
        payload = &payload[nal_len..];
    }
    if !payload.is_empty() {
        return Err(BitstreamError::Invalid(
            "trailing bytes after length-prefixed sample parse".to_string(),
        ));
    }
    Ok(AnnexBAccessUnit(out))
}

pub fn append_annexb_nalu(out: &mut Vec<u8>, nalu_or_start_coded_nalu: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(strip_start_code(nalu_or_start_coded_nalu));
}

pub fn split_access_units(
    codec: Codec,
    annexb: AnnexBAccessUnitRef<'_>,
) -> Result<Vec<AnnexBAccessUnit>, BitstreamError> {
    let nalus = split_annexb_nalus(annexb)?;
    let mut units = Vec::new();
    let mut current = Vec::new();
    let mut seen_vcl = false;
    for nalu in nalus {
        let classification = classify_nalu(codec, nalu.as_bytes());
        let boundary_before = match classification {
            NaluClassification::Boundary => seen_vcl && !current.is_empty(),
            NaluClassification::Vcl {
                first_slice: Some(false),
            } => false,
            NaluClassification::Vcl { .. } => seen_vcl && !current.is_empty(),
            NaluClassification::Other => false,
        };
        if boundary_before {
            units.push(AnnexBAccessUnit(std::mem::take(&mut current)));
            seen_vcl = false;
        }
        append_annexb_nalu(&mut current, nalu.as_bytes());
        if matches!(classification, NaluClassification::Vcl { .. }) {
            seen_vcl = true;
        }
    }
    if !current.is_empty() {
        units.push(AnnexBAccessUnit(current));
    }
    Ok(units)
}

pub fn extract_parameter_sets(
    codec: Codec,
    payload: EncodedPayloadRef<'_>,
) -> Result<ParameterSets, BitstreamError> {
    let annexb = payload_to_annexb(payload, NalLengthSize::Four)?;
    let nalus = split_annexb_nalus(annexb.as_ref())?;
    Ok(match codec {
        Codec::H264 => {
            let mut sets = H264ParameterSets::default();
            for nalu in nalus {
                match h264_nal_type(nalu.as_bytes()) {
                    Some(7) => sets.sps.push(nalu.as_bytes().to_vec()),
                    Some(8) => sets.pps.push(nalu.as_bytes().to_vec()),
                    _ => {}
                }
            }
            ParameterSets::H264(sets)
        }
        Codec::Hevc => {
            let mut sets = HevcParameterSets::default();
            for nalu in nalus {
                match hevc_nal_type(nalu.as_bytes()) {
                    Some(32) => sets.vps.push(nalu.as_bytes().to_vec()),
                    Some(33) => sets.sps.push(nalu.as_bytes().to_vec()),
                    Some(34) => sets.pps.push(nalu.as_bytes().to_vec()),
                    _ => {}
                }
            }
            ParameterSets::Hevc(sets)
        }
        Codec::Av1 => ParameterSets::Av1(Av1ConfigObus {
            config_obus: vec![payload.data.to_vec()],
        }),
    })
}

pub fn payload_to_annexb(
    payload: EncodedPayloadRef<'_>,
    nal_length_size: NalLengthSize,
) -> Result<AnnexBAccessUnit, BitstreamError> {
    match payload.layout {
        EncodedLayout::AnnexB => Ok(AnnexBAccessUnit(payload.data.to_vec())),
        EncodedLayout::Avcc | EncodedLayout::Hvcc => {
            length_prefixed_to_annexb(LengthPrefixedSampleRef(payload.data), nal_length_size)
        }
        EncodedLayout::Av1 => Ok(AnnexBAccessUnit(payload.data.to_vec())),
        EncodedLayout::Opaque => Err(BitstreamError::OpaquePayload),
    }
}

pub fn payload_to_nalus(
    payload: EncodedPayloadRef<'_>,
    nal_length_size: NalLengthSize,
) -> Result<Vec<NalUnit>, BitstreamError> {
    match payload.layout {
        EncodedLayout::Av1 => Ok(vec![NalUnit(payload.data.to_vec())]),
        _ => Ok(
            split_annexb_nalus(payload_to_annexb(payload, nal_length_size)?.as_ref())?
                .into_iter()
                .map(NalUnitRef::to_owned_nal)
                .collect(),
        ),
    }
}

pub fn payload_to_decode_payload(
    payload: EncodedPayloadRef<'_>,
    nal_length_size: NalLengthSize,
) -> Result<DecodePayload, BitstreamError> {
    match payload.layout {
        EncodedLayout::Av1 => Ok(DecodePayload(payload.data.to_vec())),
        _ => Ok(DecodePayload(
            payload_to_annexb(payload, nal_length_size)?.into_inner(),
        )),
    }
}

fn strip_start_code(data: &[u8]) -> &[u8] {
    if data.starts_with(&[0, 0, 0, 1]) {
        &data[4..]
    } else if data.starts_with(&[0, 0, 1]) {
        &data[3..]
    } else {
        data
    }
}

fn read_be_nal_length(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .fold(0usize, |value, byte| (value << 8) | usize::from(*byte))
}

fn write_be_nal_length(
    out: &mut Vec<u8>,
    value: usize,
    size: NalLengthSize,
) -> Result<(), BitstreamError> {
    let max = match size {
        NalLengthSize::One => u8::MAX as usize,
        NalLengthSize::Two => u16::MAX as usize,
        NalLengthSize::Four => u32::MAX as usize,
    };
    if value > max {
        return Err(BitstreamError::Invalid(format!(
            "NAL unit length {value} does not fit in {} bytes",
            size.bytes()
        )));
    }
    let be = (value as u32).to_be_bytes();
    out.extend_from_slice(&be[4 - size.bytes()..]);
    Ok(())
}

fn h264_nal_type(nalu: &[u8]) -> Option<u8> {
    nalu.first().map(|byte| byte & 0x1f)
}

fn hevc_nal_type(nalu: &[u8]) -> Option<u8> {
    nalu.first().map(|byte| (byte >> 1) & 0x3f)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NaluClassification {
    Vcl { first_slice: Option<bool> },
    Boundary,
    Other,
}

fn classify_nalu(codec: Codec, nalu: &[u8]) -> NaluClassification {
    match codec {
        Codec::H264 => match h264_nal_type(nalu) {
            Some(1..=5) => NaluClassification::Vcl {
                first_slice: h264_first_mb_in_slice(nalu).map(|first_mb| first_mb == 0),
            },
            Some(6..=9) => NaluClassification::Boundary,
            _ => NaluClassification::Other,
        },
        Codec::Hevc => match hevc_nal_type(nalu) {
            Some(0..=31) => NaluClassification::Vcl {
                first_slice: hevc_first_slice_segment_in_pic_flag(nalu),
            },
            Some(32..=35 | 39 | 40) => NaluClassification::Boundary,
            _ => NaluClassification::Other,
        },
        Codec::Av1 => NaluClassification::Other,
    }
}

fn h264_first_mb_in_slice(nalu: &[u8]) -> Option<u32> {
    let rbsp = rbsp_from_ebsp(nalu.get(1..)?);
    let mut bits = BitReader::new(&rbsp);
    bits.read_ue()
}

fn hevc_first_slice_segment_in_pic_flag(nalu: &[u8]) -> Option<bool> {
    let rbsp = rbsp_from_ebsp(nalu.get(2..)?);
    let mut bits = BitReader::new(&rbsp);
    bits.read_bit()
}

fn rbsp_from_ebsp(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zero_run = 0usize;
    for &byte in payload {
        if zero_run >= 2 && byte == 0x03 {
            zero_run = 0;
            continue;
        }
        out.push(byte);
        if byte == 0 {
            zero_run += 1;
        } else {
            zero_run = 0;
        }
    }
    out
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bit(&mut self) -> Option<bool> {
        if self.bit_pos >= self.data.len().checked_mul(8)? {
            return None;
        }
        let byte = self.data[self.bit_pos / 8];
        let shift = 7 - (self.bit_pos % 8);
        self.bit_pos += 1;
        Some(((byte >> shift) & 1) != 0)
    }

    fn read_bits(&mut self, bits: usize) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..bits {
            value <<= 1;
            if self.read_bit()? {
                value |= 1;
            }
        }
        Some(value)
    }

    fn read_ue(&mut self) -> Option<u32> {
        let mut leading_zero_bits = 0usize;
        while !self.read_bit()? {
            leading_zero_bits += 1;
            if leading_zero_bits > 31 {
                return None;
            }
        }
        let suffix = if leading_zero_bits == 0 {
            0
        } else {
            self.read_bits(leading_zero_bits)?
        };
        Some(((1u32 << leading_zero_bits) - 1) + suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_prefixed_roundtrip_uses_configured_length_size() {
        let annexb = AnnexBAccessUnitRef::new(&[0, 0, 0, 1, 0x67, 0x64, 0, 0, 1, 0x68]);
        let expected = [0, 0, 0, 1, 0x67, 0x64, 0, 0, 0, 1, 0x68];
        for size in [NalLengthSize::One, NalLengthSize::Two, NalLengthSize::Four] {
            let sample = annexb_to_length_prefixed(annexb, size).unwrap();
            let converted = length_prefixed_to_annexb(sample.as_ref(), size).unwrap();
            assert_eq!(converted.as_bytes(), expected);
        }
    }

    #[test]
    fn opaque_payload_is_typed_error() {
        let payload = EncodedPayloadRef {
            codec: Codec::H264,
            layout: EncodedLayout::Opaque,
            data: &[1, 2, 3],
            is_keyframe: false,
        };
        assert_eq!(
            payload_to_annexb(payload, NalLengthSize::Four),
            Err(BitstreamError::OpaquePayload)
        );
    }
}
