use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use video_hw::{Backend, DecodeOutputMode, DecodedFrame};
use video_hw_fmp4::{
    Fmp4Reader, Fmp4ReaderConfig, Fmp4ReaderReady, Fmp4ReaderStatus, FrameDecodeRangeRequest,
    FrameDecodeRequest, FrameDecodeWindowRequest, FrameDecoder, IndexMode, SampleId, SampleMeta,
    SampleRange, TrackId, TrackKind,
};

#[derive(Debug, Parser)]
#[command(about = "Benchmark fMP4 decode access patterns and frame correctness")]
struct Args {
    #[arg(long, default_value = "sample-videos/foreman_cif.mp4")]
    input: PathBuf,

    #[arg(long, default_value = "auto")]
    backend: String,

    #[arg(long, default_value_t = false)]
    require_hardware: bool,

    #[arg(long, default_value_t = 0)]
    start_index: usize,

    #[arg(long, default_value_t = 90)]
    frame_count: usize,

    #[arg(long, default_value_t = 60)]
    random_count: usize,

    #[arg(long, default_value_t = 7)]
    random_stride: usize,

    #[arg(long, default_value_t = 0)]
    cache_before: u32,

    #[arg(long, default_value_t = 8)]
    cache_after: u32,

    #[arg(long, default_value_t = 64)]
    cache_capacity: usize,

    #[arg(long, value_enum, default_value_t = ReferenceMode::Ffmpeg)]
    reference: ReferenceMode,

    #[arg(long, default_value = "ffmpeg")]
    ffmpeg: PathBuf,

    #[arg(long, default_value = "output")]
    output_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum ReferenceMode {
    Ffmpeg,
    SequentialBaseline,
    None,
}

#[derive(Debug, Clone)]
struct RgbFrame {
    sample_id: SampleId,
    width: u32,
    height: u32,
    data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct AccessStats {
    seconds: f64,
    requested: usize,
    returned: usize,
    cache_hits: usize,
    cache_misses: usize,
    samples_read: u64,
    bytes_read: u64,
    range_cache_hits: u64,
    range_cache_misses: u64,
    range_cache_evictions: u64,
    range_cache_resident_bytes: usize,
    max_mse: Option<f64>,
    min_psnr: Option<f64>,
    compared_frames: usize,
}

#[derive(Debug)]
struct CaseReport {
    name: &'static str,
    notes: &'static str,
    stats: AccessStats,
}

#[derive(Debug)]
struct FrameCache {
    capacity: usize,
    entries: HashMap<SampleId, RgbFrame>,
    lru: VecDeque<SampleId>,
}

struct StatsInput<'a> {
    seconds: f64,
    requested: usize,
    cache_hits: usize,
    cache_misses: usize,
    before: &'a Fmp4ReaderStatus,
    after: &'a Fmp4ReaderStatus,
    frames: &'a [RgbFrame],
    reference: &'a Option<HashMap<SampleId, RgbFrame>>,
}

impl FrameCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            lru: VecDeque::new(),
        }
    }

    fn get(&mut self, sample_id: SampleId) -> Option<RgbFrame> {
        let frame = self.entries.get(&sample_id).cloned()?;
        self.touch(sample_id);
        Some(frame)
    }

    fn insert(&mut self, frame: RgbFrame) {
        if self.capacity == 0 {
            return;
        }
        let sample_id = frame.sample_id;
        self.entries.insert(sample_id, frame);
        self.touch(sample_id);
        while self.entries.len() > self.capacity {
            let Some(oldest) = self.lru.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    fn touch(&mut self, sample_id: SampleId) {
        self.lru.retain(|existing| *existing != sample_id);
        self.lru.push_back(sample_id);
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.frame_count == 0 {
        bail!("--frame-count must be >= 1");
    }
    if args.random_stride == 0 {
        bail!("--random-stride must be >= 1");
    }
    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create output dir: {}", args.output_dir.display()))?;

    let backend = Backend::from_str(&args.backend)
        .with_context(|| format!("parse backend {}", args.backend))?;
    let (track_id, track_samples, width, height) = inspect_input(&args.input)?;
    let selected = select_contiguous_samples(&track_samples, args.start_index, args.frame_count)?;
    let random_sequence =
        deterministic_random_sequence(&selected, args.random_count, args.random_stride);
    let ping_pong_sequence = ping_pong_sequence(&selected);
    let reference = build_reference(&args, track_id, &track_samples, width, height, backend)?;

    let reports = vec![
        run_range_iter_case(
            &args,
            backend,
            track_id,
            &track_samples,
            &selected,
            &reference,
        )?,
        run_decode_sample_case(
            &args,
            backend,
            track_id,
            &selected,
            &reference,
            "decode_sample_sequential_no_cache",
            "One decode_sample call per contiguous sample; each call can replay from the GOP keyframe.",
        )?,
        run_window_cache_case(
            &args,
            backend,
            track_id,
            &selected,
            &reference,
            "decode_window_sequential_lru",
            "Caller-side LRU cache; misses decode a presentation window and retain nearby frames.",
        )?,
        run_decode_sample_case(
            &args,
            backend,
            track_id,
            &random_sequence,
            &reference,
            "decode_sample_random_no_cache",
            "Deterministic non-contiguous sample order without decoded-frame cache.",
        )?,
        run_window_cache_case(
            &args,
            backend,
            track_id,
            &random_sequence,
            &reference,
            "decode_window_random_lru",
            "Random access with caller-side window cache.",
        )?,
        run_window_cache_case(
            &args,
            backend,
            track_id,
            &ping_pong_sequence,
            &reference,
            "decode_window_ping_pong_lru",
            "Forward then reverse access through the same span; shows cache reuse on revisits.",
        )?,
    ];

    let report_path = write_report(&args, backend, track_id, width, height, &reports)?;
    println!("saved report: {}", report_path.display());
    Ok(())
}

fn inspect_input(input: &Path) -> Result<(TrackId, Vec<SampleMeta>, u32, u32)> {
    let mut reader = open_reader(input)?;
    let track = reader
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .cloned()
        .context("input has no video track")?;
    let description = track.codec_description();
    let width = description
        .video_width
        .map(u32::from)
        .context("video width is unavailable from sample entry")?;
    let height = description
        .video_height
        .map(u32::from)
        .context("video height is unavailable from sample entry")?;
    let samples = reader.samples(track.track_id)?.to_vec();
    Ok((track.track_id, samples, width, height))
}

fn open_reader(input: &Path) -> Result<video_hw_fmp4::Fmp4Reader<video_hw_fmp4::SyncReading>> {
    let mut config = Fmp4ReaderConfig::new(input);
    config.index_mode = IndexMode::Eager;
    Fmp4Reader::<Fmp4ReaderReady>::new(config)
        .into_sync_session()
        .with_context(|| format!("open fMP4 reader: {}", input.display()))
}

fn select_contiguous_samples(
    samples: &[SampleMeta],
    start_index: usize,
    frame_count: usize,
) -> Result<Vec<SampleId>> {
    let end = start_index
        .checked_add(frame_count)
        .context("sample selection overflows")?
        .min(samples.len());
    let selected = samples
        .get(start_index..end)
        .context("start index is beyond the video sample count")?
        .iter()
        .map(|sample| sample.sample_id)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        bail!("sample selection is empty");
    }
    Ok(selected)
}

fn deterministic_random_sequence(
    selected: &[SampleId],
    requested_count: usize,
    stride: usize,
) -> Vec<SampleId> {
    let count = requested_count.min(selected.len());
    (0..count)
        .map(|index| selected[(index * stride) % selected.len()])
        .collect()
}

fn ping_pong_sequence(selected: &[SampleId]) -> Vec<SampleId> {
    selected
        .iter()
        .copied()
        .chain(selected.iter().rev().copied())
        .collect()
}

fn build_reference(
    args: &Args,
    track_id: TrackId,
    track_samples: &[SampleMeta],
    width: u32,
    height: u32,
    backend: Backend,
) -> Result<Option<HashMap<SampleId, RgbFrame>>> {
    match args.reference {
        ReferenceMode::None => Ok(None),
        ReferenceMode::Ffmpeg => ffmpeg_reference(args, track_samples, width, height),
        ReferenceMode::SequentialBaseline => {
            let selected =
                select_contiguous_samples(track_samples, args.start_index, args.frame_count)?;
            let case = decode_range_frames(args, backend, track_id, track_samples, &selected)?;
            Ok(Some(
                case.into_iter()
                    .map(|frame| (frame.sample_id, frame))
                    .collect::<HashMap<_, _>>(),
            ))
        }
    }
}

fn ffmpeg_reference(
    args: &Args,
    track_samples: &[SampleMeta],
    width: u32,
    height: u32,
) -> Result<Option<HashMap<SampleId, RgbFrame>>> {
    let epoch = epoch_seconds()?;
    let raw_path = args
        .output_dir
        .join(format!("fmp4-decode-reference-rgb24-{epoch}.raw"));
    let status = Command::new(&args.ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&args.input)
        .args(["-an", "-pix_fmt", "rgb24", "-f", "rawvideo"])
        .arg(&raw_path)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("spawn ffmpeg: {}", args.ffmpeg.display()))?;
    if !status.success() {
        bail!("ffmpeg reference decode failed: status={status}");
    }

    let frame_size = frame_size(width, height)?;
    let mut file = fs::File::open(&raw_path)
        .with_context(|| format!("open ffmpeg raw reference: {}", raw_path.display()))?;
    let mut pts_order = track_samples.iter().collect::<Vec<_>>();
    pts_order.sort_by_key(|sample| (sample.pts.ticks, sample.sample_id));
    let mut frames = HashMap::new();
    for (index, sample) in pts_order.into_iter().enumerate() {
        let offset = u64::try_from(index)
            .context("reference frame index exceeds u64")?
            .checked_mul(u64::try_from(frame_size).context("frame size exceeds u64")?)
            .context("reference frame offset overflow")?;
        file.seek(SeekFrom::Start(offset))
            .context("seek ffmpeg reference frame")?;
        let mut data = vec![0_u8; frame_size];
        match file.read_exact(&mut data) {
            Ok(()) => {
                frames.insert(
                    sample.sample_id,
                    RgbFrame {
                        sample_id: sample.sample_id,
                        width,
                        height,
                        data,
                    },
                );
            }
            Err(_) => break,
        }
    }
    let _ = fs::remove_file(raw_path);
    Ok(Some(frames))
}

fn run_range_iter_case(
    args: &Args,
    backend: Backend,
    track_id: TrackId,
    track_samples: &[SampleMeta],
    selected: &[SampleId],
    reference: &Option<HashMap<SampleId, RgbFrame>>,
) -> Result<CaseReport> {
    let mut reader = open_reader(&args.input)?;
    reader.clear_cache();
    let before = reader.status();
    let start = Instant::now();
    let frames = {
        let mut decoder = FrameDecoder::new(&mut reader);
        let mut request =
            FrameDecodeRangeRequest::new(sample_range(track_id, track_samples, selected)?);
        request.backend = backend;
        request.require_hardware = args.require_hardware;
        request.output_mode = DecodeOutputMode::Rgb24;
        decoder
            .decode_range_iter(request)?
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .map(decoded_frame_to_rgb)
            .collect::<Result<Vec<_>>>()?
    };
    let seconds = start.elapsed().as_secs_f64();
    let after = reader.status();
    Ok(CaseReport {
        name: "decode_range_iter_contiguous",
        notes: "One decoder session over a contiguous sample range.",
        stats: stats_from_frames(StatsInput {
            seconds,
            requested: selected.len(),
            cache_hits: 0,
            cache_misses: 0,
            before: &before,
            after: &after,
            frames: &frames,
            reference,
        }),
    })
}

fn run_decode_sample_case(
    args: &Args,
    backend: Backend,
    track_id: TrackId,
    sequence: &[SampleId],
    reference: &Option<HashMap<SampleId, RgbFrame>>,
    name: &'static str,
    notes: &'static str,
) -> Result<CaseReport> {
    let mut reader = open_reader(&args.input)?;
    reader.clear_cache();
    let before = reader.status();
    let start = Instant::now();
    let mut frames = Vec::new();
    for sample_id in sequence {
        let mut decoder = FrameDecoder::new(&mut reader);
        let mut request = FrameDecodeRequest::new(track_id, *sample_id);
        request.backend = backend;
        request.require_hardware = args.require_hardware;
        request.output_mode = DecodeOutputMode::Rgb24;
        if let Some(frame) = decoder.decode_sample(request)?.target_frame() {
            frames.push(decoded_frame_to_rgb(frame.clone())?);
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    let after = reader.status();
    Ok(CaseReport {
        name,
        notes,
        stats: stats_from_frames(StatsInput {
            seconds,
            requested: sequence.len(),
            cache_hits: 0,
            cache_misses: 0,
            before: &before,
            after: &after,
            frames: &frames,
            reference,
        }),
    })
}

fn run_window_cache_case(
    args: &Args,
    backend: Backend,
    track_id: TrackId,
    sequence: &[SampleId],
    reference: &Option<HashMap<SampleId, RgbFrame>>,
    name: &'static str,
    notes: &'static str,
) -> Result<CaseReport> {
    let mut reader = open_reader(&args.input)?;
    reader.clear_cache();
    let before = reader.status();
    let start = Instant::now();
    let mut cache = FrameCache::new(args.cache_capacity);
    let mut hits = 0usize;
    let mut misses = 0usize;
    let mut frames = Vec::new();
    for sample_id in sequence {
        if let Some(frame) = cache.get(*sample_id) {
            hits = hits.saturating_add(1);
            frames.push(frame);
            continue;
        }
        misses = misses.saturating_add(1);
        let decoded = {
            let mut decoder = FrameDecoder::new(&mut reader);
            let mut request = FrameDecodeWindowRequest::new(track_id, *sample_id);
            request.backend = backend;
            request.require_hardware = args.require_hardware;
            request.output_mode = DecodeOutputMode::Rgb24;
            request.before = args.cache_before;
            request.after = args.cache_after;
            decoder.decode_window(request)?.frames
        };
        for decoded_frame in decoded {
            let frame = decoded_frame_to_rgb(decoded_frame)?;
            let is_requested = frame.sample_id == *sample_id;
            cache.insert(frame.clone());
            if is_requested {
                frames.push(frame);
            }
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    let after = reader.status();
    Ok(CaseReport {
        name,
        notes,
        stats: stats_from_frames(StatsInput {
            seconds,
            requested: sequence.len(),
            cache_hits: hits,
            cache_misses: misses,
            before: &before,
            after: &after,
            frames: &frames,
            reference,
        }),
    })
}

fn decode_range_frames(
    args: &Args,
    backend: Backend,
    track_id: TrackId,
    track_samples: &[SampleMeta],
    selected: &[SampleId],
) -> Result<Vec<RgbFrame>> {
    let mut reader = open_reader(&args.input)?;
    let mut decoder = FrameDecoder::new(&mut reader);
    let mut request =
        FrameDecodeRangeRequest::new(sample_range(track_id, track_samples, selected)?);
    request.backend = backend;
    request.require_hardware = args.require_hardware;
    request.output_mode = DecodeOutputMode::Rgb24;
    decoder
        .decode_range(request)?
        .frames
        .into_iter()
        .map(decoded_frame_to_rgb)
        .collect()
}

fn sample_range(
    track_id: TrackId,
    track_samples: &[SampleMeta],
    selected: &[SampleId],
) -> Result<SampleRange> {
    let start_sample = *selected.first().context("empty selected sample set")?;
    let last_sample = *selected.last().context("empty selected sample set")?;
    let last_index = track_samples
        .iter()
        .position(|sample| sample.sample_id == last_sample)
        .context("selected sample does not belong to track")?;
    let end_sample_exclusive = track_samples
        .get(last_index.saturating_add(1))
        .map_or(SampleId(u64::MAX), |sample| sample.sample_id);
    Ok(SampleRange {
        track_id,
        start_sample,
        end_sample_exclusive,
    })
}

fn decoded_frame_to_rgb(frame: video_hw_fmp4::DecodedSampleFrame) -> Result<RgbFrame> {
    let sample_id = frame.sample_id.context("decoded frame has no sample id")?;
    match frame.frame {
        DecodedFrame::Rgb24 { dims, data, .. } => Ok(RgbFrame {
            sample_id,
            width: dims.width.get(),
            height: dims.height.get(),
            data,
        }),
        other => bail!("expected RGB24 decoded frame, got {other:?}"),
    }
}

fn stats_from_frames(input: StatsInput<'_>) -> AccessStats {
    let (max_mse, min_psnr, compared_frames) = compare_frames(input.frames, input.reference);
    AccessStats {
        seconds: input.seconds,
        requested: input.requested,
        returned: input.frames.len(),
        cache_hits: input.cache_hits,
        cache_misses: input.cache_misses,
        samples_read: input
            .after
            .samples_read
            .saturating_sub(input.before.samples_read),
        bytes_read: input
            .after
            .bytes_read
            .saturating_sub(input.before.bytes_read),
        range_cache_hits: input
            .after
            .cache_hits
            .saturating_sub(input.before.cache_hits),
        range_cache_misses: input
            .after
            .cache_misses
            .saturating_sub(input.before.cache_misses),
        range_cache_evictions: input
            .after
            .cache_evictions
            .saturating_sub(input.before.cache_evictions),
        range_cache_resident_bytes: input.after.cache_resident_bytes,
        max_mse,
        min_psnr,
        compared_frames,
    }
}

fn compare_frames(
    frames: &[RgbFrame],
    reference: &Option<HashMap<SampleId, RgbFrame>>,
) -> (Option<f64>, Option<f64>, usize) {
    let Some(reference) = reference else {
        return (None, None, 0);
    };
    let mut max_mse = None::<f64>;
    let mut min_psnr = None::<f64>;
    let mut compared = 0usize;
    for frame in frames {
        let Some(expected) = reference.get(&frame.sample_id) else {
            continue;
        };
        if expected.width != frame.width || expected.height != frame.height {
            continue;
        }
        let mse = mse(&frame.data, &expected.data);
        let psnr = psnr(mse);
        max_mse = Some(max_mse.map_or(mse, |current| current.max(mse)));
        min_psnr = Some(min_psnr.map_or(psnr, |current| current.min(psnr)));
        compared = compared.saturating_add(1);
    }
    (max_mse, min_psnr, compared)
}

fn mse(actual: &[u8], expected: &[u8]) -> f64 {
    let len = actual.len().min(expected.len()).max(1);
    actual
        .iter()
        .zip(expected.iter())
        .map(|(a, b)| {
            let delta = f64::from(*a) - f64::from(*b);
            delta * delta
        })
        .sum::<f64>()
        / len as f64
}

fn psnr(mse: f64) -> f64 {
    if mse == 0.0 {
        return f64::INFINITY;
    }
    10.0 * ((255.0_f64 * 255.0) / mse).log10()
}

fn frame_size(width: u32, height: u32) -> Result<usize> {
    usize::try_from(width)
        .context("width exceeds usize")?
        .checked_mul(usize::try_from(height).context("height exceeds usize")?)
        .and_then(|pixels| pixels.checked_mul(3))
        .context("RGB24 frame size overflow")
}

fn write_report(
    args: &Args,
    backend: Backend,
    track_id: TrackId,
    width: u32,
    height: u32,
    reports: &[CaseReport],
) -> Result<PathBuf> {
    let epoch = epoch_seconds()?;
    let path = args
        .output_dir
        .join(format!("benchmark-fmp4-decode-access-{epoch}.md"));
    let mut text = String::new();
    writeln!(&mut text, "# fMP4 Decode Access Benchmark")?;
    writeln!(&mut text, "epoch_seconds: {epoch}")?;
    writeln!(&mut text, "input: {}", args.input.display())?;
    writeln!(&mut text, "backend: {backend}")?;
    writeln!(&mut text, "require_hardware: {}", args.require_hardware)?;
    writeln!(&mut text, "track_id: {track_id}")?;
    writeln!(&mut text, "dimensions: {width}x{height}")?;
    writeln!(&mut text, "start_index: {}", args.start_index)?;
    writeln!(&mut text, "frame_count: {}", args.frame_count)?;
    writeln!(&mut text, "reference: {:?}", args.reference)?;
    writeln!(&mut text)?;
    writeln!(
        &mut text,
        "| Case | seconds | requested | returned | app cache hit/miss | sample reads | bytes read | range hit/miss/evict | resident bytes | max MSE | min PSNR | compared |"
    )?;
    writeln!(
        &mut text,
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
    )?;
    for report in reports {
        let stats = &report.stats;
        writeln!(
            &mut text,
            "| {} | {:.3} | {} | {} | {}/{} | {} | {} | {}/{}/{} | {} | {} | {} | {} |",
            report.name,
            stats.seconds,
            stats.requested,
            stats.returned,
            stats.cache_hits,
            stats.cache_misses,
            stats.samples_read,
            stats.bytes_read,
            stats.range_cache_hits,
            stats.range_cache_misses,
            stats.range_cache_evictions,
            stats.range_cache_resident_bytes,
            fmt_optional(stats.max_mse),
            fmt_optional(stats.min_psnr),
            stats.compared_frames
        )?;
    }
    writeln!(&mut text)?;
    writeln!(&mut text, "## Case Notes")?;
    for report in reports {
        writeln!(&mut text, "- `{}`: {}", report.name, report.notes)?;
    }
    writeln!(&mut text)?;
    writeln!(&mut text, "## Interpretation")?;
    writeln!(
        &mut text,
        "- `decode_range_iter_contiguous` is the efficient path for long contiguous access because it keeps one decode session and streams samples once."
    )?;
    writeln!(
        &mut text,
        "- `decode_sample_*_no_cache` shows the lower bound of API-level reuse: byte ranges may hit the range cache, but decoded frames are not cached by `video-hw-fmp4` itself."
    )?;
    writeln!(
        &mut text,
        "- `decode_window_*_lru` models the caller-side decoded-frame cache recommended for sliders, trackers, and sparse revisits."
    )?;
    fs::write(&path, text).with_context(|| format!("write report: {}", path.display()))?;
    Ok(path)
}

fn fmt_optional(value: Option<f64>) -> String {
    value
        .map(|value| {
            if value.is_infinite() {
                "inf".to_string()
            } else {
                format!("{value:.3}")
            }
        })
        .unwrap_or_else(|| "n/a".to_string())
}

fn epoch_seconds() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?
        .as_secs())
}
