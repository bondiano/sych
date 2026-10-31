//! Trusted additions to secret redaction; built-in rules always stay in force.

use serde::{Deserialize, Serialize};

use crate::Redactor;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Redaction {
    /// A token starting with one and followed by 16+ token characters is redacted.
    pub token_prefixes: Vec<String>,
    /// A value assigned to a key containing one (case-insensitive) is redacted.
    pub secret_keys: Vec<String>,
    /// Regular expressions whose every match is redacted.
    pub patterns: Vec<String>,
}

impl Redaction {
    /// The built-in rules plus these additions; the error names the bad entry.
    pub fn redactor(&self) -> Result<Redactor, String> {
        Redactor::new(&self.token_prefixes, &self.secret_keys, &self.patterns)
    }
}
