use crate::errors::AppError;
use std::path::{Path};

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, audio_path: &Path) -> Result<String, AppError>;
}

/// A simple transcriber implementation that looks for a `.txt` file with the same
/// base name as the provided audio path and returns its contents as the transcription.
///
/// Behavior:
/// - If `<audio_basename>.txt` exists -> return its trimmed contents.
/// - If the `.txt` file is missing -> return `Ok(String::new())` (no transcription).
/// - If reading fails for other reasons -> return `Err(AppError::IO(...))`.
pub struct TextFileTranscriber;

impl TextFileTranscriber {
    pub fn new() -> Self {
        TextFileTranscriber
    }

    fn txt_path_for(&self, audio_path: &std::path::Path) -> std::path::PathBuf {
        let mut p = audio_path.to_path_buf();
        p.set_extension("txt");
        p
    }
}

impl Transcriber for TextFileTranscriber {
    fn transcribe(&self, audio_path: &std::path::Path) -> Result<String, crate::errors::AppError> {
        let txt_path = self.txt_path_for(audio_path);
        match std::fs::read_to_string(&txt_path) {
            Ok(s) => Ok(s.trim().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(crate::errors::AppError::IO(format!(
                "reading text transcript '{}': {}",
                txt_path.display(),
                e
            ))),
        }
    }
}