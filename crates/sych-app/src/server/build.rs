//! Building the engine from the trusted config, shared by `serve`, `api`, `worker` and the
//! server-backed CLI commands.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::Context;
use sych_adapters::github::{AppKey, Github};
use sych_core::Timestamp;
use sych_core::config::{ApiClient, ApiRole, Mode, RunMode, ServerConfig};
use sych_storage::{Storage, StoreConfig};
use uuid::Uuid;

use super::{App, Conn, Counters, Engine, ReviewSettings, binding};
use crate::args::{ModelSelection, Switch};
use crate::errors::Failure;

/// Every GitHub connection with its App key read from the referenced file.
pub fn connections(server: &ServerConfig) -> anyhow::Result<BTreeMap<String, Conn>> {
    let mut conns = BTreeMap::new();
    for c in &server.scm.github {
        // A self-review is done by the trusted build only, not by whatever was compiled here.
        if c.self_review && c.reviewer_commit.as_deref() != Some(crate::BUILD_COMMIT) {
            return Err(Failure::permission(format!(
                "connection `{}` reviews Sych itself and is pinned to reviewer {}, but this build is {}",
                c.id,
                c.reviewer_commit.as_deref().unwrap_or("?"),
                crate::BUILD_COMMIT
            ))
            .into());
        }

        let name = c.private_key_file_env.as_str();
        let path =
            env::var(name).with_context(|| format!("{name} is not set (App key of `{}`)", c.id))?;
        let pem = fs::read_to_string(&path)
            .with_context(|| format!("cannot read the App key of `{}`", c.id))?;
        let key = AppKey::from_pem(c.app_id, &pem)?;
        let gh = Github::new(&c.api_base_url, key)?;
        conns.insert(
            c.id.clone(),
            Conn {
                config: c.clone(),
                gh,
                binding: binding(c),
            },
        );
    }
    Ok(conns)
}

impl App {
    /// The state a CLI command needs: no webhook secrets, no API clients.
    pub fn local(storage: Storage, artifacts: PathBuf) -> Self {
        Self {
            storage,
            github: BTreeMap::new(),
            clients: Vec::new(),
            metrics_allow: Vec::new(),
            counters: Counters::default(),
            engine: OnceLock::new(),
            artifacts,
            now: Timestamp::now,
        }
    }
}

pub struct EngineConfig<'a> {
    pub config: &'a Path,
    pub server: &'a ServerConfig,
    pub store: StoreConfig,
    pub models: ModelSelection,
    pub semantic: Switch,
}

pub fn engine(app: Arc<App>, conns: BTreeMap<String, Conn>, c: EngineConfig<'_>) -> Engine {
    Engine {
        app,
        conns,
        store: c.store,
        review: ReviewSettings {
            config: c.config.to_path_buf(),
            models: c.models,
            semantic: c.semantic,
            mode: match c.server.mode {
                Mode::Personal => RunMode::PersonalService,
                Mode::Corporate => RunMode::Corporate,
            },
        },
        owner: format!("worker-{}", Uuid::now_v7()),
        lease_seconds: i64::from(c.server.workers.lease_seconds),
        public_base_url: c.server.http.public_base_url.clone(),
        now: Timestamp::now,
    }
}

/// The configured identity a local `publish`, `rerun` or `cancel` acts as.
pub fn caller(server: &ServerConfig, name: &str, needed: ApiRole) -> anyhow::Result<ApiClient> {
    let client = server
        .http
        .api_clients
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| Failure::permission(format!("`{name}` is not a configured API client")))?;
    if client.role < needed {
        return Err(Failure::permission(format!(
            "`{name}` is {:?}; {needed:?} is needed",
            client.role
        ))
        .into());
    }

    Ok(client.clone())
}
