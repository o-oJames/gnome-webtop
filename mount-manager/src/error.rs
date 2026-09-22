//! A tiny error type with a user facing message plus optional technical detail.

use std::fmt;

/// Error carrying a short, human readable `message` and optional `detail`
/// (usually captured stderr) that the UI shows in an expander.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    pub detail: Option<String>,
}

impl Error {
    pub fn new<M: Into<String>>(message: M) -> Self {
        Self {
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail<M: Into<String>, D: Into<String>>(message: M, detail: D) -> Self {
        let detail = detail.into();
        Self {
            message: message.into(),
            detail: if detail.trim().is_empty() {
                None
            } else {
                Some(detail)
            },
        }
    }

    /// Append a hint line to the detail block (creates it when missing).
    pub fn hint<M: Into<String>>(mut self, hint: M) -> Self {
        let line = hint.into();
        match self.detail {
            Some(ref mut d) => {
                if !d.is_empty() && !d.ends_with('\n') {
                    d.push('\n');
                }
                d.push_str(&line);
            }
            None => self.detail = Some(line),
        }
        self
    }

    /// `true` when the user dismissed an authentication dialog.
    pub fn is_cancelled(&self) -> bool {
        self.message.contains("cancelled") || self.message.contains("Canceled")
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(d) => write!(f, "{}\n{}", self.message, d),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::with_detail(format!("I/O error: {}", e), e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::with_detail("Could not parse internal JSON message", e.to_string())
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::new(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error::new(s)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
