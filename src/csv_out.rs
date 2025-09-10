use crate::model::RadioRecord;
use crate::errors::AppError;
use csv::Writer;
use std::path::Path;

fn excel_guard(s: &str) -> String {
    if s.starts_with('+') {
        s.trim_start_matches('+').to_string()
    } else {
        s.to_string()
    }
}

pub fn write_csv(records: &[RadioRecord], out_path: &Path) -> Result<(), AppError> {
    let mut wtr = Writer::from_path(out_path)
        .map_err(|e| AppError::IO(format!("open out csv: {}", e)))?;

    // Header (once per file)
    wtr.write_record(&[
        "record_number","datetime","frequency","radio_type","dcc",
        "slot1_tg","slot1_rid","slot1_text",
        "slot2_tg","slot2_rid","slot2_text",
    ]).map_err(|e| AppError::IO(format!("csv write header: {}", e)))?;

    for r in records {
        let datetime = r.datetime.format("%Y-%m-%d %H:%M:%S").to_string();
        let frequency = r.frequency.as_deref().unwrap_or("");
        let radio_type = r.radio_type.as_deref().unwrap_or("");
        let dcc = r.dcc.as_deref().unwrap_or("");
        let s1_tg = r.slot1.tg.as_deref().unwrap_or("");
        let s1_rid = r.slot1.rid.as_deref().unwrap_or("");
        let s1_text = r.slot1.text.as_deref().unwrap_or("");
        let s2_tg = r.slot2.tg.as_deref().unwrap_or("");
        let s2_rid = r.slot2.rid.as_deref().unwrap_or("");
        let s2_text = r.slot2.text.as_deref().unwrap_or("");

        wtr.write_record(&[
            r.record_number.to_string(),
            datetime,
            frequency.to_string(),
            excel_guard(radio_type), // ← now uses a leading single quote
            dcc.to_string(),
            s1_tg.to_string(),
            s1_rid.to_string(),
            s1_text.to_string(),
            s2_tg.to_string(),
            s2_rid.to_string(),
            s2_text.to_string(),
        ]).map_err(|e| AppError::IO(format!("csv write row: {}", e)))?;
    }

    wtr.flush().map_err(|e| AppError::IO(format!("csv flush: {}", e)))?;
    Ok(())
}
