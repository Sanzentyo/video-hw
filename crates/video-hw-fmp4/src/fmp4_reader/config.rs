use std::{collections::HashMap, fmt, num::NonZeroU32, path::PathBuf};

use anyhow::Context;
use shiguredo_mp4::{TrackKind, boxes::SampleEntry};
use video_hw::{Codec, EncodedLayout};

#[cfg(feature = "serde")]
mod serde_track_kind {
    use serde::{Deserialize, Deserializer, Serializer, de};
    use shiguredo_mp4::TrackKind;

    pub fn serialize<S>(kind: &TrackKind, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = match kind {
            TrackKind::Audio => "audio",
            TrackKind::Video => "video",
        };
        serializer.serialize_str(value)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<TrackKind, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "audio" => Ok(TrackKind::Audio),
            "video" => Ok(TrackKind::Video),
            other => Err(de::Error::unknown_variant(other, &["audio", "video"])),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrackId(pub u32);

impl fmt::Display for TrackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SampleId(pub u64);

impl fmt::Display for SampleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MediaTime {
    pub ticks: u64,
    pub timescale: NonZeroU32,
}

impl MediaTime {
    pub const fn new(ticks: u64, timescale: NonZeroU32) -> Self {
        Self { ticks, timescale }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum IndexMode {
    /// Build the full metadata index while opening the reader.
    #[default]
    Eager,
    /// Extend the metadata index on demand.
    ///
    /// APIs that return complete metadata slices, such as `samples`, still scan
    /// to EOF before returning. Point lookups and `next_sample` advance only as
    /// far as needed.
    Lazy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RangeCacheConfig {
    pub chunk_size: usize,
    pub max_bytes: usize,
    pub read_ahead_chunks: usize,
}

impl Default for RangeCacheConfig {
    fn default() -> Self {
        Self {
            chunk_size: 8 * 1024 * 1024,
            max_bytes: 512 * 1024 * 1024,
            read_ahead_chunks: 1,
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Fmp4ReaderConfig {
    pub input_path: PathBuf,
    pub index_mode: IndexMode,
    pub range_cache: RangeCacheConfig,
}

impl Fmp4ReaderConfig {
    pub fn new(input_path: impl Into<PathBuf>) -> Self {
        Self {
            input_path: input_path.into(),
            index_mode: IndexMode::default(),
            range_cache: RangeCacheConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Fmp4Track {
    pub track_id: TrackId,
    #[cfg_attr(feature = "serde", serde(with = "serde_track_kind"))]
    pub kind: TrackKind,
    pub duration: u64,
    pub timescale: NonZeroU32,
    #[cfg_attr(feature = "serde", serde(skip, default))]
    pub sample_entry: Option<SampleEntry>,
}

impl Fmp4Track {
    pub fn codec(&self) -> Option<Codec> {
        sample_entry_codec(self.sample_entry.as_ref())
    }

    pub fn encoded_layout(&self) -> Option<EncodedLayout> {
        sample_entry_layout(self.sample_entry.as_ref())
    }

    pub fn parameter_sets(&self) -> Vec<Vec<u8>> {
        sample_entry_parameter_sets(self.sample_entry.as_ref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SampleMeta {
    pub sample_id: SampleId,
    pub track_id: TrackId,
    pub offset: u64,
    pub size: u32,
    pub dts: MediaTime,
    pub pts: MediaTime,
    pub duration: u32,
    pub composition_time_offset: Option<i64>,
    pub keyframe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Mp4IndexSnapshot {
    pub tracks: Vec<Fmp4Track>,
    pub samples: Vec<SampleMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GopSegment {
    pub track_id: TrackId,
    pub keyframe_sample: SampleId,
    pub end_sample_exclusive: SampleId,
    pub start_pts: MediaTime,
    pub end_pts: MediaTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SampleRange {
    pub track_id: TrackId,
    pub start_sample: SampleId,
    pub end_sample_exclusive: SampleId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum SampleLookupMatch {
    Exact,
    Previous,
    FirstAfter,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SampleLookup {
    pub requested_pts: MediaTime,
    pub matched_sample: SampleId,
    pub matched_pts: MediaTime,
    pub delta_ticks: i128,
    pub delta_seconds: f64,
    pub match_type: SampleLookupMatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EncodedSample {
    pub meta: SampleMeta,
    #[cfg_attr(feature = "serde", serde(with = "serde_track_kind"))]
    pub kind: TrackKind,
    #[cfg_attr(feature = "serde", serde(skip, default))]
    pub sample_entry: Option<SampleEntry>,
    pub data: Vec<u8>,
}

impl EncodedSample {
    pub fn codec(&self) -> Option<Codec> {
        sample_entry_codec(self.sample_entry.as_ref())
    }

    pub fn encoded_layout(&self) -> Option<EncodedLayout> {
        sample_entry_layout(self.sample_entry.as_ref())
    }

    pub fn parameter_sets(&self) -> Vec<Vec<u8>> {
        sample_entry_parameter_sets(self.sample_entry.as_ref())
    }

    pub fn to_annexb(&self) -> anyhow::Result<Vec<u8>> {
        match self.encoded_layout() {
            Some(EncodedLayout::Avcc) | Some(EncodedLayout::Hvcc) => {
                let nal_length_size = sample_entry_nal_length_size(self.sample_entry.as_ref())?;
                let mut annexb = Vec::new();
                if self.meta.keyframe {
                    for parameter_set in self.parameter_sets() {
                        annexb.extend_from_slice(&[0, 0, 0, 1]);
                        annexb.extend_from_slice(&parameter_set);
                    }
                }
                let mut cursor = 0usize;
                while cursor < self.data.len() {
                    let len_bytes = self
                        .data
                        .get(cursor..cursor + nal_length_size)
                        .context("length-prefixed sample truncated before NAL length")?;
                    let nalu_len = read_be_nal_length(len_bytes);
                    cursor = cursor.saturating_add(nal_length_size);
                    let nalu = self
                        .data
                        .get(cursor..cursor + nalu_len)
                        .context("length-prefixed sample truncated inside NAL payload")?;
                    annexb.extend_from_slice(&[0, 0, 0, 1]);
                    annexb.extend_from_slice(nalu);
                    cursor = cursor.saturating_add(nalu_len);
                }
                Ok(annexb)
            }
            Some(EncodedLayout::AnnexB) => Ok(self.data.clone()),
            Some(EncodedLayout::Opaque) | None => {
                anyhow::bail!("cannot convert sample without a supported video sample entry")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RangeCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub resident_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrackReadStats {
    pub samples_read: u64,
    pub bytes_read: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SampleReadStats {
    pub reads: u64,
    pub bytes_read: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Fmp4ReaderStatus {
    pub samples_indexed: u64,
    pub samples_read: u64,
    pub bytes_read: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_evictions: u64,
    pub cache_resident_bytes: usize,
    pub range_cache_config: RangeCacheConfig,
    pub track_reads: HashMap<TrackId, TrackReadStats>,
    pub sample_reads: HashMap<SampleId, SampleReadStats>,
}

pub(crate) fn sample_entry_codec(sample_entry: Option<&SampleEntry>) -> Option<Codec> {
    match sample_entry? {
        SampleEntry::Avc1(_) => Some(Codec::H264),
        SampleEntry::Hev1(_) | SampleEntry::Hvc1(_) => Some(Codec::Hevc),
        _ => None,
    }
}

pub(crate) fn sample_entry_layout(sample_entry: Option<&SampleEntry>) -> Option<EncodedLayout> {
    match sample_entry? {
        SampleEntry::Avc1(_) => Some(EncodedLayout::Avcc),
        SampleEntry::Hev1(_) | SampleEntry::Hvc1(_) => Some(EncodedLayout::Hvcc),
        _ => None,
    }
}

pub(crate) fn sample_entry_parameter_sets(sample_entry: Option<&SampleEntry>) -> Vec<Vec<u8>> {
    match sample_entry {
        Some(SampleEntry::Avc1(avc1)) => avc1
            .avcc_box
            .sps_list
            .iter()
            .chain(avc1.avcc_box.pps_list.iter())
            .cloned()
            .collect(),
        Some(SampleEntry::Hev1(hev1)) => hev1
            .hvcc_box
            .nalu_arrays
            .iter()
            .flat_map(|array| array.nalus.iter().cloned())
            .collect(),
        Some(SampleEntry::Hvc1(hvc1)) => hvc1
            .hvcc_box
            .nalu_arrays
            .iter()
            .flat_map(|array| array.nalus.iter().cloned())
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn sample_entry_nal_length_size(
    sample_entry: Option<&SampleEntry>,
) -> anyhow::Result<usize> {
    let Some(sample_entry) = sample_entry else {
        anyhow::bail!("cannot determine NAL length size without a video sample entry");
    };
    let length_size_minus_one = match sample_entry {
        SampleEntry::Avc1(avc1) => avc1.avcc_box.length_size_minus_one.get(),
        SampleEntry::Hev1(hev1) => hev1.hvcc_box.length_size_minus_one.get(),
        SampleEntry::Hvc1(hvc1) => hvc1.hvcc_box.length_size_minus_one.get(),
        _ => anyhow::bail!("sample entry does not declare a supported NAL length size"),
    };
    match length_size_minus_one {
        0 => Ok(1),
        1 => Ok(2),
        3 => Ok(4),
        value => anyhow::bail!(
            "unsupported NAL length size: length_size_minus_one={} gives {} bytes",
            value,
            value + 1
        ),
    }
}

fn read_be_nal_length(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .fold(0usize, |value, byte| (value << 8) | usize::from(*byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shiguredo_mp4::{
        Uint,
        boxes::{Avc1Box, AvccBox, Hvc1Box, HvccBox, HvccNalUintArray, VisualSampleEntryFields},
    };

    #[test]
    fn h264_helpers_detect_codec_layout_and_annexb() {
        let sample_entry = h264_sample_entry(3);
        let sample = EncodedSample {
            meta: test_meta(true),
            kind: TrackKind::Video,
            sample_entry: Some(sample_entry.clone()),
            data: vec![0, 0, 0, 2, 0x65, 0x88],
        };
        assert_eq!(sample.codec(), Some(Codec::H264));
        assert_eq!(sample.encoded_layout(), Some(EncodedLayout::Avcc));
        let annexb = sample
            .to_annexb()
            .expect("annexb conversion should succeed");
        assert!(annexb.starts_with(&[0, 0, 0, 1, 0x67]));
        assert!(annexb.windows(5).any(|window| window == [0, 0, 0, 1, 0x65]));
        let track = Fmp4Track {
            track_id: TrackId(1),
            kind: TrackKind::Video,
            duration: 0,
            timescale: NonZeroU32::new(90_000).expect("non-zero timescale"),
            sample_entry: Some(sample_entry),
        };
        assert_eq!(track.codec(), Some(Codec::H264));
        assert_eq!(track.encoded_layout(), Some(EncodedLayout::Avcc));
        assert_eq!(track.parameter_sets().len(), 2);
    }

    #[test]
    fn h264_annexb_uses_declared_nal_length_size() {
        for (length_size_minus_one, data) in [
            (0, vec![2, 0x65, 0x88]),
            (1, vec![0, 2, 0x65, 0x88]),
            (3, vec![0, 0, 0, 2, 0x65, 0x88]),
        ] {
            let sample = EncodedSample {
                meta: test_meta(true),
                kind: TrackKind::Video,
                sample_entry: Some(h264_sample_entry(length_size_minus_one)),
                data,
            };
            let annexb = sample.to_annexb().expect("annexb conversion");
            assert!(annexb.windows(5).any(|window| window == [0, 0, 0, 1, 0x65]));
        }
        let invalid = EncodedSample {
            meta: test_meta(true),
            kind: TrackKind::Video,
            sample_entry: Some(h264_sample_entry(2)),
            data: vec![0, 0, 2, 0x65, 0x88],
        };
        assert!(invalid.to_annexb().is_err());
    }

    #[test]
    fn hevc_helpers_detect_codec_layout_and_annexb() {
        let sample_entry = hevc_sample_entry(3);
        let sample = EncodedSample {
            meta: test_meta(true),
            kind: TrackKind::Video,
            sample_entry: Some(sample_entry.clone()),
            data: vec![0, 0, 0, 2, 0x26, 0x01],
        };
        assert_eq!(sample.codec(), Some(Codec::Hevc));
        assert_eq!(sample.encoded_layout(), Some(EncodedLayout::Hvcc));
        let annexb = sample
            .to_annexb()
            .expect("annexb conversion should succeed");
        assert!(annexb.starts_with(&[0, 0, 0, 1, 0x40]));
        assert!(annexb.windows(5).any(|window| window == [0, 0, 0, 1, 0x26]));
        let track = Fmp4Track {
            track_id: TrackId(1),
            kind: TrackKind::Video,
            duration: 0,
            timescale: NonZeroU32::new(90_000).expect("non-zero timescale"),
            sample_entry: Some(sample_entry),
        };
        assert_eq!(track.codec(), Some(Codec::Hevc));
        assert_eq!(track.encoded_layout(), Some(EncodedLayout::Hvcc));
        assert_eq!(track.parameter_sets().len(), 3);
    }

    #[test]
    fn hevc_annexb_uses_declared_nal_length_size() {
        for (length_size_minus_one, data) in [
            (0, vec![2, 0x26, 0x01]),
            (1, vec![0, 2, 0x26, 0x01]),
            (3, vec![0, 0, 0, 2, 0x26, 0x01]),
        ] {
            let sample = EncodedSample {
                meta: test_meta(true),
                kind: TrackKind::Video,
                sample_entry: Some(hevc_sample_entry(length_size_minus_one)),
                data,
            };
            let annexb = sample.to_annexb().expect("annexb conversion");
            assert!(annexb.windows(5).any(|window| window == [0, 0, 0, 1, 0x26]));
        }
        let invalid = EncodedSample {
            meta: test_meta(true),
            kind: TrackKind::Video,
            sample_entry: Some(hevc_sample_entry(2)),
            data: vec![0, 0, 2, 0x26, 0x01],
        };
        assert!(invalid.to_annexb().is_err());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_roundtrips_reader_metadata_and_status() {
        let timescale = NonZeroU32::new(90_000).expect("non-zero timescale");
        let track = Fmp4Track {
            track_id: TrackId(7),
            kind: TrackKind::Video,
            duration: 9_000,
            timescale,
            sample_entry: Some(h264_sample_entry(3)),
        };
        let snapshot = Mp4IndexSnapshot {
            tracks: vec![track],
            samples: vec![test_meta(true)],
        };
        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        assert!(json.contains("\"kind\":\"video\""));
        assert!(!json.contains("sample_entry"));
        let decoded: Mp4IndexSnapshot = serde_json::from_str(&json).expect("deserialize snapshot");
        assert_eq!(decoded.tracks[0].kind, TrackKind::Video);
        assert!(decoded.tracks[0].sample_entry.is_none());
        assert_eq!(decoded.samples, snapshot.samples);

        let lookup = SampleLookup {
            requested_pts: MediaTime::new(1, timescale),
            matched_sample: SampleId(0),
            matched_pts: MediaTime::new(0, timescale),
            delta_ticks: -1,
            delta_seconds: -1.0 / 90_000.0,
            match_type: SampleLookupMatch::Previous,
        };
        let lookup_json = serde_json::to_string(&lookup).expect("serialize lookup");
        assert!(lookup_json.contains("\"match_type\":\"previous\""));

        let mut status = Fmp4ReaderStatus {
            samples_indexed: 1,
            ..Fmp4ReaderStatus::default()
        };
        status.track_reads.insert(
            TrackId(7),
            TrackReadStats {
                samples_read: 1,
                bytes_read: 6,
            },
        );
        status.sample_reads.insert(
            SampleId(0),
            SampleReadStats {
                reads: 1,
                bytes_read: 6,
            },
        );
        let status_json = serde_json::to_string(&status).expect("serialize status");
        let status_roundtrip: Fmp4ReaderStatus =
            serde_json::from_str(&status_json).expect("deserialize status");
        assert_eq!(status_roundtrip, status);
    }

    fn h264_sample_entry(length_size_minus_one: u8) -> SampleEntry {
        SampleEntry::Avc1(Avc1Box {
            visual: VisualSampleEntryFields {
                data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
                width: 640,
                height: 360,
                horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
                vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
                frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
                compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
                depth: VisualSampleEntryFields::DEFAULT_DEPTH,
            },
            avcc_box: AvccBox {
                avc_profile_indication: 100,
                profile_compatibility: 0,
                avc_level_indication: 30,
                length_size_minus_one: Uint::new(length_size_minus_one),
                sps_list: vec![vec![0x67, 0x64, 0x00, 0x1f]],
                pps_list: vec![vec![0x68, 0xee, 0x3c, 0x80]],
                chroma_format: Some(Uint::new(1)),
                bit_depth_luma_minus8: Some(Uint::new(0)),
                bit_depth_chroma_minus8: Some(Uint::new(0)),
                sps_ext_list: vec![],
            },
            unknown_boxes: vec![],
        })
    }

    fn hevc_sample_entry(length_size_minus_one: u8) -> SampleEntry {
        SampleEntry::Hvc1(Hvc1Box {
            visual: VisualSampleEntryFields {
                data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
                width: 640,
                height: 360,
                horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
                vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
                frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
                compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
                depth: VisualSampleEntryFields::DEFAULT_DEPTH,
            },
            hvcc_box: HvccBox {
                general_profile_space: Uint::new(0),
                general_tier_flag: Uint::new(0),
                general_profile_idc: Uint::new(1),
                general_profile_compatibility_flags: 0,
                general_constraint_indicator_flags: Uint::new(0),
                general_level_idc: 120,
                min_spatial_segmentation_idc: Uint::new(0),
                parallelism_type: Uint::new(0),
                chroma_format_idc: Uint::new(1),
                bit_depth_luma_minus8: Uint::new(0),
                bit_depth_chroma_minus8: Uint::new(0),
                avg_frame_rate: 0,
                constant_frame_rate: Uint::new(0),
                num_temporal_layers: Uint::new(0),
                temporal_id_nested: Uint::new(1),
                length_size_minus_one: Uint::new(length_size_minus_one),
                nalu_arrays: vec![
                    HvccNalUintArray {
                        array_completeness: Uint::new(1),
                        nal_unit_type: Uint::new(32),
                        nalus: vec![vec![0x40, 0x01]],
                    },
                    HvccNalUintArray {
                        array_completeness: Uint::new(1),
                        nal_unit_type: Uint::new(33),
                        nalus: vec![vec![0x42, 0x01]],
                    },
                    HvccNalUintArray {
                        array_completeness: Uint::new(1),
                        nal_unit_type: Uint::new(34),
                        nalus: vec![vec![0x44, 0x01]],
                    },
                ],
            },
            unknown_boxes: vec![],
        })
    }

    fn test_meta(keyframe: bool) -> SampleMeta {
        let timescale = NonZeroU32::new(90_000).expect("non-zero timescale");
        SampleMeta {
            sample_id: SampleId(0),
            track_id: TrackId(1),
            offset: 0,
            size: 6,
            dts: MediaTime::new(0, timescale),
            pts: MediaTime::new(0, timescale),
            duration: 3_000,
            composition_time_offset: None,
            keyframe,
        }
    }
}
