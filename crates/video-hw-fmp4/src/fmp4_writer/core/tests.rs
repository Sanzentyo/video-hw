use super::*;

#[test]
fn av1_chunk_preserves_obu_payload_and_caches_sequence_header() {
    let sequence_header = av1_obu(1, &[0x08, 0x00]);
    let frame = av1_obu(6, &[0x80]);
    let mut chunk = av1_obu(2, &[]);
    chunk.extend_from_slice(&sequence_header);
    chunk.extend_from_slice(&frame);

    let mut cached_sequence_header = None;
    let mut cached_config = None;
    let sample = av1_chunk_to_av1_sample(&chunk, &mut cached_sequence_header, &mut cached_config)
        .expect("AV1 sample conversion")
        .expect("sample payload");

    assert_eq!(sample, chunk);
    assert_eq!(
        cached_sequence_header.as_deref(),
        Some(sequence_header.as_slice())
    );
    assert!(cached_config.is_some());

    let second_frame = av1_obu(6, &[0x81]);
    let second_sample = av1_chunk_to_av1_sample(
        &second_frame,
        &mut cached_sequence_header,
        &mut cached_config,
    )
    .expect("second AV1 sample conversion")
    .expect("second sample payload");
    assert_eq!(second_sample, second_frame);
    assert_eq!(
        cached_sequence_header.as_deref(),
        Some(sequence_header.as_slice())
    );
}

#[test]
fn av1_chunk_rejects_malformed_sequence_header_without_mutating_cache() {
    let malformed_sequence_header = av1_obu(1, &[]);
    let mut cached_sequence_header = None;
    let mut cached_config = None;

    let error = av1_chunk_to_av1_sample(
        &malformed_sequence_header,
        &mut cached_sequence_header,
        &mut cached_config,
    )
    .expect_err("truncated AV1 sequence header must be rejected");

    assert!(error.to_string().contains("malformed sequence header"));
    assert!(cached_sequence_header.is_none());
    assert!(cached_config.is_none());
}

#[test]
fn av1_chunk_refreshes_cached_configuration_when_sequence_header_changes() {
    let initial_sequence_header = av1_obu(1, &[0x08, 0x00]);
    let changed_sequence_header = av1_obu(1, &[0x28, 0x00]);
    let mut cached_sequence_header = None;
    let mut cached_config = None;

    av1_chunk_to_av1_sample(
        &initial_sequence_header,
        &mut cached_sequence_header,
        &mut cached_config,
    )
    .expect("initial AV1 configuration")
    .expect("initial AV1 configuration payload");
    assert_eq!(cached_config.expect("initial summary").seq_profile, 0);

    av1_chunk_to_av1_sample(
        &changed_sequence_header,
        &mut cached_sequence_header,
        &mut cached_config,
    )
    .expect("changed AV1 configuration")
    .expect("changed AV1 configuration payload");

    assert_eq!(
        cached_sequence_header.as_deref(),
        Some(changed_sequence_header.as_slice())
    );
    assert_eq!(cached_config.expect("changed summary").seq_profile, 1);
}

#[test]
fn av1_sample_entry_uses_sequence_header_as_av1c_config_obus() {
    let sequence_header = av1_obu(1, &[0x00]);
    let entry = create_av1_sample_entry(
        320,
        180,
        &sequence_header,
        Av1ConfigSummary {
            seq_profile: 1,
            seq_level_idx_0: 8,
            seq_tier_0: 1,
            high_bitdepth: 1,
            twelve_bit: 0,
            monochrome: 0,
            chroma_subsampling_x: 1,
            chroma_subsampling_y: 0,
            chroma_sample_position: 2,
        },
    );

    let SampleEntry::Av01(av01) = entry else {
        panic!("expected av01 sample entry");
    };
    assert_eq!(av01.visual.width, 320);
    assert_eq!(av01.visual.height, 180);
    assert_eq!(av01.av1c_box.config_obus, sequence_header);
    assert_eq!(av01.av1c_box.seq_profile.get(), 1);
    assert_eq!(av01.av1c_box.seq_level_idx_0.get(), 8);
    assert_eq!(av01.av1c_box.seq_tier_0.get(), 1);
    assert_eq!(av01.av1c_box.high_bitdepth.get(), 1);
    assert_eq!(av01.av1c_box.chroma_subsampling_y.get(), 0);
    assert_eq!(av01.av1c_box.chroma_sample_position.get(), 2);
}

#[test]
fn h264_chunk_rejects_multiple_active_sequence_parameter_sets() {
    let mut chunk = Vec::new();
    video_hw::bitstream::append_annexb_nalu(&mut chunk, &[0x67, 66, 0, 30, 0x80]);
    video_hw::bitstream::append_annexb_nalu(&mut chunk, &[0x67, 66, 0, 31, 0x80]);
    let mut sps = None;
    let mut pps = None;

    let error = annexb_chunk_to_avcc_sample(&chunk, &mut sps, &mut pps)
        .expect_err("multiple active SPS values must not be silently discarded");

    assert!(error.to_string().contains("multiple active H.264 SPS"));
}

#[test]
fn annexb_split_drops_trailing_zero_bytes() {
    let bitstream = [0, 0, 0, 1, 0x65, 0x80, 0, 0, 0, 0, 1, 0x41, 0x80, 0, 0];

    assert_eq!(
        split_annexb_nalus(&bitstream),
        vec![&[0x65, 0x80][..], &[0x41, 0x80][..]]
    );
}

#[test]
fn duration_rescale_uses_wide_intermediate_arithmetic() {
    let three = NonZeroU32::new(3).expect("non-zero timescale");
    let two = NonZeroU32::new(2).expect("non-zero timescale");
    assert_eq!(
        convert_duration_timescale(u64::MAX, three, two),
        12_297_829_382_473_034_410
    );

    let one = NonZeroU32::new(1).expect("non-zero timescale");
    let max = NonZeroU32::new(u32::MAX).expect("non-zero timescale");
    assert_eq!(convert_duration_timescale(u64::MAX, one, max), u64::MAX);
}

#[test]
fn hevc_sample_entry_uses_profile_and_pixel_fields_from_sps() {
    let bitstream = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("sample-videos")
            .join("sample-10s.h265"),
    )
    .expect("repository HEVC sample should be readable");
    let nalus = split_annexb_nalus(&bitstream);
    let vps = nalus
        .iter()
        .copied()
        .find(|nalu| hevc_nal_type(nalu) == 32)
        .expect("HEVC sample should contain VPS");
    let sps = nalus
        .iter()
        .copied()
        .find(|nalu| hevc_nal_type(nalu) == 33)
        .expect("HEVC sample should contain SPS");
    let pps = nalus
        .iter()
        .copied()
        .find(|nalu| hevc_nal_type(nalu) == 34)
        .expect("HEVC sample should contain PPS");
    let parsed = scuffle_h265::SpsNALUnit::parse(std::io::Cursor::new(sps))
        .expect("repository SPS should parse");
    let entry = create_hevc_sample_entry(352, 288, vps, sps, pps)
        .expect("valid HEVC configuration should create a sample entry");
    let SampleEntry::Hvc1(hvc1) = entry else {
        panic!("expected hvc1 sample entry");
    };
    let profile = &parsed.rbsp.profile_tier_level.general_profile;

    assert_eq!(
        hvc1.hvcc_box.general_profile_space.get(),
        profile.profile_space
    );
    assert_eq!(
        hvc1.hvcc_box.general_tier_flag.get(),
        u8::from(profile.tier_flag)
    );
    assert_eq!(hvc1.hvcc_box.general_profile_idc.get(), profile.profile_idc);
    assert_eq!(hvc1.hvcc_box.general_level_idc, profile.level_idc.unwrap());
    assert_eq!(
        hvc1.hvcc_box.chroma_format_idc.get(),
        parsed.rbsp.chroma_format_idc
    );
    assert_eq!(
        hvc1.hvcc_box.bit_depth_luma_minus8.get(),
        parsed.rbsp.bit_depth_luma_minus8
    );
    assert_eq!(
        hvc1.hvcc_box.bit_depth_chroma_minus8.get(),
        parsed.rbsp.bit_depth_chroma_minus8
    );
    assert_eq!(
        hvc1.hvcc_box.num_temporal_layers.get(),
        parsed.rbsp.sps_max_sub_layers_minus1 + 1
    );
}

fn av1_obu(obu_type: u8, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 128);
    let mut out = Vec::with_capacity(payload.len() + 2);
    out.push((obu_type << 3) | 0x02);
    out.push(payload.len() as u8);
    out.extend_from_slice(payload);
    out
}
