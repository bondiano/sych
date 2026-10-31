//! Materializes one pinned commit into a scratch directory for a language server.
//!
//! Blobs are written byte-for-byte from the object database: no checkout, filters,
//! `export-subst` or hooks, so every file equals its blob and line hashes match `CodeRef`s.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::Child;
use std::str;
use std::thread;

use sych_core::{GitHashAlgorithm, GitObjectId, RepoPath, is_secret_path};

use super::parse::{TreeRecord, tree_records};
use super::{Git, SnapshotError};

/// What was written and what was deliberately left out.
#[derive(Debug, Default)]
pub struct Materialized {
    pub blobs: BTreeMap<RepoPath, GitObjectId>,
    /// Paths not written: secret, symlink, submodule, non-UTF-8 or too large.
    pub skipped: Vec<String>,
    /// Tool configuration that can name commands to run (`.cargo/config.toml`,
    /// `rust-analyzer.toml`): never written, so host tools cannot execute repository code.
    pub tool_configs: Vec<String>,
    /// Regular blobs not written because the workspace budget was spent.
    pub over_budget: usize,
}

/// Whole-workspace bounds: many paths can share one large blob, so per-blob limits alone
/// do not bound what is written to disk.
const MAX_WORKSPACE_BYTES: u64 = 256 << 20;
const MAX_WORKSPACE_FILES: usize = 50_000;

/// Whether a file configures a host tool in a way that can run a command.
fn runs_commands(path: &str) -> bool {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let parent = dir.rsplit('/').next().unwrap_or(dir);
    // Case-insensitive file systems open `.CARGO/Config.toml` as `.cargo/config.toml`.
    let is = |s: &str, names: &[&str]| names.iter().any(|n| s.eq_ignore_ascii_case(n));
    is(name, &["rust-analyzer.toml", ".rust-analyzer.toml"])
        || (is(parent, &[".cargo"]) && is(name, &["config", "config.toml"]))
}

struct Entry {
    path: RepoPath,
    oid: String,
}

fn io_failed(err: &io::Error) -> SnapshotError {
    SnapshotError::Git(format!("materialize: {err}"))
}

/// Parses `ls-tree -r -z -l --full-tree`: `<mode> <type> <oid> <size>\t<path>\0`.
fn tree_entries(out: &[u8], max_blob_bytes: u64, done: &mut Materialized) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    for record in tree_records(out) {
        let Some(TreeRecord {
            mode,
            kind,
            oid,
            size,
            path,
        }) = record
        else {
            continue;
        };
        let Ok(path) = str::from_utf8(path) else {
            done.skipped
                .push(String::from_utf8_lossy(path).into_owned());
            continue;
        };
        let size: u64 = size.parse().unwrap_or(u64::MAX);
        if runs_commands(path) {
            done.tool_configs.push(path.to_owned());
            continue;
        }

        let regular = kind == "blob" && mode != "120000";
        match path.parse::<RepoPath>() {
            Ok(repo_path)
                if regular && size <= max_blob_bytes && !is_secret_path(repo_path.as_str()) =>
            {
                if entries.len() == MAX_WORKSPACE_FILES || total + size > MAX_WORKSPACE_BYTES {
                    done.over_budget += 1;
                    done.skipped.push(path.to_owned());
                    continue;
                }

                total += size;
                entries.push(Entry {
                    path: repo_path,
                    oid: oid.to_owned(),
                });
            }
            _ => done.skipped.push(path.to_owned()),
        }
    }
    entries
}

impl Git {
    /// Writes every regular, non-secret blob of `commit` under `dest`.
    pub fn materialize(
        &self,
        algorithm: GitHashAlgorithm,
        commit: &GitObjectId,
        dest: &Path,
        max_blob_bytes: u64,
    ) -> Result<Materialized, SnapshotError> {
        let listing = self.stdout(["ls-tree", "-r", "-z", "-l", "--full-tree", commit.hex()])?;
        let mut done = Materialized::default();
        let entries = tree_entries(&listing, max_blob_bytes, &mut done);
        let request: Vec<u8> = entries
            .iter()
            .flat_map(|e| format!("{}\n", e.oid).into_bytes())
            .collect();
        let mut child = self
            .spawn_piped(["cat-file", "--batch"])
            .map_err(|e| io_failed(&e))?;
        // Stdin is fed from a thread to avoid a pipe deadlock.
        let mut stdin = child.stdin.take().expect("piped stdin");
        let writer = thread::spawn(move || stdin.write_all(&request));
        let written = write_blobs(&mut child, algorithm, entries, dest, &mut done);
        if written.is_err() {
            let _ = child.kill();
        }

        let out = child.wait_with_output().map_err(|e| io_failed(&e))?;
        let fed = writer
            .join()
            .map_err(|_| SnapshotError::Git("cat-file writer panicked".into()))?;
        // A killed reader breaks the pipe; that write error says nothing new.
        written?;
        fed.map_err(|e| io_failed(&e))?;
        if !out.status.success() {
            return Err(SnapshotError::Git(
                String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            ));
        }

        tracing::debug!(commit = %commit, written = done.blobs.len(), skipped = done.skipped.len(), over_budget = done.over_budget, tool_configs = done.tool_configs.len(), "side materialized");
        Ok(done)
    }
}

/// Streams `cat-file --batch` records straight to files, one blob in flight at a time.
fn write_blobs(
    child: &mut Child,
    algorithm: GitHashAlgorithm,
    entries: Vec<Entry>,
    dest: &Path,
    done: &mut Materialized,
) -> Result<(), SnapshotError> {
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut header = Vec::new();
    for entry in entries {
        header.clear();
        reader
            .read_until(b'\n', &mut header)
            .map_err(|e| io_failed(&e))?;
        let line = str::from_utf8(&header).unwrap_or_default().trim_end();
        let size: u64 = line
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse().ok())
            .filter(|_| line.starts_with(&entry.oid))
            .ok_or_else(|| SnapshotError::Git(format!("cat-file: {line}")))?;
        let target = dest.join(entry.path.as_str());
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| io_failed(&e))?;
        }

        let mut file = File::create(&target).map_err(|e| io_failed(&e))?;
        let copied =
            io::copy(&mut (&mut reader).take(size), &mut file).map_err(|e| io_failed(&e))?;
        let mut newline = [0];
        if copied != size || reader.read_exact(&mut newline).is_err() {
            return Err(SnapshotError::Git("truncated cat-file output".into()));
        }

        let oid = GitObjectId::new(algorithm, &entry.oid)
            .map_err(|e| SnapshotError::Git(e.to_string()))?;
        done.blobs.insert(entry.path, oid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_skips_secrets_links_submodules_and_large_blobs() {
        let oid = "a".repeat(40);
        let out = format!(
            "100644 blob {oid}       10\tsrc/lib.rs\0\
             100644 blob {oid}       10\t.env\0\
             120000 blob {oid}       10\tlink\0\
             160000 commit {oid}       -\tvendor/sub\0\
             100644 blob {oid}   999999\tbig.bin\0\
             100644 blob {oid}       10\t.cargo/config.toml\0\
             100644 blob {oid}       10\t.CARGO/Config.TOML\0\
             100644 blob {oid}       10\tcrates/x/rust-analyzer.toml\0\
             100644 blob {oid}       10\tRUST-ANALYZER.TOML\0\
             100644 blob {oid}       10\tdocs/.cargo-notes/config.toml\0"
        );
        let mut done = Materialized::default();
        let entries = tree_entries(out.as_bytes(), 1000, &mut done);
        let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["src/lib.rs", "docs/.cargo-notes/config.toml"]);
        assert_eq!(done.skipped, [".env", "link", "vendor/sub", "big.bin"]);
        assert_eq!(
            done.tool_configs,
            [
                ".cargo/config.toml",
                ".CARGO/Config.TOML",
                "crates/x/rust-analyzer.toml",
                "RUST-ANALYZER.TOML"
            ]
        );
    }

    #[test]
    fn many_paths_to_one_blob_stop_at_the_workspace_budget() {
        let oid = "a".repeat(40);
        let size: u64 = 4 << 20;
        let out = format!("100644 blob {oid} {size}\tf.bin\0").repeat(100);
        let mut done = Materialized::default();
        let entries = tree_entries(out.as_bytes(), size, &mut done);
        let fit = usize::try_from(MAX_WORKSPACE_BYTES / size).expect("fits");
        assert_eq!(entries.len(), fit);
        assert_eq!(done.over_budget, 100 - fit);
        assert_eq!(done.skipped.len(), 100 - fit);
    }
}
