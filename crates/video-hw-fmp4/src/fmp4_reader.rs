mod config;
mod core;
mod decode;
#[cfg(feature = "async-session")]
mod session_async;
mod state;

pub use config::{
    EncodedSample, Fmp4ReaderConfig, Fmp4ReaderStatus, Fmp4Track, Fmp4TrackDescription, GopSegment,
    IndexMode, MediaTime, Mp4IndexSnapshot, RangeCacheConfig, RangeCacheStats,
    SampleEntryDescription, SampleId, SampleLookup, SampleLookupMatch, SampleMeta, SampleRange,
    SampleReadStats, TrackId, TrackReadStats,
};
pub use core::EncodedSampleIter;
pub use decode::{
    DecodeDiagnostics, DecodedFrameIter, DecodedSampleFrame, FrameDecodeRangeRequest,
    FrameDecodeRangeResult, FrameDecodeRequest, FrameDecodeResult, FrameDecodeWindowRequest,
    FrameDecoder, GopCursor, ReturnedFrameOrder,
};
#[cfg(feature = "async-session")]
pub use session_async::AsyncReaderEvent;
pub use shiguredo_mp4::TrackKind;
#[cfg(feature = "async-session")]
pub use state::AsyncReading;
pub use state::{Finished, ReaderReady, SyncReading};

use anyhow::Result;
use std::marker::PhantomData;

use self::core::ReaderCore;
#[cfg(feature = "async-session")]
use self::session_async::AsyncReaderHandle;

#[derive(Debug)]
pub struct Fmp4Reader<State> {
    config: Fmp4ReaderConfig,
    state: State,
    _marker: PhantomData<fn() -> State>,
}

impl Fmp4Reader<ReaderReady> {
    pub fn new(config: Fmp4ReaderConfig) -> Self {
        Self {
            config,
            state: ReaderReady,
            _marker: PhantomData,
        }
    }

    pub fn config(&self) -> &Fmp4ReaderConfig {
        &self.config
    }

    pub fn into_sync_session(self) -> Result<Fmp4Reader<SyncReading>> {
        let core = ReaderCore::open(&self.config)?;
        Ok(Fmp4Reader {
            config: self.config,
            state: SyncReading { core },
            _marker: PhantomData,
        })
    }

    #[cfg(feature = "async-session")]
    pub fn into_async_session(self) -> Result<Fmp4Reader<AsyncReading>> {
        let (handle, tracks) = AsyncReaderHandle::spawn(self.config.clone())?;
        Ok(Fmp4Reader {
            config: self.config,
            state: AsyncReading { handle, tracks },
            _marker: PhantomData,
        })
    }
}

impl Fmp4Reader<SyncReading> {
    pub fn tracks(&self) -> &[Fmp4Track] {
        self.state.core.tracks()
    }

    pub fn samples(&mut self, track: TrackId) -> Result<&[SampleMeta]> {
        self.state.core.samples(track)
    }

    pub fn sample_meta(&mut self, sample: SampleId) -> Option<&SampleMeta> {
        self.state.core.sample_meta(sample)
    }

    pub fn iter_samples(&mut self, track: TrackId) -> Result<std::slice::Iter<'_, SampleMeta>> {
        self.state.core.iter_samples(track)
    }

    pub fn sample_at_pts(&mut self, track: TrackId, pts: MediaTime) -> Option<SampleId> {
        self.state.core.sample_at_pts(track, pts)
    }

    pub fn sample_at_pts_with_delta(
        &mut self,
        track: TrackId,
        pts: MediaTime,
    ) -> Option<SampleLookup> {
        self.state.core.sample_at_pts_with_delta(track, pts)
    }

    pub fn keyframe_before(&mut self, sample: SampleId) -> Option<SampleId> {
        self.state.core.keyframe_before(sample)
    }

    pub fn gop_for_sample(&mut self, sample: SampleId) -> Option<GopSegment> {
        self.state.core.gop_for_sample(sample)
    }

    pub fn read_sample(&mut self, sample: SampleId) -> Result<EncodedSample> {
        self.state.core.read_sample(sample)
    }

    pub fn read_gop(&mut self, sample: SampleId) -> Result<Vec<EncodedSample>> {
        self.state.core.read_gop(sample)
    }

    pub fn read_segment(&mut self, segment: GopSegment) -> Result<Vec<EncodedSample>> {
        self.state.core.read_segment(segment)
    }

    pub fn next_sample(&mut self) -> Result<Option<EncodedSample>> {
        self.state.core.next_sample()
    }

    pub fn iter_gop_for_sample(&mut self, sample: SampleId) -> Result<EncodedSampleIter<'_>> {
        self.state.core.iter_gop_for_sample(sample)
    }

    pub fn iter_encoded(&mut self, segment: GopSegment) -> Result<EncodedSampleIter<'_>> {
        self.state.core.encoded_iter(segment)
    }

    pub fn status(&self) -> Fmp4ReaderStatus {
        self.state.core.status()
    }

    pub fn cache_stats(&self) -> RangeCacheStats {
        self.state.core.cache_stats()
    }

    pub fn clear_cache(&mut self) {
        self.state.core.clear_cache();
    }

    pub fn index_snapshot(&mut self) -> Result<Mp4IndexSnapshot> {
        self.state.core.index_snapshot()
    }

    pub fn finish(self) -> Fmp4Reader<Finished> {
        Fmp4Reader {
            config: self.config,
            state: Finished {
                status: self.state.core.status(),
            },
            _marker: PhantomData,
        }
    }
}

#[cfg(feature = "async-session")]
impl Fmp4Reader<AsyncReading> {
    pub fn tracks(&self) -> &[Fmp4Track] {
        &self.state.tracks
    }

    pub async fn samples(&mut self, track: TrackId) -> Result<Vec<SampleMeta>> {
        self.state.handle.samples(track).await
    }

    pub async fn sample_meta(&mut self, sample: SampleId) -> Result<Option<SampleMeta>> {
        self.state.handle.sample_meta(sample).await
    }

    pub async fn sample_at_pts(
        &mut self,
        track: TrackId,
        pts: MediaTime,
    ) -> Result<Option<SampleId>> {
        self.state.handle.sample_at_pts(track, pts).await
    }

    pub async fn sample_at_pts_with_delta(
        &mut self,
        track: TrackId,
        pts: MediaTime,
    ) -> Result<Option<SampleLookup>> {
        self.state.handle.sample_at_pts_with_delta(track, pts).await
    }

    pub async fn keyframe_before(&mut self, sample: SampleId) -> Result<Option<SampleId>> {
        self.state.handle.keyframe_before(sample).await
    }

    pub async fn gop_for_sample(&mut self, sample: SampleId) -> Result<Option<GopSegment>> {
        self.state.handle.gop_for_sample(sample).await
    }

    pub async fn read_sample(&mut self, sample: SampleId) -> Result<EncodedSample> {
        self.state.handle.read_sample(sample).await
    }

    pub async fn read_gop(&mut self, sample: SampleId) -> Result<Vec<EncodedSample>> {
        self.state.handle.read_gop(sample).await
    }

    pub async fn read_segment(&mut self, segment: GopSegment) -> Result<Vec<EncodedSample>> {
        self.state.handle.read_segment(segment).await
    }

    pub async fn next_sample(&mut self) -> Result<Option<EncodedSample>> {
        self.state.handle.next_sample().await
    }

    pub async fn index_snapshot(&mut self) -> Result<Mp4IndexSnapshot> {
        self.state.handle.index_snapshot().await
    }

    pub async fn status(&mut self) -> Result<Fmp4ReaderStatus> {
        self.state.handle.status().await
    }

    pub async fn cache_stats(&mut self) -> Result<RangeCacheStats> {
        self.state.handle.cache_stats().await
    }

    pub async fn clear_cache(&mut self) -> Result<()> {
        self.state.handle.clear_cache().await
    }

    pub async fn recv_event(&mut self) -> Option<AsyncReaderEvent> {
        self.state.handle.recv_event().await
    }

    pub fn try_recv_event(&mut self) -> Option<AsyncReaderEvent> {
        self.state.handle.try_recv_event()
    }

    pub async fn finish(self) -> Result<Fmp4Reader<Finished>> {
        let status = self.state.handle.finish().await?;
        Ok(Fmp4Reader {
            config: self.config,
            state: Finished { status },
            _marker: PhantomData,
        })
    }
}

impl Fmp4Reader<Finished> {
    pub fn status(&self) -> &Fmp4ReaderStatus {
        &self.state.status
    }
}

#[cfg(all(test, feature = "async-session"))]
mod tests {
    use super::*;

    #[test]
    fn async_reader_exposes_indexed_seek_and_read_api() -> Result<()> {
        let rt = tokio::runtime::Builder::new_current_thread().build()?;
        rt.block_on(async {
            let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("sample-videos")
                .join("sample-10s.mp4");
            let mut reader =
                Fmp4Reader::new(Fmp4ReaderConfig::new(input_path)).into_async_session()?;
            let track_id = reader
                .tracks()
                .iter()
                .find(|track| track.kind == TrackKind::Video)
                .expect("video track should exist")
                .track_id;
            let samples = reader.samples(track_id).await?;
            assert!(!samples.is_empty());
            let first = samples[0].clone();
            assert_eq!(
                reader.sample_meta(first.sample_id).await?,
                Some(first.clone())
            );
            assert_eq!(
                reader.sample_at_pts(track_id, first.pts).await?,
                Some(first.sample_id)
            );
            let lookup = reader
                .sample_at_pts_with_delta(track_id, first.pts)
                .await?
                .expect("exact lookup");
            assert_eq!(lookup.matched_sample, first.sample_id);
            assert_eq!(
                reader.keyframe_before(first.sample_id).await?,
                Some(first.sample_id)
            );
            let gop = reader
                .gop_for_sample(first.sample_id)
                .await?
                .expect("gop segment");
            let encoded = reader.read_sample(first.sample_id).await?;
            assert_eq!(encoded.meta.sample_id, first.sample_id);
            let gop_samples = reader.read_gop(first.sample_id).await?;
            assert!(!gop_samples.is_empty());
            let segment_samples = reader.read_segment(gop).await?;
            assert_eq!(segment_samples[0].meta.sample_id, first.sample_id);
            let snapshot = reader.index_snapshot().await?;
            assert!(!snapshot.track_descriptions.is_empty());
            let status = reader.status().await?;
            assert!(status.samples_read >= 1);
            assert!(reader.cache_stats().await?.resident_bytes > 0);
            reader.clear_cache().await?;
            assert_eq!(reader.cache_stats().await?.resident_bytes, 0);
            let finished = reader.finish().await?;
            assert!(finished.status().samples_read >= 1);
            Ok::<_, anyhow::Error>(())
        })?;
        Ok(())
    }
}
