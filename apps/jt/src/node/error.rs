use std::{fmt, io, path::PathBuf};

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug)]
pub enum AppError {
    Invalid(String),
    Io {
        action: String,
        path: Option<PathBuf>,
        source: io::Error,
    },
    Command {
        action: String,
        status: i32,
        detail: String,
    },
    Decode {
        action: String,
        detail: String,
    },
    UnsafePath(String),
}

impl AppError {
    pub fn io(
        action: impl Into<String>,
        path: impl Into<Option<PathBuf>>,
        source: io::Error,
    ) -> Self {
        Self::Io {
            action: action.into(),
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::UnsafePath(message) => formatter.write_str(message),
            Self::Io {
                action,
                path,
                source,
            } => {
                write!(formatter, "{action}")?;
                if let Some(path) = path {
                    write!(formatter, " ({})", path.display())?;
                }
                write!(formatter, ": {source}")
            }
            Self::Command {
                action,
                status,
                detail,
            } => write!(formatter, "{action} failed (exit {status}): {detail}"),
            Self::Decode { action, detail } => write!(formatter, "{action}: {detail}"),
        }
    }
}

impl std::error::Error for AppError {}
