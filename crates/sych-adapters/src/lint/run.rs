//! Running a linter in an empty environment with a time bound, and the formats that are not
//! SARIF.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};
use std::{fs, thread};

use serde_json::Value;
use sych_core::CiDiagnostic;

use crate::sandbox::offline;

pub(super) const TIMEOUT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(20);
/// Directories walked looking for oxlint configs; deeper ones are not found.
const MAX_DEPTH: usize = 8;

/// How often a memory bound is checked: reading `/proc` costs more than polling the child.
const MEMORY_POLL: Duration = Duration::from_millis(100);

/// `program args` in `cwd` with only an empty `HOME` (under `scratch`), an empty `PATH` and
/// what `env` sets: no user config, credentials or helper tools reach it. Killed after `timeout`.
pub(crate) fn run(
    program: &Path,
    args: &[String],
    cwd: &Path,
    scratch: &Path,
    env: &[(String, OsString)],
    timeout: Duration,
) -> io::Result<Output> {
    run_within(program, args, cwd, scratch, env, timeout, None)
}

/// [`run`], also killed once it and every process it starts hold more than `memory` bytes
/// resident together (`ErrorKind::OutOfMemory`). The bound is read from `/proc`; where there is
/// none ([`memory_bounded`] is false), only the time bound holds.
pub(crate) fn run_within(
    program: &Path,
    args: &[String],
    cwd: &Path,
    scratch: &Path,
    env: &[(String, OsString)],
    timeout: Duration,
    memory: Option<u64>,
) -> io::Result<Output> {
    let home = scratch.join("home");
    fs::create_dir_all(&home)?;
    let mut child = offline(program)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "")
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }

            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let mut measured = started;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }

        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
        }

        if let Some(most) = memory
            && measured.elapsed() >= MEMORY_POLL
        {
            measured = Instant::now();
            if resident(child.id()).is_some_and(|held| held > most) {
                // Under bwrap `--die-with-parent` takes the sandboxed tree with it.
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    format!("held more than {} MiB resident", most >> 20),
                ));
            }
        }

        thread::sleep(POLL);
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Whether a memory bound can be kept here: Linux's `/proc` says what a process holds.
pub(crate) fn memory_bounded() -> bool {
    Path::new("/proc/self/status").is_file()
}

/// The resident bytes of `root` and every process below it, from each process's
/// `/proc/<pid>/status`; `None` without `/proc`.
fn resident(root: u32) -> Option<u64> {
    let mut parent = HashMap::new();
    let mut held = HashMap::new();
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        // A process may end while it is read.
        let Ok(status) = fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        for line in status.lines() {
            let value = |key: &str| {
                line.strip_prefix(key)
                    .and_then(|v| v.split_whitespace().next())
                    .and_then(|v| v.parse::<u64>().ok())
            };
            if let Some(ppid) = value("PPid:").and_then(|p| u32::try_from(p).ok()) {
                parent.insert(pid, ppid);
            } else if let Some(kib) = value("VmRSS:") {
                held.insert(pid, kib << 10);
            }
        }
    }
    let mut total = 0;
    let mut below = vec![root];
    while let Some(pid) = below.pop() {
        total += held.get(&pid).copied().unwrap_or(0);
        below.extend(
            parent
                .iter()
                .filter(|&(_, &p)| p == pid)
                .map(|(&child, _)| child),
        );
    }
    Some(total)
}

/// Whether an oxlint config in the tree loads JS plugins or is itself JS/TS: either would
/// execute repository code, so the project's config is not used.
pub(super) fn oxlint_runs_code(root: &Path) -> bool {
    fn walk(dir: &Path, depth: usize) -> bool {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                return depth < MAX_DEPTH
                    && !matches!(name.as_str(), "node_modules" | ".git")
                    && walk(&path, depth + 1);
            }

            name.starts_with("oxlint.config.")
                || (name.starts_with(".oxlintrc")
                    && fs::read_to_string(&path).is_ok_and(|t| t.contains("jsPlugins")))
        })
    }
    walk(root, 0)
}

/// `[{"message", "filepath", "line", "kind"}]`; anything else is malformed.
pub(super) fn actionlint(text: &str) -> Result<(Vec<CiDiagnostic>, Vec<String>), String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let errors: Vec<Value> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let diagnostics = errors
        .iter()
        .map(|e| CiDiagnostic {
            code_ref: None,
            path: e["filepath"].as_str().unwrap_or_default().to_owned(),
            line: u32::try_from(e["line"].as_u64().unwrap_or(1)).unwrap_or(u32::MAX),
            level: "error".into(),
            code: e["kind"].as_str().map(|k| format!("actionlint::{k}")),
            message: e["message"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect(),
        })
        .collect();
    Ok((diagnostics, Vec::new()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::{env, io};

    use super::*;

    fn host(program: &str) -> PathBuf {
        env::split_paths(&env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join(program))
            .find(|p| p.is_file())
            .unwrap_or_else(|| panic!("{program} on the host PATH"))
    }

    #[test]
    fn a_run_past_its_memory_bound_is_killed_and_one_within_it_finishes() {
        if !memory_bounded() {
            return;
        }

        let scratch = tempfile::tempdir().expect("scratch");
        let bounded = |program: &str, args: &[&str]| {
            let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
            run_within(
                &host(program),
                &args,
                scratch.path(),
                scratch.path(),
                &[],
                Duration::from_secs(5),
                Some(64 << 20),
            )
        };
        // dd holds its 128 MiB block for as long as it copies: past the bound, and never more,
        // should the bound fail.
        let grown = bounded(
            "dd",
            &["if=/dev/zero", "of=/dev/null", "bs=128M", "count=100000"],
        )
        .expect_err("killed");
        assert_eq!(grown.kind(), io::ErrorKind::OutOfMemory, "{grown}");
        assert!(grown.to_string().contains("64 MiB"), "{grown}");
        let small = bounded("true", &[]).expect("finishes");
        assert!(small.status.success());
    }
}
