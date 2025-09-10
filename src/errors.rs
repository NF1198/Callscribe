use thiserror::Error;

#[allow(dead_code)]
#[derive(Error, Debug)]
pub enum AppError {
    #[error("IO error: {0}")]
    IO(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("other: {0}")]
    Other(String),
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self { AppError::IO(format!("{}", e)) }
}
