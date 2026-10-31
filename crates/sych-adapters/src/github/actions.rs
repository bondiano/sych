//! GitHub Actions reads for CI evidence: runs of one exact head commit, the jobs
//! of one run attempt, the artifacts of one run and an artifact download. The download URL
//! GitHub redirects to gets no credential and must be https on an allowed host.

use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;
use sych_core::scm::{ScmCall, ScmError, ScmErrorKind, map_status};

use super::{Github, Secret};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkflowRun {
    pub id: u64,
    pub run_attempt: u64,
    pub name: Option<String>,
    /// `.github/workflows/<file>`; the trusted selector names it exactly.
    pub path: String,
    pub event: String,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub head_sha: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub head_sha: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Artifact {
    pub id: u64,
    pub name: String,
    pub expired: bool,
    pub size_in_bytes: u64,
}

fn field<T: for<'de> Deserialize<'de>>(v: &Value, key: &str) -> Result<Vec<T>, ScmError> {
    serde_json::from_value(v[key].clone())
        .map_err(|e| ScmError::new(ScmErrorKind::Transient, format!("github `{key}`: {e}")))
}

/// Whether `host` matches an allowed name; `*.example.com` matches subdomains only.
pub fn host_allowed(host: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|a| match a.strip_prefix("*.") {
        Some(domain) => host
            .strip_suffix(domain)
            .is_some_and(|rest| rest.ends_with('.') && rest.len() > 1),
        None => host == a,
    })
}

impl Github {
    /// Runs of `head_sha` (first 100); a selector picks the workflow and event exactly.
    pub async fn runs_for(
        &self,
        repo: &str,
        head_sha: &str,
        token: &Secret,
    ) -> Result<Vec<WorkflowRun>, ScmError> {
        let v: Value = self
            .get(
                &format!("/repos/{repo}/actions/runs?head_sha={head_sha}&per_page=100"),
                token,
            )
            .await?;
        field(&v, "workflow_runs")
    }

    pub async fn jobs(
        &self,
        repo: &str,
        run: u64,
        attempt: u64,
        token: &Secret,
    ) -> Result<Vec<Job>, ScmError> {
        let v: Value = self
            .get(
                &format!("/repos/{repo}/actions/runs/{run}/attempts/{attempt}/jobs?per_page=100"),
                token,
            )
            .await?;
        field(&v, "jobs")
    }

    pub async fn artifacts(
        &self,
        repo: &str,
        run: u64,
        token: &Secret,
    ) -> Result<Vec<Artifact>, ScmError> {
        let v: Value = self
            .get(
                &format!("/repos/{repo}/actions/runs/{run}/artifacts?per_page=100"),
                token,
            )
            .await?;
        field(&v, "artifacts")
    }

    /// The artifact zip, at most `max_bytes`. GitHub answers with a redirect to signed storage:
    /// that URL is fetched without the token, only over https to an allowed host.
    pub async fn download_artifact(
        &self,
        repo: &str,
        artifact: u64,
        token: &Secret,
        allowed_hosts: &[String],
        max_bytes: u64,
    ) -> Result<Vec<u8>, ScmError> {
        let refused = |m: String| ScmError::new(ScmErrorKind::UnsupportedCapability, m);
        let first = self
            .raw_get(
                &format!("/repos/{repo}/actions/artifacts/{artifact}/zip"),
                token,
            )
            .await?;
        let location = first
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| refused(format!("artifact {artifact}: no download redirect")))?;
        let url = Url::parse(location).map_err(|e| refused(format!("download URL: {e}")))?;
        let host = url.host_str().unwrap_or_default();
        // Plain http only to loopback, as for every configured endpoint.
        let secure = url.scheme() == "https"
            || url.scheme() == "http" && matches!(host, "127.0.0.1" | "localhost" | "[::1]");
        if !secure || !host_allowed(host, allowed_hosts) {
            return Err(refused(format!("download host `{host}` is not allowed")));
        }

        let mut response = self.plain().get(url).send().await.map_err(|e| {
            ScmError::new(ScmErrorKind::Transient, format!("artifact download: {e}"))
        })?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Err(ScmError::new(
                map_status(status, false, ScmCall::Read),
                format!("artifact download: HTTP {status}"),
            ));
        }

        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| {
            ScmError::new(ScmErrorKind::Transient, format!("artifact download: {e}"))
        })? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > max_bytes {
                return Err(refused(format!(
                    "artifact {artifact} is larger than {max_bytes} bytes"
                )));
            }
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_match_exactly_or_as_subdomains() {
        let allowed = vec![
            "*.blob.core.windows.net".to_owned(),
            "example.org".to_owned(),
        ];
        assert!(host_allowed(
            "productionresults.blob.core.windows.net",
            &allowed
        ));
        assert!(host_allowed("example.org", &allowed));
        assert!(!host_allowed("blob.core.windows.net", &allowed));
        assert!(!host_allowed("evilblob.core.windows.net", &allowed));
        assert!(!host_allowed("blob.core.windows.net.evil.com", &allowed));
        assert!(!host_allowed("sub.example.org", &allowed));
    }
}
