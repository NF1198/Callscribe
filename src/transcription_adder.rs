use crate::errors::AppError;
use crate::model::RadioRecord;
use crate::record_match;
use crate::transcriber::Transcriber;
use log::{debug, trace, warn};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc::{Receiver, Sender}, Semaphore};

/// Pulls records from `rx`, (optionally) finds audio & transcribes, then forwards to `tx`.
/// - If `record_dir` or `transcriber` is `None`, it just forwards the records unchanged.
/// - Limits concurrent transcriptions with `max_concurrent` (spawn_blocking).
pub async fn add_transcriptions(
    mut rx: Receiver<RadioRecord>,
    tx: Sender<RadioRecord>,
    record_dir: Option<PathBuf>,
    transcriber: Option<Arc<dyn Transcriber + Send + Sync>>,
    max_concurrent: usize,
) -> Result<(), AppError> {
    // Fast path: no transcription engine or no record directory → forward-only
    if record_dir.is_none() || transcriber.is_none() || max_concurrent == 0 {
        while let Some(rec) = rx.recv().await {
            if tx.send(rec).await.is_err() {
                warn!("downstream closed; stopping transcription_adder (fast path)");
                break;
            }
        }
        return Ok(());
    }

    let dir = record_dir.unwrap();
    let t = transcriber.unwrap();
    let sem = Arc::new(Semaphore::new(max_concurrent));

    while let Some(mut rec) = rx.recv().await {
        // Try to locate audio if not already attached
        if rec.audio_path.is_none() {
            match record_match::find_audio_for_record(&rec, Path::new(&dir)) {
                Ok(Some(p)) => {
                    trace!("audio match rec#{} -> {}", rec.record_number, p.display());
                    rec.audio_path = Some(p.to_string_lossy().to_string());
                }
                Ok(None) => {
                    trace!("no audio for rec#{}", rec.record_number);
                }
                Err(e) => {
                    debug!("audio match error rec#{}: {}", rec.record_number, e);
                }
            }
        }

        // Transcribe if we have audio and no text yet
        if rec.audio_path.is_some() && rec.slot1.text.is_none() {
            let audio = rec.audio_path.clone().unwrap();
            let tcl = t.clone();
            let permit = sem.clone().acquire_owned().await.unwrap();

            let result = tokio::task::spawn_blocking(move || {
                let _p = permit; // hold semaphore while blocking
                tcl.transcribe(Path::new(&audio))
            })
            .await
            .ok()
            .and_then(|r| r.ok());

            if let Some(text) = result {
                rec.slot1.text = Some(text);
            }
        }

        if tx.send(rec).await.is_err() {
            warn!("downstream closed; stopping transcription_adder");
            break;
        }
    }

    Ok(())
}
