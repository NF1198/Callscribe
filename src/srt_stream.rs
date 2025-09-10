use crate::errors::AppError;
use crate::model::{RadioRecord, SlotData};
use chrono::{DateTime, FixedOffset, Local, TimeZone};
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
    naive: chrono::NaiveDateTime,
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
fn parse_freq_type_dcc(line: &str) -> (Option<String>, Option<String>, Option<String>) {
    let s = strip_bom(line.trim());
    let mut it = s.split_whitespace().peekable();

    let freq = it.peek().and_then(|t| {
        let ok = t.chars().all(|c| c.is_ascii_digit() || c == '.');
        if ok { Some((*t).to_string()) } else { None }
    });
    if freq.is_some() { it.next(); }

    let mut rtype: Option<String> = None;
    let mut parts: Vec<String> = Vec::new();
    while let Some(tok) = it.next() {
        if tok.starts_with('+') {
            parts.push(tok.to_string());
            break;
        }
    }
    while let Some(&tok) = it.peek() {
        if tok.contains('=') || tok.starts_with('+') { break; }
        parts.push(tok.to_string());
        it.next();
    }
    if !parts.is_empty() {
        let joined = parts.join(" ");
        // Excel guard: drop leading '+'
        let guarded = joined.trim_start_matches('+').to_string();
        rtype = Some(guarded);
    }

    let mut dcc_or_nac: Option<String> = None;
    if let Some(i) = s.find("DCC=") {
        let val = s[i + 4..].split_whitespace().next().unwrap_or("").to_string();
        if !val.is_empty() {
            dcc_or_nac = Some(val);
        }
    }
    if dcc_or_nac.is_none() {
        if let Some(i) = s.find("NAC=") {
            let val = s[i + 4..].split_whitespace().next().unwrap_or("").to_string();
            if !val.is_empty() {
                dcc_or_nac = Some(val);
            }
        }
    }
    (freq, rtype, dcc_or_nac)
}

#[inline]
fn parse_tg_rid(s: &str) -> (Option<String>, Option<String>) {
    let s = strip_bom(s);
    let mut tg: Option<String> = None;
    let mut rid: Option<String> = None;
    for tok in s.split_whitespace() {
        if let Some(rest) = tok.strip_prefix("TG=") {
            if !rest.is_empty() { tg = Some(rest.to_string()); }
        } else if let Some(rest) = tok.strip_prefix("RID=") {
            if !rest.is_empty() { rid = Some(rest.to_string()); }
        }
    }
    (tg, rid)
}

pub async fn stream_file(
    path: &Path,
    tz_offset: Option<FixedOffset>,
    tx: Sender<RadioRecord>,
) -> Result<(), AppError> {
    let file = File::open(path)
        .await
        .map_err(|e| AppError::IO(format!("open {}: {}", path.display(), e)))?;
    let mut r = BufReader::new(file).lines();

    const ABS_DT_FMT: &str = "%Y/%m/%d %H:%M:%S";

    loop {
        // 1) index
        let idx_line = match r.next_line().await? {
            Some(s) => s,
            None => break,
        };
        if idx_line.trim().is_empty() {
            continue;
        }
        let idx_raw = strip_bom(idx_line.trim());
        let record_number = match idx_raw.parse::<usize>() {
            Ok(v) => v,
            Err(_) => {
                debug!("non-numeric index: {:?}", idx_raw);
                drain_block(&mut r).await?;
                continue;
            }
        };
        trace!("block start: index={}", record_number);

        // 2) timerange (ignored)
        let _timerange = match r.next_line().await? {
            Some(s) => s,
            None => break,
        };

        // 3) absolute datetime
        let dt_line = match r.next_line().await? {
            Some(s) => s,
            None => break,
        };
        let dt_s = strip_bom(dt_line.trim());
        let naive_dt = match chrono::NaiveDateTime::parse_from_str(dt_s, ABS_DT_FMT) {
            Ok(ndt) => ndt,
            Err(_) => {
                debug!("discarding block index={} — bad datetime line: {:?}", record_number, dt_s);
                drain_block(&mut r).await?;
                continue;
            }
        };
        let datetime = match apply_tz(naive_dt, tz_offset) {
            Ok(dt) => dt,
            Err(e) => {
                debug!("discarding block index={} — tz error: {}", record_number, e);
                drain_block(&mut r).await?;
                continue;
            }
        };

        // 4) freq/type/(DCC|NAC)
        let freq_line = match r.next_line().await? {
            Some(s) => s,
            None => break,
        };
        let (frequency, radio_type, dcc) = parse_freq_type_dcc(&freq_line);

        // 5+) details: until blank or EOF
        let mut slot1 = SlotData { tg: None, rid: None, text: None };
        let mut slot2 = SlotData { tg: None, rid: None, text: None };

        loop {
            let nxt = r.next_line().await?;
            let Some(line) = nxt else { break; };
            let s = line.trim();
            if s.is_empty() { break; }
            let s_nb = strip_bom(s);

            if let Some(rest) = s_nb.strip_prefix("Slot ") {
                let mut it = rest.split_whitespace();
                let slot_no = it.next().unwrap_or("");
                let rest_of_line = rest.get(slot_no.len()..).unwrap_or("").trim();
                let (tg, rid) = parse_tg_rid(rest_of_line);
                match slot_no {
                    "1" => { if tg.is_some() { slot1.tg = tg; } if rid.is_some() { slot1.rid = rid; } }
                    "2" => { if tg.is_some() { slot2.tg = tg; } if rid.is_some() { slot2.rid = rid; } }
                    _ => {}
                }
            } else if s_nb.starts_with("TG=") || s_nb.contains(" TG=") || s_nb.contains("RID=") {
                let (tg, rid) = parse_tg_rid(s_nb);
                if slot1.tg.is_none() && tg.is_some() { slot1.tg = tg; } else if slot2.tg.is_none() && tg.is_some() { slot2.tg = tg; }
                if slot1.rid.is_none() && rid.is_some() { slot1.rid = rid; } else if slot2.rid.is_none() && rid.is_some() { slot2.rid = rid; }
            }
        }

        let rec = RadioRecord {
            record_number,
            datetime,
            frequency,
            radio_type,
            dcc,
            slot1,
            slot2,
            audio_path: None,
        };

        if tx.send(rec).await.is_err() {
            warn!("downstream closed; aborting parser");
            break;
        }
    }

    Ok(())
}

async fn drain_block<R: AsyncBufReadExt + Unpin>(r: &mut tokio::io::Lines<R>) -> Result<(), AppError> {
    while let Some(line) = r.next_line().await? {
        if line.trim().is_empty() { break; }
    }
    Ok(())
}
