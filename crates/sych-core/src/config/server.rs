//! Server config `sych.yaml` (spec §23.1–23.2): the trusted upper limits for access, budgets,
//! models and runners. Repository policy can only tighten them.

use serde::{Deserialize, Serialize};

use super::{
    ConfigError, Execution, Http, Models, Problems, Redaction, Scm, Tooling, endpoint, parse_yaml,
};
use crate::{MicroUsd, SchemaVersion};

/// Name of an environment variable that holds a secret; the value is never stored here.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EnvName(String);

impl EnvName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EnvName {
    type Error = &'static str;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        if super::is_env_name(&name) {
            Ok(Self(name))
        } else {
            // The rejected text may be a pasted secret, so it is not echoed.
            Err("expected an UPPER_SNAKE_CASE environment variable name, not a value")
        }
    }
}

impl From<EnvName> for String {
    fn from(name: EnvName) -> Self {
        name.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Personal,
    Corporate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub schema_version: SchemaVersion,
    pub mode: Mode,
    #[serde(default)]
    pub storage: Storage,
    #[serde(default)]
    pub http: Http,
    #[serde(default)]
    pub workers: Workers,
    #[serde(default)]
    pub models: Models,
    #[serde(default)]
    pub budgets: Budgets,
    #[serde(default)]
    pub execution: Execution,
    #[serde(default)]
    pub tooling: Tooling,
    #[serde(default)]
    pub publication: Publication,
    #[serde(default)]
    pub logging: Logging,
    #[serde(default)]
    pub redaction: Redaction,
    #[serde(default)]
    pub scm: Scm,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Storage {
    pub database_url_env: Option<EnvName>,
    pub artifact_store: Option<ArtifactStore>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStoreKind {
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactStore {
    pub kind: ArtifactStoreKind,
    /// Host path, relative to the config file.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Workers {
    pub concurrency: u32,
    pub lease_seconds: u32,
    pub heartbeat_seconds: u32,
}

impl Default for Workers {
    fn default() -> Self {
        Self {
            concurrency: 1,
            lease_seconds: 60,
            heartbeat_seconds: 15,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Budget {
    pub max_cost_micro_usd: Option<MicroUsd>,
    pub max_attempts: Option<u32>,
    pub max_input_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    /// Output cap of one request, reasoning included; 2 000 when unset (spec §17.1).
    pub max_output_tokens_per_request: Option<u32>,
    pub max_parallel_calls: Option<u32>,
    pub active_deadline_seconds: Option<u32>,
}

/// Defaults are the `economical` caps of spec §17.1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Budgets {
    pub analysis: Budget,
    pub tour: Budget,
    pub followup_session: Budget,
    pub tenant_daily: Budget,
    pub tenant_monthly: Budget,
}

fn cost(micro_usd: u64) -> Budget {
    Budget {
        max_cost_micro_usd: Some(MicroUsd(micro_usd)),
        ..Budget::default()
    }
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            analysis: Budget {
                max_attempts: Some(12),
                max_input_tokens: Some(512_000),
                max_output_tokens: Some(24_000),
                max_parallel_calls: Some(2),
                active_deadline_seconds: Some(300),
                ..cost(600_000)
            },
            tour: Budget {
                max_attempts: Some(2),
                ..cost(100_000)
            },
            followup_session: cost(150_000),
            tenant_daily: cost(3_000_000),
            tenant_monthly: cost(60_000_000),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationMode {
    #[default]
    Advisory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Publication {
    pub mode: PublicationMode,
    pub max_new_inline_comments: u32,
    pub publish_check: bool,
}

impl Default for Publication {
    fn default() -> Self {
        Self {
            mode: PublicationMode::Advisory,
            max_new_inline_comments: 8,
            publish_check: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Logging {
    pub raw_prompts: bool,
    pub raw_responses: bool,
}

fn positive(p: &mut Problems<'_>, path: &str, value: Option<u64>) {
    p.check(value != Some(0), path, "must be greater than zero");
}

impl ServerConfig {
    /// Parses and validates `sych.yaml`; returns every problem found.
    pub fn parse(file: &str, text: &str) -> Result<Self, Vec<ConfigError>> {
        let config: Self = parse_yaml(file, text).map_err(|e| vec![e])?;
        let errors = config.validate(file);
        if errors.is_empty() {
            Ok(config)
        } else {
            Err(errors)
        }
    }

    /// Whether a request URL is inside a configured provider endpoint (spec §22.3).
    pub fn egress_allowed(&self, url: &str) -> bool {
        self.models
            .providers
            .values()
            .filter_map(|p| p.endpoint.as_deref())
            .any(|endpoint| endpoint::inside(endpoint, url))
    }

    /// Every referenced environment variable with the key that names it; values stay unread.
    pub fn env_refs(&self) -> Vec<(String, &EnvName)> {
        let mut refs: Vec<(String, &EnvName)> = Vec::new();
        if let Some(env) = &self.storage.database_url_env {
            refs.push(("storage.database_url_env".into(), env));
        }

        for (name, p) in &self.models.providers {
            for (key, env) in [
                ("api_key_env", &p.api_key_env),
                ("connection_ref_env", &p.connection_ref_env),
                ("executable_env", &p.executable_env),
            ] {
                if let Some(env) = env {
                    refs.push((format!("models.providers.{name}.{key}"), env));
                }
            }
        }
        for (name, p) in &self.models.profiles {
            if let Some(env) = &p.model_env {
                refs.push((format!("models.profiles.{name}.model_env"), env));
            }
        }
        refs.extend(self.scm.env_refs());
        refs.extend(self.http.env_refs());
        refs
    }

    pub fn validate(&self, file: &str) -> Vec<ConfigError> {
        let mut p = Problems::new(file);
        let w = &self.workers;
        p.check(
            w.concurrency > 0,
            "workers.concurrency",
            "must be greater than zero",
        );
        p.check(
            w.heartbeat_seconds > 0 && w.heartbeat_seconds < w.lease_seconds,
            "workers.heartbeat_seconds",
            "must be greater than zero and shorter than lease_seconds",
        );
        self.http.validate(self.mode, &mut p);
        self.validate_models(&mut p);
        self.validate_budgets(&mut p);
        self.scm.validate(&mut p);
        for (field, problem) in self.execution.problems() {
            p.check(false, field, problem);
        }
        for (field, problem) in self.tooling.problems() {
            p.check(false, field, problem);
        }
        if let Err(e) = self.redaction.redactor() {
            let (field, problem) = e.split_once(": ").unwrap_or(("", &e));
            p.push(format!("redaction.{field}"), problem);
        }

        p.errors
    }

    fn validate_budgets(&self, p: &mut Problems<'_>) {
        let b = &self.budgets;
        for (name, budget) in [
            ("analysis", &b.analysis),
            ("tour", &b.tour),
            ("followup_session", &b.followup_session),
            ("tenant_daily", &b.tenant_daily),
            ("tenant_monthly", &b.tenant_monthly),
        ] {
            let path = |field: &str| format!("budgets.{name}.{field}");
            positive(
                p,
                &path("max_cost_micro_usd"),
                budget.max_cost_micro_usd.map(|c| c.0),
            );
            for (field, value) in [
                ("max_attempts", budget.max_attempts),
                ("max_input_tokens", budget.max_input_tokens),
                ("max_output_tokens", budget.max_output_tokens),
                (
                    "max_output_tokens_per_request",
                    budget.max_output_tokens_per_request,
                ),
                ("max_parallel_calls", budget.max_parallel_calls),
                ("active_deadline_seconds", budget.active_deadline_seconds),
            ] {
                positive(p, &path(field), value.map(u64::from));
            }
        }
        for (name, budget) in [("analysis", &b.analysis), ("tour", &b.tour)] {
            let fits = budget
                .max_output_tokens_per_request
                .zip(budget.max_output_tokens)
                .is_none_or(|(request, run)| request <= run);
            p.check(
                fits,
                format!("budgets.{name}.max_output_tokens_per_request"),
                "exceeds max_output_tokens of the run",
            );
        }

        let cap = |budget: &Budget| budget.max_cost_micro_usd;
        let within = |inner: Option<MicroUsd>, outer: Option<MicroUsd>| {
            inner.zip(outer).is_none_or(|(i, o)| i <= o)
        };
        for (name, budget) in [
            ("analysis", &b.analysis),
            ("tour", &b.tour),
            ("followup_session", &b.followup_session),
        ] {
            p.check(
                within(cap(budget), cap(&b.tenant_daily)),
                format!("budgets.{name}.max_cost_micro_usd"),
                "exceeds budgets.tenant_daily.max_cost_micro_usd",
            );
        }
        p.check(
            within(cap(&b.tenant_daily), cap(&b.tenant_monthly)),
            "budgets.tenant_daily.max_cost_micro_usd",
            "exceeds budgets.tenant_monthly.max_cost_micro_usd",
        );
    }
}
