//! Local-CLI-only external model adapter protocol (`sych-schema-adapter-<name>`).
//!
//! Never invoked in server or PR review modes. Runs a trusted local executable
//! already installed on `PATH`, bounds both elapsed time and buffered stdout/stderr
//! bytes, and strictly validates its stdout JSON against [`DbModelManifest`].
//! `kill_on_drop(true)` terminates the direct child process on timeout or cap overflow.

use std::io;
use std::process::Stdio;
use std::time::Duration;

use sych_core::db_schema::DbModelManifest;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::db_schema::DbSchemaError;

/// Maximum stdout bytes buffered from an external adapter (16 MiB).
const MAX_ADAPTER_STDOUT_BYTES: u64 = 16 * 1024 * 1024;
/// Maximum stderr bytes buffered for error diagnostics (256 KiB).
const MAX_ADAPTER_STDERR_BYTES: u64 = 256 * 1024;

/// Runs `sych-schema-adapter-<name>` with `args` and parses its stdout as a [`DbModelManifest`].
pub async fn run_external_adapter(
    name: &str,
    args: &[String],
    limit: Duration,
) -> Result<DbModelManifest, DbSchemaError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(DbSchemaError::ExternalAdapter {
            adapter: name.to_owned(),
            detail: "adapter name must match [a-z0-9_-]+".into(),
        });
    }

    let binary = format!("sych-schema-adapter-{name}");
    let mut cmd = Command::new(&binary);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| DbSchemaError::ExternalAdapter {
        adapter: binary.clone(),
        detail: format!("could not execute `{binary}` on PATH: {e}"),
    })?;

    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| DbSchemaError::ExternalAdapter {
            adapter: binary.clone(),
            detail: "failed to capture adapter stdout".into(),
        })?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| DbSchemaError::ExternalAdapter {
            adapter: binary.clone(),
            detail: "failed to capture adapter stderr".into(),
        })?;

    let run = async {
        let read_stdout = async {
            let mut buf = Vec::new();
            let mut limited = (&mut stdout_pipe).take(MAX_ADAPTER_STDOUT_BYTES + 1);
            limited.read_to_end(&mut buf).await?;
            Ok::<_, io::Error>(buf)
        };
        let read_stderr = async {
            let mut buf = Vec::new();
            let mut limited = (&mut stderr_pipe).take(MAX_ADAPTER_STDERR_BYTES);
            limited.read_to_end(&mut buf).await?;
            Ok::<_, io::Error>(buf)
        };
        let (out_res, err_res, status_res) = tokio::join!(read_stdout, read_stderr, child.wait());
        Ok::<_, io::Error>((out_res?, err_res?, status_res?))
    };

    let (stdout_bytes, stderr_bytes, status) = timeout(limit, run)
        .await
        .map_err(|_| DbSchemaError::ExternalAdapter {
            adapter: binary.clone(),
            detail: format!("timed out after {}", format_duration(limit)),
        })?
        .map_err(|e| DbSchemaError::ExternalAdapter {
            adapter: binary.clone(),
            detail: format!("I/O error while reading `{binary}` output: {e}"),
        })?;

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_owned();
        let detail = if stderr.is_empty() {
            let stdout_hint: String = String::from_utf8_lossy(&stdout_bytes)
                .trim()
                .chars()
                .take(512)
                .collect();
            if stdout_hint.is_empty() {
                format!("exited with status {status}")
            } else {
                format!("exited with status {status}: {stdout_hint}")
            }
        } else {
            stderr
        };
        return Err(DbSchemaError::ExternalAdapter {
            adapter: binary,
            detail,
        });
    }

    if stdout_bytes.len() as u64 > MAX_ADAPTER_STDOUT_BYTES {
        return Err(DbSchemaError::ExternalAdapter {
            adapter: binary,
            detail: format!("stdout exceeded {MAX_ADAPTER_STDOUT_BYTES} byte limit"),
        });
    }

    let stdout = String::from_utf8(stdout_bytes).map_err(|e| DbSchemaError::ExternalAdapter {
        adapter: binary.clone(),
        detail: format!("stdout is not valid UTF-8: {e}"),
    })?;
    DbModelManifest::parse(&stdout).map_err(|e| DbSchemaError::ExternalAdapter {
        adapter: binary,
        detail: e.to_string(),
    })
}

fn format_duration(limit: Duration) -> String {
    if limit.as_secs() > 0 && limit.subsec_millis() == 0 {
        format!("{}s", limit.as_secs())
    } else {
        format!("{}ms", limit.as_millis())
    }
}
