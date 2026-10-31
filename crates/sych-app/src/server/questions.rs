//! `/v1` follow-up questions on a tour step. A comprehension
//! question calls the model under the caller's own session budget, with the run's code read
//! from the engine's mirror; a fact is the caller's own statement; a risk decision records a
//! scoped waiver and needs the maintainer role. The exchange is the caller's alone: it never
//! changes the tour, the result, its obligations or statuses, and is never published.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use sych_core::analysis::followup::Reply;
use sych_core::api::QuestionRequest;
use sych_core::config::ApiRole;
use sych_core::tour::{QuestionKind, TourArtifact, TourStep};
use sych_storage::{RunView, Storage};

use super::api::{
    Caller, check_waiver, engine, idempotent, now, parse, record_waiver, status, storage_error,
    to_value, visible_run,
};
use super::error::ApiError;
use super::tours::{open, run_of, tour_id};
use super::{App, mirrors, worker};
use crate::errors::{EXIT_NO_RESULT, exit_code};
use crate::followup::{self, Asker};
use crate::storage::{block_on, report_of};
use crate::tour_reader;

const ENDPOINT: &str = "POST /v1/runs/:id/tours/:tour/questions";

fn invalid(message: impl Into<String>) -> ApiError {
    status(StatusCode::BAD_REQUEST, "INVALID_REQUEST", message)
}

/// `POST /v1/runs/:id/tours/:tour/questions`.
pub async fn ask(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, tour)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    caller.need(ApiRole::Reader)?;
    let request: QuestionRequest = parse(&body)?;
    if request.question.trim().is_empty() {
        return Err(invalid("the question is empty"));
    }

    let run_view = visible_run(&app, &caller, &id).await?;
    let terms = match (request.kind, &request.waiver) {
        (QuestionKind::RiskDecision, Some(terms)) => {
            check_waiver(&app, &caller, run_view.change_id, terms).await?;
            Some(terms)
        }
        (QuestionKind::RiskDecision, None) => {
            return Err(invalid("a risk decision needs waiver terms"));
        }
        (_, Some(_)) => return Err(invalid("waiver terms come only with RISK_DECISION")),
        (_, None) => None,
    };
    let run = run_of(&run_view)?;
    let tour = tour_id(&app, &caller, run, Some(&tour)).await?;
    let resource = format!("{id}/{tour}");
    idempotent(
        &app,
        &caller,
        ENDPOINT,
        &resource,
        &headers,
        &body,
        || async {
            let (t, _) = open(&app, &caller, run, tour).await?;
            let step = followup::step(&t, &request.step_id).map_err(invalid)?;
            let reply = match (request.kind, terms) {
                (QuestionKind::RiskDecision, Some(terms)) => Reply::RiskDecision {
                    waiver_id: record_waiver(&app, &caller, run_view.change_id, terms).await?,
                    rule_id: terms.rule_id.clone(),
                },
                (QuestionKind::FactRequest, _) => Reply::HumanStatement {
                    text: request.question.clone(),
                },
                _ => explain(&app, &caller, &run_view, (&t, step), &request.question).await?,
            };
            let exchange = followup::exchange(
                (&t, step),
                &caller.0.name,
                (request.kind, &request.question),
                now(&app),
                reply,
            );
            app.storage
                .add_exchange(&caller.0.tenant_id, run, &exchange)
                .await
                .map_err(|e| storage_error(&e))?;
            Ok((StatusCode::CREATED, to_value(&exchange)?))
        },
    )
    .await
}

/// A comprehension answer from the engine's models over the mirror's pinned blobs; the model
/// calls run on their own thread like analysis does.
async fn explain(
    app: &App,
    caller: &Caller,
    run_view: &RunView,
    (tour, step): (&TourArtifact, &TourStep),
    question: &str,
) -> Result<Reply, ApiError> {
    let engine = engine(app)?;
    let unavailable = |m: &str| status(StatusCode::CONFLICT, "SOURCE_UNAVAILABLE", m);
    let (Some(conn), Some(repo)) = (&run_view.connection_id, &run_view.full_name) else {
        return Err(unavailable(
            "the run has no repository to read its code from",
        ));
    };
    let mirror = mirrors::path(&engine.store.artifacts, conn, repo);
    let run = run_of(run_view)?;
    let tenant = caller.0.tenant_id.clone();
    let report = report_of(&app.storage, &tenant, run)
        .await
        .map_err(|e| status(StatusCode::CONFLICT, "NO_RESULT", e.to_string()))?;
    let reader = tour_reader::pinned(report.snapshot.as_ref(), &mirror)
        .ok_or_else(|| unavailable("the run's source is pruned or no longer in the mirror"))?;
    let (store, settings) = (engine.store.clone(), &engine.review);
    let (config, models, mode) = (
        settings.config.clone(),
        settings.models.clone(),
        settings.mode,
    );
    let (actor, tour, step) = (caller.0.name.clone(), tour.clone(), step.clone());
    let question = question.to_owned();
    let answered = worker::blocking(move || {
        block_on(async {
            let storage = Storage::open(&store).await?;
            let asker = Asker {
                config: &config,
                models: &models,
                mode,
                storage: &storage,
                tenant: &tenant,
                actor: &actor,
            };
            followup::explain(&asker, run, (&tour, &step), &report, &reader, &question).await
        })?
    })
    .await
    .map_err(|e| storage_error(&e))?;
    answered.map_err(|e| {
        if exit_code(&e) == EXIT_NO_RESULT {
            status(StatusCode::CONFLICT, "NO_ANSWER", format!("{e:#}"))
        } else {
            storage_error(&format!("{e:#}"))
        }
    })
}
