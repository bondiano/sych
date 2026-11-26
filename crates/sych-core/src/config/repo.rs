//! Repository config `.sych/config.yaml`, read only from a trusted ref.

use serde::{Deserialize, Serialize};

use super::{Problems, ServerConfig};
use crate::architecture::Limits;
use crate::db_schema::SchemaLintSettings;
use crate::{RepoPath, ReviewLanguage, SchemaVersion};

/// Policy files live here; anything else in the directory is rejected.
pub const POLICY_DIR: &str = ".sych/";
pub const REPO_CONFIG_FILE: &str = ".sych/config.yaml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewProfile {
    Correctness,
    ProjectRules,
    SideEffects,
    MigrationCompatibility,
    Infrastructure,
    Nitpicks,
    /// Deterministic checks of added lines: committed secrets and hidden text (`scan`).
    Security,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftPolicy {
    ManualOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ReviewSettings {
    /// `en` or `ru`; any other value is a config error.
    pub language: ReviewLanguage,
    pub profiles: Vec<ReviewProfile>,
    pub draft_policy: DraftPolicy,
    /// Globs of labelled data whose code is not the product (eval cases, golden files): a
    /// candidate anchored only there is dropped, since a planted defect in a fixture is the point.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fixture_paths: Vec<String>,
    /// Globs of rule ids (`SEC-*`) whose open findings fail the check run instead of leaving it
    /// neutral, as does a failed run; empty keeps the review advisory.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub block_on: Vec<String>,
}

impl Default for ReviewSettings {
    fn default() -> Self {
        Self {
            language: ReviewLanguage::En,
            profiles: vec![
                ReviewProfile::Correctness,
                ReviewProfile::ProjectRules,
                ReviewProfile::SideEffects,
                ReviewProfile::MigrationCompatibility,
                ReviewProfile::Infrastructure,
                ReviewProfile::Nitpicks,
                ReviewProfile::Security,
            ],
            draft_policy: DraftPolicy::ManualOnly,
            fixture_paths: Vec::new(),
            block_on: Vec::new(),
        }
    }
}

/// Policy is read from the target tip; the reviewed head is never a source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRef {
    #[default]
    TargetTip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PolicySettings {
    pub source: PolicyRef,
    pub rules_file: RepoPath,
    pub effects_file: RepoPath,
    pub deployment_file: RepoPath,
    /// Optional schema linter config and exclusions file under `.sych/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_lint_file: Option<RepoPath>,
}

fn path(p: &str) -> RepoPath {
    p.parse().expect("static policy path is valid")
}

impl Default for PolicySettings {
    fn default() -> Self {
        Self {
            source: PolicyRef::TargetTip,
            rules_file: path(".sych/rules.yaml"),
            effects_file: path(".sych/effects.yaml"),
            deployment_file: path(".sych/deploy.yaml"),
            schema_lint_file: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ContextSettings {
    pub initial_tokens_per_unit: u32,
    pub max_tokens_per_request: u32,
    pub max_source_files: u32,
    pub graph_depth_initial: u32,
    pub graph_depth_max: u32,
    pub expansion_rounds: u32,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            // Bytes of shown code, not model tokens; a smaller window made each review see
            // a slice of the change and find one problem per revision.
            initial_tokens_per_unit: 16_000,
            max_tokens_per_request: 48_000,
            max_source_files: 80,
            graph_depth_initial: 1,
            graph_depth_max: 3,
            expansion_rounds: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportFormat {
    /// `cargo check/clippy/build --message-format=json`.
    CargoJson,
    Junit,
    Sarif,
    GitlabCodeQuality,
    Lcov,
    Cobertura,
    SychManifest,
    GoTestJson,
    GoCover,
    GoVetJson,
    TerraformPlan,
    KubernetesRendered,
    DbSchemaSnapshot,
    DbModelManifest,
}

/// A report ID only; the server maps it to trusted job selectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedReport {
    pub id: String,
    pub applies_to: Vec<String>,
    pub format: ReportFormat,
    #[serde(default)]
    pub required_for: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CiSettings {
    pub wait_seconds: u32,
    pub expected_reports: Vec<ExpectedReport>,
}

impl Default for CiSettings {
    fn default() -> Self {
        Self {
            wait_seconds: 180,
            expected_reports: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NitpickPresentation {
    #[default]
    CollapsedSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NitpickSettings {
    pub enabled: bool,
    pub presentation: NitpickPresentation,
}

impl Default for NitpickSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            presentation: NitpickPresentation::CollapsedSummary,
        }
    }
}

/// Architecture review: the structure of each project a change touches, on the diff base and
/// on head. Read from the target tip, as every setting here, so a change cannot loosen the
/// bounds its own code is judged by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ArchitectureSettings {
    pub enabled: bool,
    pub limits: Limits,
}

impl ArchitectureSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for ArchitectureSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            limits: Limits::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TourGeneration {
    #[default]
    OnRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TourSettings {
    pub enabled: bool,
    pub generate: TourGeneration,
    pub target_steps: u32,
}

impl Default for TourSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            generate: TourGeneration::OnRequest,
            target_steps: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoConfig {
    pub schema_version: SchemaVersion,
    #[serde(default)]
    pub review: ReviewSettings,
    #[serde(default)]
    pub policy: PolicySettings,
    #[serde(default)]
    pub context: ContextSettings,
    #[serde(default)]
    pub ci: CiSettings,
    #[serde(default)]
    pub nitpicks: NitpickSettings,
    #[serde(default)]
    pub tour: TourSettings,
    /// Left out of the policy snapshot while it is the default, so a config without it keeps
    /// its policy id.
    #[serde(default, skip_serializing_if = "ArchitectureSettings::is_default")]
    pub architecture: ArchitectureSettings,
    /// Optional database and application schema linter settings (off by default). Left out of the
    /// policy snapshot while at its default, so existing configs keep their policy id.
    #[serde(default, skip_serializing_if = "SchemaLintSettings::is_default")]
    pub schema_lint: SchemaLintSettings,
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            schema_version: SchemaVersion,
            review: ReviewSettings::default(),
            policy: PolicySettings::default(),
            context: ContextSettings::default(),
            ci: CiSettings::default(),
            nitpicks: NitpickSettings::default(),
            tour: TourSettings::default(),
            architecture: ArchitectureSettings::default(),
            schema_lint: SchemaLintSettings::default(),
        }
    }
}

impl RepoConfig {
    /// Local checks plus the server ceilings the repository may not exceed.
    pub(crate) fn validate(&self, server: &ServerConfig, p: &mut Problems<'_>) {
        let policy = &self.policy;
        for (key, file) in [
            ("rules_file", &policy.rules_file),
            ("effects_file", &policy.effects_file),
            ("deployment_file", &policy.deployment_file),
        ] {
            let s = file.as_str();
            p.check(
                s.starts_with(POLICY_DIR)
                    && !s.starts_with(".sych/contracts/")
                    && s != REPO_CONFIG_FILE,
                format!("policy.{key}"),
                &format!("`{s}` must be a file under {POLICY_DIR}, outside contracts/"),
            );
        }
        if let Some(file) = &policy.schema_lint_file {
            let s = file.as_str();
            p.check(
                s.starts_with(POLICY_DIR)
                    && !s.starts_with(".sych/contracts/")
                    && s != REPO_CONFIG_FILE,
                "policy.schema_lint_file",
                &format!("`{s}` must be a file under {POLICY_DIR}, outside contracts/"),
            );
        }

        let c = &self.context;
        p.check(
            c.initial_tokens_per_unit <= c.max_tokens_per_request,
            "context.initial_tokens_per_unit",
            "exceeds context.max_tokens_per_request",
        );
        p.check(
            c.graph_depth_initial <= c.graph_depth_max,
            "context.graph_depth_initial",
            "exceeds context.graph_depth_max",
        );
        if let Some(ceiling) = server.budgets.analysis.max_input_tokens {
            p.check(
                c.max_tokens_per_request <= ceiling,
                "context.max_tokens_per_request",
                &format!("exceeds server ceiling budgets.analysis.max_input_tokens = {ceiling}"),
            );
        }

        let limits = &self.architecture.limits;
        for (key, value) in [
            ("tangle_members", limits.tangle_members),
            ("shallow_functions", limits.shallow_functions),
            ("wide_directory", limits.wide_directory),
            ("long_file", limits.long_file),
        ] {
            p.check(
                value > 0,
                format!("architecture.limits.{key}"),
                "must be at least 1",
            );
        }

        self.schema_lint.validate(p);

        for (i, report) in self.ci.expected_reports.iter().enumerate() {
            p.check(
                !report.id.is_empty() && !report.applies_to.is_empty(),
                format!("ci.expected_reports[{i}]"),
                "needs an id and at least one applies_to pattern",
            );
        }
    }
}
