use video_hw::{
    annexb::{parse_annexb, parse_annexb_for_stream},
    packer::{AnnexBPacker, AvccHvccPacker, SamplePacker},
    AccessUnit, Codec,
};

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

fn chunk_sizes(total_len: usize) -> Vec<usize> {
    let mut sizes = Vec::new();
    let mut remaining = total_len;
    let mut state = 0x1234_5678_u64;

    while remaining > 0 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let candidate = ((state >> 32) as usize % 11) + 1;
        let step = candidate.min(remaining);
        sizes.push(step);
        remaining -= step;
    }

    sizes
}

#[test]
fn chunked_parse_converges_to_full_parse() {
    let data = h264_sample_annexb();
    let full = parse_annexb(&data, Codec::H264).expect("full parse should succeed");

    let mut cumulative = Vec::new();
    let mut pos = 0usize;
    for step in chunk_sizes(data.len()) {
        cumulative.extend_from_slice(&data[pos..pos + step]);
        pos += step;

        let _ = parse_annexb_for_stream(&cumulative, Codec::H264)
            .expect("stream parse should not fail on prefixes");
    }

    let streamed = parse_annexb_for_stream(&cumulative, Codec::H264)
        .expect("final stream parse should succeed");

    assert_eq!(full.parameter_sets, streamed.parameter_sets);
    assert_eq!(full.access_units.len(), streamed.access_units.len());
    assert_eq!(
        full.access_units[0].is_keyframe,
        streamed.access_units[0].is_keyframe
    );
    assert_eq!(
        full.access_units[0].nalus.len(),
        streamed.access_units[0].nalus.len()
    );
}

#[test]
fn packers_produce_expected_framing() {
    let access_unit = AccessUnit {
        nalus: vec![vec![0x67, 0x01, 0x02], vec![0x68, 0x03]],
        codec: Codec::H264,
        pts_90k: Some(0),
        is_keyframe: true,
    };

    let mut avcc = AvccHvccPacker;
    let avcc_sample = avcc.pack(&access_unit).expect("avcc pack should work");
    assert_eq!(
        avcc_sample.data,
        vec![0, 0, 0, 3, 0x67, 0x01, 0x02, 0, 0, 0, 2, 0x68, 0x03]
    );

    let mut annexb = AnnexBPacker;
    let annexb_sample = annexb.pack(&access_unit).expect("annexb pack should work");
    assert_eq!(
        annexb_sample.data,
        vec![0, 0, 0, 1, 0x67, 0x01, 0x02, 0, 0, 0, 1, 0x68, 0x03]
    );
}
