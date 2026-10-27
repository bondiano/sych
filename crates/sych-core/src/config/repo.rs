//! Repository config `.sych/config.yaml` (spec §23.3), read only from a trusted ref.

use serde::{Deserialize, Serialize};

use super::{Problems, ServerConfig};
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
            ],
            draft_policy: DraftPolicy::ManualOnly,
        }
    }
}

/// Policy is read from the target tip; the reviewed head is never a source (INV-12).
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
            initial_tokens_per_unit: 6000,
            max_tokens_per_request: 14_000,
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
}

/// A report ID only; the server maps it to trusted job selectors (spec §23.3).
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
        }
    }
}

impl RepoConfig {
    /// Local checks plus the server ceilings the repository may not exceed (spec §23.3).
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

        for (i, report) in self.ci.expected_reports.iter().enumerate() {
            p.check(
                !report.id.is_empty() && !report.applies_to.is_empty(),
                format!("ci.expected_reports[{i}]"),
                "needs an id and at least one applies_to pattern",
            );
        }
    }
}
