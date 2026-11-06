//! Outbound notifications: a Slack incoming webhook and SMTP mail. Errors never carry the
//! webhook URL or the SMTP password.

use std::time::Duration;

use lettre::message::header::ContentType;
use lettre::message::{Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use reqwest::Client;
use reqwest::redirect::Policy;
use serde_json::json;
use sych_core::config::{Email, SmtpSecurity};
use sych_core::redact;

use crate::github::Secret;

const TIMEOUT: Duration = Duration::from_secs(30);

/// A failed send, redacted.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct NotifyError(pub String);

fn failure(text: &str) -> NotifyError {
    NotifyError(redact(text).into_owned())
}

/// A Slack incoming webhook; its URL is the credential.
pub struct Slack {
    http: Client,
    url: Secret,
}

impl Slack {
    pub fn new(url: Secret) -> Result<Self, NotifyError> {
        let http = Client::builder()
            .timeout(TIMEOUT)
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .map_err(|e| failure(&format!("http client: {e}")))?;
        Ok(Self { http, url })
    }

    /// Posts `text` as Slack mrkdwn.
    pub async fn send(&self, text: &str) -> Result<(), NotifyError> {
        let response = self
            .http
            .post(self.url.expose())
            .json(&json!({ "text": text }))
            .send()
            .await
            // The error names the URL, which is the secret.
            .map_err(|e| failure(&e.without_url().to_string()))?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(NotifyError(format!("Slack answered HTTP {status}")))
        }
    }
}

/// SMTP submission to fixed recipients.
pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    to: Vec<Mailbox>,
}

fn mailbox(text: &str) -> Result<Mailbox, NotifyError> {
    text.parse()
        .map_err(|e| failure(&format!("address {text}: {e}")))
}

impl Mailer {
    pub fn new(email: &Email, password: Option<Secret>) -> Result<Self, NotifyError> {
        let builder = match email.security {
            SmtpSecurity::Starttls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&email.host)
            }
            SmtpSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&email.host),
            // Validation allows plain only to a loopback host.
            SmtpSecurity::Plain => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                &email.host,
            )),
        }
        .map_err(|e| failure(&format!("smtp {}: {e}", email.host)))?;
        let builder = builder.port(email.port).timeout(Some(TIMEOUT));
        let builder = match (&email.username, password) {
            (Some(user), Some(password)) => {
                builder.credentials(Credentials::new(user.clone(), password.expose().to_owned()))
            }
            _ => builder,
        };
        Ok(Self {
            transport: builder.build(),
            from: mailbox(&email.from)?,
            to: email
                .to
                .iter()
                .map(|t| mailbox(t))
                .collect::<Result<_, _>>()?,
        })
    }

    /// The plain-text message `send` delivers.
    pub fn message(&self, subject: &str, body: &str) -> Result<Message, NotifyError> {
        let builder = (self.to.iter()).fold(
            Message::builder().from(self.from.clone()).subject(subject),
            |b, to| b.to(to.clone()),
        );
        builder
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_owned())
            .map_err(|e| failure(&format!("message: {e}")))
    }

    pub async fn send(&self, subject: &str, body: &str) -> Result<(), NotifyError> {
        let message = self.message(subject, body)?;
        self.transport
            .send(message)
            .await
            .map(drop)
            .map_err(|e| failure(&format!("smtp: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use sych_core::config::{Email, SmtpSecurity};

    use super::Mailer;

    #[test]
    fn a_digest_mail_goes_to_every_recipient_as_plain_text() {
        let email = Email {
            host: "127.0.0.1".into(),
            port: 1025,
            security: SmtpSecurity::Plain,
            username: None,
            password_env: None,
            from: "Sych <sych@example.com>".into(),
            to: vec!["a@example.com".into(), "b@example.com".into()],
        };
        let mailer = Mailer::new(&email, None).expect("mailer");
        let text = String::from_utf8(
            mailer
                .message("Sych weekly report", "# Sych weekly report\n")
                .expect("message")
                .formatted(),
        )
        .expect("utf8");
        assert!(text.contains("To: a@example.com, b@example.com"), "{text}");
        assert!(text.contains("Subject: Sych weekly report"), "{text}");
        assert!(text.contains("Content-Type: text/plain"), "{text}");
        assert!(text.ends_with("# Sych weekly report\r\n"), "{text}");

        let bad = Email {
            to: vec!["not an address".into()],
            ..email
        };
        assert!(Mailer::new(&bad, None).is_err());
    }
}
