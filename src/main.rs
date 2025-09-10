// src/main.rs

mod cli;
mod srt_stream;
mod model;
mod csv_sink;
mod errors;
mod transcriber;
mod filter;
mod transcription_adder;
mod rle_filter;
mod event_stream;

use crate::errors::AppError;
use chrono::{FixedOffset, Utc};
use chrono_tz::Tz;
use env_logger::Env;
use log::{info, warn};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

fn setup_logging(level: &str) {
    let env = Env::default().filter_or(
        "RUST_LOG",
        match level {
            "essential" => "info",
            "debug" => "debug",
            "trace" => "trace",
            "warn" => "warn",
            "error" => "error",
            _ => "info",
        },
    );
    env_logger::Builder::from_env(env).init();
}

fn compute_tz_offset(args_tz: &Option<String>) -> Option<FixedOffset> {
    if let Some(tzname) = args_tz.as_ref() {
        match tzname.parse::<Tz>() {
            Ok(tz) => {
                // Map "now" in that time zone to a fixed offset
                let now_utc = Utc::now();
                let now_tz = now_utc.with_timezone(&tz);
                let seconds = (now_tz.naive_local() - now_tz.naive_utc()).num_seconds();
                FixedOffset::east_opt(seconds as i32)
            }
            Err(_) => {
                warn!("Timezone parse failed; falling back to local");
                None
            }
        }
    } else {
        None
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), AppError> {
    let args = cli::parse_cli();
    setup_logging(&args.log_level);
    info!("Starting: processing {} files", args.input_files.len());

    // Build transcriber (only "text" is supported; uses incremental day-sharded index)
    let transcriber: Option<Arc<dyn transcriber::Transcriber + Send + Sync>> =
        match args.transcriber.as_str() {
            "text" => {
                if let Some(root) = args.record_dir.as_ref() {
                    let t = transcriber::TextFileTranscriber::new_indexed(root)?;
                    Some(Arc::new(t))
                } else {
                    warn!("--transcriber text used without --record-dir; no transcripts will be found");
                    let t = transcriber::TextFileTranscriber::new();
                    Some(Arc::new(t))
                }
            }
            "" => None,
            _ => {
                warn!(
                    "Unknown transcriber '{}' — proceeding without transcription",
                    args.transcriber
                );
                None
            }
        };

    let tz_offset = compute_tz_offset(&args.tz);

    // Shared filter config
    let cfg = Arc::new(filter::FilterConfig {
        freqs: args.freqs.clone(),
        rtypes: args.rtypes.clone(),
        rids: args.rids.clone(),
        tgs: args.tgs.clone(),
        nacs: args.nacs.clone(),
    });

    // Launch one pipeline per input file
    let mut tasks = Vec::with_capacity(args.input_files.len());
    for in_path in &args.input_files {
        let in_path = in_path.clone();
        let cfg = Arc::clone(&cfg);
        let transcriber = transcriber.clone();
        let record_dir = args.record_dir.clone();
        let tz_offset = tz_offset;

        let t = tokio::spawn(async move {
            if let Err(e) = run_pipeline(in_path, tz_offset, cfg, transcriber, record_dir).await {
                warn!("pipeline failed: {}", e);
            }
        });
        tasks.push(t);
    }

    for t in tasks {
        let _ = t.await;
    }

    info!("Done.");
    Ok(())
}

async fn run_pipeline(
    in_path: PathBuf,
    tz_offset: Option<FixedOffset>,
    cfg: Arc<filter::FilterConfig>,
    transcriber: Option<Arc<dyn transcriber::Transcriber + Send + Sync>>,
    record_dir: Option<PathBuf>,
) -> Result<(), AppError> {
    use model::RadioRecord;

    info!("Reading file {}", in_path.display());

    // Channels:
    // parse -> filter -> rle -> transcriber -> csv
    let (tx_parse, rx_parse) = mpsc::channel::<RadioRecord>(1024);
    let (tx_filt, rx_filt) = mpsc::channel::<RadioRecord>(1024);
    let (tx_rle, rx_rle) = mpsc::channel::<RadioRecord>(1024);
    let (tx_rows, rx_rows) = mpsc::channel::<RadioRecord>(1024);

    // 1) Parser (producer)
    let p_in = in_path.clone();
    let producer = tokio::spawn(async move {
        match p_in.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
            "event" => event_stream::stream_file(&p_in, tz_offset, tx_parse).await,
            "srt"   => srt_stream::stream_file(&p_in, tz_offset, tx_parse).await,
            _       => {
                // Heuristic: *.event often lacks blocks; default to event parser, else SRT
                // If you prefer strictness, return an error instead.
                if p_in.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase().ends_with(".event") {
                    event_stream::stream_file(&p_in, tz_offset, tx_parse).await
                } else {
                    srt_stream::stream_file(&p_in, tz_offset, tx_parse).await
                }
            }
        }
    });

    // 2) Filter (drop non-matching)
    let f_cfg = Arc::clone(&cfg);
    let filter_task = tokio::spawn(async move {
        filter::filter_stream(f_cfg, rx_parse, tx_filt).await;
        Ok::<_, AppError>(())
    });

    // 3) RLE compressor (collapse adjacent identical radio-info into a single record w/ duration)
    let rle_task = tokio::spawn(async move {
        rle_filter::rle_compress_stream(rx_filt, tx_rle).await;
        Ok::<_, AppError>(())
    });

    // 4) Transcription adder (enrich first record in a run; concurrency bound = 4)
    let t_record_dir = record_dir.clone();
    let t_transcriber = transcriber.clone();
    let trans_task = tokio::spawn(async move {
        transcription_adder::add_transcriptions(rx_rle, tx_rows, t_record_dir, t_transcriber, 4).await
    });

    // 5) CSV sink (one CSV per input file)
    let out_path = in_path.with_extension("csv");
    let sink = tokio::spawn(async move { csv_sink::write_csv_stream(out_path.as_path(), rx_rows).await });

    // Join all
    let p_res = producer
        .await
        .unwrap_or_else(|e| Err(AppError::IO(format!("producer join: {e}"))));
    let f_res = filter_task
        .await
        .unwrap_or_else(|e| Err(AppError::IO(format!("filter join: {e}"))));
    let rle_res = rle_task
        .await
        .unwrap_or_else(|e| Err(AppError::IO(format!("rle join: {e}"))));
    let t_res = trans_task
        .await
        .unwrap_or_else(|e| Err(AppError::IO(format!("transcriber join: {e}"))));
    let s_res = sink
        .await
        .unwrap_or_else(|e| Err(AppError::IO(format!("sink join: {e}"))));

    p_res?;
    f_res?;
    rle_res?;
    t_res?;
    s_res?;

    info!("Finished {}", in_path.display());
    Ok(())
}
