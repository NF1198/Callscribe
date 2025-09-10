use crate::errors::AppError;
use crate::model::{RadioRecord, SlotData};
use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, TimeZone};
use log::{debug, trace, warn};
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::Sender;

#[inline]
fn strip_bom<'a>(s: &'a str) -> &'a str {
    s.strip_prefix('\u{FEFF}').unwrap_or(s)
}

#[inline]
fn apply_tz(
    naive: NaiveDateTime,
    tz_offset: Option<FixedOffset>,
) -> Result<DateTime<FixedOffset>, AppError> {
    match tz_offset {
        Some(off) => off
            .from_local_datetime(&naive)
            .single()
            .ok_or_else(|| AppError::Parse("ambiguous/invalid local datetime for provided timezone".into()))
            .map(|dt| dt.fixed_offset()),
        None => Local
            .from_local_datetime(&naive)
            .single()
            .ok_or_else(|| AppError::Parse("ambiguous/invalid local datetime".into()))
            .map(|dt| dt.fixed_offset()),
    }
}

#[inline]
fn normalize_freq(s: &str) -> String {
    match s.parse::<f64>() {
        Ok(v) => format!("{:.6}", v),
        Err(_) => s.to_string(),
    }
}

/// Parse one event line if it is a "Group call;" line.
/// Returns a fully-populated RadioRecord (duration set), or None for non-call / noise lines.
fn parse_event_line(
    line: &str,
    record_number: usize,
    tz_offset: Option<FixedOffset>,
) -> Result<Option<RadioRecord>, AppError> {
    let s = strip_bom(line).trim();
    if s.is_empty() {
        return Ok(None);
    }
    // Only keep group call lines
    if !s.contains("Group call;") {
        return Ok(None);
    }

    // Tokenize coarsely; first two tokens should be date and time.
    // Example:
    // 2025/09/09  18:39:20  Freq=153.450000  NAC=293  Group call; TG=2  RID=4506   Pri0  7s
    let mut parts = s.split_whitespace();

    let date_tok = match parts.next() {
        Some(t) => t,
        None => return Ok(None),
    };
    let time_tok = match parts.next() {
        Some(t) => t,
        None => return Ok(None),
    };

    // Compose naive datetime
    let dt_str = format!("{} {}", date_tok, time_tok);
    let naive = match NaiveDateTime::parse_from_str(&dt_str, "%Y/%m/%d %H:%M:%S") {
        Ok(ndt) => ndt,
        Err(_) => {
            debug!("event: bad datetime '{}'", dt_str);
            return Ok(None);
        }
    };
    let datetime = apply_tz(naive, tz_offset)?;

    // We’ll scan the rest of the line with simple substring searches.
    // Extract Freq
    let freq = s.find("Freq=").and_then(|i| {
        let tail = &s[i + 5..];
        let tok = tail.split_whitespace().next().unwrap_or("");
        if tok.is_empty() { None } else { Some(normalize_freq(tok)) }
    });

    // Extract NAC or DCC (mutually exclusive in these examples)
    let mut dcc_or_nac: Option<String> = None;
    if let Some(i) = s.find("NAC=") {
        let tail = &s[i + 4..];
        let tok = tail.split_whitespace().next().unwrap_or("");
        if !tok.is_empty() {
            dcc_or_nac = Some(tok.to_string());
        }
    }
    if dcc_or_nac.is_none() {
        if let Some(i) = s.find("DCC=") {
            let tail = &s[i + 4..];
            let tok = tail.split_whitespace().next().unwrap_or("");
            if !tok.is_empty() {
                dcc_or_nac = Some(tok.to_string());
            }
        }
    }

    // Infer radio type based on NAC vs DCC
    // (We keep names consistent with your SRT parser sans leading '+')
    let radio_type = if s.contains("NAC=") {
        Some("P25p1".to_string()) // phase cannot be reliably inferred; default to P25p1
    } else if s.contains("DCC=") {
        Some("DMR".to_string())
    } else {
        None
    };

    // Extract TG and RID
    let tg = s.find("TG=").and_then(|i| {
        let tail = &s[i + 3..];
        let tok = tail.split_whitespace().next().unwrap_or("");
        if tok.is_empty() { None } else { Some(tok.to_string()) }
    });
    let rid = s.find("RID=").and_then(|i| {
        let tail = &s[i + 4..];
        let tok = tail.split_whitespace().next().unwrap_or("");
        if tok.is_empty() { None } else { Some(tok.to_string()) }
    });

    // Optional Slot=1/2 (on DMR only)
    let slot = s.find("Slot=").and_then(|i| {
        let tail = &s[i + 5..];
        let tok = tail.split_whitespace().next().unwrap_or("");
        tok.parse::<u8>().ok()
    });

    // Duration: look for a trailing token like "7s" / "3s"; if none, default 1
    let mut duration = 1u32;
    if let Some(last_tok) = s.split_whitespace().rev().find(|t| t.ends_with('s')) {
        // e.g., "7s" → 7
        if let Some(num) = last_tok.strip_suffix('s').and_then(|n| n.parse::<u32>().ok()) {
            // guard out accidental "Pri0" or similar (which doesn't end with 's' anyway)
            duration = num.max(1);
        }
    }

    // Fill slot fields
    let mut slot1 = SlotData { tg: None, rid: None, text: None };
    let mut slot2 = SlotData { tg: None, rid: None, text: None };
    match (radio_type.as_deref(), slot) {
        (Some("DMR"), Some(1)) => {
            slot1.tg = tg.clone();
            slot1.rid = rid.clone();
        }
        (Some("DMR"), Some(2)) => {
            slot2.tg = tg.clone();
            slot2.rid = rid.clone();
        }
        _ => {
            // P25 and others map to slot1
            slot1.tg = tg.clone();
            slot1.rid = rid.clone();
        }
    }

    let rec = RadioRecord {
        record_number,
        datetime,
        frequency: freq,
        radio_type,
        dcc: dcc_or_nac, // NAC or DCC captured here
        slot1,
        slot2,
        duration,
    };

    Ok(Some(rec))
}

pub async fn stream_file(
    path: &Path,
    tz_offset: Option<FixedOffset>,
    tx: Sender<RadioRecord>,
) -> Result<(), AppError> {
    let file = File::open(path)
        .await
        .map_err(|e| AppError::IO(format!("open {}: {}", path.display(), e)))?;
    let mut lines = BufReader::new(file).lines();

    let mut recno: usize = 1;

    while let Some(line) = lines.next_line().await? {
        match parse_event_line(&line, recno, tz_offset) {
            Ok(Some(rec)) => {
                // Only send valid “Group call;” lines
                if tx.send(rec).await.is_err() {
                    warn!("event_stream: downstream closed; aborting");
                    break;
                }
                recno = recno.saturating_add(1);
            }
            Ok(None) => {
                // Non-call / noise; skip silently
                trace!("event_stream: skipped line");
            }
            Err(e) => {
                debug!("event_stream: parse error: {}", e);
            }
        }
    }

    Ok(())
}
