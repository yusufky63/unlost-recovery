use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Format(#[from] unlost_formats::Error),
    /// The destination shares a physical disk with the source.
    #[error("the destination is on the same physical disk as the source ({0})")]
    SameDisk(String),
    /// The destination's physical disk could not be determined and the user did not confirm.
    #[error("cannot tell which physical disk the destination is on: {0}")]
    UnknownDisk(String),
    #[error("administrator rights are required")]
    NeedsAdmin,
    /// The administrator account that approved UAC is not the signed-in user.
    #[error("the administrator account is not the signed-in user")]
    OtherUser,
    #[error("the request was refused: {0}")]
    Refused(String),
    #[error("the broker stopped: {0}")]
    Broker(String),
    #[error("the job was cancelled")]
    Cancelled,
    #[error("{0}")]
    Windows(String),
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }

    /// Stable machine-readable code the UI maps to a translated message.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Io { source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied => {
                "access_denied"
            }
            Error::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                "not_found"
            }
            Error::Io { source, .. } if source.kind() == std::io::ErrorKind::AlreadyExists => {
                "exists"
            }
            Error::Io { .. } => "io",
            Error::Format(unlost_formats::Error::Cancelled) => "cancelled",
            Error::Format(e) => e.code(),
            Error::SameDisk(_) => "same_disk",
            Error::UnknownDisk(_) => "unknown_disk",
            Error::NeedsAdmin => "needs_admin",
            Error::OtherUser => "other_user",
            Error::Refused(_) => "refused",
            Error::Broker(_) => "broker",
            Error::Cancelled => "cancelled",
            Error::Windows(_) => "windows",
            Error::Other(_) => "other",
        }
    }
}

impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Error::Windows(e.message())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Other(e.to_string())
    }
}

/// Error shape sent to the UI and over the broker pipe: a code for translation plus the raw
/// message for details.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
}

impl From<&Error> for ErrorPayload {
    fn from(e: &Error) -> Self {
        ErrorPayload {
            code: e.code().to_string(),
            message: e.to_string(),
        }
    }
}

impl From<Error> for ErrorPayload {
    fn from(e: Error) -> Self {
        ErrorPayload::from(&e)
    }
}

impl From<ErrorPayload> for Error {
    fn from(p: ErrorPayload) -> Self {
        match p.code.as_str() {
            "cancelled" => Error::Cancelled,
            "needs_admin" => Error::NeedsAdmin,
            "other_user" => Error::OtherUser,
            "same_disk" => Error::SameDisk(p.message),
            "unknown_disk" => Error::UnknownDisk(p.message),
            "refused" => Error::Refused(p.message),
            _ => Error::Other(p.message),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
