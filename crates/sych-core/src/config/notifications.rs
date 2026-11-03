//! Where the server sends what it reports on its own: the weekly digest to a Slack incoming
//! webhook and by email. Secrets are env names; the webhook URL is one.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use super::{EnvName, Problems};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Notifications {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weekly_digest: Option<WeeklyDigest>,
}

impl Notifications {
    /// Left out of the effective config when unset, so the config hash stays as it was.
    pub fn is_empty(&self) -> bool {
        self.weekly_digest.is_none()
    }
}

/// `GET /v1/reports/weekly` of one tenant, every repository, sent once a week.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeeklyDigest {
    #[serde(default = "local")]
    pub tenant_id: String,
    #[serde(default)]
    pub weekday: Weekday,
    /// The UTC hour on `weekday` the digest is due; a worker down then sends it within a day.
    #[serde(default = "nine")]
    pub hour_utc: u8,
    pub slack: Option<SlackWebhook>,
    pub email: Option<Email>,
}

fn local() -> String {
    "local".into()
}

const fn nine() -> u8 {
    9
}

const DAY: i64 = 86_400;
const WEEK: i64 = 7 * DAY;

impl WeeklyDigest {
    /// The latest slot at or before `now` (Unix seconds), while it is less than a day old:
    /// a worker that was down longer skips that week rather than send a stale digest.
    pub fn due(&self, now: i64) -> Option<i64> {
        // 1970-01-05, the first Monday of the epoch, 00:00 UTC.
        let base = 4 * DAY + self.weekday.from_monday() * DAY + i64::from(self.hour_utc) * 3600;
        let slot = base + (now - base).div_euclid(WEEK) * WEEK;
        (now - slot < DAY).then_some(slot)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    #[default]
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    /// Days after Monday.
    pub fn from_monday(self) -> i64 {
        match self {
            Self::Monday => 0,
            Self::Tuesday => 1,
            Self::Wednesday => 2,
            Self::Thursday => 3,
            Self::Friday => 4,
            Self::Saturday => 5,
            Self::Sunday => 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlackWebhook {
    /// The incoming webhook URL; it is a credential.
    pub webhook_url_env: EnvName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Email {
    pub host: String,
    #[serde(default = "submission")]
    pub port: u16,
    #[serde(default)]
    pub security: SmtpSecurity,
    pub username: Option<String>,
    pub password_env: Option<EnvName>,
    pub from: String,
    pub to: Vec<String>,
}

const fn submission() -> u16 {
    587
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpSecurity {
    /// STARTTLS, required.
    #[default]
    Starttls,
    /// TLS from the first byte (port 465).
    Tls,
    /// No encryption; only to a loopback host, such as a local relay.
    Plain,
}

fn loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// An address the way an SMTP envelope takes it, checked loosely; the transport parses it.
fn address(text: &str) -> bool {
    text.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && (domain.contains('.') || domain == "localhost")
    })
}

impl Notifications {
    pub(super) fn validate(&self, p: &mut Problems<'_>) {
        let Some(d) = &self.weekly_digest else {
            return;
        };
        let at = "notifications.weekly_digest";
        p.check(d.hour_utc < 24, format!("{at}.hour_utc"), "must be 0 to 23");
        p.check(
            d.slack.is_some() || d.email.is_some(),
            at,
            "needs slack, email or both",
        );
        if let Some(e) = &d.email {
            p.check(
                !e.host.is_empty(),
                format!("{at}.email.host"),
                "must not be empty",
            );
            p.check(
                e.security != SmtpSecurity::Plain || loopback(&e.host),
                format!("{at}.email.security"),
                "plain only to a loopback host",
            );
            p.check(
                e.password_env.is_none() || e.username.is_some(),
                format!("{at}.email.password_env"),
                "needs username",
            );
            p.check(
                address(&e.from),
                format!("{at}.email.from"),
                "must be an address",
            );
            p.check(
                !e.to.is_empty() && e.to.iter().all(|t| address(t)),
                format!("{at}.email.to"),
                "list one or more addresses",
            );
        }
    }

    pub(super) fn env_refs(&self) -> Vec<(String, &EnvName)> {
        let Some(d) = &self.weekly_digest else {
            return Vec::new();
        };
        let slack = (d.slack.as_ref()).map(|s| {
            (
                "notifications.weekly_digest.slack.webhook_url_env".to_owned(),
                &s.webhook_url_env,
            )
        });
        let email = (d.email.as_ref())
            .and_then(|e| e.password_env.as_ref())
            .map(|env| {
                (
                    "notifications.weekly_digest.email.password_env".to_owned(),
                    env,
                )
            });
        slack.into_iter().chain(email).collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::config::ServerConfig;

    fn problems(digest: &str) -> Vec<String> {
        let text = format!(
            "schema_version: 1\nmode: personal\nnotifications:\n  weekly_digest:\n{digest}"
        );
        match ServerConfig::parse("sych.yaml", &text) {
            Ok(_) => Vec::new(),
            Err(errors) => errors.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn a_digest_needs_a_channel_and_sane_email_settings() {
        let slack = "    slack: {webhook_url_env: SLACK_URL}\n";
        assert!(problems(slack).is_empty(), "{:?}", problems(slack));
        let email = "    email: {host: smtp.example.com, username: sych, password_env: SMTP_PASSWORD, \
                     from: sych@example.com, to: [team@example.com]}\n";
        assert!(problems(email).is_empty(), "{:?}", problems(email));

        let none = problems("    hour_utc: 24\n");
        assert!(none.iter().any(|p| p.contains("slack, email")), "{none:?}");
        assert!(none.iter().any(|p| p.contains("0 to 23")), "{none:?}");
        let plain = problems(
            "    email: {host: smtp.example.com, security: plain, from: a@b.c, to: [d@e.f]}\n",
        );
        assert!(plain.iter().any(|p| p.contains("loopback")), "{plain:?}");
        let local = problems(
            "    email: {host: 127.0.0.1, port: 1025, security: plain, from: a@localhost, to: [b@localhost]}\n",
        );
        assert!(local.is_empty(), "{local:?}");
        let anonymous =
            problems("    email: {host: smtp.example.com, password_env: P, from: a@b.c, to: []}\n");
        assert!(
            anonymous.iter().any(|p| p.contains("needs username")),
            "{anonymous:?}"
        );
        assert!(
            anonymous.iter().any(|p| p.contains("one or more")),
            "{anonymous:?}"
        );
    }

    #[test]
    fn a_digest_is_due_for_a_day_after_its_weekly_slot() {
        let digest: super::WeeklyDigest =
            serde_yaml_ng::from_str("{weekday: tuesday, hour_utc: 9, slack: {webhook_url_env: S}}")
                .expect("digest");
        // 2026-11-03 is a Tuesday; 09:00 UTC is 1_793_696_400.
        let slot = 1_793_696_400;
        assert_eq!(digest.due(slot - 1), None, "a week ago is long past");
        assert_eq!(digest.due(slot), Some(slot));
        assert_eq!(digest.due(slot + 86_399), Some(slot));
        assert_eq!(digest.due(slot + 86_400), None, "a stale digest is skipped");
        assert_eq!(digest.due(slot + 7 * 86_400), Some(slot + 7 * 86_400));
    }
}
