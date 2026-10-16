#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("read failed at byte {offset}: {source}")]
    Read {
        offset: u64,
        #[source]
        source: std::io::Error,
    },
    #[error("{what}: {detail}")]
    Invalid { what: &'static str, detail: String },
    #[error("{0}")]
    Unsupported(String),
    #[error("limit reached: {0}")]
    Limit(String),
    #[error("the job was cancelled")]
    Cancelled,
}

impl Error {
    pub fn invalid(what: &'static str, detail: impl Into<String>) -> Self {
        Error::Invalid {
            what,
            detail: detail.into(),
        }
    }

    /// Stable machine-readable code, shared with `unlost-core` errors.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Read { .. } => "read_failed",
            Error::Invalid { .. } => "invalid_format",
            Error::Unsupported(_) => "unsupported",
            Error::Limit(_) => "limit",
            Error::Cancelled => "cancelled",
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
