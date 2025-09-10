use crate::errors::AppError;
use crate::model::RadioRecord;
use crate::transcriber::Transcriber;
use log::{debug, trace, warn};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Semaphore;

/// Stage: consumes records, optionally adds transcription text, forwards downstream.
///
/// Behavior:
/// - If `record_dir` or `transcriber` is `None`, or `max_concurrent == 0`,
///   this stage becomes a pass-through.
/// - Otherwise, transcriptions are performed in a bounded `spawn_blocking` pool.
/// - This stage does not perform any file-system probing itself; it delegates
///   responsibility entirely to the provided `Transcriber`.
pub async fn add_transcriptions(
    mut rx: Receiver<RadioRecord>,
    tx: Sender<RadioRecord>,
    record_dir: Option<PathBuf>,
    transcriber: Option<Arc<dyn Transcriber + Send + Sync>>,
    max_concurrent: usize,
) -> Result<(), AppError> {
    // Fast path: no enrichment, just forward records.
    if record_dir.is_none() || transcriber.is_none() || max_concurrent == 0 {
        trace!("transcription_adder: fast-path (no transcriber/dir or concurrency==0)");
        while let Some(rec) = rx.recv().await {
            if tx.send(rec).await.is_err() {
                warn!("transcription_adder: downstream closed (fast-path)");
                break;
            }
        }
        return Ok(());
    }

    let dir = record_dir.unwrap();
    let t = transcriber.unwrap();
    let sem = Arc::new(Semaphore::new(max_concurrent));

    while let Some(mut rec) = rx.recv().await {
        // Only attempt transcription if we don't already have text.
        if rec.slot1.text.is_none() {
            // Clone minimal state into the blocking task. If RadioRecord is large,
            // this clone is still cheaper than blocking the async runtime thread.
            let rec_for_lookup = rec.clone();
            let dir_clone = dir.clone();
            let t_clone = t.clone();
            let permit = sem.clone().acquire_owned().await.unwrap();

            let res = tokio::task::spawn_blocking(move || {
                let _guard = permit; // hold the semaphore while we do blocking work
                t_clone.transcribe(&rec_for_lookup, &dir_clone)
            })
            .await
            .map_err(|e| AppError::IO(format!("transcriber join error: {e}")))?;

            match res {
                Ok(Some(text)) => {
                    debug!("transcription_adder: rec#{} -> text attached", rec.record_number);
                    rec.slot1.text = Some(text);
                }
                Ok(None) => {
                    // No transcript available; proceed silently.
                }
                Err(Some(e)) => {
                    debug!("transcription_adder: rec#{} transcription error: {}", rec.record_number, e);
                }
                Err(None) => {
                    // Soft failure; intentionally ignored.
                }
            }
        }

        if tx.send(rec).await.is_err() {
            warn!("transcription_adder: downstream closed");
            break;
        }
    }

    Ok(())
}
