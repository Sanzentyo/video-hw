use crate::{Codec, EncodedLayout};

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum BitstreamError {
    #[error("invalid bitstream: {0}")]
    Invalid(String),
    #[error("unsupported bitstream layout: codec={codec}, layout={layout}")]
    UnsupportedLayout { codec: Codec, layout: EncodedLayout },
    #[error("opaque encoded payload cannot be converted")]
    OpaquePayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnexBAccessUnit(Vec<u8>);

impl AnnexBAccessUnit {
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

    /// Reports whether this VCL NAL starts a new coded picture.
    ///
    /// `None` means that the codec does not use NAL units, the NAL is not a
    /// supported VCL type, or its slice header is truncated or malformed.
    pub fn starts_new_picture(self, codec: Codec) -> Option<bool> {
        match codec {
            Codec::H264 if matches!(h264_nal_type(self.0), Some(1..=5)) => {
                h264_first_mb_in_slice(self.0).map(|first_mb| first_mb == 0)
            }
            Codec::Hevc if matches!(hevc_nal_type(self.0), Some(0..=31)) => {
                hevc_first_slice_segment_in_pic_flag(self.0)
            }
            Codec::H264 | Codec::Hevc | Codec::Av1 => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LengthPrefixedSample(Vec<u8>);

impl LengthPrefixedSample {
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
    #[must_use]
    pub const fn codec(&self) -> Codec {
        match self {
            Self::H264(_) => Codec::H264,
            Self::Hevc(_) => Codec::Hevc,
            Self::Av1(_) => Codec::Av1,
        }
    }

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodePayload(Vec<u8>);

impl DecodePayload {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.0
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
        let mut end = starts
            .get(index + 1)
            .map(|(next, _)| *next)
            .unwrap_or(bytes.len());
        while end > payload_start && bytes[end - 1] == 0 {
            end -= 1;
        }
        if payload_start >= end {
            return Err(BitstreamError::Invalid(format!(
                "Annex-B payload contains an empty NAL unit at byte {start}"
            )));
        }
        nalus.push(NalUnitRef(&bytes[payload_start..end]));
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
    if payload.is_empty() {
        return Err(BitstreamError::Invalid(
            "length-prefixed sample is empty".to_string(),
        ));
    }
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
    payload: EncodedPayloadRef<'_>,
) -> Result<ParameterSets, BitstreamError> {
    validate_layout(payload.codec, payload.layout)?;
    if payload.codec == Codec::Av1 {
        return Ok(ParameterSets::Av1(Av1ConfigObus {
            config_obus: vec![payload.data.to_vec()],
        }));
    }

    let annexb = payload_to_annexb(payload, NalLengthSize::Four)?;
    let nalus = split_annexb_nalus(annexb.as_ref())?;
    Ok(match payload.codec {
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
        Codec::Av1 => unreachable!("AV1 is returned before Annex-B parsing"),
    })
}

pub fn payload_to_annexb(
    payload: EncodedPayloadRef<'_>,
    nal_length_size: NalLengthSize,
) -> Result<AnnexBAccessUnit, BitstreamError> {
    validate_layout(payload.codec, payload.layout)?;
    match payload.layout {
        EncodedLayout::AnnexB => Ok(AnnexBAccessUnit(payload.data.to_vec())),
        EncodedLayout::Avcc | EncodedLayout::Hvcc => {
            length_prefixed_to_annexb(LengthPrefixedSampleRef(payload.data), nal_length_size)
        }
        EncodedLayout::Av1 => Err(BitstreamError::UnsupportedLayout {
            codec: payload.codec,
            layout: payload.layout,
        }),
        EncodedLayout::Opaque => Err(BitstreamError::OpaquePayload),
    }
}

pub fn payload_to_nalus(
    payload: EncodedPayloadRef<'_>,
    nal_length_size: NalLengthSize,
) -> Result<Vec<NalUnit>, BitstreamError> {
    validate_layout(payload.codec, payload.layout)?;
    match payload.layout {
        EncodedLayout::Av1 => Err(BitstreamError::UnsupportedLayout {
            codec: payload.codec,
            layout: payload.layout,
        }),
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
    validate_layout(payload.codec, payload.layout)?;
    match payload.layout {
        EncodedLayout::Av1 => Ok(DecodePayload(payload.data.to_vec())),
        _ => Ok(DecodePayload(
            payload_to_annexb(payload, nal_length_size)?.into_inner(),
        )),
    }
}

fn validate_layout(codec: Codec, layout: EncodedLayout) -> Result<(), BitstreamError> {
    if layout == EncodedLayout::Opaque {
        return Err(BitstreamError::OpaquePayload);
    }
    let valid = matches!(
        (codec, layout),
        (Codec::H264, EncodedLayout::AnnexB | EncodedLayout::Avcc)
            | (Codec::Hevc, EncodedLayout::AnnexB | EncodedLayout::Hvcc)
            | (Codec::Av1, EncodedLayout::Av1)
    );
    if valid {
        Ok(())
    } else {
        Err(BitstreamError::UnsupportedLayout { codec, layout })
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
    fn empty_length_prefixed_sample_is_rejected() {
        let error =
            length_prefixed_to_annexb(LengthPrefixedSampleRef::new(&[]), NalLengthSize::Four)
                .unwrap_err();

        assert_eq!(
            error,
            BitstreamError::Invalid("length-prefixed sample is empty".to_string())
        );
    }

    #[test]
    fn opaque_payload_is_typed_error() {
        let payload = EncodedPayloadRef {
            codec: Codec::H264,
            layout: EncodedLayout::Opaque,
            data: &[1, 2, 3],
        };
        assert_eq!(
            payload_to_annexb(payload, NalLengthSize::Four),
            Err(BitstreamError::OpaquePayload)
        );
    }

    #[test]
    fn codec_and_layout_mismatch_is_rejected() {
        let payload = EncodedPayloadRef {
            codec: Codec::H264,
            layout: EncodedLayout::Hvcc,
            data: &[0, 0, 0, 1],
        };

        assert_eq!(
            payload_to_decode_payload(payload, NalLengthSize::Four),
            Err(BitstreamError::UnsupportedLayout {
                codec: Codec::H264,
                layout: EncodedLayout::Hvcc,
            })
        );
    }

    #[test]
    fn av1_payload_is_not_mislabeled_as_annex_b_or_nal_units() {
        let payload = EncodedPayloadRef {
            codec: Codec::Av1,
            layout: EncodedLayout::Av1,
            data: &[0x12, 0x00],
        };

        assert!(matches!(
            payload_to_annexb(payload, NalLengthSize::Four),
            Err(BitstreamError::UnsupportedLayout { .. })
        ));
        assert!(matches!(
            payload_to_nalus(payload, NalLengthSize::Four),
            Err(BitstreamError::UnsupportedLayout { .. })
        ));
        assert_eq!(
            payload_to_decode_payload(payload, NalLengthSize::Four)
                .expect("AV1 decode payload remains supported")
                .as_bytes(),
            payload.data
        );
    }

    #[test]
    fn annexb_split_excludes_trailing_zero_bytes_between_nalus() {
        let data =
            AnnexBAccessUnitRef::new(&[0, 0, 0, 1, 0x65, 0x80, 0, 0, 0, 0, 1, 0x41, 0x80, 0, 0]);

        let nalus = split_annexb_nalus(data).expect("valid Annex-B payload");

        assert_eq!(
            nalus
                .into_iter()
                .map(NalUnitRef::as_bytes)
                .collect::<Vec<_>>(),
            vec![&[0x65, 0x80][..], &[0x41, 0x80][..]]
        );
    }

    #[test]
    fn slice_headers_distinguish_first_and_following_h264_slices() {
        assert_eq!(
            NalUnitRef::new(&[0x65, 0x80]).starts_new_picture(Codec::H264),
            Some(true)
        );
        assert_eq!(
            NalUnitRef::new(&[0x41, 0x40]).starts_new_picture(Codec::H264),
            Some(false)
        );
        assert_eq!(
            NalUnitRef::new(&[0x41]).starts_new_picture(Codec::H264),
            None
        );
    }
}
