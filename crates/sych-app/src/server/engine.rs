//! The scheduler half of the server (spec §19.3, §8.3–8.5): due intents become decisions over
//! the API state of the change; open changes are reconciled after downtime and periodically.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sych_adapters::github::{Github, PullState, Secret, capabilities};
use sych_core::Timestamp;
use sych_core::config::{GithubConnection, RunMode};
use sych_core::schedule::{
    CI_WAIT_SECONDS, Decision, IntentKind, MAX_RETRIES, RunInputs, ScmState, decide, retry_at,
};
use sych_core::scm::{ChangeSignal, RepositoryRef, ScmError, WebhookEvent};
use sych_core::{canonical_json, sha256};
use sych_storage::{ChangeRow, Connection, Intent, StoreConfig};
use tokio::sync::watch;
use tokio::{task, time};

use super::App;
use crate::args::{ModelSelection, Switch};

/// Bound of one analysis job once its evidence is due: active deadline and slack.
pub(super) const AFTER_EVIDENCE_SECONDS: i64 = 300 + 120;
/// Default bound of one analysis job, retries included: the default CI wait, then the above.
/// PREPARE extends it to the CI wait the target's policy asks for.
pub(super) const JOB_SECONDS: i64 = CI_WAIT_SECONDS as i64 + AFTER_EVIDENCE_SECONDS;
/// Write access lets an actor run commands (spec §22.5).
const COMMAND_PERMISSIONS: &[&str] = &["admin", "maintain", "write"];

/// One GitHub connection with its client.
pub struct Conn {
    pub config: GithubConnection,
    pub gh: Github,
    pub binding: Connection,
}

/// How server runs review: the trusted config file and the model and semantic settings.
#[derive(Clone)]
pub struct ReviewSettings {
    pub config: PathBuf,
    pub models: ModelSelection,
    pub semantic: Switch,
    /// `personal_service` or `corporate`: never the local-only mode of a signed-in owner.
    pub mode: RunMode,
}

pub struct Engine {
    pub app: Arc<App>,
    pub conns: BTreeMap<String, Conn>,
    pub store: StoreConfig,
    pub review: ReviewSettings,
    pub owner: String,
    pub lease_seconds: i64,
    /// Interval of lease renewals while a job is held (`workers.heartbeat_seconds`).
    pub heartbeat: Duration,
    /// `http.public_base_url`, for links to full results.
    pub public_base_url: Option<String>,
    pub now: fn() -> Timestamp,
}

pub fn binding(config: &GithubConnection) -> Connection {
    Connection {
        tenant_id: config.tenant_id.clone(),
        connection_id: config.id.clone(),
        provider_instance: config.provider_instance.clone(),
    }
}

pub(super) fn scm_state(pull: &PullState, full_name: &str) -> ScmState {
    ScmState {
        open: pull.open,
        merged: pull.merged,
        draft: pull.draft,
        head_sha: pull.head_sha.clone(),
        base_sha: pull.base_sha.clone(),
        base_ref: pull.base_ref.clone(),
        fork: pull.fork(full_name),
    }
}

impl Engine {
    pub(super) async fn token(&self, conn: &Conn, change: &ChangeRow) -> Result<Secret, ScmError> {
        let now = (self.now)().as_second();
        let installation = match change.installation_id {
            Some(id) => id,
            None => conn.gh.installation(&change.full_name, now).await?.id,
        };
        conn.gh.token(installation, &change.full_name, now).await
    }

    /// Handles every due intent once; returns how many it handled.
    pub async fn schedule_once(&self) -> anyhow::Result<usize> {
        let now = (self.now)();
        let due: Vec<Intent> = self
            .app
            .storage
            .open_intents()
            .await?
            .into_iter()
            .filter(|i| i.due_at <= now.as_second())
            .collect();
        for intent in &due {
            self.handle(intent).await?;
        }
        Ok(due.len())
    }

    async fn handle(&self, intent: &Intent) -> anyhow::Result<()> {
        let storage = &self.app.storage;
        let now = (self.now)();
        let change = storage.change(intent.change_id).await?;
        let Some(conn) = self.conns.get(&change.connection_id) else {
            storage
                .log_event(change.id, None, "SKIPPED", "CONNECTION_REMOVED", now)
                .await?;
            return Ok(storage.consume_intent(intent.id, now).await?);
        };
        let Some(kind) = IntentKind::parse(&intent.kind) else {
            return Ok(storage.consume_intent(intent.id, now).await?);
        };
        let read = async {
            let token = self.token(conn, &change).await?;
            let pull = conn
                .gh
                .pull(&change.full_name, change.number, &token)
                .await?;
            let actor = match kind {
                IntentKind::Rerun | IntentKind::Deep | IntentKind::Stop => {
                    self.authorized_actor(conn, &change, intent, &token).await?
                }
                _ => None,
            };
            Ok::<_, ScmError>((pull, actor))
        };
        let (pull, actor) = match read.await {
            Ok(read) => read,
            Err(e) => {
                let attempt = u32::try_from(intent.events)
                    .unwrap_or(u32::MAX)
                    .min(MAX_RETRIES);
                let at = now.as_second();
                let retry = retry_at(
                    e.kind,
                    attempt,
                    e.retry_after_seconds,
                    0,
                    at,
                    at + JOB_SECONDS,
                );
                storage
                    .log_event(change.id, None, "SCM_READ_FAILED", &e.to_string(), now)
                    .await?;
                match retry {
                    Some(due) => storage.postpone_intent(intent.id, due).await?,
                    None => storage.consume_intent(intent.id, now).await?,
                }
                return Ok(());
            }
        };
        let state = scm_state(&pull, &change.full_name);
        storage.sync_change(change.id, &state, now).await?;
        let latest = storage.latest(change.id).await?;
        let decision = decide(kind, &state, latest.as_ref(), actor.as_deref());
        tracing::info!(change = change.id, intent = %intent.kind, ?decision, "intent decided");
        match &decision {
            Decision::Skip { reason } => {
                storage
                    .log_event(change.id, None, "SKIPPED", reason, now)
                    .await?;
            }
            Decision::Generate { inputs } => {
                let deadline = now.as_second() + JOB_SECONDS;
                let inputs = RunInputs {
                    publish: conn.config.publish,
                    checks: conn.config.publish && conn.config.checks,
                    ..inputs.clone()
                };
                storage
                    .generate(change.id, &inputs, deadline, MAX_RETRIES + 1, now)
                    .await?;
            }
            Decision::Cancel { reason } => {
                storage.cancel_change(change.id, reason, now).await?;
            }
            Decision::Wake { run_id } => {
                storage.wake(run_id, now).await?;
                storage
                    .log_event(change.id, Some(run_id), "WOKEN", "CI_COMPLETED", now)
                    .await?;
            }
        }
        Ok(storage.consume_intent(intent.id, now).await?)
    }

    /// The command's author if they have write access; refusals are logged, not answered.
    async fn authorized_actor(
        &self,
        conn: &Conn,
        change: &ChangeRow,
        intent: &Intent,
        token: &Secret,
    ) -> Result<Option<String>, ScmError> {
        let event = self.app.storage.inbox_event(intent.last_inbox_id).await;
        let Ok(WebhookEvent {
            signal: Some(ChangeSignal::Command { actor, .. }),
            ..
        }) = event
        else {
            return Ok(None);
        };
        let level = conn.gh.permission(&change.full_name, &actor, token).await?;
        Ok(COMMAND_PERMISSIONS
            .contains(&level.as_str())
            .then_some(actor))
    }

    /// Binds every listed repository with its capabilities and turns the provider's open
    /// changes into intents; a change believed open but no longer listed gets a close intent.
    /// Synthetic delivery ids make a repeat a no-op.
    pub async fn reconcile_once(&self) -> anyhow::Result<()> {
        let storage = &self.app.storage;
        let now = (self.now)();
        for conn in self.conns.values() {
            for full_name in &conn.config.repositories {
                let read = async {
                    let installation = conn.gh.installation(full_name, now.as_second()).await?;
                    let token = conn
                        .gh
                        .token(installation.id, full_name, now.as_second())
                        .await?;
                    let repo = conn.gh.repository(full_name, &token).await?;
                    let pulls = conn.gh.open_pulls(full_name, &token).await?;
                    Ok::<_, ScmError>((installation, repo, pulls))
                };
                let (installation, repo, pulls) = match read.await {
                    Ok(read) => read,
                    Err(e) => {
                        tracing::warn!(repository = %full_name, error = %e, "reconciliation skipped");
                        continue;
                    }
                };
                let caps = capabilities(&installation.permissions);
                let missing = caps.missing(conn.config.checks);
                if !missing.is_empty() {
                    tracing::warn!(repository = %full_name, ?missing, "connection lacks capabilities");
                }

                storage
                    .bind_repository(&conn.binding, &repo, installation.id, &caps, now)
                    .await?;
                let open: Vec<u64> = pulls.iter().map(|p| p.number).collect();
                for pull in &pulls {
                    let delivery = format!(
                        "reconcile:{}:{}:{}",
                        pull.number, pull.head_sha, pull.base_sha
                    );
                    self.synthetic(
                        conn,
                        &repo,
                        installation.id,
                        pull.number,
                        &delivery,
                        ChangeSignal::Opened,
                    )
                    .await?;
                }
                for change in storage.open_changes(&conn.config.id).await? {
                    if change.full_name == repo.full_name && !open.contains(&change.number) {
                        let delivery =
                            format!("reconcile-closed:{}:{}", change.number, now.as_second());
                        let signal = ChangeSignal::Closed { merged: false };
                        self.synthetic(
                            conn,
                            &repo,
                            installation.id,
                            change.number,
                            &delivery,
                            signal,
                        )
                        .await?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn synthetic(
        &self,
        conn: &Conn,
        repo: &RepositoryRef,
        installation: u64,
        number: u64,
        delivery: &str,
        signal: ChangeSignal,
    ) -> anyhow::Result<()> {
        let event = WebhookEvent {
            event: "reconcile".into(),
            action: None,
            repository: Some(repo.clone()),
            installation_id: Some(installation),
            change_numbers: vec![number],
            fork: false,
            sender: None,
            signal: Some(signal),
        };
        let digest = sha256(&canonical_json(&event)?);
        self.app
            .storage
            .admit(&conn.binding, delivery, &digest, &event, true, (self.now)())
            .await?;
        Ok(())
    }
}

/// Reconciliation period after the startup pass.
const RECONCILE_SECONDS: i64 = 600;
/// Reaper period (spec §8.2 default).
const REAP_SECONDS: i64 = 15;

impl Engine {
    /// Scheduler with reconcile and reaper on one task, the worker on another, until `stop`.
    pub async fn run(self: Arc<Self>, stop: watch::Receiver<bool>) {
        let engine = self;
        let scheduler = {
            let (engine, mut stop) = (Arc::clone(&engine), stop.clone());
            task::spawn_local(async move {
                let (mut reconciled, mut reaped, mut verified) = (i64::MIN, i64::MIN, i64::MIN);
                let mut tick = time::interval(Duration::from_secs(1));
                loop {
                    tokio::select! {
                        _ = stop.changed() => break,
                        _ = tick.tick() => {}
                    }
                    let now = (engine.now)().as_second();
                    if now.saturating_sub(reconciled) >= RECONCILE_SECONDS {
                        reconciled = now;
                        report(engine.reconcile_once().await, "reconcile");
                    }

                    if now.saturating_sub(reaped) >= REAP_SECONDS {
                        reaped = now;
                        report(engine.reap_once().await, "reap");
                    }

                    if now.saturating_sub(verified) >= RECONCILE_SECONDS {
                        verified = now;
                        report(
                            engine.verify_published_once().await.map(drop),
                            "verify publications",
                        );
                    }

                    report(engine.schedule_once().await.map(drop), "schedule");
                }
            })
        };
        let worker = {
            let (engine, mut stop) = (Arc::clone(&engine), stop);
            task::spawn_local(async move {
                let mut tick = time::interval(Duration::from_secs(1));
                loop {
                    tokio::select! {
                        _ = stop.changed() => break,
                        _ = tick.tick() => {}
                    }
                    while !*stop.borrow() {
                        match engine.publish_once().await {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(e) => report::<()>(Err(e), "publish"),
                        }
                        match engine.work_once().await {
                            Ok(true) => {}
                            Ok(false) => break,
                            Err(e) => {
                                report::<()>(Err(e), "work");
                                break;
                            }
                        }
                    }
                }
            })
        };
        let _ = tokio::join!(scheduler, worker);
    }
}

impl Engine {
    /// Expired job leases back to the queue, lost publishing leases to `UNCERTAIN`, and one
    /// reconciliation pass over uncertain publications.
    pub async fn reap_once(&self) -> anyhow::Result<()> {
        let storage = &self.app.storage;
        storage.reap((self.now)()).await?;
        storage
            .reap_publications(super::publisher::UNCERTAIN_WINDOW, (self.now)())
            .await?;
        self.lineage_once().await?;
        self.reconcile_publications_once().await
    }
}

fn report<T>(outcome: anyhow::Result<T>, step: &str) {
    if let Err(e) = outcome {
        tracing::warn!(step, error = %format!("{e:#}"), "server step failed");
    }
}
