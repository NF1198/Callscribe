mod cli;
mod model;
mod errors;
mod transcriber;
mod record_match;

mod srt_stream;
mod csv_sink;
mod filter;

use crate::errors::AppError;
use chrono::{FixedOffset, Utc};
use chrono_tz::Tz;
use env_logger::Env;
use log::{debug, info, warn};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

fn setup_logging(level: &str) {
    let env = Env::default().filter_or(
        "RUST_LOG",
        match level {
            "info" => "info",
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

    // Transcriber (optional); keep behavior-based trait and spawn_blocking when used.
    let transcriber: Option<Arc<dyn transcriber::Transcriber + Send + Sync>> =
        match args.transcriber.as_str() {
            "text" => Some(Arc::new(transcriber::TextFileTranscriber::new())),
            _ => None,
        };

    let tz_offset = compute_tz_offset(&args.tz);

    // Build filter config once; share via Arc
    let cfg = Arc::new(filter::FilterConfig {
        freqs: args.freqs.clone(),
        rtypes: args.rtypes.clone(),
        rids: args.rids.clone(),
        tgs: args.tgs.clone(),
        nacs: args.nacs.clone(),
    });

    // Process each file as its own concurrent pipeline
    let mut tasks = Vec::new();
    for in_path in &args.input_files {
        let in_path = in_path.clone();
        let tz_offset = tz_offset;
        let cfg = Arc::clone(&cfg);
        let transcriber = transcriber.clone();
        let record_dir = args.record_dir.clone();

        let t = tokio::spawn(async move {
            if let Err(e) = run_pipeline(in_path, tz_offset, cfg, transcriber, record_dir).await {
                warn!("pipeline failed: {}", e);
            }
        });
        tasks.push(t);
    }

    // Wait all
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

    // Channels between stages
    // srt_stream -> filter/transcribe
    let (tx_records, rx_records) = mpsc::channel::<RadioRecord>(1024);
    // filtered/transcribed -> csv sink
    let (tx_rows, rx_rows) = mpsc::channel::<RadioRecord>(1024);

    // 1) SRT parser task (producer)
    let p_in = in_path.clone();
    let producer = tokio::spawn(async move {
        srt_stream::stream_file(&p_in, tz_offset, tx_records).await
    });

    // 2) Filter (+ optional transcription) stage
    let f_cfg = Arc::clone(&cfg);
    let f_record_dir = record_dir.clone();
    let f_transcriber = transcriber.clone();

    let filter_stage = tokio::spawn(async move {
        use tokio::sync::Semaphore;
        let sem = Arc::new(Semaphore::new(4)); // limit parallel transcriptions

        let mut rx = rx_records;
        while let Some(mut rec) = rx.recv().await {
            if !f_cfg.accept(&rec) {
                continue;
            }

            // Optional transcription on slot1.text using audio match
            if let (Some(dir), Some(t)) = (&f_record_dir, &f_transcriber) {
                if rec.audio_path.is_none() {
                    if let Ok(Some(p)) = record_match::find_audio_for_record(&rec, dir.as_path()) {
                        rec.audio_path = Some(p.to_string_lossy().to_string());
                    }
                }
                if rec.audio_path.is_some() && rec.slot1.text.is_none() {
                    let p = rec.audio_path.clone().unwrap();
                    let t = t.clone();
                    let permit = sem.clone().acquire_owned().await.unwrap();
                    // Run transcription in blocking pool to avoid starving reactor
                    let text = tokio::task::spawn_blocking(move || {
                        let _permit = permit; // hold until done
                        t.transcribe(std::path::Path::new(&p))
                    })
                    .await
                    .ok()
                    .and_then(|r| r.ok());
                    if let Some(txt) = text {
                        rec.slot1.text = Some(txt);
                    }
                }
            }

            if tx_rows.send(rec).await.is_err() {
                break;
            }
        }
        Ok::<_, AppError>(())
    });

    // 3) CSV sink task (consumer)
    let out_path = in_path.with_extension("csv");
    let sink = tokio::spawn(async move {
        csv_sink::write_csv_stream(out_path.as_path(), rx_rows).await
    });

    // Join stages
    let p_res = producer.await.unwrap_or_else(|e| Err(AppError::IO(format!("producer join: {e}"))));
    let f_res = filter_stage.await.unwrap_or_else(|e| Err(AppError::IO(format!("filter join: {e}"))));
    let s_res = sink.await.unwrap_or_else(|e| Err(AppError::IO(format!("sink join: {e}"))));

    // Report
    if let Err(e) = &p_res { debug!("producer error: {}", e); }
    if let Err(e) = &f_res { debug!("filter error: {}", e); }
    if let Err(e) = &s_res { debug!("sink error: {}", e); }

    // Prefer earlier stage error if any
    p_res?;
    f_res?;
    s_res?;
    info!("Finished {}", in_path.display());
    Ok(())
}
