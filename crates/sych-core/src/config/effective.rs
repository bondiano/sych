//! Effective config: engine defaults, then the trusted server config, then the accepted
//! repository policy. The reviewed head is only diffed, never applied.

use std::collections::{BTreeMap, BTreeSet};
use std::str;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{
    CONTRACTS_DIR, ConfigError, DeploymentFile, Effect, EffectsFile, Mode, POLICY_DIR, Problems,
    REPO_CONFIG_FILE, RepoConfig, Rule, RulesFile, ServerConfig, parse_yaml,
};
use crate::{
    PolicySnapshot, PolicySnapshotId, PolicySource, RepoPath, SchemaVersion, Sha256Digest,
    Timestamp, canonical_sha256, redact, sha256,
};

/// Bumped when the rules for who may run, waive or publish change.
pub const AUTHORIZATION_POLICY_VERSION: u32 = 1;

/// The `.sych/` tree of one commit or directory, as raw bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyFiles {
    pub files: BTreeMap<RepoPath, Vec<u8>>,
    /// Entries that are not plain files (symlink, submodule) or too large, with the reason.
    pub unreadable: BTreeMap<RepoPath, String>,
}

/// Normalized accepted repository policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryPolicy {
    pub config: RepoConfig,
    pub rules: Vec<Rule>,
    pub effects: Vec<Effect>,
    pub deployment: Option<DeploymentFile>,
    /// Contract documents by path with their content hash.
    pub contracts: BTreeMap<RepoPath, Sha256Digest>,
    /// Their trusted text, for the obligations whose rules cite them; not UTF-8 is left out.
    #[serde(default)]
    pub contract_texts: BTreeMap<RepoPath, String>,
}

fn text<'a>(p: &mut Problems<'_>, bytes: &'a [u8]) -> Option<&'a str> {
    let decoded = str::from_utf8(bytes).ok();
    p.check(decoded.is_some(), ".", "policy file is not UTF-8");
    decoded
}

fn parse_file<T: DeserializeOwned>(
    files: &PolicyFiles,
    path: &RepoPath,
) -> Result<Option<T>, Vec<ConfigError>> {
    let Some(bytes) = files.files.get(path) else {
        return Ok(None);
    };
    let mut p = Problems::new(path.as_str());
    let Some(text) = text(&mut p, bytes) else {
        return Err(p.errors);
    };
    parse_yaml(path.as_str(), text)
        .map(Some)
        .map_err(|e| vec![e])
}

impl RepositoryPolicy {
    /// Parses and validates a `.sych/` tree against the server ceilings; collects every problem.
    pub fn parse(files: &PolicyFiles, server: &ServerConfig) -> Result<Self, Vec<ConfigError>> {
        let mut errors: Vec<ConfigError> = files
            .unreadable
            .iter()
            .map(|(path, reason)| ConfigError::new(path.as_str(), ".", reason))
            .collect();
        let config_path: RepoPath = REPO_CONFIG_FILE.parse().expect("static path is valid");
        let config = match parse_file::<RepoConfig>(files, &config_path) {
            Ok(config) => config.unwrap_or_default(),
            Err(mut e) => {
                errors.append(&mut e);
                return Err(errors);
            }
        };
        let mut p = Problems::new(REPO_CONFIG_FILE);
        config.validate(server, &mut p);
        errors.append(&mut p.errors);

        let known = [
            &config_path,
            &config.policy.rules_file,
            &config.policy.effects_file,
            &config.policy.deployment_file,
        ];
        let contracts: BTreeMap<RepoPath, Sha256Digest> = files
            .files
            .iter()
            .filter(|(path, _)| path.as_str().starts_with(CONTRACTS_DIR))
            .map(|(path, bytes)| (path.clone(), sha256(bytes)))
            .collect();
        for path in files.files.keys() {
            if !known.contains(&path) && !contracts.contains_key(path) {
                errors.push(ConfigError::new(
                    path.as_str(),
                    ".",
                    &format!("unknown policy file; {POLICY_DIR} holds only configured files and {CONTRACTS_DIR}"),
                ));
            }
        }
        let contract_texts = files
            .files
            .iter()
            .filter(|(path, _)| contracts.contains_key(*path))
            .filter_map(|(path, bytes)| {
                Some((path.clone(), String::from_utf8(bytes.clone()).ok()?))
            })
            .collect();
        let refs: BTreeSet<&RepoPath> = contracts.keys().collect();
        let rules = parse_file::<RulesFile>(files, &config.policy.rules_file);
        let effects = parse_file::<EffectsFile>(files, &config.policy.effects_file);
        let deployment = parse_file::<DeploymentFile>(files, &config.policy.deployment_file);
        let (rules, effects, deployment) = match (rules, effects, deployment) {
            (Ok(r), Ok(e), Ok(d)) => (r, e, d),
            (r, e, d) => {
                errors.extend(r.err().into_iter().flatten());
                errors.extend(e.err().into_iter().flatten());
                errors.extend(d.err().into_iter().flatten());
                return Err(errors);
            }
        };
        if let Some(rules) = &rules {
            let mut p = Problems::new(config.policy.rules_file.as_str());
            rules.validate(&mut p, &refs);
            errors.append(&mut p.errors);
        }

        if let Some(effects) = &effects {
            let mut p = Problems::new(config.policy.effects_file.as_str());
            effects.validate(&mut p, &refs);
            errors.append(&mut p.errors);
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(Self {
            config,
            rules: rules.map(|r| r.rules).unwrap_or_default(),
            effects: effects.map(|e| e.effects).unwrap_or_default(),
            deployment,
            contracts,
            contract_texts,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Engine,
    Server,
    Repository,
}

/// Where one layer of the effective config came from, without secret values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub layer: Layer,
    pub origin: String,
    pub sha256: Option<Sha256Digest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveConfig {
    pub schema_version: SchemaVersion,
    /// Hash of the normalized server and repository config.
    pub config_hash: Sha256Digest,
    /// Hash of the normalized rules, effects, deployment model and contracts.
    pub rules_hash: Sha256Digest,
    pub server: ServerConfig,
    pub repository: RepositoryPolicy,
    pub provenance: Vec<Provenance>,
}

/// Raw server config input; `None` means engine defaults in personal mode.
pub struct ServerInput<'a> {
    pub origin: &'a str,
    pub text: Option<&'a str>,
}

/// Raw repository policy input; `origin` names the trusted ref and commit, or a directory.
pub struct PolicyInput<'a> {
    pub origin: &'a str,
    pub files: &'a PolicyFiles,
}

impl ServerConfig {
    /// Engine defaults: personal mode, no providers, execution off.
    pub fn engine_default() -> Self {
        Self {
            schema_version: SchemaVersion,
            mode: Mode::Personal,
            storage: super::Storage::default(),
            http: super::Http::default(),
            workers: super::Workers::default(),
            models: super::Models::default(),
            budgets: super::Budgets::default(),
            execution: super::Execution::default(),
            tooling: super::Tooling::default(),
            publication: super::Publication::default(),
            logging: super::Logging::default(),
            redaction: super::Redaction::default(),
            scm: super::Scm::default(),
        }
    }
}

fn hash_failed(e: &serde_json::Error) -> Vec<ConfigError> {
    vec![ConfigError::new(
        "<effective>",
        ".",
        &format!("cannot hash: {e}"),
    )]
}

impl EffectiveConfig {
    pub fn build(
        server: &ServerInput<'_>,
        policy: &PolicyInput<'_>,
    ) -> Result<Self, Vec<ConfigError>> {
        let mut provenance = vec![Provenance {
            layer: Layer::Engine,
            origin: concat!("sych ", env!("CARGO_PKG_VERSION")).to_owned(),
            sha256: None,
        }];
        let server_config = match server.text {
            Some(text) => {
                provenance.push(Provenance {
                    layer: Layer::Server,
                    origin: server.origin.to_owned(),
                    sha256: Some(sha256(text.as_bytes())),
                });
                ServerConfig::parse(server.origin, text)?
            }
            None => ServerConfig::engine_default(),
        };
        let repository = RepositoryPolicy::parse(policy.files, &server_config)?;
        provenance.extend(policy.files.files.iter().map(|(path, bytes)| Provenance {
            layer: Layer::Repository,
            origin: format!("{path}@{}", policy.origin),
            sha256: Some(sha256(bytes)),
        }));
        let config_hash =
            canonical_sha256(&(&server_config, &repository.config)).map_err(|e| hash_failed(&e))?;
        let rules_hash = canonical_sha256(&(
            &repository.rules,
            &repository.effects,
            &repository.deployment,
            &repository.contracts,
        ))
        .map_err(|e| hash_failed(&e))?;
        Ok(Self {
            schema_version: SchemaVersion,
            config_hash,
            rules_hash,
            server: server_config,
            repository,
            provenance,
        })
    }

    /// Pins this config to its source; same source and content give the same id.
    pub fn policy_snapshot(
        &self,
        source: PolicySource,
        created_at: Timestamp,
    ) -> Result<PolicySnapshot, serde_json::Error> {
        let id = canonical_sha256(&(
            &source,
            &self.rules_hash,
            &self.config_hash,
            AUTHORIZATION_POLICY_VERSION,
        ))?;
        Ok(PolicySnapshot {
            schema_version: SchemaVersion,
            id: PolicySnapshotId(id),
            source,
            rules_hash: self.rules_hash.clone(),
            config_hash: self.config_hash.clone(),
            authorization_policy_version: AUTHORIZATION_POLICY_VERSION,
            created_at,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PolicyChange {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyFileChange {
    pub path: RepoPath,
    pub change: PolicyChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleChange {
    pub id: String,
    pub change: PolicyChange,
}

/// Policy proposed by the reviewed head, shown but not applied to this run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDiff {
    pub files: Vec<PolicyFileChange>,
    pub rules: Vec<RuleChange>,
    /// Problems of the proposed policy, redacted; they never fail the run.
    pub head_errors: Vec<String>,
}

fn changes<'a, K: Ord + Clone + 'a, V: PartialEq + 'a>(
    before: impl IntoIterator<Item = (&'a K, V)>,
    after: impl IntoIterator<Item = (&'a K, V)>,
) -> Vec<(K, PolicyChange)> {
    let before: BTreeMap<&K, V> = before.into_iter().collect();
    let after: BTreeMap<&K, V> = after.into_iter().collect();
    let keys: BTreeSet<&&K> = before.keys().chain(after.keys()).collect();
    keys.into_iter()
        .filter_map(|k| {
            let change = match (before.get(k), after.get(k)) {
                (Some(_), None) => PolicyChange::Removed,
                (None, Some(_)) => PolicyChange::Added,
                (Some(a), Some(b)) if a != b => PolicyChange::Modified,
                _ => return None,
            };
            Some(((*k).clone(), change))
        })
        .collect()
}

fn entries(files: &PolicyFiles) -> impl Iterator<Item = (&RepoPath, Result<&[u8], &str>)> {
    files
        .files
        .iter()
        .map(|(p, b)| (p, Ok(b.as_slice())))
        .chain(files.unreadable.iter().map(|(p, r)| (p, Err(r.as_str()))))
}

/// What a rule asks; title, owner, rationale and exceptions may change within one version.
fn meaning(r: &Rule) -> impl PartialEq + '_ {
    (
        r.kind,
        r.required,
        r.severity_on_violation,
        &r.applies_to,
        &r.statement,
        &r.acceptance,
        &r.contract_refs,
    )
}

/// Compares the trusted policy with the head's proposal; head content is untrusted data.
pub fn policy_diff(
    trusted: &RepositoryPolicy,
    trusted_files: &PolicyFiles,
    head_files: &PolicyFiles,
    server: &ServerConfig,
) -> PolicyDiff {
    let files = changes(entries(trusted_files), entries(head_files))
        .into_iter()
        .map(|(path, change)| PolicyFileChange { path, change })
        .collect::<Vec<_>>();
    if files.is_empty() {
        return PolicyDiff::default();
    }

    let head_errors = match RepositoryPolicy::parse(head_files, server) {
        Ok(_) => Vec::new(),
        Err(errors) => errors
            .iter()
            .map(|e| redact(&e.to_string()).into_owned())
            .collect(),
    };
    // Rules are compared at the trusted path even when the rest of the head policy is invalid.
    let head_rules = parse_file::<RulesFile>(head_files, &trusted.config.policy.rules_file)
        .ok()
        .map(|file| file.map(|f| f.rules).unwrap_or_default());
    let mut head_errors = head_errors;
    if let Some(head) = &head_rules {
        for rule in head {
            let before = trusted.rules.iter().find(|r| r.id == rule.id);
            if before.is_some_and(|b| b.version == rule.version && meaning(b) != meaning(rule)) {
                head_errors.push(format!(
                    "rule `{}` changes its meaning without a version bump (still v{})",
                    rule.id, rule.version
                ));
            }
        }
    }

    let rules = head_rules
        .map(|head| {
            changes(
                trusted.rules.iter().map(|r| (&r.id, r)),
                head.iter().map(|r| (&r.id, r)),
            )
            .into_iter()
            .map(|(id, change)| RuleChange { id, change })
            .collect()
        })
        .unwrap_or_default();
    PolicyDiff {
        files,
        rules,
        head_errors,
    }
}
