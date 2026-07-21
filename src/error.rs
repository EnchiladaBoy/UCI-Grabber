use std::path::PathBuf;

/// Errors returned by UCI Grabber's validation and installation pipeline.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid recipe: {0}")]
    InvalidRecipe(String),
    #[error("invalid catalog: {0}")]
    InvalidCatalog(String),
    #[error("catalog signature verification failed")]
    InvalidSignature,
    #[error("unsupported platform `{0}`")]
    UnsupportedPlatform(String),
    #[error("download failed for {url}: {message}")]
    Download { url: String, message: String },
    #[error("download exceeded its declared or configured size limit ({limit} bytes)")]
    DownloadTooLarge { limit: u64 },
    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("unsafe archive entry `{0}`")]
    UnsafeArchiveEntry(String),
    #[error("archive exceeds safety limit: {0}")]
    ArchiveLimit(String),
    #[error("UCI validation failed: {0}")]
    UciValidation(String),
    #[error("installed engine is not present: {0}")]
    MissingInstall(PathBuf),
    #[error("operation cancelled")]
    Cancelled,
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
