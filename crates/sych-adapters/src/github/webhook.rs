//! GitHub webhook admission checks: signature over the raw body before anything
//! is parsed, then a normalized [`WebhookEvent`]. GitHub signs no timestamp, so there is no
//! replay window to check; the delivery id deduplicates instead.

use std::fmt::Write;

use ring::hmac;
use serde_json::Value;
use sych_core::scm::{ChangeSignal, RepositoryRef, ScmCommand, WebhookEvent};

/// Event types the server accepts; anything else is refused before parsing.
pub const EVENTS: &[&str] = &[
    "ping",
    "pull_request",
    "issue_comment",
    "workflow_run",
    "check_suite",
    "installation",
    "installation_repositories",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("body is larger than {0} bytes")]
    TooLarge(u32),
    #[error("content type must be application/json")]
    ContentType,
    #[error("missing header {0}")]
    MissingHeader(&'static str),
    #[error("signature does not match")]
    BadSignature,
    #[error("event `{0}` is not accepted")]
    Event(String),
    #[error("malformed payload: {0}")]
    Malformed(String),
}

fn hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }

    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// `X-Hub-Signature-256` over the raw body; `ring` compares in constant time.
pub fn verify(secret: &[u8], body: &[u8], header: Option<&str>) -> Result<(), Rejection> {
    let header = header.ok_or(Rejection::MissingHeader("X-Hub-Signature-256"))?;
    let tag = header
        .strip_prefix("sha256=")
        .and_then(hex)
        .ok_or(Rejection::BadSignature)?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    hmac::verify(&key, body, &tag).map_err(|_| Rejection::BadSignature)
}

/// The signature GitHub would send; for tests and the sandbox smoke.
pub fn sign(secret: &[u8], body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let tag = hmac::sign(&key, body);
    let mut out = String::from("sha256=");
    for b in tag.as_ref() {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn str_at<'a>(v: &'a Value, pointer: &str) -> Option<&'a str> {
    v.pointer(pointer)?.as_str()
}

/// A `/sych <command>` line; anything else in a comment is ignored.
fn command(body: &str) -> Option<ScmCommand> {
    body.lines()
        .find_map(|line| match line.trim().strip_prefix("/sych")?.trim() {
            "rerun" => Some(ScmCommand::Rerun),
            "deep" => Some(ScmCommand::Deep),
            "stop" => Some(ScmCommand::Stop),
            _ => None,
        })
}

fn pull_request(v: &Value, action: &str) -> Option<ChangeSignal> {
    let pr = v.get("pull_request")?;
    let draft = pr["draft"].as_bool().unwrap_or(false);
    match action {
        "opened" | "reopened" if draft => Some(ChangeSignal::Draft),
        "opened" | "reopened" | "ready_for_review" => Some(ChangeSignal::Opened),
        "synchronize" => Some(ChangeSignal::SourcePush {
            head_sha: str_at(pr, "/head/sha")?.to_owned(),
        }),
        "edited" if v.pointer("/changes/base").is_some() => Some(ChangeSignal::TargetChanged),
        "converted_to_draft" => Some(ChangeSignal::Draft),
        "closed" => Some(ChangeSignal::Closed {
            merged: pr["merged"].as_bool().unwrap_or(false),
        }),
        _ => None,
    }
}

/// Parses a verified delivery. `bot_login` is the App's own user: its comments never act.
pub fn parse(event: &str, body: &[u8], bot_login: &str) -> Result<WebhookEvent, Rejection> {
    if !EVENTS.contains(&event) {
        return Err(Rejection::Event(event.to_owned()));
    }

    let v: Value = serde_json::from_slice(body).map_err(|e| Rejection::Malformed(e.to_string()))?;
    let action = str_at(&v, "/action").unwrap_or_default();
    let sender = str_at(&v, "/sender/login").map(str::to_owned);
    let repository = v.get("repository").and_then(|r| {
        Some(RepositoryRef {
            external_id: r.get("id")?.as_u64()?.to_string(),
            full_name: r.get("full_name")?.as_str()?.to_owned(),
        })
    });
    let mut out = WebhookEvent {
        event: event.to_owned(),
        action: (!action.is_empty()).then(|| action.to_owned()),
        repository,
        installation_id: v.pointer("/installation/id").and_then(Value::as_u64),
        change_numbers: Vec::new(),
        fork: false,
        sender: sender.clone(),
        signal: None,
    };
    match event {
        "pull_request" => {
            let pr = &v["pull_request"];
            out.change_numbers.extend(pr["number"].as_u64());
            // A deleted fork has no head repository: it is still not the base.
            out.fork = str_at(pr, "/head/repo/full_name") != str_at(pr, "/base/repo/full_name");
            out.signal = pull_request(&v, action);
        }
        "issue_comment" if action == "created" && v.pointer("/issue/pull_request").is_some() => {
            out.change_numbers
                .extend(v.pointer("/issue/number").and_then(Value::as_u64));
            let body = str_at(&v, "/comment/body").unwrap_or_default();
            out.signal = match sender {
                Some(login) if login.eq_ignore_ascii_case(bot_login) => {
                    Some(ChangeSignal::BotComment)
                }
                Some(actor) => {
                    command(body).map(|command| ChangeSignal::Command { actor, command })
                }
                None => None,
            };
        }
        "workflow_run" | "check_suite" if action == "completed" => {
            let run = &v[event];
            out.change_numbers.extend(
                run["pull_requests"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p["number"].as_u64()),
            );
            out.signal = Some(ChangeSignal::CiCompleted {
                run_id: run["id"]
                    .as_u64()
                    .ok_or_else(|| Rejection::Malformed(format!("{event}.id")))?,
                head_sha: str_at(run, "/head_sha")
                    .ok_or_else(|| Rejection::Malformed(format!("{event}.head_sha")))?
                    .to_owned(),
            });
        }
        _ => {}
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn signature_is_checked_on_the_raw_bytes() {
        let body = br#"{"zen":"x"}"#;
        let header = sign(b"secret", body);
        assert_eq!(verify(b"secret", body, Some(&header)), Ok(()));
        assert_eq!(
            verify(b"secret", br#"{"zen": "x"}"#, Some(&header)),
            Err(Rejection::BadSignature),
            "re-serialized JSON is a different body"
        );
        assert_eq!(
            verify(b"other", body, Some(&header)),
            Err(Rejection::BadSignature)
        );
        assert_eq!(
            verify(b"secret", body, Some("sha1=00")),
            Err(Rejection::BadSignature)
        );
        assert!(matches!(
            verify(b"secret", body, None),
            Err(Rejection::MissingHeader(_))
        ));
    }

    fn pr(action: &str, draft: bool, head_repo: &str) -> Vec<u8> {
        json!({"action": action, "number": 7, "sender": {"login": "dev"},
            "repository": {"id": 42, "full_name": "o/r"}, "installation": {"id": 9},
            "pull_request": {"number": 7, "draft": draft, "merged": false,
                "head": {"sha": "abc", "repo": {"full_name": head_repo}},
                "base": {"repo": {"full_name": "o/r"}}}})
        .to_string()
        .into_bytes()
    }

    #[test]
    fn pull_request_actions_normalize() {
        let e = parse("pull_request", &pr("opened", false, "o/r"), "sych[bot]").expect("ok");
        assert_eq!(e.signal, Some(ChangeSignal::Opened));
        assert_eq!(e.change_numbers, [7]);
        assert!(!e.fork);
        let draft = parse("pull_request", &pr("opened", true, "o/r"), "b").expect("ok");
        assert_eq!(draft.signal, Some(ChangeSignal::Draft));
        let push = parse("pull_request", &pr("synchronize", false, "f/r"), "b").expect("ok");
        assert_eq!(
            push.signal,
            Some(ChangeSignal::SourcePush {
                head_sha: "abc".into()
            })
        );
        assert!(push.fork);
        let labeled = parse("pull_request", &pr("labeled", false, "o/r"), "b").expect("ok");
        assert_eq!(labeled.signal, None);
    }

    #[test]
    fn bot_comments_never_act_and_commands_need_the_prefix() {
        let comment = |login: &str, body: &str| {
            json!({"action": "created", "sender": {"login": login},
                "issue": {"number": 7, "pull_request": {}}, "comment": {"body": body}})
            .to_string()
        };
        let bot = parse(
            "issue_comment",
            comment("Sych[bot]", "/sych rerun").as_bytes(),
            "sych[bot]",
        );
        assert_eq!(bot.expect("ok").signal, Some(ChangeSignal::BotComment));
        let cmd = parse(
            "issue_comment",
            comment("dev", "please\n/sych rerun").as_bytes(),
            "sych[bot]",
        );
        assert_eq!(
            cmd.expect("ok").signal,
            Some(ChangeSignal::Command {
                actor: "dev".into(),
                command: ScmCommand::Rerun
            })
        );
        let prose = parse(
            "issue_comment",
            comment("dev", "rerun it").as_bytes(),
            "sych[bot]",
        );
        assert_eq!(prose.expect("ok").signal, None);
    }

    #[test]
    fn unknown_events_are_refused() {
        assert_eq!(
            parse("push", b"{}", "b"),
            Err(Rejection::Event("push".into()))
        );
    }
}
