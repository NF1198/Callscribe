use crate::errors::AppError;
use crate::model::{RadioRecord, SlotData};
use chrono::{DateTime, FixedOffset, Local, TimeZone};
use log::{trace, debug, warn};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

pub fn parse_srt_file(path: &Path, tz_offset: Option<FixedOffset>) -> Result<Vec<RadioRecord>, AppError> {
    let f = File::open(path).map_err(|e| AppError::IO(format!("open {}: {}", path.display(), e)))?;
    let r = BufReader::new(f);
    parse_srt_reader(r, tz_offset)
}

pub fn parse_srt_reader<R: BufRead>(mut reader: R, tz_offset: Option<FixedOffset>) -> Result<Vec<RadioRecord>, AppError> {
    const ABS_DT_FMT: &str = "%Y/%m/%d %H:%M:%S";

    let mut out: Vec<RadioRecord> = Vec::new();
    let mut line = String::new();

    #[inline]
    fn trim_cr(s: &mut String) {
        if s.ends_with('\r') { s.pop(); }
    }
    #[inline]
    fn strip_bom<'a>(s: &'a str) -> &'a str {
        s.strip_prefix('\u{FEFF}').unwrap_or(s)
    }
    #[inline]
    fn read_next_line<R: BufRead>(r: &mut R, buf: &mut String) -> io::Result<usize> {
        buf.clear();
        let n = r.read_line(buf)?;
        if n > 0 { trim_cr(buf); }
        Ok(n)
    }
    #[inline]
    fn read_nonempty_line<R: BufRead>(r: &mut R, buf: &mut String) -> io::Result<bool> {
        loop {
            buf.clear();
            let n = r.read_line(buf)?;
            if n == 0 { return Ok(false); }
            trim_cr(buf);
            if !buf.trim().is_empty() { return Ok(true); }
        }
    }
    #[inline]
    fn parse_abs_dt(s: &str) -> Option<chrono::NaiveDateTime> {
        chrono::NaiveDateTime::parse_from_str(strip_bom(s.trim()), ABS_DT_FMT).ok()
    }
    #[inline]
    fn apply_tz(naive: chrono::NaiveDateTime, tz_offset: Option<FixedOffset>) -> Result<DateTime<FixedOffset>, AppError> {
        match tz_offset {
            Some(off) => off.from_local_datetime(&naive)
                .single()
                .ok_or_else(|| AppError::Parse("ambiguous/invalid local datetime for provided timezone".into()))
                .map(|dt| dt.fixed_offset()),
            None => Local.from_local_datetime(&naive)
                .single()
                .ok_or_else(|| AppError::Parse("ambiguous/invalid local datetime".into()))
                .map(|dt| dt.fixed_offset()),
        }
    }

    #[inline]
    fn parse_freq_type_dcc(line: &str) -> (Option<String>, Option<String>, Option<String>) {
        // Accepts:
        //  "425.263000  +DMR  DCC=5"
        //  "153.762500  +P25p2  NAC=6A2"
        //  "153.450000  +IDAS VOICE  NAC=6A2"
        let s = strip_bom(line.trim());

        // freq = first whitespace-separated numeric token
        let mut it = s.split_whitespace().peekable();
        let freq = it.peek().and_then(|t| {
            let ok = t.chars().all(|c| c.is_ascii_digit() || c == '.');
            if ok { Some((*t).to_string()) } else { None }
        });
        if freq.is_some() { it.next(); }

        // type: first token starting with '+', possibly multi-word until we hit a KEY= token
        let mut rtype: Option<String> = None;
        let mut parts: Vec<String> = Vec::new();
        // seek start
        while let Some(tok) = it.next() {
            if tok.starts_with('+') {
                parts.push(tok.to_string());
                break;
            }
        }
        // extend type with following tokens until we hit something like X=Y
        while let Some(&tok) = it.peek() {
            if tok.contains('=') || tok.starts_with('+') { break; }
            parts.push(tok.to_string());
            it.next();
        }
        if !parts.is_empty() { rtype = Some(parts.join(" ")); }

        // DCC or NAC anywhere on the line
        let mut dcc_or_nac: Option<String> = None;
        if let Some(i) = s.find("DCC=") {
            let val = s[i + 4 ..].split_whitespace().next().unwrap_or("").to_string();
            if !val.is_empty() { dcc_or_nac = Some(val); }
        }
        if dcc_or_nac.is_none() {
            if let Some(i) = s.find("NAC=") {
                let val = s[i + 4 ..].split_whitespace().next().unwrap_or("").to_string();
                if !val.is_empty() { dcc_or_nac = Some(val); }
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

    // MAIN STREAM
    loop {
        // 1) index
        if !read_nonempty_line(&mut reader, &mut line)? { break; }
        let idx_raw = strip_bom(line.trim());
        let record_number = idx_raw.parse::<usize>().unwrap_or(0);
        trace!("block start: index={}", record_number);

        // 2) SRT timerange (ignored)
        if !read_nonempty_line(&mut reader, &mut line)? { break; }
        trace!("timerange: {}", strip_bom(line.trim()));

        // 3) absolute datetime
        if !read_nonempty_line(&mut reader, &mut line)? { break; }
        let dt_line = strip_bom(line.trim());
        let naive_dt = match parse_abs_dt(dt_line) {
            Some(ndt) => ndt,
            None => {
                debug!("discarding block index={} — bad datetime line: {:?}", record_number, dt_line);
                // Drain lines until blank/EOF
                loop {
                    let n = read_next_line(&mut reader, &mut line)?;
                    if n == 0 || line.trim().is_empty() { break; }
                }
                continue;
            }
        };
        let datetime = match apply_tz(naive_dt, tz_offset) {
            Ok(dt) => dt,
            Err(e) => {
                debug!("discarding block index={} — timezone mapping error: {}", record_number, e);
                // Drain lines until blank/EOF
                loop {
                    let n = read_next_line(&mut reader, &mut line)?;
                    if n == 0 || line.trim().is_empty() { break; }
                }
                continue;
            }
        };
        trace!("datetime -> {}", datetime);

        // 4) frequency/type/(DCC|NAC)
        if !read_nonempty_line(&mut reader, &mut line)? { break; }
        let (frequency, radio_type, dcc) = parse_freq_type_dcc(&line);
        trace!("freq/type/dcc: freq={:?} type={:?} dcc/nac={:?}", frequency, radio_type, dcc);

        // 5+) details until blank line / EOF
        let mut slot1 = SlotData { tg: None, rid: None, text: None };
        let mut slot2 = SlotData { tg: None, rid: None, text: None };

        loop {
            let n = read_next_line(&mut reader, &mut line)?;
            if n == 0 { break; }                 // EOF ends this block
            let s = line.trim();
            if s.is_empty() { break; }           // blank line separates blocks

            let s_nobom = strip_bom(s);

            if let Some(rest) = s_nobom.strip_prefix("Slot ") {
                // "Slot X  TG=...  RID=..."
                let mut it = rest.split_whitespace();
                let slot_no = it.next().unwrap_or("");
                let rest_of_line = rest.get(slot_no.len()..).unwrap_or("").trim();
                let (tg, rid) = parse_tg_rid(rest_of_line);
                trace!("slot line: slot={} tg={:?} rid={:?}", slot_no, tg, rid);
                match slot_no {
                    "1" => { if tg.is_some() { slot1.tg = tg; } if rid.is_some() { slot1.rid = rid; } }
                    "2" => { if tg.is_some() { slot2.tg = tg; } if rid.is_some() { slot2.rid = rid; } }
                    _   => {}
                }
            } else if s_nobom.starts_with("TG=") || s_nobom.contains(" TG=") || s_nobom.contains("RID=") {
                // single-slot protocols → Slot1.*
                let (tg, rid) = parse_tg_rid(s_nobom);
                trace!("single-slot line: tg={:?} rid={:?}", tg, rid);
                if slot1.tg.is_none() && tg.is_some() { slot1.tg = tg; } else if slot2.tg.is_none() && tg.is_some() { slot2.tg = tg; }
                if slot1.rid.is_none() && rid.is_some() { slot1.rid = rid; } else if slot2.rid.is_none() && rid.is_some() { slot2.rid = rid; }
            } else {
                trace!("ignoring detail line: {}", s_nobom);
            }
        }

        debug!(
            "emit record idx={} dt={} freq={:?} type={:?} dcc/nac={:?} s1(tg={:?},rid={:?}) s2(tg={:?},rid={:?})",
            record_number, datetime, frequency, radio_type, dcc, slot1.tg, slot1.rid, slot2.tg, slot2.rid
        );

        out.push(RadioRecord {
            record_number,
            datetime,
            frequency,
            radio_type,
            dcc,
            slot1,
            slot2,
            audio_path: None,
        });
    }

    if out.is_empty() {
        warn!("parser produced 0 records — enable --log trace to see per-block decisions");
    }
    Ok(out)
}
