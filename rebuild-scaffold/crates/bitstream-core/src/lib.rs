use backend_contract::Codec;

#[derive(Debug, Clone)]
pub struct AccessUnit {
    pub nalus: Vec<Vec<u8>>,
    pub codec: Codec,
    pub pts_90k: Option<i64>,
    pub is_keyframe: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ParameterSetCache {
    pub parameter_sets: Vec<Vec<u8>>,
}

#[derive(Debug, Default)]
pub struct StatefulBitstreamAssembler {
    pending: Vec<u8>,
    emitted_access_units: usize,
}

impl StatefulBitstreamAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_chunk(
        &mut self,
        chunk: &[u8],
        _codec: Codec,
        _pts_90k: Option<i64>,
    ) -> (Vec<AccessUnit>, ParameterSetCache) {
        if !chunk.is_empty() {
            self.pending.extend_from_slice(chunk);
        }

        let _ = self.emitted_access_units;
        (Vec::new(), ParameterSetCache::default())
    }
}
