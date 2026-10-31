//! Reading pinned content of any file of a side, and literal search.

use std::collections::HashMap;
use std::str;
use std::sync::Arc;
use std::time::Duration;

use sych_core::{
    CodeRef, ContentKind, DiffManifest, GitObjectId, ReadError, RepoPath, SnapshotReader,
    SourceSide, SourceSnapshotId, SourceSpan, is_secret_path, sha256,
};

use super::SnapshotError;
use super::parse::{TreeRecord, tree_records};
use super::snapshot::{LocalSnapshot, span};

/// Largest blob read as text.
const MAX_TEXT_BLOB_BYTES: u64 = 512 << 10;
/// Wall-clock bound of one model-requested literal search.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Texts kept per run; past this many bytes new blobs are read but not kept.
const MAX_CACHED_BYTES: usize = 64 << 20;

#[derive(Default)]
pub struct BlobCache {
    texts: HashMap<(SourceSide, RepoPath), (GitObjectId, Arc<str>)>,
    bytes: usize,
}

impl LocalSnapshot {
    /// Whether `path` is a directory on `side`, e.g. a Python namespace package.
    pub(crate) fn is_dir(&self, side: SourceSide, path: &RepoPath) -> bool {
        self.git
            .stdout([
                "ls-tree",
                "-z",
                "-l",
                self.commit(side).hex(),
                "--",
                path.as_str(),
            ])
            .is_ok_and(|listing| {
                tree_records(&listing)
                    .next()
                    .flatten()
                    .is_some_and(|r| r.kind == "tree")
            })
    }

    /// Blob id and text of `path` on `side`, read once per run: see [`Self::read_blob`].
    pub fn blob(
        &self,
        side: SourceSide,
        path: &RepoPath,
    ) -> Result<(GitObjectId, Arc<str>), ReadError> {
        let key = (side, path.clone());
        if let Some(hit) = self.blobs.lock().expect("blob cache").texts.get(&key) {
            return Ok(hit.clone());
        }

        let (oid, text) = self.read_blob(side, path)?;
        let text: Arc<str> = text.into();
        let mut cache = self.blobs.lock().expect("blob cache");
        if cache.bytes + text.len() <= MAX_CACHED_BYTES {
            cache.bytes += text.len();
            cache.texts.insert(key, (oid.clone(), Arc::clone(&text)));
        }

        Ok((oid, text))
    }

    /// Blob id and text of `path` on `side`: from the manifest for changed files, otherwise
    /// from the pinned tree. Unchanged secret paths, links, submodules and large blobs are
    /// not read.
    fn read_blob(
        &self,
        side: SourceSide,
        path: &RepoPath,
    ) -> Result<(GitObjectId, String), ReadError> {
        if let Some(file) = self.file_side(side, path) {
            return Ok((file.oid.clone(), self.text(file)?));
        }

        let missing = || ReadError::NotInSnapshot {
            path: path.to_string(),
            oid: String::new(),
            side,
        };
        if is_secret_path(path.as_str()) {
            return Err(ReadError::Unavailable(format!("{path} is a secret path")));
        }

        let unavailable = |e: SnapshotError| ReadError::Unavailable(e.to_string());
        let listing = self
            .git
            .stdout([
                "ls-tree",
                "-z",
                "-l",
                self.commit(side).hex(),
                "--",
                path.as_str(),
            ])
            .map_err(unavailable)?;
        let TreeRecord {
            mode,
            kind,
            oid,
            size: bytes,
            ..
        } = tree_records(&listing)
            .next()
            .flatten()
            .ok_or_else(missing)?;
        let content = match (mode, kind) {
            ("120000", _) => Some(ContentKind::Symlink),
            (_, "commit") => Some(ContentKind::Submodule),
            (_, "blob") => None,
            _ => return Err(missing()),
        };
        if let Some(kind) = content {
            return Err(ReadError::NotText {
                path: path.to_string(),
                kind,
            });
        }

        if bytes
            .parse::<u64>()
            .map_or(true, |n| n > MAX_TEXT_BLOB_BYTES)
        {
            return Err(ReadError::Unavailable(format!(
                "{path} is larger than the text limit"
            )));
        }

        let oid = GitObjectId::new(self.snapshot.head.algorithm(), oid)
            .map_err(|e| ReadError::Unavailable(e.to_string()))?;
        let bytes = self
            .git
            .stdout(["cat-file", "blob", oid.hex()])
            .map_err(unavailable)?;
        let text = String::from_utf8(bytes).map_err(|_| ReadError::NotText {
            path: path.to_string(),
            kind: ContentKind::NonUtf8Text,
        })?;
        Ok((oid, text))
    }
}

impl SnapshotReader for LocalSnapshot {
    fn read_span(&self, reference: &CodeRef) -> Result<SourceSpan, ReadError> {
        if reference.snapshot_id != self.snapshot.id {
            return Err(ReadError::UnknownSnapshot(
                reference.snapshot_id.0.to_string(),
            ));
        }

        let not_here = || ReadError::NotInSnapshot {
            path: reference.path.to_string(),
            oid: reference.blob_oid.to_string(),
            side: reference.side,
        };
        let (oid, text) = match self.blob(reference.side, &reference.path) {
            Err(ReadError::NotInSnapshot { .. }) => return Err(not_here()),
            other => other?,
        };
        if oid != reference.blob_oid {
            return Err(not_here());
        }

        let span = span(&text, reference.start_line, reference.end_line)?;
        if sha256(span.as_bytes()) != reference.content_sha256 {
            return Err(ReadError::HashMismatch(reference.path.to_string()));
        }

        Ok(SourceSpan {
            reference: reference.clone(),
            text: span.to_owned(),
        })
    }

    fn code_ref(
        &self,
        side: SourceSide,
        path: &RepoPath,
        start_line: u32,
        end_line: u32,
    ) -> Result<CodeRef, ReadError> {
        Self::code_ref(self, side, path, start_line, end_line)
    }

    /// `git grep -F` over the pinned tree: no regex, so the cost stays linear in the tree.
    fn search(
        &self,
        side: SourceSide,
        literal: &str,
        paths: &[RepoPath],
        limit: usize,
    ) -> Result<Vec<CodeRef>, ReadError> {
        let commit = self.commit(side).hex().to_owned();
        let mut args = vec![
            "grep",
            "-F",
            "-I",
            "-n",
            "-z",
            "--no-color",
            "-e",
            literal,
            &commit,
            "--",
        ];
        args.extend(paths.iter().map(RepoPath::as_str));
        let out = self
            .git
            .output_within(&args, SEARCH_TIMEOUT)
            .map_err(|e| ReadError::Unavailable(e.to_string()))?
            .ok_or_else(|| {
                ReadError::Unavailable(format!(
                    "search timed out after {} s",
                    SEARCH_TIMEOUT.as_secs()
                ))
            })?;
        // Exit code 1 is "no match".
        if !out.status.success() && out.status.code() != Some(1) {
            return Err(ReadError::Unavailable(
                String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            ));
        }

        let mut hits = Vec::new();
        for line in out.stdout.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
            let mut fields = line.splitn(3, |&b| b == 0);
            let (Some(name), Some(number)) = (fields.next(), fields.next()) else {
                continue;
            };
            let name = str::from_utf8(name).unwrap_or_default();
            let name = name.strip_prefix(&format!("{commit}:")).unwrap_or(name);
            let (Ok(path), Some(n)) = (
                name.parse::<RepoPath>(),
                str::from_utf8(number).ok().and_then(|n| n.parse().ok()),
            ) else {
                continue;
            };
            if is_secret_path(path.as_str()) {
                continue;
            }

            hits.push(self.code_ref(side, &path, n, n)?);
            if hits.len() == limit {
                break;
            }
        }
        Ok(hits)
    }

    fn diff(&self, snapshot: &SourceSnapshotId) -> Result<DiffManifest, ReadError> {
        if snapshot == &self.snapshot.id {
            Ok(self.manifest.clone())
        } else {
            Err(ReadError::UnknownSnapshot(snapshot.0.to_string()))
        }
    }
}
