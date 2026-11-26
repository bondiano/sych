//! Synthetic commits for a local index or working tree.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{fs, io};

use ring::rand::{SecureRandom, SystemRandom};
use sych_core::{GitHashAlgorithm, GitObjectId, SnapshotHeadKind, sha256};

use super::command::command;
use super::{Git, SnapshotError};

/// The ref protects the commit until the run takes ownership or capture fails.
pub struct LocalHead {
    repository: PathBuf,
    commit: GitObjectId,
    owned: bool,
}

impl LocalHead {
    pub fn commit(&self) -> &GitObjectId {
        &self.commit
    }

    pub fn repository(&self) -> &Path {
        &self.repository
    }

    /// Transfers ref ownership to the persisted run.
    pub fn keep(&mut self) {
        self.owned = false;
    }
}

impl Drop for LocalHead {
    fn drop(&mut self) {
        if self.owned
            && let Err(error) = delete_local_ref(&self.repository, &self.commit)
        {
            tracing::warn!(%error, "could not release an unclaimed local snapshot");
        }
    }
}

fn pin_name(commit: &GitObjectId) -> String {
    format!("refs/sych/local/{}", commit.hex())
}

/// Releases a pin only when it still points to the expected commit.
pub fn delete_local_ref(repo: &Path, commit: &GitObjectId) -> Result<(), SnapshotError> {
    let name = pin_name(commit);
    let git = |args: &[&str]| {
        command()
            .arg("--git-dir")
            .arg(repo)
            .args(args)
            .output()
            .map_err(|e| io_error("git", &e))
    };
    let deleted = git(&["update-ref", "--no-deref", "-d", &name, commit.hex()])?;
    if deleted.status.success()
        || git(&["show-ref", "--verify", "--quiet", &name])?
            .status
            .code()
            == Some(1)
    {
        return Ok(());
    }

    Err(SnapshotError::Git(
        String::from_utf8_lossy(&deleted.stderr).trim().into(),
    ))
}

fn io_error(step: &str, error: &io::Error) -> SnapshotError {
    SnapshotError::Git(format!("{step}: {error}"))
}

pub(super) fn run(cmd: &mut Command, step: &str) -> Result<Vec<u8>, SnapshotError> {
    let output = cmd.output().map_err(|e| io_error(step, &e))?;
    if !output.status.success() {
        return Err(SnapshotError::Git(format!(
            "{step}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(output.stdout)
}

pub(super) fn line(bytes: &[u8]) -> Result<&str, SnapshotError> {
    str::from_utf8(bytes.strip_suffix(b"\n").unwrap_or(bytes))
        .map_err(|_| SnapshotError::Git("git output is not UTF-8".into()))
}

fn in_repo(repo: &Path) -> Command {
    let mut cmd = command();
    cmd.arg("-C").arg(repo);
    cmd
}

/// Uses a private index and pins a synthetic commit whose single parent is the current HEAD.
pub fn capture_local_head(repo: &Path, kind: SnapshotHeadKind) -> Result<LocalHead, SnapshotError> {
    if kind == SnapshotHeadKind::Committed {
        return Err(SnapshotError::InvalidInput(
            "expected a staged or worktree snapshot".into(),
        ));
    }

    let git = Git::open(repo)?;
    let root = PathBuf::from(line(&run(
        in_repo(repo).args(["rev-parse", "--show-toplevel"]),
        "locate the working tree",
    )?)?)
    .canonicalize()
    .map_err(|e| io_error("resolve the working tree", &e))?;
    let parent = git.stdout(["rev-parse", "--verify", "HEAD^{commit}"])?;
    let parent = line(&parent)?;
    let original = git.stdout(["rev-parse", "--path-format=absolute", "--git-path", "index"])?;
    let temp = tempfile::tempdir_in(git.git_dir())
        .map_err(|e| io_error("create a private index directory", &e))?;
    let index = temp.path().join("index");
    match fs::copy(line(&original)?, &index) {
        Ok(_) => (),
        // Git treats a missing index as empty.
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(io_error("copy the index", &error)),
    }
    let indexed = || {
        let mut cmd = in_repo(&root);
        cmd.env("GIT_INDEX_FILE", &index).args([
            "-c",
            "core.splitIndex=false",
            "-c",
            "core.untrackedCache=false",
        ]);
        cmd
    };
    if kind == SnapshotHeadKind::Worktree {
        super::local_worktree::overlay(&root, indexed, parent.len())?;
    }

    let tree = run(indexed().arg("write-tree"), "write the synthetic tree")?;
    if line(&git.stdout(["rev-parse", "--verify", "HEAD^{commit}"])?)? != parent {
        return Err(SnapshotError::InvalidInput(
            "HEAD changed while capturing the index".into(),
        ));
    }

    let mut nonce = [0; 16];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| SnapshotError::Git("cannot create a unique local snapshot".into()))?;
    let message = format!("Sych local snapshot {}", sha256(&nonce).hex());
    let head = run(
        indexed()
            .env("GIT_AUTHOR_NAME", "Sych")
            .env("GIT_AUTHOR_EMAIL", "snapshot@sych.invalid")
            .env("GIT_COMMITTER_NAME", "Sych")
            .env("GIT_COMMITTER_EMAIL", "snapshot@sych.invalid")
            .args([
                "commit-tree",
                "--no-gpg-sign",
                line(&tree)?,
                "-p",
                parent,
                "-m",
                &message,
            ]),
        "write the synthetic commit",
    )?;
    let head = line(&head)?;
    let algorithm = if parent.len() == 64 {
        GitHashAlgorithm::Sha256
    } else {
        GitHashAlgorithm::Sha1
    };
    let commit = GitObjectId::new(algorithm, head)
        .map_err(|e| SnapshotError::Git(format!("invalid synthetic commit id: {e}")))?;
    let name = pin_name(&commit);
    let common = git.stdout(["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let repository = Path::new(line(&common)?)
        .canonicalize()
        .map_err(|e| io_error("resolve the common Git directory", &e))?;
    run(
        command().arg("--git-dir").arg(&repository).args([
            "update-ref",
            "--no-deref",
            &name,
            commit.hex(),
            &"0".repeat(head.len()),
        ]),
        "pin the synthetic commit",
    )?;
    Ok(LocalHead {
        repository,
        commit,
        owned: true,
    })
}
