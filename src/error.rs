use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorkerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Download error: {0}")]
    Download(String),

    #[error("{0}")]
    Other(String),

    #[error("Anyhow error: {0}")]
    Anyhow(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, WorkerError>;
