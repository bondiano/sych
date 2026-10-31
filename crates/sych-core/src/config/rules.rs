//! Rule contract `rules.yaml`, effect registry `effects.yaml`
//! and the deployment model file.

use crate::deploy::DeploymentModel;
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Problems;
use crate::{EvidenceKind, RepoPath, SchemaVersion, Severity, Timestamp};

pub const CONTRACTS_DIR: &str = ".sych/contracts/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    Invariant,
    Preference,
}

/// YAML spelling of [`Severity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSeverity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl From<RuleSeverity> for Severity {
    fn from(s: RuleSeverity) -> Self {
        match s {
            RuleSeverity::Info => Self::Info,
            RuleSeverity::Low => Self::Low,
            RuleSeverity::Medium => Self::Medium,
            RuleSeverity::High => Self::High,
            RuleSeverity::Critical => Self::Critical,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AppliesTo {
    pub paths: Vec<String>,
    pub change_kinds: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acceptance {
    pub evidence_kinds: Vec<EvidenceKind>,
    #[serde(default)]
    pub criteria: Vec<String>,
}

/// A scoped waiver; it never rewrites the original `VIOLATED/UNKNOWN`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleException {
    pub paths: Vec<String>,
    pub reason: String,
    pub owner: String,
    pub author: String,
    pub expires_at: Option<Timestamp>,
    /// Completion condition when there is no fixed expiry.
    pub until: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub version: u32,
    pub title: String,
    pub kind: RuleKind,
    pub required: bool,
    pub severity_on_violation: Option<RuleSeverity>,
    /// Team or role from configuration, not an arbitrary person.
    pub owner: String,
    #[serde(default)]
    pub applies_to: AppliesTo,
    pub statement: String,
    pub rationale: Option<String>,
    #[serde(default)]
    pub contract_refs: Vec<RepoPath>,
    pub acceptance: Option<Acceptance>,
    #[serde(default)]
    pub exceptions: Vec<RuleException>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesFile {
    pub schema_version: SchemaVersion,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    DbWrite,
    Event,
    Network,
    Cache,
    Filesystem,
    Subscription,
    Ui,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectMatch {
    pub language: String,
    pub symbol: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Guarantees {
    pub durable_with_transaction: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Effect {
    pub id: String,
    #[serde(rename = "match")]
    pub matcher: EffectMatch,
    pub effect_kind: EffectKind,
    #[serde(default)]
    pub guarantees: Guarantees,
    #[serde(default)]
    pub implementation_refs: Vec<RepoPath>,
    #[serde(default)]
    pub contract_refs: Vec<RepoPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectsFile {
    pub schema_version: SchemaVersion,
    #[serde(default)]
    pub effects: Vec<Effect>,
}

/// `deploy.yaml`, versioned and hashed as normalized JSON.
pub type DeploymentFile = DeploymentModel;

fn is_rule_id(id: &str) -> bool {
    id.starts_with(|c: char| c.is_ascii_uppercase())
        && id
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn check_contract_refs(
    p: &mut Problems<'_>,
    path: &str,
    refs: &[RepoPath],
    contracts: &BTreeSet<&RepoPath>,
) {
    for (i, r) in refs.iter().enumerate() {
        p.check(
            contracts.contains(r),
            format!("{path}.contract_refs[{i}]"),
            &format!("`{r}` is not a file under {CONTRACTS_DIR} in the trusted policy"),
        );
    }
}

impl RulesFile {
    /// Duplicate IDs are a conflict between mandatory rules and are never resolved silently.
    pub(crate) fn validate(&self, p: &mut Problems<'_>, contracts: &BTreeSet<&RepoPath>) {
        let mut seen = BTreeMap::new();
        for (i, rule) in self.rules.iter().enumerate() {
            let path = format!("rules[{i}]");
            if let Some(first) = seen.insert(rule.id.as_str(), i) {
                p.push(
                    format!("{path}.id"),
                    &format!(
                        "rule `{}` conflicts with rules[{first}]: one ID, one definition",
                        rule.id
                    ),
                );
            }

            p.check(
                is_rule_id(&rule.id),
                format!("{path}.id"),
                "must be UPPER-CASE letters, digits, `-` or `_`",
            );
            p.check(
                rule.version > 0,
                format!("{path}.version"),
                "must be 1 or greater",
            );
            p.check(
                !rule.owner.trim().is_empty() && !rule.statement.trim().is_empty(),
                &path,
                "owner and statement are required",
            );
            if rule.required {
                p.check(
                    rule.kind == RuleKind::Invariant,
                    format!("{path}.required"),
                    "a preference cannot be required",
                );
                p.check(
                    rule.severity_on_violation.is_some(),
                    format!("{path}.severity_on_violation"),
                    "required rules declare the severity of a violation",
                );
            }

            check_contract_refs(p, &path, &rule.contract_refs, contracts);
            for (j, ex) in rule.exceptions.iter().enumerate() {
                p.check(
                    ex.expires_at.is_some() || ex.until.is_some(),
                    format!("{path}.exceptions[{j}]"),
                    "needs expires_at or an until condition",
                );
                p.check(
                    !ex.paths.is_empty(),
                    format!("{path}.exceptions[{j}].paths"),
                    "an exception is scoped to at least one path",
                );
            }
        }
    }
}

impl EffectsFile {
    pub(crate) fn validate(&self, p: &mut Problems<'_>, contracts: &BTreeSet<&RepoPath>) {
        let mut seen = BTreeSet::new();
        for (i, effect) in self.effects.iter().enumerate() {
            let path = format!("effects[{i}]");
            p.check(
                seen.insert(effect.id.as_str()),
                format!("{path}.id"),
                &format!("duplicate effect `{}`", effect.id),
            );
            check_contract_refs(p, &path, &effect.contract_refs, contracts);
        }
    }
}
