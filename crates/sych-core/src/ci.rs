//! Imported CI facts (spec §5.4, §15; roadmap M1.14): cargo/clippy JSON messages and `JUnit`
//! test reports, normalized and bound to what was actually tested.
//!
//! A locally imported file proves nothing about its origin, so it is an author claim; a
//! disabled lint or a skipped test is recorded as such and never reads as a fix.

use crate::kubernetes::RenderedManifests;
use crate::terraform::TerraformPlan;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    BindingKind, CodeRef, Completeness, Evidence, EvidenceBinding, EvidenceId, EvidenceKind,
    GitObjectId, Producer, SchemaVersion, SourceSnapshot, SourceSnapshotId, Timestamp, TrustClass,
    Validity, canonical_sha256, derived_uuid,
};

/// Sidecar describing the job that produced the reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CiManifest {
    pub schema_version: SchemaVersion,
    /// Job identity, e.g. `github:owner/repo/actions/runs/1/jobs/2`.
    pub job: String,
    /// Commit the job tested: plain hex as `git rev-parse HEAD` prints it, or `sha1:<hex>`.
    pub tested_oid: String,
    pub toolchain: String,
    pub target: String,
    #[serde(default)]
    pub features: Vec<String>,
}

/// Lines a coverage report saw executed in one file; execution, not correctness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CiCoverage {
    pub path: String,
    pub lines_found: u32,
    pub lines_hit: u32,
    /// Executed line numbers, ascending; bounded per file.
    pub hit: Vec<u32>,
}

/// How the tested tree relates to the reviewed head (spec §15.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CiBindingKind {
    /// The job's checkout manifest names the requested head.
    SourceHead,
    /// A merge commit whose parents are a target commit and the requested head.
    MergeResult,
    /// The actual checkout is not proven; not evidence for a strict obligation.
    Unverified,
}

/// Where an authenticated report came from: exact provider ids, never "the latest green run".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CiOrigin {
    pub provider_instance: String,
    pub repository: String,
    pub workflow: String,
    pub run_id: u64,
    pub run_attempt: u64,
    pub job_id: u64,
    pub job_name: String,
    pub event: String,
    pub job_status: String,
    pub job_conclusion: Option<String>,
    /// Not reported by the provider: unknown, never assumed false.
    pub allow_failure: Option<bool>,
    pub execution_sha: String,
    pub source_head_sha: String,
    pub target_tip_sha: Option<String>,
    pub tested_sha: Option<String>,
    pub binding: CiBindingKind,
    pub checkout_manifest_sha256: Option<String>,
    pub artifact_id: u64,
    pub artifact_name: String,
    pub artifact_sha256: String,
    pub report_path: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// A concrete reason an expected report is not evidence (spec §15.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CiGap {
    pub report: String,
    /// `CI_JOB_MISSING`, `CI_JOB_SKIPPED`, `CI_JOB_CANCELLED`, `CI_JOB_INCOMPLETE`,
    /// `CI_ARTIFACT_MISSING`, `CI_ARTIFACT_EXPIRED`, `CI_ARCHIVE_REJECTED`,
    /// `CI_REPORT_MISSING`, `CI_REPORT_MALFORMED`, `CI_BINDING_UNVERIFIED`, `CI_NOT_MAPPED`,
    /// `CI_UNAVAILABLE`.
    pub code: String,
    pub detail: String,
}

impl fmt::Display for CiGap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} for report `{}`: {}",
            self.code, self.report, self.detail
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CiFormat {
    /// `cargo check/clippy/build --message-format=json`.
    CargoJson,
    Junit,
    Sarif,
    GitlabCodeQuality,
    Lcov,
    Cobertura,
    /// A small project JSON manifest of command and test outcomes.
    SychManifest,
    /// `go test -json` (test2json events), with `-race` reports in its output.
    GoTestJson,
    /// `go test -coverprofile`.
    GoCover,
    /// `go vet -json`.
    GoVetJson,
    /// `terraform show -json` of a saved plan; reduced to allowlisted structure.
    TerraformPlan,
    /// Rendered Kubernetes manifests (`helm template`, `kustomize build`), multi-document YAML.
    KubernetesRendered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CiDiagnostic {
    /// Resolved only when the report is bound to the reviewed head.
    pub code_ref: Option<CodeRef>,
    pub path: String,
    pub line: u32,
    /// `error`, `warning`, `note`, …
    pub level: String,
    /// rustc or clippy code, e.g. `E0308` or `clippy::unwrap_used`.
    pub code: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TestStatus {
    Passed,
    Failed,
    Errored,
    /// Not run: never evidence that the behaviour works.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CiTest {
    pub name: String,
    pub suite: String,
    /// Source file when the report names one.
    pub file: Option<String>,
    pub status: TestStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CiFacts {
    pub schema_version: SchemaVersion,
    pub source_snapshot_id: SourceSnapshotId,
    pub format: CiFormat,
    /// File name of the report as given, without directories.
    pub report: String,
    pub manifest: Option<CiManifest>,
    pub diagnostics: Vec<CiDiagnostic>,
    pub tests: Vec<CiTest>,
    /// `build-finished.success` of a cargo report.
    pub build_success: Option<bool>,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default)]
    pub coverage: Vec<CiCoverage>,
    /// Set when the report was fetched from authenticated CI; local imports have none.
    #[serde(default)]
    pub origin: Option<CiOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terraform: Option<TerraformPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kubernetes: Option<RenderedManifests>,
}

impl CiManifest {
    fn tested(&self, like: &GitObjectId) -> Option<GitObjectId> {
        let hex = self.tested_oid.rsplit(':').next().unwrap_or_default();
        GitObjectId::new(like.algorithm(), hex).ok()
    }
}

impl CiFacts {
    pub fn new(source_snapshot_id: SourceSnapshotId, format: CiFormat, report: String) -> Self {
        Self {
            schema_version: SchemaVersion,
            source_snapshot_id,
            format,
            report,
            manifest: None,
            diagnostics: Vec::new(),
            tests: Vec::new(),
            build_success: None,
            limitations: Vec::new(),
            coverage: Vec::new(),
            origin: None,
            terraform: None,
            kubernetes: None,
        }
    }

    pub fn evidence_id(&self) -> EvidenceId {
        let hash = canonical_sha256(self)
            .map(|d| d.hex().to_owned())
            .unwrap_or_default();
        EvidenceId(derived_uuid(&[
            "ci",
            &self.source_snapshot_id.0.to_string(),
            &hash,
        ]))
    }

    /// Bound to the reviewed head only when the manifest says the head was tested, or an
    /// authenticated origin proved a source-head or merge-result checkout of it.
    pub fn bound_to(&self, snapshot: &SourceSnapshot) -> bool {
        if let Some(origin) = &self.origin {
            return origin.binding != CiBindingKind::Unverified
                && snapshot.head.hex() == origin.source_head_sha;
        }

        self.manifest
            .as_ref()
            .and_then(|m| m.tested(&snapshot.head))
            .is_some_and(|oid| oid == snapshot.head)
    }

    /// Import marks a report that tested another commit, or whose commit is unknown, with a
    /// `STALE`/`UNBOUND` limitation; the planner has only these facts, not the snapshot.
    pub fn head_bound(&self) -> bool {
        !self
            .limitations
            .iter()
            .any(|l| l.starts_with("STALE:") || l.starts_with("UNBOUND:"))
    }

    /// The build of the reviewed head itself succeeded, per a report bound to that head.
    pub fn compiled(&self, snapshot: &SourceSnapshot) -> bool {
        self.build_success == Some(true) && self.bound_to(snapshot)
    }

    /// External input evidence: `Ci` binding, author claim until a trusted CI transport
    /// authenticates the job (M2).
    pub fn to_evidence(
        &self,
        snapshot: &SourceSnapshot,
        observed_at: Timestamp,
    ) -> Result<Evidence, serde_json::Error> {
        let kind = match self.format {
            CiFormat::CargoJson
            | CiFormat::Sarif
            | CiFormat::GitlabCodeQuality
            | CiFormat::GoVetJson => EvidenceKind::Static,
            CiFormat::Junit
            | CiFormat::SychManifest
            | CiFormat::Lcov
            | CiFormat::Cobertura
            | CiFormat::GoTestJson
            | CiFormat::GoCover => EvidenceKind::Test,
            CiFormat::TerraformPlan | CiFormat::KubernetesRendered => EvidenceKind::Infrastructure,
        };
        let mut limitations = self.limitations.clone();
        let trust_class = if let Some(origin) = &self.origin {
            // Provenance, not oracle independence (spec §15.4).
            limitations.push(format!(
                "AUTHENTICATED_CI {}: origin is established, the test oracle is not independent",
                origin.workflow
            ));
            TrustClass::AuthenticatedCi
        } else {
            limitations.push("imported locally: origin is not authenticated".into());
            TrustClass::AuthorClaim
        };
        if !self.coverage.is_empty() {
            limitations.push("coverage shows execution, not correct assertions".into());
        }

        let validity = if self.bound_to(snapshot) {
            Validity::Valid
        } else {
            Validity::Stale
        };
        Ok(Evidence {
            schema_version: SchemaVersion,
            id: self.evidence_id(),
            kind,
            source_snapshot_id: self.source_snapshot_id.clone(),
            producer: Producer {
                name: match self.format {
                    CiFormat::CargoJson => "cargo",
                    CiFormat::Junit => "junit",
                    CiFormat::Sarif => "sarif",
                    CiFormat::GitlabCodeQuality => "gitlab_code_quality",
                    CiFormat::Lcov => "lcov",
                    CiFormat::Cobertura => "cobertura",
                    CiFormat::SychManifest => "sych_manifest",
                    CiFormat::GoTestJson => "go_test_json",
                    CiFormat::GoCover => "go_cover",
                    CiFormat::GoVetJson => "go_vet_json",
                    CiFormat::TerraformPlan => "terraform_plan",
                    CiFormat::KubernetesRendered => "kubernetes_rendered",
                }
                .into(),
                version: self
                    .manifest
                    .as_ref()
                    .map_or_else(|| "unknown".into(), |m| m.toolchain.clone()),
                config_hash: None,
            },
            binding: EvidenceBinding {
                kind: BindingKind::Ci,
                head_oid: Some(snapshot.head.clone()),
                target_tip_oid: Some(snapshot.target_tip.clone()),
                tested_oid: match &self.origin {
                    Some(o) => o
                        .tested_sha
                        .as_deref()
                        .and_then(|t| GitObjectId::new(snapshot.head.algorithm(), t).ok()),
                    None => self
                        .manifest
                        .as_ref()
                        .and_then(|m| m.tested(&snapshot.head)),
                },
                environment_id: match &self.origin {
                    Some(o) => Some(format!(
                        "{}:{}/actions/runs/{}/attempts/{}/jobs/{}",
                        o.provider_instance, o.repository, o.run_id, o.run_attempt, o.job_id
                    )),
                    None => self.manifest.as_ref().map(|m| m.job.clone()),
                },
            },
            validity,
            completeness: Completeness::Partial,
            trust_class,
            artifact_sha256: canonical_sha256(self)?,
            code_refs: self
                .diagnostics
                .iter()
                .filter_map(|d| d.code_ref.clone())
                .collect(),
            limitations,
            observed_at,
        })
    }
}
