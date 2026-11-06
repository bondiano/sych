//! Slack slash commands: `/sych` answers who asks in the workspace with what the dashboard
//! shows, read only and without a model. The workspace reads as one reader of the listed
//! repositories; the signing secret is an env name.

use serde::{Deserialize, Serialize};

use super::{EnvName, Problems};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlackCommands {
    /// The Slack app's signing secret.
    pub signing_secret_env: EnvName,
    #[serde(default = "local")]
    pub tenant_id: String,
    /// `connection/owner/name` the workspace may read; `*` for every bound repository.
    pub repositories: Vec<String>,
}

fn local() -> String {
    "local".into()
}

impl SlackCommands {
    pub fn allows(&self, connection: &str, full_name: &str) -> bool {
        self.repositories
            .iter()
            .any(|r| r == "*" || r.eq_ignore_ascii_case(&format!("{connection}/{full_name}")))
    }

    pub(super) fn validate(&self, p: &mut Problems<'_>) {
        p.check(
            !self.repositories.is_empty(),
            "slack.repositories",
            "list `connection/owner/name` or `*`",
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::config::ServerConfig;

    #[test]
    fn slack_commands_read_only_the_listed_repositories() {
        let parse = |repos: &str| {
            ServerConfig::parse(
                "sych.yaml",
                &format!(
                    "schema_version: 1\nmode: personal\nslack:\n  signing_secret_env: SLACK_SIGNING_SECRET\n  repositories: {repos}\n"
                ),
            )
        };
        let slack = parse("[gh/o/r]").expect("config").slack.expect("slack");
        assert!(slack.allows("gh", "O/R"));
        assert!(!slack.allows("gh", "o/other"));
        assert!(parse("[]").is_err(), "no repositories");
    }
}
