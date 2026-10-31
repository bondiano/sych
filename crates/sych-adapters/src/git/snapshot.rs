//! Pins target tip, diff base and head and builds the full diff manifest.

use std::path::Path;
use std::str;
use std::sync::Mutex;

use sych_core::config::PolicyFiles;
use sych_core::{
    ChangeStatus, CodeRef, ComparisonMode, ContentKind, DiffManifest, FileChange, FileSide,
    GitHashAlgorithm, GitObjectId, Hunk, ReadError, RepoPath, SchemaVersion, SnapshotTrees,
    SourceSide, SourceSnapshot, SourceSnapshotId, Timestamp, canonical_sha256, count_lines,
    is_secret_path, line_range, sha256,
};

use super::attributes::mark_generated;
use super::parse::{self, RawEntry};
use super::policy::read_policy_files;
use super::reader::BlobCache;
use super::{Git, Materialized, SnapshotError};

/// Recorded in the manifest and therefore in snapshot identity.
const DIFF_METHOD: &str = "git diff-tree -r -M; blob diff -U0 --diff-algorithm=myers";
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1\n";
/// Same window Git uses to detect binary content.
const BINARY_PROBE: usize = 8000;
const GENERATED_PROBE: usize = 1024;
const GENERATED_MARKERS: &[&[u8]] = &[b"@generated", b"DO NOT EDIT", b"Code generated"];

pub struct SnapshotRequest<'a> {
    pub repo: &'a Path,
    /// Target ref; its tip is pinned and merge-base is computed from it.
    pub base: &'a str,
    pub head: &'a str,
    /// Explicit two-dot comparison base instead of merge-base.
    pub diff_base: Option<&'a str>,
    /// Larger blobs stay in the manifest as `NOT_READ` with a limitation.
    pub max_blob_bytes: u64,
}

/// A pinned snapshot that reads its content only from Git objects.
pub struct LocalSnapshot {
    pub(super) git: Git,
    pub snapshot: SourceSnapshot,
    pub manifest: DiffManifest,
    /// Texts already read in this run; the broker, verifier and syntax facts reread blobs.
    pub(super) blobs: Mutex<BlobCache>,
}

impl LocalSnapshot {
    pub fn capture(request: &SnapshotRequest<'_>) -> Result<Self, SnapshotError> {
        let git = Git::open(request.repo)?;
        let algorithm = object_format(&git)?;
        let target_tip = resolve(&git, algorithm, request.base, "--base")?;
        let head = resolve(&git, algorithm, request.head, "--head")?;
        let (diff_base, comparison) = match request.diff_base {
            Some(rev) => (
                resolve(&git, algorithm, rev, "--diff-base")?,
                ComparisonMode::ExplicitDiffBase,
            ),
            None => (
                merge_base(&git, algorithm, &target_tip, &head)?,
                ComparisonMode::MergeBase,
            ),
        };
        let trees = SnapshotTrees {
            target_tip: tree_of(&git, algorithm, &target_tip)?,
            diff_base: tree_of(&git, algorithm, &diff_base)?,
            head: tree_of(&git, algorithm, &head)?,
        };
        let raw = git.stdout([
            "diff-tree",
            "-r",
            "-z",
            "--raw",
            "--no-abbrev",
            "-M",
            "--no-ext-diff",
            "--no-textconv",
            "--no-commit-id",
            diff_base.hex(),
            head.hex(),
        ])?;
        let entries = parse::raw_diff(&raw)
            .ok_or_else(|| SnapshotError::Git("unparseable diff-tree output".into()))?;
        let mut files = entries
            .iter()
            .map(|entry| file_change(&git, algorithm, entry, request.max_blob_bytes))
            .collect::<Result<Vec<_>, _>>()?;
        files.sort_by(|a, b| sort_key(a).cmp(sort_key(b)));
        mark_generated(&git, &trees.target_tip, &mut files)?;

        let manifest = DiffManifest {
            schema_version: SchemaVersion,
            method: DIFF_METHOD.to_owned(),
            diff_base: diff_base.clone(),
            head: head.clone(),
            files,
            omissions: Vec::new(),
        };
        let hash_failed = |e: serde_json::Error| SnapshotError::Git(format!("hashing: {e}"));
        let source_manifest_hash = canonical_sha256(&trees).map_err(hash_failed)?;
        let diff_manifest_hash = canonical_sha256(&manifest).map_err(hash_failed)?;
        let repository = format!(
            "local:{}",
            sha256(git.git_dir().as_os_str().as_encoded_bytes())
        );
        let mut snapshot = SourceSnapshot {
            schema_version: SchemaVersion,
            id: SourceSnapshotId(diff_manifest_hash.clone()),
            repository,
            target_ref: Some(request.base.to_owned()),
            target_tip,
            diff_base,
            head,
            comparison,
            source_manifest_hash,
            diff_manifest_hash,
            created_at: Timestamp::now(),
        };
        snapshot.id = snapshot.identity().map_err(hash_failed)?;
        tracing::info!(
            snapshot = %snapshot.id.0,
            target_tip = %snapshot.target_tip,
            diff_base = %snapshot.diff_base,
            head = %snapshot.head,
            files = manifest.files.len(),
            "comparison pinned"
        );
        Ok(Self {
            git,
            snapshot,
            manifest,
            blobs: Mutex::default(),
        })
    }

    /// The `.sych/` tree at `side`; only the target tip is trusted policy.
    pub fn policy_files(&self, side: SourceSide) -> Result<PolicyFiles, SnapshotError> {
        read_policy_files(&self.git, self.commit(side).hex())
    }

    /// Policy files of another revision, for an explicit run with a new trusted policy.
    pub fn policy_files_at(&self, rev: &str) -> Result<(GitObjectId, PolicyFiles), SnapshotError> {
        let algorithm = self.snapshot.target_tip.algorithm();
        let commit = resolve(&self.git, algorithm, rev, "--policy-ref")?;
        let files = read_policy_files(&self.git, commit.hex())?;
        Ok((commit, files))
    }

    /// Writes the tree at `side` under `dest` for a semantic backend.
    pub fn materialize(
        &self,
        side: SourceSide,
        dest: &Path,
        max_blob_bytes: u64,
    ) -> Result<Materialized, SnapshotError> {
        let commit = self.commit(side);
        self.git
            .materialize(commit.algorithm(), commit, dest, max_blob_bytes)
    }

    pub(super) fn commit(&self, side: SourceSide) -> &GitObjectId {
        match side {
            SourceSide::TargetTip => &self.snapshot.target_tip,
            SourceSide::DiffBase => &self.snapshot.diff_base,
            SourceSide::Head => &self.snapshot.head,
        }
    }

    pub(super) fn file_side(&self, side: SourceSide, path: &RepoPath) -> Option<&FileSide> {
        self.manifest
            .files
            .iter()
            .filter_map(|f| f.side(side))
            .find(|s| &s.path == path)
    }

    /// Builds a [`CodeRef`] for lines of any text file on `side`; the harness, not a model,
    /// owns coordinates.
    pub fn code_ref(
        &self,
        side: SourceSide,
        path: &RepoPath,
        start_line: u32,
        end_line: u32,
    ) -> Result<CodeRef, ReadError> {
        let (oid, text) = self.blob(side, path)?;
        let span = span(&text, start_line, end_line)?;
        Ok(CodeRef {
            snapshot_id: self.snapshot.id.clone(),
            side,
            path: path.clone(),
            blob_oid: oid,
            start_line,
            end_line,
            content_sha256: sha256(span.as_bytes()),
        })
    }

    pub(super) fn text(&self, file: &FileSide) -> Result<String, ReadError> {
        if file.content != ContentKind::Text {
            return Err(ReadError::NotText {
                path: file.path.to_string(),
                kind: file.content,
            });
        }

        let bytes = self
            .git
            .stdout(["cat-file", "blob", file.oid.hex()])
            .map_err(|e| ReadError::Unavailable(e.to_string()))?;
        String::from_utf8(bytes).map_err(|_| ReadError::NotText {
            path: file.path.to_string(),
            kind: ContentKind::NonUtf8Text,
        })
    }
}

pub(super) fn span(text: &str, start: u32, end: u32) -> Result<&str, ReadError> {
    line_range(text, start, end).ok_or(ReadError::LineRange {
        start,
        end,
        line_count: count_lines(text.as_bytes()),
    })
}

fn sort_key(file: &FileChange) -> &str {
    file.new
        .as_ref()
        .or(file.old.as_ref())
        .map_or("", |side| side.path.as_str())
}

fn object_format(git: &Git) -> Result<GitHashAlgorithm, SnapshotError> {
    match git
        .stdout(["rev-parse", "--show-object-format"])?
        .as_slice()
    {
        b"sha1\n" => Ok(GitHashAlgorithm::Sha1),
        b"sha256\n" => Ok(GitHashAlgorithm::Sha256),
        other => Err(SnapshotError::Git(format!(
            "unsupported object format {}",
            String::from_utf8_lossy(other).trim()
        ))),
    }
}

fn oid(algorithm: GitHashAlgorithm, out: &[u8]) -> Result<GitObjectId, SnapshotError> {
    let hex = str::from_utf8(out).unwrap_or_default().trim_end();
    GitObjectId::new(algorithm, hex).map_err(|e| SnapshotError::Git(e.to_string()))
}

fn resolve(
    git: &Git,
    algorithm: GitHashAlgorithm,
    rev: &str,
    flag: &str,
) -> Result<GitObjectId, SnapshotError> {
    let out = git.output([
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &format!("{rev}^{{commit}}"),
    ])?;
    if out.status.success() {
        oid(algorithm, &out.stdout)
    } else {
        Err(SnapshotError::InvalidInput(format!(
            "{flag} `{rev}` does not resolve to a commit in this repository"
        )))
    }
}

fn tree_of(
    git: &Git,
    algorithm: GitHashAlgorithm,
    commit: &GitObjectId,
) -> Result<GitObjectId, SnapshotError> {
    oid(
        algorithm,
        &git.stdout([
            "rev-parse",
            "--verify",
            &format!("{}^{{tree}}", commit.hex()),
        ])?,
    )
}

/// Never falls back to an arbitrary commit.
fn merge_base(
    git: &Git,
    algorithm: GitHashAlgorithm,
    target_tip: &GitObjectId,
    head: &GitObjectId,
) -> Result<GitObjectId, SnapshotError> {
    let out = git.output(["merge-base", "--all", target_tip.hex(), head.hex()])?;
    let bases: Vec<&[u8]> = out
        .stdout
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .collect();
    match (out.status.code(), bases.as_slice()) {
        (Some(0), [one]) => oid(algorithm, one),
        (Some(0), _) => Err(SnapshotError::Unavailable {
            reason_code: "AMBIGUOUS_MERGE_BASE",
            detail: format!(
                "{} merge-bases; pass --diff-base for an explicit comparison",
                bases.len()
            ),
        }),
        (Some(1), []) => {
            let shallow = git.stdout(["rev-parse", "--is-shallow-repository"])? == b"true\n";
            Err(SnapshotError::Unavailable {
                reason_code: if shallow {
                    "SHALLOW_HISTORY"
                } else {
                    "NO_MERGE_BASE"
                },
                detail: "target and head share no available history; deepen the clone or pass \
                         --diff-base"
                    .into(),
            })
        }
        _ => Err(SnapshotError::Git(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        )),
    }
}

fn repo_path(raw: &[u8], limitations: &mut Vec<String>) -> Result<RepoPath, SnapshotError> {
    let text = String::from_utf8_lossy(raw);
    if str::from_utf8(raw).is_err() {
        limitations.push("PATH_NOT_UTF8".into());
    }

    text.parse()
        .map_err(|e: sych_core::FormatError| SnapshotError::Git(e.to_string()))
}

fn classify(bytes: &[u8]) -> ContentKind {
    if bytes.starts_with(LFS_POINTER) {
        ContentKind::LfsPointer
    } else if bytes[..bytes.len().min(BINARY_PROBE)].contains(&0) {
        ContentKind::Binary
    } else if str::from_utf8(bytes).is_ok() {
        ContentKind::Text
    } else {
        ContentKind::NonUtf8Text
    }
}

fn is_generated(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(GENERATED_PROBE)];
    GENERATED_MARKERS
        .iter()
        .any(|m| head.windows(m.len()).any(|w| w == *m))
}

struct SideContent {
    side: FileSide,
    generated: bool,
}

fn read_side(
    git: &Git,
    algorithm: GitHashAlgorithm,
    mode: &str,
    oid_hex: &str,
    raw_path: &[u8],
    max_blob_bytes: u64,
    limitations: &mut Vec<String>,
) -> Result<Option<SideContent>, SnapshotError> {
    if mode == "000000" {
        return Ok(None);
    }

    let path = repo_path(raw_path, limitations)?;
    let oid =
        GitObjectId::new(algorithm, oid_hex).map_err(|e| SnapshotError::Git(e.to_string()))?;
    let make_side = |content, line_count| FileSide {
        path: path.clone(),
        mode: mode.to_owned(),
        oid: oid.clone(),
        content,
        line_count,
    };
    let fixed = match mode {
        "160000" => Some(ContentKind::Submodule),
        // The link target is data; it is never resolved on the host.
        "120000" => Some(ContentKind::Symlink),
        _ => None,
    };
    if let Some(kind) = fixed {
        return Ok(Some(SideContent {
            side: make_side(kind, None),
            generated: false,
        }));
    }

    let size: u64 = String::from_utf8_lossy(&git.stdout(["cat-file", "-s", oid_hex])?)
        .trim()
        .parse()
        .map_err(|_| SnapshotError::Git(format!("cannot read size of {oid_hex}")))?;
    if size > max_blob_bytes {
        limitations.push(format!("BLOB_TOO_LARGE: {size} bytes > {max_blob_bytes}"));
        return Ok(Some(SideContent {
            side: make_side(ContentKind::NotRead, None),
            generated: false,
        }));
    }

    let bytes = git.stdout(["cat-file", "blob", oid_hex])?;
    let kind = classify(&bytes);
    if kind == ContentKind::NonUtf8Text {
        limitations.push("CONTENT_NOT_UTF8".into());
    }

    let lines =
        matches!(kind, ContentKind::Text | ContentKind::NonUtf8Text).then(|| count_lines(&bytes));
    Ok(Some(SideContent {
        side: make_side(kind, lines),
        generated: is_generated(&bytes),
    }))
}

fn is_text(side: Option<&SideContent>) -> bool {
    side.is_some_and(|s| matches!(s.side.content, ContentKind::Text | ContentKind::NonUtf8Text))
}

fn file_change(
    git: &Git,
    algorithm: GitHashAlgorithm,
    entry: &RawEntry,
    max_blob_bytes: u64,
) -> Result<FileChange, SnapshotError> {
    let mut limitations = Vec::new();
    let old = read_side(
        git,
        algorithm,
        &entry.old_mode,
        &entry.old_oid,
        &entry.old_path,
        max_blob_bytes,
        &mut limitations,
    )?;
    let new = read_side(
        git,
        algorithm,
        &entry.new_mode,
        &entry.new_oid,
        &entry.new_path,
        max_blob_bytes,
        &mut limitations,
    )?;
    if [&old, &new]
        .into_iter()
        .flatten()
        .any(|s| is_secret_path(s.side.path.as_str()))
    {
        // Kept in the manifest; context and model egress must skip its content.
        limitations.push("SECRET_PATH".into());
    }

    limitations.dedup();
    if entry.status == ChangeStatus::Unknown {
        limitations.push("UNKNOWN_GIT_STATUS".into());
    }

    let line_count = |s: &Option<SideContent>| s.as_ref().and_then(|s| s.side.line_count);
    let hunks = match (is_text(old.as_ref()), is_text(new.as_ref())) {
        (true, true) if entry.old_oid != entry.new_oid => blob_hunks(git, entry)?,
        (false, true) => whole_file(0, line_count(&new).unwrap_or(0), true),
        (true, false) => whole_file(line_count(&old).unwrap_or(0), 0, false),
        _ => Vec::new(),
    };
    Ok(FileChange {
        status: entry.status,
        generated: old.as_ref().is_some_and(|s| s.generated)
            || new.as_ref().is_some_and(|s| s.generated),
        old: old.map(|s| s.side),
        new: new.map(|s| s.side),
        similarity: entry.similarity,
        hunks,
        limitations,
    })
}

fn whole_file(old_lines: u32, new_lines: u32, added: bool) -> Vec<Hunk> {
    if old_lines == 0 && new_lines == 0 {
        return Vec::new();
    }

    vec![Hunk {
        old_start: u32::from(!added),
        old_lines,
        new_start: u32::from(added),
        new_lines,
    }]
}

fn blob_hunks(git: &Git, entry: &RawEntry) -> Result<Vec<Hunk>, SnapshotError> {
    let patch = git.stdout([
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--diff-algorithm=myers",
        "-U0",
        &entry.old_oid,
        &entry.new_oid,
    ])?;
    parse::hunks(&patch).ok_or_else(|| SnapshotError::Git("unparseable hunk header".into()))
}
