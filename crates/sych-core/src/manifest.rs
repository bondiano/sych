//! Diff manifest, resolved source spans and the snapshot reader port.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{CodeRef, GitObjectId, RepoPath, SchemaVersion, SourceSide, SourceSnapshotId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChangeStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    /// File type changed, e.g. regular file to symlink.
    TypeChanged,
    /// Git reported a status this build does not interpret; kept, never dropped.
    Unknown,
}

/// What a tree entry holds. Symlinks, submodules and LFS pointers are never followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContentKind {
    Text,
    /// Text bytes that are not valid UTF-8.
    NonUtf8Text,
    Binary,
    LfsPointer,
    Symlink,
    Submodule,
    /// Content was not read; the file stays in the manifest with a limitation.
    NotRead,
}

/// One side (old or new) of a changed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileSide {
    pub path: RepoPath,
    /// Octal Git mode, e.g. `100644`, `120000`, `160000`.
    pub mode: String,
    pub oid: GitObjectId,
    pub content: ContentKind,
    /// Number of lines when the content is text.
    pub line_count: Option<u32>,
}

/// Changed line range; `*_start` is 1-based, a zero count means an insertion point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileChange {
    pub status: ChangeStatus,
    /// `DIFF_BASE` side; absent for additions.
    pub old: Option<FileSide>,
    /// `HEAD` side; absent for deletions.
    pub new: Option<FileSide>,
    /// Rename/copy similarity in percent.
    pub similarity: Option<u8>,
    /// Header marker such as `@generated` or `DO NOT EDIT`.
    pub generated: bool,
    pub hunks: Vec<Hunk>,
    /// Explicit codes such as `PATH_NOT_UTF8`, `BLOB_TOO_LARGE`; the file is never dropped.
    pub limitations: Vec<String>,
}

impl FileChange {
    pub fn side(&self, side: SourceSide) -> Option<&FileSide> {
        match side {
            SourceSide::DiffBase => self.old.as_ref(),
            SourceSide::Head => self.new.as_ref(),
            SourceSide::TargetTip => None,
        }
    }

    /// Changed 1-based inclusive line ranges on `side`. A zero-count hunk is an insertion
    /// point; it is reported as the line it follows, so the enclosing construct counts.
    pub fn changed_lines(&self, side: SourceSide) -> Vec<(u32, u32)> {
        self.hunks
            .iter()
            .map(|h| h.on(side))
            .map(|(start, count)| (start.max(1), (start + count.max(1) - 1).max(1)))
            .collect()
    }
}

impl Hunk {
    /// Start line and line count on `side`; the target tip has no hunks of its own.
    pub fn on(&self, side: SourceSide) -> (u32, u32) {
        match side {
            SourceSide::DiffBase => (self.old_start, self.old_lines),
            _ => (self.new_start, self.new_lines),
        }
    }
}

/// Root trees of the three pinned commits; together they identify all snapshot content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotTrees {
    pub target_tip: GitObjectId,
    pub diff_base: GitObjectId,
    pub head: GitObjectId,
}

/// Every file changed between `diff_base` and `head`, including binary, generated,
/// symlink, submodule and LFS entries. Files are sorted by path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffManifest {
    pub schema_version: SchemaVersion,
    /// How the diff was produced; part of snapshot identity.
    pub method: String,
    pub diff_base: GitObjectId,
    pub head: GitObjectId,
    pub files: Vec<FileChange>,
    /// Parts of the diff that could not be obtained; non-empty means the diff is partial.
    #[serde(default)]
    pub omissions: Vec<String>,
}

/// Exact lines of a pinned blob, verified against the [`CodeRef`] hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpan {
    pub reference: CodeRef,
    pub text: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReadError {
    #[error("snapshot {0} is not available to this reader")]
    UnknownSnapshot(String),
    #[error("{path} with blob {oid} is not part of the snapshot on side {side:?}")]
    NotInSnapshot {
        path: String,
        oid: String,
        side: SourceSide,
    },
    #[error("{path} is {kind:?}, not readable text")]
    NotText { path: String, kind: ContentKind },
    #[error("lines {start}..={end} are outside 1..={line_count}")]
    LineRange {
        start: u32,
        end: u32,
        line_count: u32,
    },
    #[error("span content hash mismatch for {0}")]
    HashMismatch(String),
    #[error("source object unavailable: {0}")]
    Unavailable(String),
}

/// Reads pinned snapshot content; implementations never read outside Git objects.
pub trait SnapshotReader {
    fn read_span(&self, reference: &CodeRef) -> Result<SourceSpan, ReadError>;
    /// Exact reference to lines of any text file on `side`, changed or not.
    fn code_ref(
        &self,
        side: SourceSide,
        path: &RepoPath,
        start_line: u32,
        end_line: u32,
    ) -> Result<CodeRef, ReadError>;
    /// Fixed-string search on `side`, optionally under `paths`; at most `limit` hits, one
    /// line each.
    fn search(
        &self,
        side: SourceSide,
        literal: &str,
        paths: &[RepoPath],
        limit: usize,
    ) -> Result<Vec<CodeRef>, ReadError>;
    fn diff(&self, snapshot: &SourceSnapshotId) -> Result<DiffManifest, ReadError>;
}

/// Inclusive 1-based line range of `text` with line terminators, or `None` when out of range.
pub fn line_range(text: &str, start: u32, end: u32) -> Option<&str> {
    if start == 0 || end < start {
        return None;
    }

    let skip = usize::try_from(start - 1).ok()?;
    let take = usize::try_from(end - start + 1).ok()?;
    let mut lines = text.split_inclusive('\n');
    let begin: usize = lines.by_ref().take(skip).map(str::len).sum();
    let (len, n) = lines
        .take(take)
        .fold((0, 0), |(len, n), l| (len + l.len(), n + 1));
    (n == take).then(|| &text[begin..begin + len])
}

/// Number of lines, counting a final line without terminator.
#[expect(clippy::naive_bytecount, reason = "not worth the bytecount dependency")]
pub fn count_lines(text: &[u8]) -> u32 {
    let newlines = text.iter().filter(|&&b| b == b'\n').count();
    let tail = usize::from(!text.is_empty() && !text.ends_with(b"\n"));
    u32::try_from(newlines + tail).unwrap_or(u32::MAX)
}

/// UTF-16 code unit column of a UTF-8 byte offset within one line (LSP `utf-16` encoding).
/// Returns `None` when the offset is not on a character boundary.
pub fn utf16_column(line: &str, byte_offset: usize) -> Option<u32> {
    let prefix = line.get(..byte_offset)?;
    u32::try_from(prefix.encode_utf16().count()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_range_is_inclusive_and_bounded() {
        let text = "one\nдва\nthree";
        assert_eq!(line_range(text, 1, 1), Some("one\n"));
        assert_eq!(line_range(text, 2, 3), Some("два\nthree"));
        assert_eq!(line_range(text, 3, 4), None);
        assert_eq!(line_range(text, 0, 1), None);
        assert_eq!(line_range(text, 4, 4), None);
        assert_eq!(line_range("a\n", 2, 2), None);
    }

    #[test]
    fn counts_lines_with_and_without_trailing_newline() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a"), 1);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"a\nb"), 2);
    }

    #[test]
    fn utf16_columns_differ_from_utf8_bytes() {
        let line = "я😀x";
        assert_eq!(utf16_column(line, 2), Some(1));
        assert_eq!(utf16_column(line, 6), Some(3));
        assert_eq!(utf16_column(line, 1), None);
    }
}
