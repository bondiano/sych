//! Fetching pinned commits into a local bare mirror: exact object ids, no tags,
//! hooks or lazy fetches; the installation token travels in `GIT_CONFIG_*` environment, never in
//! argv or the URL. Nothing is checked out or executed.

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::SnapshotError;
use super::command::{command, run_within};

/// Fetches `commits` (full object ids) from `url` into `mirror`, creating it on first use. The
/// commits stay referenced under `refs/sych/` so later runs read them without the network.
pub fn fetch_commits(
    mirror: &Path,
    url: &str,
    token: Option<&str>,
    commits: &[&str],
    timeout: Duration,
) -> Result<(), SnapshotError> {
    let https = url.starts_with("https://");
    if !https && !url.starts_with("file://") {
        return Err(SnapshotError::InvalidInput(
            "a mirror fetches over https (or file:// for local fixtures)".into(),
        ));
    }

    if commits
        .iter()
        .any(|c| c.len() < 40 || !c.chars().all(|ch| ch.is_ascii_hexdigit()))
    {
        return Err(SnapshotError::InvalidInput(
            "only full commit ids are fetched".into(),
        ));
    }

    if !mirror.join("HEAD").exists() {
        let mut init = command();
        init.args(["init", "--bare", "--quiet"]).arg(mirror);
        expect_success(run_within(init, timeout)?, "init")?;
    }

    let mut fetch = command();
    if let Some(token) = token.filter(|_| https) {
        let basic = STANDARD.encode(format!("x-access-token:{token}"));
        fetch
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "http.extraHeader")
            .env(
                "GIT_CONFIG_VALUE_0",
                format!("Authorization: Basic {basic}"),
            );
    }

    fetch
        .args([
            "-c",
            "protocol.https.allow=always",
            "-c",
            "protocol.file.allow=always",
        ])
        .arg("--git-dir")
        .arg(mirror)
        .args([
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-recurse-submodules",
            "--",
        ])
        .arg(url)
        .args(commits.iter().map(|c| format!("+{c}:refs/sych/{c}")));
    expect_success(run_within(fetch, timeout)?, "fetch")
}

fn expect_success(output: Option<Output>, step: &str) -> Result<(), SnapshotError> {
    match output {
        None => Err(SnapshotError::Git(format!("{step} timed out"))),
        Some(out) if out.status.success() => Ok(()),
        // Git never prints the extra header; stderr is safe to keep.
        Some(out) => Err(SnapshotError::Git(format!(
            "{step} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}
