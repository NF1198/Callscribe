mod cli;
mod srt;
mod model;
mod csv_out;
mod errors;
mod transcriber;
mod record_match;

use crate::errors::AppError;
use log::{info, warn, debug};
use env_logger::Env;
use chrono_tz::Tz;
use chrono::{FixedOffset, Utc};
use std::sync::Arc;
use std::path::Path;

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

fn record_matches_filters(
    r: &model::RadioRecord,
    freqs: &[String],
    rtypes: &[String],
    rids: &[String],
    tgs: &[String],
    nacs: &[String],
) -> bool {
    if !freqs.is_empty() {
        match &r.frequency {
            Some(f) if freqs.iter().any(|q| q == f) => {}
            _ => return false,
        }
    }
    if !rtypes.is_empty() {
        match &r.radio_type {
            Some(t) if rtypes.iter().any(|q| q == t) => {}
            _ => return false,
        }
    }
    if !rids.is_empty() {
        match &r.slot1.rid {
            Some(id) if rids.iter().any(|q| q == id) => {}
            _ => return false,
        }
    }
    if !tgs.is_empty() {
        match &r.slot1.tg {
            Some(tg) if tgs.iter().any(|q| q == tg) => {}
            _ => return false,
        }
    }
    if !nacs.is_empty() {
        match &r.dcc {
            Some(d) if nacs.iter().any(|q| q == d) => {}
            _ => return false,
        }
    }
    true
}

fn main() -> Result<(), AppError> {
    let args = cli::parse_cli();
    setup_logging(&args.log_level);
    info!("Starting: processing {} files", args.input_files.len());

    // Transcriber selection (extend as you add engines)
    let transcriber: Option<Arc<dyn transcriber::Transcriber>> = match args.transcriber.as_str() {
        "text" => Some(Arc::new(transcriber::TextFileTranscriber::new())),
        // "web" | "local_audacity" branches elided on purpose here; add as needed.
        _ => None,
    };

    // Timezone: compute current fixed offset for the provided IANA tz (or None -> Local)
    let tz_offset = if let Some(tzname) = args.tz.as_ref() {
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
    };

    // Process each input independently → per-file CSV with same basename
    for path in &args.input_files {
        info!("Reading file {}", path.display());

        // Parse
        let mut recs = match srt::parse_srt_file(path, tz_offset) {
            Ok(v) => {
                info!("Parsed {} records from {}", v.len(), path.display());
                v
            }
            Err(e) => {
                warn!("Parse error for {}: {}", path.display(), e);
                continue;
            }
        };

        // Optional: match audio + transcribe
        if let Some(ref record_dir) = args.record_dir {
            debug!("Matching audio in {}", record_dir.display());
            for r in recs.iter_mut() {
                if let Ok(Some(audio)) = record_match::find_audio_for_record(&r, Path::new(record_dir)) {
                    r.audio_path = Some(audio.to_string_lossy().to_string());
                    if let Some(ref t) = transcriber {
                        match t.transcribe(Path::new(r.audio_path.as_ref().unwrap())) {
                            Ok(text) => r.slot1.text = Some(text),
                            Err(e) => warn!("Transcription failed for rec {}: {}", r.record_number, e),
                        }
                    }
                }
            }
        }

        // Apply filters
        let filtered: Vec<_> = recs
            .into_iter()
            .filter(|r| record_matches_filters(r, &args.freqs, &args.rtypes, &args.rids, &args.tgs, &args.nacs))
            .collect();

        // Write per-input CSV (same basename, .csv extension)
        let out_path = path.with_extension("csv");
        info!("Writing {} CSV records to {}", filtered.len(), out_path.display());
        csv_out::write_csv(&filtered, &out_path)?;
    }

    info!("Done.");
    Ok(())
}
