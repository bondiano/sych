//! The weekly digest: at its slot a worker builds the tenant's weekly report and sends it to
//! Slack and by email. A claimed row per slot and channel keeps several workers from sending it
//! twice; see `sych_storage::digest`.

use std::env;

use anyhow::Context;
use sych_adapters::github::Secret;
use sych_adapters::notify::{Mailer, Slack};
use sych_core::Timestamp;
use sych_core::api::WeeklyReport;
use sych_core::config::{ServerConfig, WeeklyDigest};

use super::Engine;
use super::analytics::{Scope, weekly_report};
use super::views::DAY;

/// A configured digest with its channels, secrets read at start.
pub struct Digest {
    pub config: WeeklyDigest,
    slack: Option<Slack>,
    mail: Option<Mailer>,
    /// The dashboard's analytics page, linked from each digest.
    link: Option<String>,
}

fn secret(
    lookup: &dyn Fn(&str) -> Option<String>,
    name: &str,
    key: &str,
) -> anyhow::Result<Secret> {
    lookup(name)
        .filter(|s| !s.is_empty())
        .map(Secret::new)
        .with_context(|| format!("{name} is not set ({key})"))
}

impl Digest {
    /// `notifications.weekly_digest` with its secrets from the environment, or `None` when it
    /// is not configured.
    pub fn configured(server: &ServerConfig) -> anyhow::Result<Option<Self>> {
        Self::configured_with(server, &|name| env::var(name).ok())
    }

    /// [`Digest::configured`] reading secrets through `lookup`.
    pub fn configured_with(
        server: &ServerConfig,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Option<Self>> {
        let Some(config) = server.notifications.weekly_digest.clone() else {
            return Ok(None);
        };
        let slack = match &config.slack {
            Some(s) => Some(Slack::new(secret(
                lookup,
                s.webhook_url_env.as_str(),
                "notifications.weekly_digest.slack.webhook_url_env",
            )?)?),
            None => None,
        };
        let mail = match &config.email {
            Some(e) => {
                let password = match &e.password_env {
                    Some(env) => Some(secret(
                        lookup,
                        env.as_str(),
                        "notifications.weekly_digest.email.password_env",
                    )?),
                    None => None,
                };
                Some(Mailer::new(e, password)?)
            }
            None => None,
        };
        let link = (server.http.public_base_url.as_deref())
            .map(|base| format!("{}/ui/analytics", base.trim_end_matches('/')));
        Ok(Some(Self {
            config,
            slack,
            mail,
            link,
        }))
    }
}

impl Engine {
    /// Sends the digest of the slot now due to each channel this worker claims.
    pub async fn digest_once(&self) -> anyhow::Result<()> {
        let Some(d) = &self.digest else {
            return Ok(());
        };
        let now = (self.now)().as_second();
        let Some(slot) = d.config.due(now) else {
            return Ok(());
        };
        let tenant = d.config.tenant_id.as_str();
        let storage = &self.app.storage;
        let mut claimed = Vec::new();
        for channel in [
            d.slack.is_some().then_some("SLACK"),
            d.mail.is_some().then_some("EMAIL"),
        ]
        .into_iter()
        .flatten()
        {
            if storage.claim_digest(tenant, slot, channel, now).await? {
                claimed.push(channel);
            }
        }
        if claimed.is_empty() {
            return Ok(());
        }

        let report = weekly_report(&self.app, &Scope::tenant(tenant, now - 7 * DAY), now)
            .await
            .map_err(|e| format!("report: {}: {}", e.code, e.message));
        for channel in claimed {
            let outcome = match &report {
                Ok(r) => self.send_digest(d, channel, r).await,
                Err(e) => Err(e.clone()),
            };
            if let Err(e) = &outcome {
                tracing::warn!(channel, error = %e, "weekly digest not sent");
            }

            storage
                .finish_digest(tenant, slot, channel, outcome.err().as_deref(), now)
                .await?;
        }
        Ok(())
    }

    async fn send_digest(
        &self,
        d: &Digest,
        channel: &str,
        report: &WeeklyReport,
    ) -> Result<(), String> {
        let link = d.link.as_deref();
        match (channel, &d.slack, &d.mail) {
            ("SLACK", Some(slack), _) => {
                let open = link.map_or_else(String::new, |l| format!("\n<{l}|Open in Sych>"));
                slack.send(&format!("{}{open}", report.slack())).await
            }
            ("EMAIL", _, Some(mail)) => {
                let open = link.map_or_else(String::new, |l| format!("\n{l}\n"));
                let subject = format!("Sych weekly report to {}", day(report.until));
                mail.send(&subject, &format!("{}{open}", report.markdown()))
                    .await
            }
            _ => return Err(format!("{channel} is not configured")),
        }
        .map_err(|e| e.0)
    }
}

fn day(at: i64) -> String {
    Timestamp::from_second(at)
        .map_or_else(|_| at.to_string(), |t| t.strftime("%Y-%m-%d").to_string())
}
