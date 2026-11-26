//! What one review is asked to do, whoever asks: `sych review`, a server job or an eval case.

use std::path::PathBuf;

use sych_core::analysis::Discovered;
use sych_core::config::{CheckRequest, Execution, RuleException, RunMode};
use sych_core::{RunId, SnapshotHeadKind};

use crate::args::{Comparison, ModelSelection, Switch};

pub struct Request {
    pub comparison: Comparison,
    pub head_kind: SnapshotHeadKind,
    /// Server config; engine defaults when `None`.
    pub config: Option<PathBuf>,
    /// `None` lets the CLI review use the trusted config's default selector.
    pub models: Option<ModelSelection>,
    /// Scripted evidence and assessments for the fake adapter.
    pub fake_responses: Option<PathBuf>,
    /// The policy of this revision instead of the target tip's.
    pub policy_ref: Option<String>,
    pub semantic: Switch,
    /// CI reports to import, as files: author claims, not authenticated CI.
    pub ci_facts: Vec<PathBuf>,
    pub ci_manifest: Option<PathBuf>,
    /// Only these execution profiles, instead of every allowed one.
    pub checks: Vec<CheckRequest>,
    /// Execution profiles of one eval case instead of the server's.
    pub execution: Option<Execution>,
    /// The extra high-risk search; only `eval measure` turns it off, for baseline B.
    pub risk_audit: bool,
    /// Candidate verification; only `eval measure` turns it off, for baseline A.
    pub verify: bool,
    /// Reports a server run fetched from authenticated CI, already normalized.
    pub ci_imported: Vec<sych_core::CiFacts>,
    /// Expected reports that are not evidence, as `CiGap` text; each keeps coverage partial.
    pub ci_gaps: Vec<String>,
    /// Scoped waivers recorded on the server, as `(rule id, exception)`; they lower the risk
    /// decision and never rewrite a status.
    pub waivers: Vec<(String, RuleException)>,
    /// Where the review runs: a server run never uses a provider allowed only locally (such as
    /// a personal subscription sign-in).
    pub run_mode: RunMode,
    /// A `deep` manual server run: roles start at `models.deep_roles`.
    pub deep: bool,
    /// The parent of a successor server run, whose checkpoints may be reused.
    pub parent: Option<RunId>,
    /// `eval measure --replay`: discoveries of an earlier run to verify again, instead of a
    /// new search.
    pub replay: Option<Vec<Discovered>>,
    /// The author's description of the change, instead of its commit messages.
    pub intent_file: Option<PathBuf>,
    /// Local CLI override for `.sych/config.yaml` `schema_lint.enabled`.
    pub schema_lint: Option<Switch>,
    /// Local CLI live read-only database DSN for schema linting.
    pub db_dsn: Option<String>,
    /// Local CLI explicit application model manifest JSON for schema linting.
    pub db_model_manifest: Option<PathBuf>,
    /// `owner/name` of the repository on a server run, used to enforce `schema_lint.connections` allowlists.
    pub repository_full_name: Option<String>,
}

impl Request {
    /// A local review of `comparison` with everything else at its default: semantic facts on,
    /// the whole pipeline, no imported CI, waivers or parent.
    pub fn new(comparison: Comparison, models: Option<ModelSelection>) -> Self {
        Self {
            comparison,
            head_kind: SnapshotHeadKind::Committed,
            config: None,
            models,
            fake_responses: None,
            policy_ref: None,
            semantic: Switch::On,
            ci_facts: Vec::new(),
            ci_manifest: None,
            checks: Vec::new(),
            execution: None,
            risk_audit: true,
            verify: true,
            ci_imported: Vec::new(),
            ci_gaps: Vec::new(),
            waivers: Vec::new(),
            run_mode: RunMode::PersonalLocal,
            deep: false,
            parent: None,
            replay: None,
            intent_file: None,
            schema_lint: None,
            db_dsn: None,
            db_model_manifest: None,
            repository_full_name: None,
        }
    }
}
