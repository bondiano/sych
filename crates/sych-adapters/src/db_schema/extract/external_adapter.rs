//! Local-CLI-only external model adapter protocol (`sych-schema-adapter-<name>`).
//!
//! Never invoked in server or PR review modes. Runs an executable already installed
//! on `PATH` and strictly validates its stdout JSON against [`DbModelManifest`].

use std::time::Duration;

use sych_core::db_schema::DbModelManifest;
use tokio::process::Command;
use tokio::time::timeout;

use crate::db_schema::DbSchemaError;

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
    cmd.kill_on_drop(true);

    let output = timeout(limit, cmd.output())
        .await
        .map_err(|_| DbSchemaError::ExternalAdapter {
            adapter: binary.clone(),
            detail: format!("timed out after {}s", limit.as_secs()),
        })?
        .map_err(|e| DbSchemaError::ExternalAdapter {
            adapter: binary.clone(),
            detail: format!("could not execute `{binary}` on PATH: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(DbSchemaError::ExternalAdapter {
            adapter: binary,
            detail: if stderr.is_empty() {
                format!("exited with status {}", output.status)
            } else {
                stderr
            },
        });
    }

    let stdout = String::from_utf8(output.stdout).map_err(|e| DbSchemaError::ExternalAdapter {
        adapter: binary.clone(),
        detail: format!("stdout is not valid UTF-8: {e}"),
    })?;
    DbModelManifest::parse(&stdout).map_err(|e| DbSchemaError::ExternalAdapter {
        adapter: binary,
        detail: e.to_string(),
    })
}
