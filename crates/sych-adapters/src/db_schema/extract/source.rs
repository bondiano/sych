//! Uniform read-only source file tree over either a pinned Git [`LocalSnapshot`]
//! side or a local filesystem directory (never follows symlinks, never reads secrets).

use std::fs;
use std::path::{Path, PathBuf};

use sych_core::{RepoPath, SourceSide, glob_match, is_secret_path};

use crate::git::LocalSnapshot;

const MAX_FILE_BYTES: u64 = 512 << 10;
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "vendor",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".sych",
];

/// Read-only view of text files in a repository tree.
pub enum SourceTree<'a> {
    Snapshot {
        snapshot: &'a LocalSnapshot,
        side: SourceSide,
    },
    Directory(PathBuf),
}

impl<'a> SourceTree<'a> {
    /// Creates a tree backed by one side of a pinned Git snapshot.
    pub const fn from_snapshot(snapshot: &'a LocalSnapshot, side: SourceSide) -> Self {
        Self::Snapshot { snapshot, side }
    }

    /// Creates a tree backed by a local filesystem directory.
    pub fn from_dir(dir: &Path) -> Self {
        Self::Directory(dir.to_path_buf())
    }

    /// Lists all regular, non-secret repository-relative paths in deterministic sorted order.
    pub fn list_files(&self) -> Vec<String> {
        match self {
            Self::Snapshot { snapshot, side } => snapshot
                .list_paths(*side)
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.to_string())
                .collect(),
            Self::Directory(root) => {
                let mut out = Vec::new();
                walk_dir(root, root, &mut out);
                out.sort();
                out
            }
        }
    }

    /// Lists files filtered by extension (lowercase, without leading dot) and optional globs.
    pub fn files_with_extensions(&self, exts: &[&str], globs: &[String]) -> Vec<String> {
        self.list_files()
            .into_iter()
            .filter(|path| {
                let ext_ok = exts.is_empty()
                    || path
                        .rsplit_once('.')
                        .is_some_and(|(_, ext)| exts.iter().any(|e| ext.eq_ignore_ascii_case(e)));
                let glob_ok = globs.is_empty() || globs.iter().any(|g| glob_match(g, path));
                ext_ok && glob_ok
            })
            .collect()
    }

    /// Reads UTF-8 text of `rel_path`, returning `None` for unreadable, binary or secret files.
    pub fn read_text(&self, rel_path: &str) -> Option<String> {
        if is_secret_path(rel_path) {
            return None;
        }

        match self {
            Self::Snapshot { snapshot, side } => {
                let rp = rel_path.parse::<RepoPath>().ok()?;
                snapshot
                    .blob(*side, &rp)
                    .ok()
                    .map(|(_, text)| text.to_string())
            }
            Self::Directory(root) => {
                let full = root.join(rel_path);
                let meta = fs::symlink_metadata(&full).ok()?;
                if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
                    return None;
                }

                fs::read_to_string(&full).ok()
            }
        }
    }
}

fn walk_dir(root: &Path, current: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }

        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if meta.is_dir() {
            if SKIP_DIRS.contains(&name_str.as_ref()) {
                continue;
            }

            walk_dir(root, &path, out);
        } else if meta.is_file() && meta.len() <= MAX_FILE_BYTES {
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel_str = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if !rel_str.is_empty() && !is_secret_path(&rel_str) {
                out.push(rel_str);
            }
        }
    }
}
