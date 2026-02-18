use crate::{annexb::AccessUnit, error::Result, Codec};

pub struct PackedSample {
    pub data: Vec<u8>,
}

pub trait SamplePacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample>;
}

#[derive(Debug, Default)]
pub struct AvccHvccPacker;

impl SamplePacker for AvccHvccPacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample> {
        let _codec = access_unit.codec;
        let total_size = access_unit
            .nalus
            .iter()
            .map(|nal| nal.len().saturating_add(4))
            .sum();
        let mut data = Vec::with_capacity(total_size);

        for nal in &access_unit.nalus {
            let len = (nal.len() as u32).to_be_bytes();
            data.extend_from_slice(&len);
            data.extend_from_slice(nal);
        }

        Ok(PackedSample { data })
    }
}

#[derive(Debug, Default)]
pub struct AnnexBPacker;

impl SamplePacker for AnnexBPacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample> {
        let _codec: Codec = access_unit.codec;
        let total_size = access_unit
            .nalus
            .iter()
            .map(|nal| nal.len().saturating_add(4))
            .sum();
        let mut data = Vec::with_capacity(total_size);

        for nal in &access_unit.nalus {
            data.extend_from_slice(&[0, 0, 0, 1]);
            data.extend_from_slice(nal);
        }

        Ok(PackedSample { data })
    }
}
