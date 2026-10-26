//! SCM connections of the server config (spec §19.5, §22.3): the API address, credentials and
//! repositories of each connection are trusted config; a webhook URL never chooses them.

use serde::{Deserialize, Serialize};

use super::{EnvName, Problems, endpoint};

/// Default webhook body limit (spec §19.2).
pub const WEBHOOK_BODY_LIMIT: u32 = 2 * 1024 * 1024;
/// Production connections wait for the full-feature release gate and a separate owner
/// decision (roadmap R1, M2.9); until then the validator refuses them.
pub const PRODUCTION_ALLOWED: bool = false;
/// Repositories that are production by definition, whatever a connection says.
pub const PRODUCTION_REPOSITORIES: &[&str] = &["bondiano/sych"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    #[default]
    Sandbox,
    Production,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Scm {
    pub github: Vec<GithubConnection>,
}

/// One GitHub App installation scope; `id` is the `:connection` segment of the webhook URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubConnection {
    pub id: String,
    #[serde(default = "local")]
    pub tenant_id: String,
    #[serde(default = "github_com")]
    pub provider_instance: String,
    #[serde(default = "github_api")]
    pub api_base_url: String,
    /// Where `owner/name.git` is fetched from; `file://` only for local fixtures.
    #[serde(default = "github_git")]
    pub git_base_url: String,
    pub app_id: u64,
    /// Variable that holds the path of the App's PEM private key.
    pub private_key_file_env: EnvName,
    /// Variable that holds the webhook secret; never the same as the API credential.
    pub webhook_secret_env: EnvName,
    /// `owner/name` of every repository the App may review; others are refused at admission.
    pub repositories: Vec<String>,
    /// Login of the App's bot user; its own comments never schedule work.
    pub bot_login: String,
    /// Checks write is requested only when check runs are published (spec §18.7).
    #[serde(default)]
    pub checks: bool,
    /// Advisory summary and inline comments are published; off until the owner turns it on.
    #[serde(default)]
    pub publish: bool,
    #[serde(default = "body_limit")]
    pub max_body_bytes: u32,
    #[serde(default)]
    pub environment: Environment,
    /// Sych reviews its own repository: the running reviewer must be the trusted build pinned in
    /// `reviewer_commit`, never the code under review.
    #[serde(default)]
    pub self_review: bool,
    pub reviewer_commit: Option<String>,
    /// Trusted mapping of repository `expected_reports` ids to exact CI producers (§15.2).
    #[serde(default)]
    pub ci_reports: Vec<CiReportSelector>,
    /// Hosts an artifact download may be redirected to; it never carries a credential.
    #[serde(default = "artifact_hosts")]
    pub artifact_hosts: Vec<String>,
}

/// Where one expected report comes from: never "a job with a similar name".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiReportSelector {
    /// `expected_reports[].id` of the repository policy.
    pub report: String,
    /// Workflow file, e.g. `.github/workflows/ci.yml`.
    pub workflow: String,
    #[serde(default = "pull_request")]
    pub event: String,
    pub job: String,
    pub artifact: String,
    /// Report path inside the artifact.
    pub path: String,
    /// `CiManifest` JSON inside the artifact naming the tested commit; without it the checkout
    /// stays unverified.
    pub checkout_manifest: Option<String>,
}

fn pull_request() -> String {
    "pull_request".into()
}

fn artifact_hosts() -> Vec<String> {
    vec![
        "*.blob.core.windows.net".into(),
        "*.actions.githubusercontent.com".into(),
    ]
}

fn local() -> String {
    "local".into()
}

fn github_com() -> String {
    "github.com".into()
}

fn github_api() -> String {
    "https://api.github.com".into()
}

fn github_git() -> String {
    "https://github.com".into()
}

fn body_limit() -> u32 {
    WEBHOOK_BODY_LIMIT
}

impl GithubConnection {
    pub fn allows(&self, full_name: &str) -> bool {
        self.repositories
            .iter()
            .any(|r| r.eq_ignore_ascii_case(full_name))
    }
}

impl Scm {
    pub fn github(&self, id: &str) -> Option<&GithubConnection> {
        self.github.iter().find(|c| c.id == id)
    }

    pub fn env_refs(&self) -> Vec<(String, &EnvName)> {
        self.github
            .iter()
            .enumerate()
            .flat_map(|(i, c)| {
                [
                    (
                        format!("scm.github[{i}].private_key_file_env"),
                        &c.private_key_file_env,
                    ),
                    (
                        format!("scm.github[{i}].webhook_secret_env"),
                        &c.webhook_secret_env,
                    ),
                ]
            })
            .collect()
    }

    pub(super) fn validate(&self, p: &mut Problems<'_>) {
        for (i, c) in self.github.iter().enumerate() {
            let path = format!("scm.github[{i}]");
            p.check(
                !c.id.is_empty()
                    && c.id
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
                format!("{path}.id"),
                "must be a non-empty [A-Za-z0-9_-] name",
            );
            p.check(
                self.github.iter().filter(|o| o.id == c.id).count() == 1,
                format!("{path}.id"),
                "is used by another connection",
            );
            p.check(
                endpoint::callable(&c.api_base_url).is_some(),
                format!("{path}.api_base_url"),
                "must be an https URL (http only to loopback) without credentials",
            );
            p.check(
                c.git_base_url.starts_with("https://") || c.git_base_url.starts_with("file:///"),
                format!("{path}.git_base_url"),
                "must be an https URL (file:/// only for local fixtures)",
            );
            p.check(
                c.private_key_file_env != c.webhook_secret_env,
                format!("{path}.webhook_secret_env"),
                "the webhook secret and the API credential must be different",
            );
            p.check(
                !c.repositories.is_empty()
                    && c.repositories.iter().all(|r| {
                        r.split_once('/').is_some_and(|(o, n)| {
                            !o.is_empty() && !n.is_empty() && !n.contains('/')
                        })
                    }),
                format!("{path}.repositories"),
                "list at least one `owner/name`",
            );
            p.check(
                !c.bot_login.is_empty(),
                format!("{path}.bot_login"),
                "must name the App's bot user",
            );
            let production = c.environment == Environment::Production
                || c.repositories.iter().any(|r| {
                    PRODUCTION_REPOSITORIES
                        .iter()
                        .any(|p| p.eq_ignore_ascii_case(r))
                });
            p.check(
                PRODUCTION_ALLOWED || !production,
                format!("{path}.repositories"),
                "production repositories wait for the R1 gate and an owner decision; use a sandbox mirror",
            );
            p.check(
                !c.self_review
                    || c.reviewer_commit.as_deref().is_some_and(|h| {
                        h.len() == 40 && h.chars().all(|ch| ch.is_ascii_hexdigit())
                    }),
                format!("{path}.reviewer_commit"),
                "self-review needs the full commit of the trusted reviewer build",
            );
            for (j, r) in c.ci_reports.iter().enumerate() {
                p.check(
                    c.ci_reports.iter().filter(|o| o.report == r.report).count() == 1,
                    format!("{path}.ci_reports[{j}].report"),
                    "each report has exactly one producer",
                );
                p.check(
                    r.workflow.starts_with(".github/workflows/") && !r.path.is_empty(),
                    format!("{path}.ci_reports[{j}]"),
                    "needs a `.github/workflows/` file and a report path",
                );
            }
            p.check(
                c.max_body_bytes > 0,
                format!("{path}.max_body_bytes"),
                "must be greater than zero",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::ServerConfig;

    fn errors(extra: &str) -> Vec<String> {
        let text = format!(
            "schema_version: 1\nmode: personal\nscm:\n  github:\n    - id: gh\n      app_id: 1\n      \
             private_key_file_env: KEY\n      webhook_secret_env: HOOK\n      bot_login: b\n{extra}"
        );
        ServerConfig::parse("sych.yaml", &text)
            .err()
            .unwrap_or_default()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn production_waits_for_r1_and_self_review_needs_a_pinned_build() {
        assert!(errors("      repositories: [bondiano/sych-sandbox]\n").is_empty());
        let prod = errors("      repositories: [bondiano/sych]\n");
        assert!(prod.iter().any(|e| e.contains("R1 gate")), "{prod:?}");
        let flagged = errors("      repositories: [o/r]\n      environment: production\n");
        assert!(flagged.iter().any(|e| e.contains("R1 gate")), "{flagged:?}");
        let unpinned = errors("      repositories: [o/r]\n      self_review: true\n");
        assert!(
            unpinned
                .iter()
                .any(|e| e.contains("trusted reviewer build")),
            "{unpinned:?}"
        );
        let pinned = errors(&format!(
            "      repositories: [o/r]\n      self_review: true\n      reviewer_commit: \"{}\"\n",
            "a".repeat(40)
        ));
        assert!(pinned.is_empty(), "{pinned:?}");
    }
}
