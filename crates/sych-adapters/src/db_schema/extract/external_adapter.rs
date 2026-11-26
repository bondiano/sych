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
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::timeout;

use crate::db_schema::DbSchemaError;

/// Maximum stdout bytes buffered from an external adapter (16 MiB).
const MAX_ADAPTER_STDOUT_BYTES: usize = 16 * 1024 * 1024;
/// Maximum stderr bytes buffered for error diagnostics (256 KiB).
const MAX_ADAPTER_STDERR_BYTES: usize = 256 * 1024;
/// Maximum characters kept from stderr/stdout in user-facing error details.
const MAX_ERROR_DETAIL_CHARS: usize = 512;

enum AdapterRunError {
    StdoutOverflow,
    ReadOutput(io::Error),
    Wait(io::Error),
}

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
    cmd.args(args);
    run_adapter_command(
        cmd,
        binary,
        limit,
        MAX_ADAPTER_STDOUT_BYTES,
        MAX_ADAPTER_STDERR_BYTES,
    )
    .await
}

async fn run_adapter_command(
    mut cmd: Command,
    binary: String,
    limit: Duration,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
) -> Result<DbModelManifest, DbSchemaError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| DbSchemaError::ExternalAdapter {
        adapter: binary.clone(),
        detail: format!("could not execute `{binary}` on PATH: {e}"),
    })?;

    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| DbSchemaError::ExternalAdapter {
            adapter: binary.clone(),
            detail: "failed to capture adapter stdout".into(),
        })?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| DbSchemaError::ExternalAdapter {
            adapter: binary.clone(),
            detail: "failed to capture adapter stderr".into(),
        })?;

    let run = async {
        // Move both pipes into the reader futures so cancelling `try_join!` on
        // stdout overflow closes the pipes immediately and dropping `child`
        // (`kill_on_drop(true)`) terminates a runaway process without deadlocking.
        let (stdout_bytes, stderr_bytes) = tokio::try_join!(
            read_stdout_bounded(stdout_pipe, max_stdout_bytes),
            read_stderr_bounded(stderr_pipe, max_stderr_bytes),
        )?;
        let status = child.wait().await.map_err(AdapterRunError::Wait)?;
        Ok::<_, AdapterRunError>((stdout_bytes, stderr_bytes, status))
    };

    let (stdout_bytes, stderr_bytes, status) = timeout(limit, run)
        .await
        .map_err(|_| DbSchemaError::ExternalAdapter {
            adapter: binary.clone(),
            detail: format!("timed out after {}", format_duration(limit)),
        })?
        .map_err(|err| match err {
            AdapterRunError::StdoutOverflow => DbSchemaError::ExternalAdapter {
                adapter: binary.clone(),
                detail: format!("stdout exceeded {max_stdout_bytes} byte limit"),
            },
            AdapterRunError::ReadOutput(e) => DbSchemaError::ExternalAdapter {
                adapter: binary.clone(),
                detail: format!("I/O error while reading `{binary}` output: {e}"),
            },
            AdapterRunError::Wait(e) => DbSchemaError::ExternalAdapter {
                adapter: binary.clone(),
                detail: format!("I/O error while waiting for `{binary}`: {e}"),
            },
        })?;

    if !status.success() {
        let stderr = truncate_diagnostic(&stderr_bytes);
        let detail = if stderr.is_empty() {
            let stdout_hint = truncate_diagnostic(&stdout_bytes);
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

    let stdout = String::from_utf8(stdout_bytes).map_err(|e| DbSchemaError::ExternalAdapter {
        adapter: binary.clone(),
        detail: format!("stdout is not valid UTF-8: {e}"),
    })?;
    DbModelManifest::parse(&stdout).map_err(|e| DbSchemaError::ExternalAdapter {
        adapter: binary,
        detail: e.to_string(),
    })
}

async fn read_stdout_bounded<R: AsyncRead + Unpin>(
    mut pipe: R,
    max_bytes: usize,
) -> Result<Vec<u8>, AdapterRunError> {
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 8192];
    loop {
        let n = pipe
            .read(&mut chunk)
            .await
            .map_err(AdapterRunError::ReadOutput)?;
        if n == 0 {
            return Ok(buf);
        }

        if buf.len().saturating_add(n) > max_bytes {
            return Err(AdapterRunError::StdoutOverflow);
        }

        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn read_stderr_bounded<R: AsyncRead + Unpin>(
    mut pipe: R,
    max_bytes: usize,
) -> Result<Vec<u8>, AdapterRunError> {
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 8192];
    loop {
        let n = pipe
            .read(&mut chunk)
            .await
            .map_err(AdapterRunError::ReadOutput)?;
        if n == 0 {
            return Ok(buf);
        }

        let remaining = max_bytes.saturating_sub(buf.len());
        if remaining > 0 {
            buf.extend_from_slice(&chunk[..n.min(remaining)]);
        }
    }
}

fn truncate_diagnostic(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    let prefix: String = chars.by_ref().take(MAX_ERROR_DETAIL_CHARS).collect();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

fn format_duration(limit: Duration) -> String {
    if limit.as_secs() > 0 && limit.subsec_millis() == 0 {
        format!("{}s", limit.as_secs())
    } else {
        format!("{}ms", limit.as_millis())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn over_cap_stdout_fails_with_size_limit_not_timeout() {
        // Write 512 KiB (> typical OS pipe buffer) while cap is 1 KiB and timeout is 10s.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "dd if=/dev/zero bs=4096 count=128 2>/dev/null"]);
        let err = run_adapter_command(
            cmd,
            "sych-schema-adapter-overflow".into(),
            Duration::from_secs(10),
            1024,
            256,
        )
        .await
        .expect_err("must reject runaway stdout");
        let msg = err.to_string();
        assert!(msg.contains("stdout exceeded 1024 byte limit"), "{msg}");
        assert!(!msg.contains("timed out"), "{msg}");
    }

    #[tokio::test]
    async fn non_zero_exit_truncates_large_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf '%0.sX' $(seq 1 900) >&2; exit 2"]);
        let err = run_adapter_command(
            cmd,
            "sych-schema-adapter-stderr".into(),
            Duration::from_secs(5),
            1024,
            4096,
        )
        .await
        .expect_err("must fail on non-zero exit");
        let msg = err.to_string();
        assert!(msg.ends_with("..."), "{msg}");
        assert!(
            msg.len() < 700,
            "unexpectedly long error detail: {}",
            msg.len()
        );
    }
}
