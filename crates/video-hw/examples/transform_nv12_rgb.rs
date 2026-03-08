use std::time::Duration;

use anyhow::{Result, anyhow};
use clap::Parser;
use video_hw::{TransformDispatcher, TransformJob, TransformResult, make_argb_to_nv12_dummy};

#[derive(Debug, Parser)]
#[command(about = "Run async NV12->RGB transforms on CPU workers")]
struct Args {
    #[arg(long, default_value_t = 4)]
    workers: usize,
    #[arg(long, default_value_t = 16)]
    jobs: usize,
    #[arg(long, default_value_t = 640)]
    width: usize,
    #[arg(long, default_value_t = 360)]
    height: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let dispatcher = TransformDispatcher::new(args.workers, args.jobs.max(1));

    for _ in 0..args.jobs {
        let frame = make_argb_to_nv12_dummy(args.width, args.height);
        dispatcher
            .submit(TransformJob::Nv12ToRgb(frame))
            .map_err(|e| anyhow!("submit transform job failed: {e:?}"))?;
    }

    let mut completed = 0usize;
    while completed < args.jobs {
        let result = dispatcher
            .recv_timeout(Duration::from_secs(2))
            .map_err(|e| anyhow!("waiting transform result timed out: {e:?}"))??;
        match result {
            TransformResult::Rgb(frame) => {
                completed += 1;
                if completed == 1 {
                    println!(
                        "first_result: {}x{}, bytes={}",
                        frame.width,
                        frame.height,
                        frame.data.len()
                    );
                }
            }
        }
    }

    println!(
        "transform_completed={}, workers={}, jobs={}",
        completed, args.workers, args.jobs
    );
    Ok(())
}
