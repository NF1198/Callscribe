// src/rle_filter.rs
use crate::model::RadioRecord;
use log::{trace, warn};
use tokio::sync::mpsc::{Receiver, Sender};

/// Two records are the "same identity" if their radio-defining fields match.
/// Time does NOT factor into identity; the stream order defines runs.
fn same_identity(a: &RadioRecord, b: &RadioRecord) -> bool {
    // Frequency, radio type, and DCC/NAC
    if a.frequency != b.frequency { return false; }
    if a.radio_type != b.radio_type { return false; }
    if a.dcc != b.dcc { return false; }

    // Slot 1 TG/RID
    if a.slot1.tg != b.slot1.tg { return false; }
    if a.slot1.rid != b.slot1.rid { return false; }

    // Slot 2 TG/RID
    if a.slot2.tg != b.slot2.tg { return false; }
    if a.slot2.rid != b.slot2.rid { return false; }

    true
}

/// Async stage that run-length compresses adjacent records by "radio identity".
/// - Preserves the first record_number and datetime of the run.
/// - Accumulates `duration` in **blocks** (1 per input record), regardless of
///   absolute datetime gaps or duplicates.
/// - Any change in identity starts a new run.
pub async fn rle_compress_stream(mut rx: Receiver<RadioRecord>, tx: Sender<RadioRecord>) {
    let mut cur: Option<RadioRecord> = None;

    while let Some(mut next) = rx.recv().await {
        // Each parsed SRT block contributes at least 1s of duration.
        if next.duration == 0 {
            next.duration = 1;
        }

        match &mut cur {
            None => {
                // Start a new run
                cur = Some(next);
            }
            Some(run) => {
                if same_identity(run, &next) {
                    // Extend the current run by one block (one second equivalent)
                    run.duration = run.duration.saturating_add(1);

                    // Keep the *first* record's timestamp/ID and text, per your spec.
                    // If you ever want to fill missing text from later blocks, you can opt-in:
                    // if run.slot1.text.is_none() { run.slot1.text = next.slot1.text.clone(); }
                    // if run.slot2.text.is_none() { run.slot2.text = next.slot2.text.clone(); }

                    trace!("RLE: extended run rec#{} to {} blocks", run.record_number, run.duration);
                } else {
                    // Identity changed → flush current run and start a new one
                    if tx.send(run.clone()).await.is_err() {
                        warn!("rle_filter: downstream closed on flush; aborting");
                        return;
                    }
                    cur = Some(next);
                }
            }
        }
    }

    // Flush trailing run (if any)
    if let Some(run) = cur {
        let _ = tx.send(run).await;
    }
}
