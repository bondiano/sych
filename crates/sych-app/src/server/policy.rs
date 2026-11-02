//! `GET /v1/repositories/:id/policy`: the policy of a repository's newest run that pinned one.
//! The snapshot keeps only its source and hashes, so `.sych/` is read again from the mirror at
//! that source and shown as `sych config validate --repo` prints it. Read only: policy changes
//! by a pull request, never here.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::Value;
use sych_adapters::git::{Git, read_policy_files};
use sych_core::api::RepositoryPolicyView;
use sych_core::config::{ApiRole, PolicyFiles};
use sych_core::{PolicySnapshot, PolicySource, RunId};
use sych_storage::StorageError;

use super::api::{Caller, engine, status, storage_error};
use super::error::ApiError;
use super::views::{grants, not_found};
use super::worker::blocking;
use super::{App, mirrors};
use crate::config;

fn unavailable(message: String) -> ApiError {
    status(StatusCode::CONFLICT, "POLICY_UNAVAILABLE", message)
}

pub async fn policy(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<i64>,
) -> Result<Json<RepositoryPolicyView>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let repository = grants(&app, &caller)
        .await?
        .into_iter()
        .map(|g| g.repository)
        .find(|r| r.id == id)
        .ok_or_else(not_found)?;
    let tenant = &caller.tenant_id;
    let run = app
        .storage
        .latest_policy_run(tenant, id)
        .await
        .map_err(|e| storage_error(&e))?
        .and_then(|run| run.parse().ok())
        .map(RunId)
        .ok_or_else(|| {
            let message = "the repository has no run with a pinned policy yet";
            status(StatusCode::NOT_FOUND, "NO_RUN", message)
        })?;
    let snapshot: PolicySnapshot = match app
        .storage
        .load_artifact(tenant, run, "policy_snapshot")
        .await
    {
        Ok(snapshot) => snapshot,
        Err(e @ StorageError::ArtifactUnavailable(_)) => return Err(unavailable(e.to_string())),
        Err(e) => return Err(storage_error(&e)),
    };
    let config = engine(&app)?.review.config.clone();
    let mirror = mirrors::path(
        &app.artifacts,
        &repository.connection_id,
        &repository.full_name,
    );
    let source = snapshot.source.clone();
    let effective = blocking(move || {
        let (files, origin) = match source {
            PolicySource::EngineDefaults => (PolicyFiles::default(), String::new()),
            PolicySource::TrustedRef { ref_name, commit } => {
                let files = Git::open(&mirror)
                    .and_then(|git| read_policy_files(&git, commit.hex()))
                    .map_err(|e| {
                        // The detail names host paths; the caller learns only what is missing.
                        tracing::warn!(error = %e, "cannot read the policy from the mirror");
                        unavailable(format!(
                            "the mirror has no {commit}; the repository's next run fetches it again"
                        ))
                    })?;
                (files, format!("{ref_name}@{commit}"))
            }
        };
        config::with_policy(Some(&config), &files, &origin)
            .map_err(|e| status(StatusCode::CONFLICT, "POLICY_INVALID", format!("{e:#}")))
    })
    .await
    .map_err(|e| storage_error(&e))??;
    let matches_snapshot = effective.rules_hash == snapshot.rules_hash
        && effective.config_hash == snapshot.config_hash;
    let mut shown = config::redacted(&effective).map_err(|e| storage_error(&e))?;
    // The server layer names settings beyond this repository: an operator's `/v1/admin/config`.
    if let Some(doc) = shown.as_object_mut() {
        doc.remove("server");
        // Contract texts are the repository's own files; `contracts` keeps their hashes.
        if let Some(Value::Object(repository)) = doc.get_mut("repository") {
            repository.remove("contract_texts");
        }

        if let Some(Value::Array(layers)) = doc.get_mut("provenance") {
            layers.retain(|l| l["layer"] != "server");
        }
    }

    Ok(Json(RepositoryPolicyView {
        repository,
        run_id: run.to_string(),
        snapshot,
        matches_snapshot,
        effective: shown,
    }))
}
