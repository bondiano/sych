//! `POST /slack/commands`: the `/sych` slash command, read only and without a model. The
//! signature is checked on the raw body first; the workspace reads as one reader of the
//! repositories `slack.repositories` lists. Answers are ephemeral: only who asked sees them.

use std::fmt::Write;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde_json::{Value, json};
use sych_adapters::slack;
use sych_core::api::ChangeSummary;
use sych_storage::{ChangeFilter, Visibility};
use url::form_urlencoded;

use super::App;
use super::analytics::{Scope, weekly_report};
use super::api::{now, storage_error};
use super::error::ApiError;
use super::views::DAY;

const HELP: &str = "`/sych owner/name#123` — the latest review of that pull request\n\
                    `/sych digest` — the last seven days, as the weekly digest\n\
                    `/sych help` — this list";

#[derive(Debug, PartialEq, Eq)]
enum Command<'a> {
    Help,
    Digest,
    Change { full_name: &'a str, number: i64 },
}

/// The command in the slash command's text, or `None` when it is none of them.
fn command(text: &str) -> Option<Command<'_>> {
    match text.trim() {
        "" | "help" => Some(Command::Help),
        "digest" => Some(Command::Digest),
        other => {
            let (full_name, number) = other.split_once('#')?;
            let number = number.parse().ok().filter(|n| *n > 0)?;
            let valid =
                full_name.split('/').count() == 2 && !full_name.contains(char::is_whitespace);
            valid.then_some(Command::Change { full_name, number })
        }
    }
}

fn reply(text: &str) -> Json<Value> {
    Json(json!({"response_type": "ephemeral", "text": text}))
}

fn refused(status: StatusCode, message: &str) -> ApiError {
    ApiError::new(status, "SLACK_REFUSED", message)
}

pub async fn commands(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let Some((config, secret)) = &app.slack else {
        return Err(refused(
            StatusCode::NOT_FOUND,
            "Slack commands are not configured",
        ));
    };
    let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
    slack::verify(
        secret.expose().as_bytes(),
        header("x-slack-request-timestamp"),
        header("x-slack-signature"),
        &body,
        now(&app).as_second(),
    )
    .map_err(|e| refused(StatusCode::UNAUTHORIZED, &e.to_string()))?;

    let text = form_urlencoded::parse(&body)
        .find(|(k, _)| k == "text")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    let tenant = config.tenant_id.as_str();
    let readable: Vec<_> = app
        .storage
        .repositories(tenant)
        .await
        .map_err(|e| storage_error(&e))?
        .into_iter()
        .filter(|r| config.allows(&r.connection_id, &r.full_name))
        .collect();
    let answer = match command(&text) {
        None => format!("Not a command I know.\n{HELP}"),
        Some(Command::Help) => HELP.to_owned(),
        Some(Command::Digest) => {
            let until = now(&app).as_second();
            let ids = readable.iter().map(|r| r.id).collect();
            let scope = Scope::repositories(tenant, ids, until - 7 * DAY);
            weekly_report(&app, &scope, until).await?.slack()
        }
        Some(Command::Change { full_name, number }) => {
            let Some(repo) = readable
                .iter()
                .find(|r| r.full_name.eq_ignore_ascii_case(full_name))
            else {
                return Ok(reply(&format!(
                    "No pull request {full_name}#{number} here."
                )));
            };
            let filter = ChangeFilter {
                repository_id: Some(repo.id),
                number: Some(number),
                limit: 1,
                ..ChangeFilter::default()
            };
            let ids = [repo.id];
            let visible = Visibility {
                tenant_id: tenant,
                repositories: Some(&ids),
            };
            let page =
                (app.storage.changes(visible, &filter).await).map_err(|e| storage_error(&e))?;
            match page.items.first() {
                Some(change) => summary(change, app.public_base_url.as_deref()),
                None => format!("No pull request {full_name}#{number} here."),
            }
        }
    };
    Ok(reply(&answer))
}

/// A pull request's latest run as the dashboard lists it.
fn summary(c: &ChangeSummary, base: Option<&str>) -> String {
    let number = c.number.map_or_else(|| "?".into(), |n| n.to_string());
    let mut out = format!(
        "*{}#{number}* · {}\n",
        c.repository.full_name,
        c.scm_state.as_deref().unwrap_or("unknown state")
    );
    match &c.latest_run {
        Some(r) => {
            let _ = writeln!(
                out,
                "Latest review: {} · coverage {} · {} unknown obligations",
                r.state,
                r.coverage.as_deref().unwrap_or("not yet known"),
                r.unknown_obligations
                    .map_or_else(|| "not yet counted".into(), |n| n.to_string())
            );
            let unknown = match r.unknown_cost_attempts {
                0 => String::new(),
                n => format!(", plus {n} calls of unknown cost"),
            };
            let _ = writeln!(
                out,
                "Open findings: {} · cost ${}.{:06}{unknown}",
                c.open_findings,
                r.cost_known_micro_usd / 1_000_000,
                r.cost_known_micro_usd % 1_000_000
            );
        }
        None => out.push_str("Not reviewed yet.\n"),
    }
    if let Some(base) = base {
        let _ = write!(
            out,
            "<{}/ui/changes/{}|Open in Sych>",
            base.trim_end_matches('/'),
            c.id
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{Command, command};

    #[test]
    fn the_text_names_help_the_digest_or_one_pull_request() {
        assert_eq!(command(""), Some(Command::Help));
        assert_eq!(command(" help "), Some(Command::Help));
        assert_eq!(command("digest"), Some(Command::Digest));
        assert_eq!(
            command("o/r#7"),
            Some(Command::Change {
                full_name: "o/r",
                number: 7
            })
        );
        for unknown in ["o/r#0", "o/r#x", "r#7", "o/r/s#7", "o /r#7", "rerun o/r#7"] {
            assert_eq!(command(unknown), None, "{unknown}");
        }
    }
}
