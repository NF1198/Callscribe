mod cli;
mod srt_stream;
mod model;
mod csv_sink;
mod errors;
mod transcriber;
mod record_match;
mod filter;
mod transcription_adder;

use crate::errors::AppError;
use chrono::{FixedOffset, Utc};
use chrono_tz::Tz;
use env_logger::Env;
use log::{info, warn};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

fn setup_logging(level: &str) {
    let env = Env::default().filter_or("RUST_LOG", match level {
        "essential" => "info",
        "debug" => "debug",
        "trace" => "trace",
        "warn" => "warn",
        "error" => "error",
        _ => "info",
    });
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

    let transcriber: Option<Arc<dyn transcriber::Transcriber + Send + Sync>> =
        match args.transcriber.as_str() {
            "text" => Some(Arc::new(transcriber::TextFileTranscriber::new())),
            _ => None,
        };

    let tz_offset = compute_tz_offset(&args.tz);

    let cfg = Arc::new(filter::FilterConfig {
        freqs: args.freqs.clone(),
        rtypes: args.rtypes.clone(),
        rids: args.rids.clone(),
        tgs: args.tgs.clone(),
        nacs: args.nacs.clone(),
    });

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

    let (tx_parse, rx_parse) = mpsc::channel::<RadioRecord>(1024);
    let (tx_filt, rx_filt) = mpsc::channel::<RadioRecord>(1024);
    let (tx_rows, rx_rows) = mpsc::channel::<RadioRecord>(1024);

    // 1) Parser
    let p_in = in_path.clone();
    let producer = tokio::spawn(async move {
        srt_stream::stream_file(&p_in, tz_offset, tx_parse).await
    });

    // 2) Filter
    let f_cfg = Arc::clone(&cfg);
    let filter_task = tokio::spawn(async move {
        filter::filter_stream(f_cfg, rx_parse, tx_filt).await;
        Ok::<_, AppError>(())
    });

    // 3) Transcription adder
    let t_record_dir = record_dir.clone();
    let t_transcriber = transcriber.clone();
    let trans_task = tokio::spawn(async move {
        transcription_adder::add_transcriptions(rx_filt, tx_rows, t_record_dir, t_transcriber, 4).await
    });

    // 4) CSV sink
    let out_path = in_path.with_extension("csv");
    let sink = tokio::spawn(async move {
        csv_sink::write_csv_stream(out_path.as_path(), rx_rows).await
    });

    let p_res = producer.await.unwrap_or_else(|e| Err(AppError::IO(format!("producer join: {e}"))));
    let f_res = filter_task.await.unwrap_or_else(|e| Err(AppError::IO(format!("filter join: {e}"))));
    let t_res = trans_task.await.unwrap_or_else(|e| Err(AppError::IO(format!("transcriber join: {e}"))));
    let s_res = sink.await.unwrap_or_else(|e| Err(AppError::IO(format!("sink join: {e}"))));

    p_res?;
    f_res?;
    t_res?;
    s_res?;
    info!("Finished {}", in_path.display());
    Ok(())
}
