//! Typed server and repository configuration, trusted policy and effective config
//! (spec §11, §23). YAML enums are lowercase `snake_case`; unknown keys are errors.

mod effective;
mod endpoint;
mod execution;
mod http;
mod models;
mod redaction;
mod repo;
mod rules;
mod scm;
mod server;
mod server_models;
mod tooling;

use std::error::Error;
use std::fmt;

use serde::de::DeserializeOwned;

use crate::redact;

pub use effective::*;
pub use execution::*;
pub use http::*;
pub use models::*;
pub use redaction::*;
pub use repo::*;
pub use rules::*;
pub use scm::*;
pub use server::*;
pub use tooling::*;

/// One configuration problem at an exact location. The message is already redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// File the problem is in, e.g. `sych.yaml` or `.sych/rules.yaml`.
    pub file: String,
    /// Dotted key path inside the file; `.` for the document root.
    pub path: String,
    pub message: String,
}

impl ConfigError {
    pub fn new(file: impl Into<String>, path: impl Into<String>, message: &str) -> Self {
        Self {
            file: file.into(),
            path: path.into(),
            message: redact(message).into_owned(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.file, self.path, self.message)
    }
}

impl Error for ConfigError {}

/// Parses one of our YAML documents strictly, keeping the key path of the first error.
pub fn parse_yaml<T: DeserializeOwned>(file: &str, text: &str) -> Result<T, ConfigError> {
    serde_path_to_error::deserialize(serde_yaml_ng::Deserializer::from_str(text)).map_err(|e| {
        let path = e.path().to_string();
        let message = e.into_inner().to_string();
        // The YAML error repeats the parent key path as a prefix; keep only the full one.
        let message = message
            .split_once(": ")
            .filter(|(prefix, _)| path.starts_with(prefix))
            .map_or(message.as_str(), |(_, rest)| rest);
        ConfigError::new(file, path, message)
    })
}

/// Accumulates validation problems for one file.
pub(crate) struct Problems<'a> {
    pub file: &'a str,
    pub errors: Vec<ConfigError>,
}

impl<'a> Problems<'a> {
    pub fn new(file: &'a str) -> Self {
        Self {
            file,
            errors: Vec::new(),
        }
    }

    pub fn push(&mut self, path: impl Into<String>, message: &str) {
        self.errors.push(ConfigError::new(self.file, path, message));
    }

    pub fn check(&mut self, ok: bool, path: impl Into<String>, message: &str) {
        if !ok {
            self.push(path, message);
        }
    }
}

/// Environment variable names hold references to secrets, never values.
pub(crate) fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_names_are_upper_snake_case() {
        assert!(is_env_name("SYCH_OPENROUTER_API_KEY"));
        assert!(!is_env_name("sk-or-v1-0123456789abcdef"));
        assert!(!is_env_name(""));
        assert!(!is_env_name("1ABC"));
    }
}
