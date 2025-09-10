use crate::model::RadioRecord;
use crate::errors::AppError;
use regex::Regex;
use walkdir::WalkDir;
use std::path::{Path, PathBuf};

/// Look for audio file in record_dir/YYYYMMDD with filename pattern matching the record.
/// This implementation uses frequency and time to find a close filename. It's tolerant:
/// it will try multiple filename patterns used by DSDPlus recordings.
pub fn find_audio_for_record(record: &RadioRecord, record_dir: &Path) -> Result<Option<PathBuf>, AppError> {
    let date = record.datetime.date_naive().format("%Y%m%d").to_string();
    let subdir = record_dir.join(date);
    if !subdir.exists() {
        return Ok(None);
    }

    // Regex build: time (HHMMSS) + "_" + freq (e.g., 153.450000) ... we will accept many patterns.
    let time_str = record.datetime.time().format("%H%M%S").to_string();
    let freq_str = record.frequency.clone().unwrap_or_default();
    // build candidate patterns
    let pat1 = format!(r"^{}[_-].*{}.*\.(mp3|wav|txt)$", regex::escape(&time_str), regex::escape(&freq_str));
    let pat2 = format!(r"^{}_{}.*\.(mp3|wav|txt)$", regex::escape(&time_str), regex::escape(&freq_str));
    let re1 = Regex::new(&pat1).unwrap();
    let re2 = Regex::new(&pat2).unwrap();

    for entry in WalkDir::new(&subdir).min_depth(1).max_depth(1) {
        let e = entry.map_err(|e| AppError::IO(format!("walkdir: {}", e)))?;
        if !e.file_type().is_file() { continue; }
        let fname = e.file_name().to_string_lossy().to_string();
        if re1.is_match(&fname) || re2.is_match(&fname) {
            return Ok(Some(e.path().to_path_buf()));
        }
    }
    Ok(None)
}
