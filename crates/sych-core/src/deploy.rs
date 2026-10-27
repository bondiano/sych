//! Deployment model and transition matrix (spec §13.1–13.2): what the environment owner
//! declared about rollout, each claim with its source. Nothing here is guessed from Git:
//! `diff_base` and `target_tip` are never the deployed version.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::hash::derived_uuid;
use crate::{
    BindingKind, Completeness, Evidence, EvidenceBinding, EvidenceId, EvidenceKind, Producer,
    SchemaVersion, SourceSnapshot, SourceSnapshotId, Timestamp, TrustClass, Validity,
    canonical_sha256,
};

/// A claim and who or what states it (a runbook, a deploy config, the owner).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sourced<T> {
    pub value: T,
    pub source: String,
}

impl<T> Sourced<T> {
    /// A claim without a source is not a claim.
    fn known(this: Option<&Self>) -> Option<&T> {
        this.filter(|s| !s.source.trim().is_empty())
            .map(|s| &s.value)
    }
}

/// The deployed starting point; without the build → source link it pins nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitialVersions {
    /// Image or build identity actually running, e.g. `ghcr.io/o/app@sha256:…`.
    pub app_image: Option<String>,
    /// The source commit that image was built from.
    pub source_commit: Option<String>,
    /// Schema/data phase, e.g. `expanded`, `backfilling`, `contracted`.
    pub schema_phase: Option<String>,
    /// Client versions still supported (open tabs, mobile builds, external consumers).
    #[serde(default)]
    pub supported_clients: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Schema,
    App,
    Frontend,
    Jobs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    CompatibleRollback,
    RollForwardRepair,
    RestoreReplay,
    Other,
}

/// `deploy.yaml` of one environment (spec §13.1). Every field is optional: what the owner
/// has not declared stays unknown and is reported, never filled in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeploymentModel {
    pub schema_version: SchemaVersion,
    pub environment: Option<String>,
    pub owner: Option<String>,
    pub initial_versions: Option<Sourced<InitialVersions>>,
    pub order: Option<Sourced<Vec<Component>>>,
    pub mixed_versions: Option<Sourced<bool>>,
    pub downtime: Option<Sourced<bool>>,
    /// How long old clients and consumers stay supported, e.g. `14d`.
    pub compatibility_window: Option<Sourced<String>>,
    pub backfill: Option<Sourced<String>>,
    pub contract: Option<Sourced<String>>,
    pub recovery: Option<Sourced<Recovery>>,
    /// States the rollout cannot reach; each needs its own confirmed constraint.
    #[serde(default)]
    pub excluded_states: Vec<Sourced<TransitionState>>,
    /// The environment's actual transition state as observed (e.g. "backfill done up to id
    /// 120000, 2026-10-20"); only this turns a migration review into `OBSERVED_TRANSITION`.
    pub observed: Option<Sourced<String>>,
}

/// The reachable combinations of spec §13.2.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TransitionState {
    OldAppExpandedSchema,
    MixedApps,
    OldFrontendNewBackend,
    PartialBackfillConcurrentWrites,
    BackfillRetry,
    ContractPhase,
    RecoveryAfterNewWrites,
}

impl TransitionState {
    pub const ALL: [Self; 7] = [
        Self::OldAppExpandedSchema,
        Self::MixedApps,
        Self::OldFrontendNewBackend,
        Self::PartialBackfillConcurrentWrites,
        Self::BackfillRetry,
        Self::ContractPhase,
        Self::RecoveryAfterNewWrites,
    ];

    /// What must hold in this state.
    pub fn obligation(self) -> &'static str {
        match self {
            Self::OldAppExpandedSchema => "old readers and writers keep working",
            Self::MixedApps => "data, API and events stay compatible between versions",
            Self::OldFrontendNewBackend => "the declared contract of the old client is supported",
            Self::PartialBackfillConcurrentWrites => {
                "fresher data is not lost and both forms are handled"
            }
            Self::BackfillRetry => "the backfill is idempotent or resumes correctly",
            Self::ContractPhase => {
                "the old path is no longer needed within the compatibility window"
            }
            Self::RecoveryAfterNewWrites => "the restored version handles the actual data state",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Reachability {
    Reachable,
    /// Excluded by a confirmed deploy constraint, cited.
    Excluded {
        source: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MatrixRow {
    pub state: TransitionState,
    pub obligation: String,
    #[serde(flatten)]
    pub reachability: Reachability,
}

/// The matrix and the inputs the owner has not provided; dependent obligations stay
/// `UNKNOWN` on these gaps, unrelated static review does not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransitionMatrix {
    pub environment: Option<String>,
    pub rows: Vec<MatrixRow>,
    pub missing_inputs: Vec<String>,
    pub observed: Option<Sourced<String>>,
}

impl TransitionMatrix {
    pub fn evidence_id(snapshot: &SourceSnapshotId) -> EvidenceId {
        EvidenceId(derived_uuid(&[
            "transition-matrix",
            &snapshot.0.to_string(),
        ]))
    }

    /// Derived contract evidence: what the owner declared, as the obligations used it.
    pub fn to_evidence(
        &self,
        snapshot: &SourceSnapshot,
        observed_at: Timestamp,
    ) -> Result<Evidence, serde_json::Error> {
        Ok(Evidence {
            schema_version: SchemaVersion,
            id: Self::evidence_id(&snapshot.id),
            kind: EvidenceKind::Contract,
            source_snapshot_id: snapshot.id.clone(),
            producer: Producer {
                name: "sych-transition-matrix".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                config_hash: None,
            },
            binding: EvidenceBinding {
                kind: BindingKind::Source,
                head_oid: Some(snapshot.head.clone()),
                target_tip_oid: Some(snapshot.target_tip.clone()),
                tested_oid: None,
                environment_id: self.environment.clone(),
            },
            validity: Validity::Valid,
            completeness: if self.missing_inputs.is_empty() {
                Completeness::CompleteForQuery
            } else {
                Completeness::Partial
            },
            trust_class: TrustClass::DerivedLocal,
            artifact_sha256: canonical_sha256(self)?,
            code_refs: Vec::new(),
            limitations: self.missing_inputs.clone(),
            observed_at,
        })
    }
}

/// Spec §13.2: every state is reachable unless a sourced constraint excludes it; a missing
/// description is never grounds for `NOT_APPLICABLE`.
pub fn matrix(model: Option<&DeploymentModel>) -> TransitionMatrix {
    let default = DeploymentModel::default();
    let model = model.unwrap_or(&default);
    let no_mixing = model
        .mixed_versions
        .as_ref()
        .filter(|m| !m.value && !m.source.trim().is_empty());
    let rows = TransitionState::ALL
        .into_iter()
        .map(|state| {
            let declared = model
                .excluded_states
                .iter()
                .find(|e| e.value == state && !e.source.trim().is_empty())
                .map(|e| e.source.clone());
            let implied = (state == TransitionState::MixedApps)
                .then(|| no_mixing.map(|m| m.source.clone()))
                .flatten();
            MatrixRow {
                state,
                obligation: state.obligation().to_owned(),
                reachability: match declared.or(implied) {
                    Some(source) => Reachability::Excluded { source },
                    None => Reachability::Reachable,
                },
            }
        })
        .collect();
    TransitionMatrix {
        environment: model.environment.clone(),
        rows,
        missing_inputs: missing(model),
        observed: model
            .observed
            .clone()
            .filter(|o| !o.source.trim().is_empty()),
    }
}

fn missing(model: &DeploymentModel) -> Vec<String> {
    let initial = Sourced::known(model.initial_versions.as_ref());
    let mut gaps = Vec::new();
    if model.environment.is_none() || model.owner.is_none() {
        gaps.push("ENVIRONMENT_OWNER: environment and owner are not declared".to_owned());
    }

    match initial {
        None => gaps.push(
            "INITIAL_VERSION_UNKNOWN: the deployed version is not declared; merge-base is not it"
                .to_owned(),
        ),
        Some(v) if v.app_image.is_none() || v.source_commit.is_none() => gaps.push(
            "BUILD_SOURCE_UNKNOWN: the running image is not linked to its source commit".to_owned(),
        ),
        Some(_) => {}
    }
    let absent = [
        (
            "ORDER_UNKNOWN",
            Sourced::known(model.order.as_ref()).is_none(),
        ),
        (
            "COMPATIBILITY_WINDOW_UNKNOWN",
            Sourced::known(model.compatibility_window.as_ref()).is_none(),
        ),
        (
            "RECOVERY_UNKNOWN",
            Sourced::known(model.recovery.as_ref()).is_none(),
        ),
    ];
    gaps.extend(
        absent
            .into_iter()
            .filter(|(_, gap)| *gap)
            .map(|(code, _)| format!("{code}: not declared in the deployment model")),
    );
    if initial.is_none_or(|v| v.supported_clients.is_empty()) {
        gaps.push(
            "EXTERNAL_CONSUMERS_UNKNOWN: supported clients and external consumers are not listed; \
             searching this repository does not prove a removal safe"
                .to_owned(),
        );
    }

    gaps
}
