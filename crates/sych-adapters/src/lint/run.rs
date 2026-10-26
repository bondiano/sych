//! Running a linter in an empty environment with a time bound, and the formats that are not
//! SARIF.

use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use std::{fs, thread};

use serde_json::Value;
use sych_core::CiDiagnostic;

const TIMEOUT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(20);
/// Directories walked looking for oxlint configs; deeper ones are not found.
const MAX_DEPTH: usize = 8;

/// `program args` in `cwd` with only an empty `HOME` (under `scratch`) and an empty `PATH`:
/// no user config, credentials or helper tools reach it. Killed after [`TIMEOUT`].
pub(super) fn run(
    program: &Path,
    args: &[String],
    cwd: &Path,
    scratch: &Path,
) -> io::Result<Output> {
    let home = scratch.join("home");
    fs::create_dir_all(&home)?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "")
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
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }

        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
        }

        thread::sleep(POLL);
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
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
