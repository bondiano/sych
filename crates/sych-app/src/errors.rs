//! Exit codes (spec §24.1) and how an error chain maps to one.
//!
//! Commands return `anyhow::Result` and add context as they go; libraries keep their typed
//! errors. A [`Failure`] states the exit code explicitly; otherwise the first typed cause in
//! the chain decides, and anything else is "no result".

use std::fmt::Display;

use sych_adapters::git::SnapshotError;
use sych_core::config::ConfigError;
use sych_storage::StorageError;
use thiserror::Error;

pub const EXIT_INVALID: u8 = 2;
pub const EXIT_NO_RESULT: u8 = 3;
pub const EXIT_PERMISSION: u8 = 4;
pub const EXIT_VIOLATION: u8 = 10;
pub const EXIT_INCOMPLETE: u8 = 11;
pub const EXIT_CANCELLED: u8 = 130;
/// Some eval or validation cases did not meet their expectations.
pub const EXIT_EVAL_FAILED: u8 = 1;

/// An error with a decided exit code; its message may hold several lines.
#[derive(Debug, Error)]
pub enum Failure {
    /// Bad flags, refs, paths or configuration (exit 2).
    #[error("{0}")]
    Invalid(String),
    /// The command could not produce its result (exit 3).
    #[error("{0}")]
    NoResult(String),
    /// Missing credentials or access (exit 4).
    #[error("{0}")]
    Permission(String),
    /// Stopped by the user (exit 130).
    #[error("{0}")]
    Cancelled(String),
}

impl Failure {
    pub fn invalid(message: impl Display) -> Self {
        Self::Invalid(message.to_string())
    }

    pub fn no_result(message: impl Display) -> Self {
        Self::NoResult(message.to_string())
    }

    pub fn permission(message: impl Display) -> Self {
        Self::Permission(message.to_string())
    }

    /// Every configuration diagnostic on its own line.
    pub fn config(errors: &[ConfigError]) -> Self {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        Self::Invalid(lines.join("\n"))
    }

    pub fn code(&self) -> u8 {
        match self {
            Self::Invalid(_) => EXIT_INVALID,
            Self::NoResult(_) => EXIT_NO_RESULT,
            Self::Permission(_) => EXIT_PERMISSION,
            Self::Cancelled(_) => EXIT_CANCELLED,
        }
    }
}

/// The exit code of an error chain: the outermost decided cause wins.
pub fn exit_code(err: &anyhow::Error) -> u8 {
    for cause in err.chain() {
        if let Some(failure) = cause.downcast_ref::<Failure>() {
            return failure.code();
        }

        if let Some(SnapshotError::InvalidInput(_)) = cause.downcast_ref::<SnapshotError>() {
            return EXIT_INVALID;
        }

        if let Some(StorageError::UnknownRun(_)) = cause.downcast_ref::<StorageError>() {
            return EXIT_INVALID;
        }
    }

    EXIT_NO_RESULT
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;

    #[test]
    fn context_keeps_the_decided_exit_code() {
        let err = Err::<(), _>(Failure::permission("OPENROUTER_KEY is not set"))
            .context("cannot resolve profile cheap_api")
            .expect_err("error");
        assert_eq!(exit_code(&err), EXIT_PERMISSION);
        assert_eq!(
            format!("{err:#}"),
            "cannot resolve profile cheap_api: OPENROUTER_KEY is not set"
        );
        let snapshot = anyhow::Error::from(SnapshotError::InvalidInput("bad ref".into()))
            .context("cannot pin");
        assert_eq!(exit_code(&snapshot), EXIT_INVALID);
        assert_eq!(exit_code(&anyhow::anyhow!("disk full")), EXIT_NO_RESULT);
    }
}
