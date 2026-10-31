//! `sych serve`: one process with webhook admission and health; the
//! scheduler, worker and publisher join it as they land.

pub mod api;
mod auth;
pub mod build;
mod caller;
mod ci;
mod engine;
mod error;
pub mod findings;
mod handoff;
mod hooks;
pub mod metrics;
pub mod mirrors;
mod publisher;
mod questions;
pub mod requests;
pub mod smoke;
mod tours;
pub mod ui;
pub mod views;
mod worker;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::future::pending;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use sych_adapters::github::Secret;
use sych_core::Timestamp;
use sych_core::config::{ApiClient, GithubConnection, ServerConfig};
use sych_storage::Storage;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::watch;
use tokio::task::{self, LocalSet};

pub use engine::{Conn, Engine, JOB_SLACK_SECONDS, ReviewSettings, binding};
pub use error::ApiError;
pub use metrics::Counters;

/// Shared state of the HTTP handlers; secrets are read once at start.
pub struct App {
    pub storage: Storage,
    pub github: BTreeMap<String, (GithubConnection, Secret)>,
    /// `/v1` callers with their tokens.
    pub clients: Vec<(ApiClient, Secret)>,
    pub metrics_allow: Vec<IpAddr>,
    pub counters: Counters,
    /// The engine, in roles that run one; `/v1` requests that start work need it.
    pub engine: OnceLock<Arc<Engine>>,
    /// The artifact directory readiness checks.
    pub artifacts: PathBuf,
    pub now: fn() -> Timestamp,
    /// `http.public_base_url`: the origin a browser session's changes must come from.
    pub public_base_url: Option<String>,
    /// OAuth client secrets of the connections people sign in through.
    pub oauth: BTreeMap<String, Secret>,
    /// The web UI's build, served at `/ui`.
    pub ui_dir: Option<PathBuf>,
}

impl App {
    /// Reads every webhook secret; a missing one stops the start, it is never defaulted.
    pub fn new(
        config: &ServerConfig,
        storage: Storage,
        artifacts: PathBuf,
        now: fn() -> Timestamp,
    ) -> anyhow::Result<Self> {
        let mut github = BTreeMap::new();
        for conn in &config.scm.github {
            let name = conn.webhook_secret_env.as_str();
            let secret = env::var(name)
                .ok()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("{name} is not set (webhook secret of `{}`)", conn.id)
                })?;
            github.insert(conn.id.clone(), (conn.clone(), Secret::new(secret)));
        }
        let mut oauth = BTreeMap::new();
        for (conn, o) in config
            .scm
            .github
            .iter()
            .filter_map(|c| Some((c, c.oauth.as_ref()?)))
        {
            let name = o.client_secret_env.as_str();
            let secret = env::var(name)
                .ok()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("{name} is not set (OAuth client secret of `{}`)", conn.id)
                })?;
            oauth.insert(conn.id.clone(), Secret::new(secret));
        }
        if let Some(dir) = &config.http.ui_dir
            && !dir.join("index.html").is_file()
        {
            anyhow::bail!("http.ui_dir {} has no index.html", dir.display());
        }

        let mut clients = Vec::new();
        for client in &config.http.api_clients {
            let name = client.token_env.as_str();
            let token = env::var(name)
                .ok()
                .filter(|t| t.len() >= 16)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{name} is not set or shorter than 16 bytes (API client `{}`)",
                        client.name
                    )
                })?;
            clients.push((client.clone(), Secret::new(token)));
        }
        Ok(Self {
            storage,
            github,
            clients,
            metrics_allow: config.http.metrics_allow.clone(),
            counters: Counters::default(),
            engine: OnceLock::new(),
            artifacts,
            now,
            public_base_url: config.http.public_base_url.clone(),
            oauth,
            ui_dir: config.http.ui_dir.clone(),
        })
    }

    fn body_limit(&self) -> usize {
        self.github
            .values()
            .map(|(c, _)| c.max_body_bytes as usize)
            .max()
            .unwrap_or(0)
    }
}

async fn live() -> Json<serde_json::Value> {
    Json(json!({"status": "live"}))
}

/// Ready when the database answers and the artifact store is writable; nothing about config
/// or secrets in the reply.
async fn ready(State(app): State<Arc<App>>) -> (StatusCode, Json<serde_json::Value>) {
    let store = fs::create_dir_all(&app.artifacts)
        .and_then(|()| tempfile::NamedTempFile::new_in(&app.artifacts).map(drop));
    match (app.storage.ping().await, store) {
        (Ok(()), Ok(())) => (StatusCode::OK, Json(json!({"status": "ready"}))),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "not_ready"})),
        ),
    }
}

/// `/v1` request bodies are small JSON documents.
const API_BODY_LIMIT: usize = 1 << 20;

pub fn router(app: Arc<App>) -> Router {
    // One byte over the largest connection limit, so the handler reports 413 itself.
    let limit = app.body_limit() + 1;
    let v1 = Router::new()
        .route("/v1/reviews", post(api::create_review))
        .route("/v1/runs/{id}", get(api::get_run))
        .route("/v1/runs/{id}/result", get(api::get_result))
        .route("/v1/runs/{id}/cancel", post(api::cancel_run))
        .route("/v1/changes/{id}/reruns", post(api::rerun))
        .route("/v1/findings/{id}/feedback", post(api::feedback))
        .route("/v1/waivers", get(views::waivers).post(api::waiver))
        .route("/v1/waivers/{id}/revoke", post(views::revoke))
        .route("/v1/repositories", get(views::repositories))
        .route("/v1/changes", get(views::changes))
        .route("/v1/changes/{id}", get(views::change))
        .route("/v1/findings", get(views::findings))
        .route("/v1/findings/{id}", get(views::finding))
        .route("/v1/runs/{id}/attempts", get(views::attempts))
        .route("/v1/summary", get(views::summary))
        .route("/v1/usage", get(views::usage))
        .route("/v1/audit", get(views::audit))
        .route("/v1/me", get(auth::me))
        .route("/auth/github", get(auth::sign_ins))
        .route("/auth/github/{connection}/login", get(auth::login))
        .route("/auth/github/{connection}/callback", get(auth::callback))
        .route("/auth/token", post(auth::token))
        .route("/auth/logout", post(auth::logout))
        .route("/v1/publications/{id}", get(api::publication))
        .route("/v1/runs/{id}/handoff", post(handoff::handoff))
        .route("/v1/runs/{id}/tour", get(tours::get_tour))
        .route("/v1/runs/{id}/tours/{tour}/reveal", post(tours::reveal))
        .route("/v1/runs/{id}/tours/{tour}/complete", post(tours::complete))
        .route("/v1/runs/{id}/tours/{tour}/dismiss", post(tours::dismiss))
        .route("/v1/runs/{id}/tours/{tour}/questions", post(questions::ask))
        .layer(DefaultBodyLimit::max(API_BODY_LIMIT));
    let ui = app.ui_dir.as_deref().map(ui::routes).unwrap_or_default();
    Router::new()
        .route("/hooks/github/{connection}", post(hooks::github))
        .layer(DefaultBodyLimit::max(limit))
        .merge(v1)
        .merge(ui)
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/metrics", get(metrics::metrics))
        .with_state(app)
}

/// Which halves of the server a process runs: `serve` is both, `api` takes
/// requests and webhooks without claiming work, `worker` claims work and answers only health and
/// metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    All,
    Api,
    Worker,
}

/// Serves until Ctrl-C: admission stops first, then the engine stops claiming; a job in
/// flight keeps its lease until it expires and the reaper hands it to the next start.
pub async fn serve(
    app: Arc<App>,
    bind: SocketAddr,
    engine: Option<Engine>,
    role: Role,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    let metrics_listener = match env::var("SYCH_METRICS_BIND") {
        Ok(address) => Some(TcpListener::bind(address.parse::<SocketAddr>()?).await?),
        Err(env::VarError::NotPresent) => None,
        Err(error) => return Err(error.into()),
    };
    tracing::info!(%bind, ?role, "serving");
    let (stop_tx, stop_rx) = watch::channel(false);
    let engine = engine.map(Arc::new);
    if let Some(engine) = &engine {
        let _ = app.engine.set(Arc::clone(engine));
    }

    let routes = match role {
        Role::Worker => Router::new()
            .route("/health/live", get(live))
            .route("/health/ready", get(ready))
            .route("/metrics", get(metrics::metrics))
            .with_state(Arc::clone(&app)),
        Role::All | Role::Api => router(Arc::clone(&app)),
    };
    let local = LocalSet::new();
    local
        .run_until(async move {
            let loops = engine.filter(|_| role != Role::Api);
            let background = loops.map(|e| task::spawn_local(e.run(stop_rx)));
            let metrics = async {
                if let Some(listener) = metrics_listener {
                    let routes = Router::new()
                        .route("/metrics", get(metrics::scrape))
                        .with_state(app);
                    axum::serve(listener, routes).await?;
                    anyhow::bail!("metrics listener stopped");
                }

                pending::<anyhow::Result<()>>().await
            };
            let result = tokio::select! {
                result = axum::serve(
                    listener,
                    routes.into_make_service_with_connect_info::<SocketAddr>(),
                ).with_graceful_shutdown(async {
                    signal::ctrl_c().await.ok();
                }) => result.map_err(anyhow::Error::from),
                result = metrics => result,
            };
            let _ = stop_tx.send(true);
            if let Some(background) = background {
                background.await?;
            }

            result
        })
        .await
}
