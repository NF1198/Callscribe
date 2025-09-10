// src/transcriber.rs
use crate::errors::AppError;
use crate::model::RadioRecord;
use log::{debug, trace};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use walkdir::WalkDir;

/// New contract:
/// - Ok(Some(text))    => transcription available
/// - Ok(None)          => no transcription available for this record
/// - Err(Some(err))    => hard failure (I/O, parse) that should be logged
/// - Err(None)         => soft failure (intentionally suppressed)
pub trait Transcriber: Send + Sync {
    fn transcribe(
        &self,
        rec: &RadioRecord,
        record_dir: &Path,
    ) -> Result<Option<String>, Option<AppError>>;
}

/// Key for per-day indexing (date is implicit in the day shard).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct K {
    time: u32,         // HHMMSS
    freq: String,      // "153.450000"
    tg: Option<u32>,
    rid: Option<u32>,
}

#[derive(Default)]
struct DayIndex {
    // Most specific first; fallbacks after
    full: HashMap<K, PathBuf>,      // time+freq+tg+rid
    rid_only: HashMap<K, PathBuf>,  // time+freq+rid (tg=None)
    tg_only: HashMap<K, PathBuf>,   // time+freq+tg  (rid=None)
    bare: HashMap<K, PathBuf>,      // time+freq     (tg=None, rid=None)
}

impl DayIndex {
    fn insert_all(&mut self, k: K, path: PathBuf) {
        // full
        self.full.entry(k.clone()).or_insert_with(|| path.clone());
        // rid_only
        self.rid_only
            .entry(K { tg: None, ..k.clone() })
            .or_insert_with(|| path.clone());
        // tg_only
        self.tg_only
            .entry(K { rid: None, ..k.clone() })
            .or_insert_with(|| path.clone());
        // bare
        self.bare
            .entry(K { tg: None, rid: None, ..k })
            .or_insert_with(|| path);
    }

    fn lookup(&self, k: &K) -> Option<&PathBuf> {
        if let Some(p) = self.full.get(k) {
            return Some(p);
        }
        let rid_k = K { tg: None, ..k.clone() };
        if let Some(p) = self.rid_only.get(&rid_k) {
            return Some(p);
        }
        let tg_k = K { rid: None, ..k.clone() };
        if let Some(p) = self.tg_only.get(&tg_k) {
            return Some(p);
        }
        let bare_k = K {
            tg: None,
            rid: None,
            ..k.clone()
        };
        self.bare.get(&bare_k)
    }
}

#[derive(Default)]
struct Index {
    // Sharded by YYYYMMDD
    days: HashMap<u32, DayIndex>,
}

/// Incremental, on-demand, per-day indexed text transcriber.
/// - Walks the Record directory lazily for the day shard requested by each record.
/// - Two-level traversal: `<root>/<YYYYMMDD>/*.txt`
/// - Filename pattern examples:
///     "064356_153.450000_004_P25__GC_2_4506.txt"
///     HHMMSS  FREQ                       TG  RID
pub struct TextFileTranscriber {
    // Root "Record" directory (top level that contains YYYYMMDD subfolders)
    root: PathBuf,
    // Mutable, lazily-populated day shards
    index: Arc<RwLock<Index>>,
}

impl TextFileTranscriber {
    /// Construct an incremental-indexer transcriber.
    /// It does not pre-scan; day folders are indexed on first use.
    pub fn new_indexed(root: &Path) -> Result<Self, AppError> {
        Ok(Self {
            root: root.to_path_buf(),
            index: Arc::new(RwLock::new(Index::default())),
        })
    }

    /// Convenience empty constructor (will error at runtime if no root is passed in at transcribe()).
    pub fn new() -> Self {
        Self {
            root: PathBuf::new(),
            index: Arc::new(RwLock::new(Index::default())),
        }
    }

    /// Ensure the YYYYMMDD shard is present; if not, scan `<root>/<day>/` once.
    fn ensure_day_indexed(&self, day: u32, record_dir: &Path) -> Result<(), AppError> {
        // Fast path: read lock says it's already indexed
        {
            let guard = self
                .index
                .read()
                .map_err(|_| AppError::Parse("index read lock poisoned".into()))?;
            if guard.days.contains_key(&day) {
                return Ok(());
            }
        }

        // Slow path: acquire write and double-check
        let mut guard = self
            .index
            .write()
            .map_err(|_| AppError::Parse("index write lock poisoned".into()))?;
        if guard.days.contains_key(&day) {
            return Ok(());
        }

        let day_dir = record_dir.join(format!("{:08}", day));
        if !day_dir.is_dir() {
            trace!("TextFileTranscriber: day dir missing: {}", day_dir.display());
            // insert empty shard to avoid re-probing
            guard.days.insert(day, DayIndex::default());
            return Ok(());
        }

        let mut shard = DayIndex::default();

        for entry in WalkDir::new(&day_dir).max_depth(1).min_depth(1) {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    trace!("walk error: {}", e);
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let name = match entry.file_name().to_str() {
                Some(s) => s,
                None => continue,
            };
            if !name.ends_with(".txt") {
                continue;
            }

            // Expect "HHMMSS_FREQ_....txt"
            let stem = name.trim_end_matches(".txt");
            let parts: Vec<&str> = stem.split('_').collect();
            if parts.len() < 2 {
                continue;
            }

            // HHMMSS
            let time = match parts[0].parse::<u32>() {
                Ok(v) => v,
                Err(_) => continue,
            };
            // Frequency
            let freq = normalize_freq(parts[1]);

            // Extract TG and RID from numeric tokens in the remainder (heuristic).
            // Example tail: "... _GC_2_4506" → TG=2, RID=4506
            let mut tg: Option<u32> = None;
            let mut rid: Option<u32> = None;
            for &tok in &parts[2..] {
                if let Ok(n) = tok.parse::<u32>() {
                    if tg.is_none() {
                        tg = Some(n);
                    } else if rid.is_none() {
                        rid = Some(n);
                    } else {
                        // already have both; ignore extras
                    }
                }
            }

            let k = K {
                time,
                freq,
                tg,
                rid,
            };
            let p = entry.into_path();
            shard.insert_all(k, p);
        }

        debug!(
            "TextFileTranscriber: indexed day {} (files: full={}, rid_only={}, tg_only={}, bare={})",
            day,
            shard.full.len(),
            shard.rid_only.len(),
            shard.tg_only.len(),
            shard.bare.len()
        );
        guard.days.insert(day, shard);
        Ok(())
    }

    fn lookup_in_day(&self, day: u32, k: &K) -> Option<PathBuf> {
        let guard = self.index.read().ok()?;
        let di = guard.days.get(&day)?;
        di.lookup(k).cloned()
    }

    fn day_from_rec(rec: &RadioRecord) -> Option<u32> {
        rec.datetime
            .format("%Y%m%d")
            .to_string()
            .parse::<u32>()
            .ok()
    }

    fn key_from_rec(rec: &RadioRecord) -> Option<K> {
        let time = rec
            .datetime
            .format("%H%M%S")
            .to_string()
            .parse::<u32>()
            .ok()?;
        let freq_s = rec.frequency.as_deref()?;
        let freq = normalize_freq(freq_s);
        let tg = rec
            .slot1
            .tg
            .as_deref()
            .and_then(|s| s.parse::<u32>().ok());
        let rid = rec
            .slot1
            .rid
            .as_deref()
            .and_then(|s| s.parse::<u32>().ok());
        Some(K { time, freq, tg, rid })
    }
}

impl Transcriber for TextFileTranscriber {
    fn transcribe(
        &self,
        rec: &RadioRecord,
        record_dir: &Path,
    ) -> Result<Option<String>, Option<AppError>> {
        // Determine day shard & key
        let day = match Self::day_from_rec(rec) {
            Some(d) => d,
            None => return Ok(None),
        };
        let key = match Self::key_from_rec(rec) {
            Some(k) => k,
            None => return Ok(None),
        };

        // Choose the correct root:
        // - Prefer `record_dir` provided by the pipeline
        // - Fallback to the root specified at construction time
        let root = if record_dir.as_os_str().is_empty() {
            if self.root.as_os_str().is_empty() {
                return Err(Some(AppError::Parse(
                    "TextFileTranscriber has no record_dir root".into(),
                )));
            }
            self.root.as_path()
        } else {
            record_dir
        };

        // Ensure the day shard is available
        if let Err(e) = self.ensure_day_indexed(day, root) {
            return Err(Some(e));
        }

        // Lookup with fallbacks (full → rid_only → tg_only → bare)
        if let Some(path) = self.lookup_in_day(day, &key) {
            debug!("TextFileTranscriber: using {}", path.display());
            match fs::read_to_string(&path) {
                Ok(s) => return Ok(Some(s)),
                Err(e) => return Err(Some(AppError::IO(format!("read {}: {}", path.display(), e)))),
            }
        }

        trace!(
            "TextFileTranscriber: no transcript for day={} key={:?}",
            day,
            key
        );
        Ok(None)
    }
}

fn normalize_freq(s: &str) -> String {
    match s.parse::<f64>() {
        Ok(v) => format!("{:.6}", v),
        Err(_) => s.to_string(),
    }
}
