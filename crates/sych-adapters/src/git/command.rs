//! Hardened `git` invocation.
//!
//! The environment is cleared, system and global config are ignored, replace refs and lazy
//! fetches are disabled, and every command addresses the object database through
//! `--git-dir`, so no work tree, index, hook, filter, textconv or external diff is involved.
//! Arguments are passed as argv, never through a shell.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};
use std::{env, io, thread};

use super::SnapshotError;

const HARDENING: &[&str] = &[
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "protocol.allow=never",
    "-c",
    "core.quotePath=false",
    "--no-pager",
    "--no-replace-objects",
];

#[derive(Debug, Clone)]
pub struct Git {
    git_dir: PathBuf,
}

pub(super) fn command() -> Command {
    let mut cmd = Command::new("git");
    cmd.env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    if let Some(path) = env::var_os("PATH") {
        cmd.env("PATH", path);
    }

    cmd.args(HARDENING);
    cmd
}

fn spawn_failed(err: &io::Error) -> SnapshotError {
    SnapshotError::Git(format!("cannot run git: {err}"))
}

impl Git {
    /// Locates the repository containing `repo` and pins its absolute git directory.
    pub fn open(repo: &Path) -> Result<Self, SnapshotError> {
        let out = command()
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--absolute-git-dir"])
            .output()
            .map_err(|e| spawn_failed(&e))?;
        if !out.status.success() {
            return Err(SnapshotError::InvalidInput(format!(
                "{} is not a readable Git repository: {}",
                repo.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }

        let dir = String::from_utf8(out.stdout)
            .map_err(|_| SnapshotError::Git("git directory path is not UTF-8".into()))?;
        Ok(Self {
            git_dir: PathBuf::from(dir.trim_end_matches('\n')),
        })
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Runs a read-only git subcommand and returns raw output, whatever the exit status.
    pub fn output<I, S>(&self, args: I) -> Result<Output, SnapshotError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        command()
            .arg("--git-dir")
            .arg(&self.git_dir)
            .args(args)
            .output()
            .map_err(|e| spawn_failed(&e))
    }

    /// Like [`Self::output`], but kills git once `timeout` passes; `None` means it ran out of time.
    pub fn output_within<I, S>(
        &self,
        args: I,
        timeout: Duration,
    ) -> Result<Option<Output>, SnapshotError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = command();
        cmd.arg("--git-dir").arg(&self.git_dir).args(args);
        run_within(cmd, timeout)
    }

    /// Spawns a read-only git subcommand with piped stdin, stdout and stderr.
    pub(super) fn spawn_piped<I, S>(&self, args: I) -> io::Result<Child>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        command()
            .arg("--git-dir")
            .arg(&self.git_dir)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    }

    /// Runs a read-only git subcommand that must succeed and returns stdout.
    pub fn stdout<I, S>(&self, args: I) -> Result<Vec<u8>, SnapshotError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.output(args)?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(SnapshotError::Git(
                String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            ))
        }
    }
}

/// Runs `cmd` to completion or kills it once `timeout` passes; `None` means it ran out of time.
pub(super) fn run_within(
    mut cmd: Command,
    timeout: Duration,
) -> Result<Option<Output>, SnapshotError> {
    let io_failed = |e: io::Error| spawn_failed(&e);
    // Files, not pipes: a full pipe would stall git until the deadline.
    let mut stdout = tempfile::tempfile().map_err(io_failed)?;
    let mut stderr = tempfile::tempfile().map_err(io_failed)?;
    let mut child = cmd
        .stdout(stdout.try_clone().map_err(io_failed)?)
        .stderr(stderr.try_clone().map_err(io_failed)?)
        .spawn()
        .map_err(io_failed)?;
    let started = Instant::now();
    let status = loop {
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }

        match child.try_wait().map_err(io_failed)? {
            Some(status) => break status,
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    let read = |file: &mut File| -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        file.rewind()?;
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    Ok(Some(Output {
        status,
        stdout: read(&mut stdout).map_err(io_failed)?,
        stderr: read(&mut stderr).map_err(io_failed)?,
    }))
}
