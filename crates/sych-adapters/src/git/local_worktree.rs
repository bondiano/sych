//! Worktree overlay for a private index, assembled from raw file bytes.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use sych_core::RepoPath;

use super::SnapshotError;
use super::command::command;
use super::local_head::{line, run};

#[derive(Clone, Copy)]
struct ModeOptions {
    trust_file_mode: bool,
    has_symlinks: bool,
}

pub(super) fn overlay(
    root: &Path,
    indexed: impl Fn() -> Command,
    hash_length: usize,
) -> Result<(), SnapshotError> {
    let paths = changed_paths(&indexed)?;
    if paths.is_empty() {
        return Ok(());
    }

    let modes = ModeOptions {
        trust_file_mode: cfg!(unix) && mode_enabled(&indexed, "core.filemode")?,
        has_symlinks: mode_enabled(&indexed, "core.symlinks")?,
    };
    let entries = paths
        .into_iter()
        .map(|path| overlay_entry(root, path, &indexed, modes))
        .collect::<Result<Vec<_>, _>>()?;
    write_overlay(indexed, &entries, hash_length)
}

fn mode_enabled(indexed: impl Fn() -> Command, key: &str) -> Result<bool, SnapshotError> {
    let value = run(
        indexed().args(["config", "--type=bool", "--default", "true", "--get", key]),
        &format!("read Git {key} configuration"),
    )?;
    Ok(line(&value)? == "true")
}

fn changed_paths(indexed: impl Fn() -> Command) -> Result<BTreeSet<RepoPath>, SnapshotError> {
    // Status may run clean filters to compare entries whose index stats have changed.
    let keys = indexed()
        .args([
            "config",
            "--name-only",
            "--null",
            "--get-regexp",
            r"^filter\..*\.(clean|process|required)$",
        ])
        .output()
        .map_err(|e| unavailable("read filter configuration", &e))?;
    if !(keys.status.success() || keys.status.code() == Some(1)) {
        return Err(SnapshotError::Git(
            "cannot inspect Git filter configuration".into(),
        ));
    }

    let keys = str::from_utf8(&keys.stdout)
        .map_err(|_| SnapshotError::Git("Git filter configuration is not UTF-8".into()))?;
    let keys = keys.split_terminator('\0');
    let mut status = indexed();
    status.env("GIT_CONFIG_COUNT", keys.clone().count().to_string());
    for (index, key) in keys.enumerate() {
        let value = if key.ends_with(".required") {
            "false"
        } else {
            ""
        };
        status
            .env(format!("GIT_CONFIG_KEY_{index}"), key)
            .env(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
    let bytes = run(
        status.args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=dirty",
            "--no-renames",
        ]),
        "find worktree changes",
    )?;
    let mut paths = BTreeSet::new();
    for entry in bytes
        .strip_suffix(b"\0")
        .unwrap_or(&bytes)
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
    {
        let [_, worktree, b' ', path @ ..] = entry else {
            return Err(SnapshotError::Git("unexpected Git status entry".into()));
        };
        if *worktree == b' ' {
            continue;
        }

        // Git reports an untracked nested repository as one directory entry.
        if path.ends_with(b"/") {
            return Err(SnapshotError::InvalidInput(
                "worktree contains an untracked nested Git repository".into(),
            ));
        }

        let path = str::from_utf8(path)
            .ok()
            .and_then(|path| path.parse().ok())
            .ok_or_else(|| {
                SnapshotError::InvalidInput("worktree contains an unsupported path".into())
            })?;
        paths.insert(path);
    }
    Ok(paths)
}

type Entry = (RepoPath, Option<(&'static str, String)>);

fn overlay_entry(
    root: &Path,
    path: RepoPath,
    indexed: impl Fn() -> Command,
    modes: ModeOptions,
) -> Result<Entry, SnapshotError> {
    let absolute = root.join(path.as_str());
    for ancestor in absolute.ancestors().skip(1).take_while(|p| *p != root) {
        // File/directory transitions can hide an indexed descendant.
        if optional_metadata(ancestor)?.is_none_or(|m| !m.is_dir()) {
            return Ok((path, None));
        }
    }
    let Some(info) = optional_metadata(&absolute)? else {
        return Ok((path, None));
    };
    let (mode, file) = if info.is_file() {
        let file = open_regular(&absolute).map_err(|e| unavailable("open worktree file", &e))?;
        let opened = file
            .metadata()
            .map_err(|e| unavailable("inspect opened worktree file", &e))?;
        let final_path = absolute
            .canonicalize()
            .map_err(|e| unavailable("resolve worktree file", &e))?;
        if final_path != absolute || !same_file(&info, &opened) {
            return Err(SnapshotError::Git(
                "worktree file changed while being captured".into(),
            ));
        }

        let mode = worktree_file_mode(&path, &info, &indexed, modes)?;
        (mode, file)
    } else if info.is_symlink() {
        let target = fs::read_link(&absolute).map_err(|e| unavailable("read symlink", &e))?;
        let mut file =
            tempfile::tempfile().map_err(|e| unavailable("create private symlink input", &e))?;
        file.write_all(target.as_os_str().as_encoded_bytes())
            .and_then(|()| file.rewind())
            .map_err(|e| unavailable("prepare symlink blob", &e))?;
        ("120000", file)
    } else if info.is_dir() {
        let gitlink = staged_gitlink(&path, &absolute, &indexed)?;
        return Ok((path, gitlink));
    } else {
        return Err(SnapshotError::InvalidInput(format!(
            "unsupported worktree file type: {}",
            path.as_str()
        )));
    };
    let object = hash_file(indexed, file)?;
    Ok((path, Some((mode, object))))
}

fn staged_entries(
    path: &RepoPath,
    indexed: impl Fn() -> Command,
) -> Result<Vec<u8>, SnapshotError> {
    run(
        indexed().env("GIT_LITERAL_PATHSPECS", "1").args([
            "ls-files",
            "--stage",
            "-z",
            "--",
            path.as_str(),
        ]),
        "inspect the indexed path",
    )
}

fn worktree_file_mode(
    path: &RepoPath,
    info: &fs::Metadata,
    indexed: impl Fn() -> Command,
    modes: ModeOptions,
) -> Result<&'static str, SnapshotError> {
    if !modes.has_symlinks || !modes.trust_file_mode {
        let entries = staged_entries(path, indexed)?;
        let staged_mode = entries.split(|&byte| byte == 0).find_map(|entry| {
            let mut fields = entry.splitn(2, |&byte| byte == b'\t');
            let metadata = fields.next()?;
            if !metadata.ends_with(b" 0") || fields.next() != Some(path.as_str().as_bytes()) {
                return None;
            }

            metadata.split(|&byte| byte == b' ').next()
        });
        match staged_mode {
            // Git represents an indexed symlink as a file containing its target.
            Some(b"120000") if !modes.has_symlinks => return Ok("120000"),
            Some(b"100755") if !modes.trust_file_mode => return Ok("100755"),
            _ => (),
        }
    }

    Ok(if modes.trust_file_mode {
        regular_mode(info)
    } else {
        "100644"
    })
}

fn staged_gitlink(
    path: &RepoPath,
    absolute: &Path,
    indexed: impl Fn() -> Command,
) -> Result<Option<(&'static str, String)>, SnapshotError> {
    let staged = staged_entries(path, indexed)?;
    if !staged.starts_with(b"160000 ") {
        return Ok(None);
    }

    // An initialized submodule contributes its current commit, never its files.
    let mut git = command();
    git.arg("-C").arg(absolute);
    let prefix = run(
        git.args(["rev-parse", "--show-prefix"]),
        "locate the submodule",
    )?;
    if !line(&prefix)?.is_empty() {
        return Ok(None);
    }

    let head = run(
        command()
            .arg("-C")
            .arg(absolute)
            .args(["rev-parse", "--verify", "HEAD^{commit}"]),
        "resolve the submodule HEAD",
    )?;
    Ok(Some(("160000", line(&head)?.to_owned())))
}

fn open_regular(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    // Published Unix targets use these platform ABI flags. Avoid following a
    // replaced final component or blocking if it becomes a FIFO after lstat.
    #[cfg(target_os = "macos")]
    set_flags(&mut options, 0x2000_0000 | 0x0004);
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    set_flags(&mut options, 0x20000 | 0x0800);
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    set_flags(&mut options, 0x8000 | 0x0800);
    options.open(path)
}

#[cfg(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
))]
fn set_flags(options: &mut OpenOptions, flags: i32) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(flags);
}

fn optional_metadata(path: &Path) -> Result<Option<fs::Metadata>, SnapshotError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(err) => Err(unavailable("inspect worktree path", &err)),
    }
}

fn unavailable(step: &str, error: &io::Error) -> SnapshotError {
    SnapshotError::Git(format!("{step}: {error}"))
}

#[cfg(unix)]
fn regular_mode(metadata: &fs::Metadata) -> &'static str {
    use std::os::unix::fs::MetadataExt;
    if metadata.mode() & 0o100 == 0 {
        "100644"
    } else {
        "100755"
    }
}

#[cfg(not(unix))]
fn regular_mode(_: &fs::Metadata) -> &'static str {
    "100644"
}

#[cfg(unix)]
fn same_file(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    after.is_file() && before.dev() == after.dev() && before.ino() == after.ino()
}

#[cfg(not(unix))]
fn same_file(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    after.is_file()
        && before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
}

fn write_overlay(
    indexed: impl Fn() -> Command,
    entries: &[Entry],
    hash_length: usize,
) -> Result<(), SnapshotError> {
    let mut input =
        tempfile::tempfile().map_err(|e| unavailable("create private index input", &e))?;
    let zeros = "0".repeat(hash_length);
    let result = (|| -> io::Result<()> {
        // Remove entries before adding them so file/directory transitions work.
        for (path, _) in entries {
            write!(input, "0 {zeros}\t{}\0", path.as_str())?;
        }
        for (path, entry) in entries {
            if let Some((mode, object)) = entry {
                write!(input, "{mode} {object}\t{}\0", path.as_str())?;
            }
        }
        input.rewind()?;
        Ok(())
    })();
    result.map_err(|e| unavailable("prepare the index overlay", &e))?;
    run(
        indexed()
            .args(["update-index", "-z", "--index-info"])
            .stdin(Stdio::from(input)),
        "update the private index",
    )?;
    Ok(())
}

fn hash_file(indexed: impl Fn() -> Command, file: File) -> Result<String, SnapshotError> {
    let object = run(
        indexed()
            .args(["hash-object", "-w", "--no-filters", "--stdin"])
            .stdin(Stdio::from(file)),
        "store the worktree blob",
    )?;
    Ok(line(&object)?.to_owned())
}
