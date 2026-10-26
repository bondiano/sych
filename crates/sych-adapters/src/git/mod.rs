//! Local Git snapshot service (spec §6).

mod command;
mod fetch;
mod parse;
mod policy;
mod reader;
mod snapshot;
mod workspace;

use thiserror::Error;

pub use command::Git;
pub use fetch::fetch_commits;
pub use policy::{MAX_POLICY_FILE_BYTES, read_policy_files};
pub use snapshot::{LocalSnapshot, SnapshotRequest};
pub use workspace::Materialized;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SnapshotError {
    /// Bad repository path, ref or flag combination (CLI exit 2).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Substantive review is impossible for a declared reason (coverage `UNAVAILABLE`).
    #[error("snapshot unavailable ({reason_code}): {detail}")]
    Unavailable {
        reason_code: &'static str,
        detail: String,
    },
    #[error("git failed: {0}")]
    Git(String),
}
