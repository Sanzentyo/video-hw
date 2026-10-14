//! Shared decoder/encoder timings, copy counters, and sampling utilities.

use super::*;

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct StageTiming {
    pub(super) pack: Duration,
    pub(super) sdk: Duration,
    pub(super) upload: Duration,
    pub(super) input_prep: Duration,
    pub(super) output_lock: Duration,
    pub(super) reap: Duration,
}

#[derive(Debug, Default, Clone)]
pub(super) struct SampleStats {
    pub(super) samples: Vec<f64>,
}

impl SampleStats {
    pub(super) fn push_duration_ms(&mut self, value: Duration) {
        self.samples.push(value.as_secs_f64() * 1_000.0);
    }

    pub(super) fn push_value(&mut self, value: f64) {
        self.samples.push(value);
    }

    pub(super) fn mean(&self) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        self.samples.iter().sum::<f64>() / self.samples.len() as f64
    }

    pub(super) fn percentile(&self, percentile: f64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        let rank = ((percentile / 100.0) * n as f64)
            .ceil()
            .clamp(1.0, n as f64) as usize;
        sorted[rank - 1]
    }

    pub(super) fn p95(&self) -> f64 {
        self.percentile(95.0)
    }

    pub(super) fn p99(&self) -> f64 {
        self.percentile(99.0)
    }

    pub(super) fn peak(&self) -> f64 {
        self.samples
            .iter()
            .copied()
            .max_by(f64::total_cmp)
            .unwrap_or(0.0)
    }
}

pub(super) fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct CopyStats {
    pub(super) input_upload_bytes: u64,
    pub(super) input_upload_frames: u64,
    pub(super) output_copy_bytes: u64,
    pub(super) output_copy_packets: u64,
}
