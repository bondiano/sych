//! Review-level database schema facts: what the linter checked on one schema
//! target (live database, CI snapshot or static SQL), its diff against the base
//! when available, and its translation into Sych evidence.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::config::DbSchemaExclusion;
use super::model::{DbSchemaFinding, DbSchemaObject, DbSchemaSource, SqlDialect};
use crate::{
    BackendStatus, BindingKind, CodeRef, Completeness, Evidence, EvidenceBinding, EvidenceId,
    EvidenceKind, Producer, SchemaVersion, SourceSnapshot, SourceSnapshotId, Timestamp, TrustClass,
    Validity, canonical_sha256, derived_uuid,
};

/// How one finding compares with the diff base when both sides were checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DbSchemaChange {
    New,
    Same,
}

/// One schema finding stored inside [`DbSchemaFacts`] with optional diff status
/// and resolved HEAD anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DbSchemaFactFinding {
    pub rule: String,
    pub object: DbSchemaObject,
    pub message: String,
    pub fix: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<DbSchemaChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_ref: Option<CodeRef>,
}

/// A finding present on the diff base that head no longer has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FixedDbSchemaFinding {
    pub rule: String,
    pub object: DbSchemaObject,
    pub message: String,
}

/// Stable identity key for comparing schema findings across revisions.
pub fn finding_key(rule: &str, object: &DbSchemaObject) -> String {
    format!("{rule}\0{}", object.format_exclusion())
}

/// Labels head findings against `base` findings and returns `(labeled, fixed)`.
pub fn diff_findings(
    base: Option<&[DbSchemaFinding]>,
    head: Vec<(DbSchemaFinding, Option<CodeRef>)>,
) -> (Vec<DbSchemaFactFinding>, Vec<FixedDbSchemaFinding>) {
    let Some(base) = base else {
        let labeled = head
            .into_iter()
            .map(|(f, code_ref)| DbSchemaFactFinding {
                rule: f.rule,
                object: f.object,
                message: f.message,
                fix: f.fix,
                change: None,
                code_ref,
            })
            .collect();
        return (labeled, Vec::new());
    };

    let base_keys: BTreeSet<String> = base
        .iter()
        .map(|f| finding_key(&f.rule, &f.object))
        .collect();
    let head_keys: BTreeSet<String> = head
        .iter()
        .map(|(f, _)| finding_key(&f.rule, &f.object))
        .collect();

    let labeled = head
        .into_iter()
        .map(|(f, code_ref)| {
            let key = finding_key(&f.rule, &f.object);
            let change = if base_keys.contains(&key) {
                DbSchemaChange::Same
            } else {
                DbSchemaChange::New
            };
            DbSchemaFactFinding {
                rule: f.rule,
                object: f.object,
                message: f.message,
                fix: f.fix,
                change: Some(change),
                code_ref,
            }
        })
        .collect();

    let fixed = base
        .iter()
        .filter(|f| !head_keys.contains(&finding_key(&f.rule, &f.object)))
        .map(|f| FixedDbSchemaFinding {
            rule: f.rule.clone(),
            object: f.object.clone(),
            message: f.message.clone(),
        })
        .collect();

    (labeled, fixed)
}

/// Deterministic database and application schema check facts for one target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DbSchemaFacts {
    pub schema_version: SchemaVersion,
    pub source_snapshot_id: SourceSnapshotId,
    pub producer: Producer,
    /// Label of this schema target (e.g. `migrations/postgres`, `live:postgres`, `ci:db-schema`).
    pub target: String,
    pub dialect: SqlDialect,
    pub source: DbSchemaSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_extractor: Option<String>,
    pub status: BackendStatus,
    /// True when the diff base was also evaluated so new findings are separated from existing debt.
    pub base_checked: bool,
    pub findings: Vec<DbSchemaFactFinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed: Vec<FixedDbSchemaFinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmatched_exclusions: Vec<DbSchemaExclusion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_rules: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

impl DbSchemaFacts {
    pub fn evidence_id(&self) -> EvidenceId {
        EvidenceId(derived_uuid(&[
            "db_schema",
            &self.producer.name,
            &self.source_snapshot_id.0.to_string(),
            &self.target,
        ]))
    }

    /// Converts these facts into a Sych [`Evidence`] record.
    pub fn to_evidence(
        &self,
        snapshot: &SourceSnapshot,
        observed_at: Timestamp,
    ) -> Result<Evidence, serde_json::Error> {
        let validity = match self.status {
            BackendStatus::Ready => Validity::Valid,
            BackendStatus::Unsupported => Validity::Unavailable,
            BackendStatus::Failed => Validity::Failed,
        };
        let completeness = if self.status == BackendStatus::Ready && self.limitations.is_empty() {
            Completeness::CompleteForQuery
        } else {
            Completeness::Partial
        };
        let (kind, binding_kind, trust_class) = match self.source {
            DbSchemaSource::StaticSql => (
                EvidenceKind::Static,
                BindingKind::Source,
                TrustClass::DerivedLocal,
            ),
            DbSchemaSource::LiveDatabase => (
                EvidenceKind::Infrastructure,
                BindingKind::Environment,
                TrustClass::DerivedLocal,
            ),
            DbSchemaSource::CiSnapshot => (
                EvidenceKind::Infrastructure,
                BindingKind::Ci,
                TrustClass::AuthenticatedCi,
            ),
        };
        let mut limitations = self.limitations.clone();
        limitations.extend(self.warnings.iter().cloned());
        if !self.skipped_rules.is_empty() {
            limitations.push(format!(
                "rules skipped for {}: {}",
                self.dialect,
                self.skipped_rules.join(", ")
            ));
        }

        Ok(Evidence {
            schema_version: SchemaVersion,
            id: self.evidence_id(),
            kind,
            source_snapshot_id: self.source_snapshot_id.clone(),
            producer: self.producer.clone(),
            binding: EvidenceBinding {
                kind: binding_kind,
                head_oid: Some(snapshot.head.clone()),
                target_tip_oid: Some(snapshot.target_tip.clone()),
                tested_oid: None,
                environment_id: match self.source {
                    DbSchemaSource::StaticSql => None,
                    DbSchemaSource::LiveDatabase | DbSchemaSource::CiSnapshot => {
                        Some(self.target.clone())
                    }
                },
            },
            validity,
            completeness,
            trust_class,
            artifact_sha256: canonical_sha256(self)?,
            code_refs: self
                .findings
                .iter()
                .filter_map(|f| f.code_ref.clone())
                .collect(),
            limitations,
            observed_at,
        })
    }
}
