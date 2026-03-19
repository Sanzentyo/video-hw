use std::{num::NonZeroU32, path::PathBuf};

use shiguredo_mp4::{TrackKind, boxes::SampleEntry};
use video_hw::{Codec, EncodedLayout};

#[derive(Debug, Clone)]
pub struct Fmp4ReaderConfig {
    pub input_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fmp4Track {
    pub track_id: u32,
    pub kind: TrackKind,
    pub duration: u64,
    pub timescale: NonZeroU32,
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
pub struct Fmp4ReadSample {
    pub track_id: u32,
    pub kind: TrackKind,
    pub sample_entry: Option<SampleEntry>,
    pub keyframe: bool,
    pub timestamp: u64,
    pub duration: u32,
    pub composition_time_offset: Option<i64>,
    pub data: Vec<u8>,
}

impl Fmp4ReadSample {
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
                let mut annexb = Vec::new();
                if self.keyframe {
                    for parameter_set in self.parameter_sets() {
                        annexb.extend_from_slice(&[0, 0, 0, 1]);
                        annexb.extend_from_slice(&parameter_set);
                    }
                }
                let mut cursor = 0usize;
                while cursor < self.data.len() {
                    let len_bytes = self
                        .data
                        .get(cursor..cursor + 4)
                        .context("length-prefixed sample truncated before NAL length")?;
                    let nalu_len =
                        u32::from_be_bytes(len_bytes.try_into().expect("slice length")) as usize;
                    cursor = cursor.saturating_add(4);
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
pub struct Fmp4ReaderStatus {
    pub samples_read: u64,
}

fn sample_entry_codec(sample_entry: Option<&SampleEntry>) -> Option<Codec> {
    match sample_entry? {
        SampleEntry::Avc1(_) => Some(Codec::H264),
        SampleEntry::Hev1(_) | SampleEntry::Hvc1(_) => Some(Codec::Hevc),
        _ => None,
    }
}

fn sample_entry_layout(sample_entry: Option<&SampleEntry>) -> Option<EncodedLayout> {
    match sample_entry? {
        SampleEntry::Avc1(_) => Some(EncodedLayout::Avcc),
        SampleEntry::Hev1(_) | SampleEntry::Hvc1(_) => Some(EncodedLayout::Hvcc),
        _ => None,
    }
}

fn sample_entry_parameter_sets(sample_entry: Option<&SampleEntry>) -> Vec<Vec<u8>> {
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

use anyhow::Context;

#[cfg(test)]
mod tests {
    use super::*;
    use shiguredo_mp4::{
        Uint,
        boxes::{Avc1Box, AvccBox, Hvc1Box, HvccBox, HvccNalUintArray, VisualSampleEntryFields},
    };

    #[test]
    fn h264_helpers_detect_codec_layout_and_annexb() {
        let sample_entry = SampleEntry::Avc1(Avc1Box {
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
                length_size_minus_one: Uint::new(3),
                sps_list: vec![vec![0x67, 0x64, 0x00, 0x1f]],
                pps_list: vec![vec![0x68, 0xee, 0x3c, 0x80]],
                chroma_format: Some(Uint::new(1)),
                bit_depth_luma_minus8: Some(Uint::new(0)),
                bit_depth_chroma_minus8: Some(Uint::new(0)),
                sps_ext_list: vec![],
            },
            unknown_boxes: vec![],
        });
        let sample = Fmp4ReadSample {
            track_id: 1,
            kind: TrackKind::Video,
            sample_entry: Some(sample_entry.clone()),
            keyframe: true,
            timestamp: 0,
            duration: 3_000,
            composition_time_offset: None,
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
            track_id: 1,
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
    fn hevc_helpers_detect_codec_layout_and_annexb() {
        let sample_entry = SampleEntry::Hvc1(Hvc1Box {
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
                length_size_minus_one: Uint::new(3),
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
        });
        let sample = Fmp4ReadSample {
            track_id: 1,
            kind: TrackKind::Video,
            sample_entry: Some(sample_entry.clone()),
            keyframe: true,
            timestamp: 0,
            duration: 3_000,
            composition_time_offset: None,
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
            track_id: 1,
            kind: TrackKind::Video,
            duration: 0,
            timescale: NonZeroU32::new(90_000).expect("non-zero timescale"),
            sample_entry: Some(sample_entry),
        };
        assert_eq!(track.codec(), Some(Codec::Hevc));
        assert_eq!(track.encoded_layout(), Some(EncodedLayout::Hvcc));
        assert_eq!(track.parameter_sets().len(), 3);
    }
}
