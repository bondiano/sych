//! `/v1/repositories/:id/suggestions`: rule suggestions computed on every read from the feedback
//! of the last 90 days against the repository's current policy, and a maintainer's decision
//! about one. Sych never edits `.sych/`: a suggestion is applied by a person's pull request.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use sych_core::api::{Repository, Suggestion, SuggestionDecision};
use sych_core::config::ApiRole;
use sych_core::suggest::{WINDOW_DAYS, suggest};
use sych_storage::Decision;

use super::App;
use super::api::{Caller, idempotent, now, parse, storage_error, to_value};
use super::error::ApiError;
use super::policy::current;
use super::views::{DAY, invalid, not_found};

/// The repository's suggestions with the decisions recorded about them.
async fn computed(
    app: &App,
    caller: &Caller,
    id: i64,
) -> Result<(Repository, Vec<Suggestion>), ApiError> {
    let (repository, _, _, effective) = current(app, caller, id).await?;
    let at = now(app);
    let s = &app.storage;
    let judged = s
        .judged_findings(&caller.tenant_id, id, at.as_second() - WINDOW_DAYS * DAY)
        .await
        .map_err(|e| storage_error(&e))?;
    let decisions = s
        .suggestion_decisions(&caller.tenant_id, id)
        .await
        .map_err(|e| storage_error(&e))?;
    let author = caller
        .actor
        .strip_prefix("github:")
        .unwrap_or(&caller.actor);
    let mut suggestions = suggest(id, &effective.repository.rules, &judged, author, at);
    for s in &mut suggestions {
        if let Some(d) = decisions.iter().find(|d| d.fingerprint == s.fingerprint) {
            s.state.clone_from(&d.state);
            s.pull_request.clone_from(&d.pull_request);
        }
    }
    Ok((repository, suggestions))
}

/// `GET /v1/repositories/:id/suggestions`.
pub async fn list(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Suggestion>>, ApiError> {
    Ok(Json(computed(&app, &caller, id).await?.1))
}

/// A pull request of `full_name`: `https://<host>/<owner>/<name>/pull/<number>`.
fn pull_request_of(url: &str, full_name: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let Some((_, path)) = rest.split_once('/') else {
        return false;
    };
    path.strip_prefix(full_name)
        .and_then(|p| p.strip_prefix("/pull/"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// `POST /v1/repositories/:id/suggestions/:fingerprint/decision` (maintainer).
pub async fn decide(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, fingerprint)): Path<(i64, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    caller.need(ApiRole::Maintainer)?;
    let r: SuggestionDecision = parse(&body)?;
    match (r.state.as_str(), &r.pull_request) {
        ("DISMISSED", Some(_)) => {
            return Err(invalid("a dismissed suggestion has no pull request"));
        }
        ("DISMISSED" | "DONE", _) => {}
        _ => return Err(invalid("state is DISMISSED or DONE")),
    }
    let pull_request = r.pull_request;
    let (repository, suggestions) = computed(&app, &caller, id).await?;
    let mut suggestion = suggestions
        .into_iter()
        .find(|s| s.fingerprint == fingerprint)
        .ok_or_else(not_found)?;
    if pull_request
        .as_deref()
        .is_some_and(|url| !pull_request_of(url, &repository.full_name))
    {
        return Err(invalid(
            "pull_request is a pull request URL of this repository",
        ));
    }

    caller
        .scope(
            &app,
            Some(&repository.connection_id),
            Some(&repository.full_name),
            ApiRole::Maintainer,
        )
        .await?;
    idempotent(
        &app,
        &caller,
        "POST /v1/repositories/:id/suggestions/:fingerprint/decision",
        &format!("{id}/{fingerprint}"),
        &headers,
        &body,
        || async {
            let decision = Decision {
                fingerprint,
                state: r.state,
                pull_request,
            };
            app.storage
                .decide_suggestion(
                    &caller.tenant_id,
                    id,
                    &decision,
                    &caller.actor,
                    now(&app).as_second(),
                )
                .await
                .map_err(|e| storage_error(&e))?;
            suggestion.state = decision.state;
            suggestion.pull_request = decision.pull_request;
            Ok((StatusCode::OK, to_value(&suggestion)?))
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::pull_request_of;

    #[test]
    fn only_a_pull_request_of_the_repository_is_accepted() {
        assert!(pull_request_of("https://github.com/o/r/pull/12", "o/r"));
        assert!(pull_request_of("https://ghe.example.com/o/r/pull/3", "o/r"));
        for url in [
            "http://github.com/o/r/pull/12",
            "https://github.com/o/rx/pull/12",
            "https://github.com/o/r/pull/12/files",
            "https://github.com/o/r/issues/12",
            "https://github.com/o/r/pull/",
        ] {
            assert!(!pull_request_of(url, "o/r"), "{url}");
        }
    }
}
