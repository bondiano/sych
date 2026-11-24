//! The review pipeline: snapshot, trusted policy, facts, plan, context packets, model or fake
//! analysis, reduce and a durable result.

use std::process::ExitCode;
use std::{fs, io};

use anyhow::Context;

use sych_adapters::git::{LocalHead, LocalSnapshot};
use sych_adapters::{execution, syntax};
use sych_core::analysis::Discovered;
use sych_core::broker::{EvidencePacket, Limits};
use sych_core::config::{Budgets, Rule};
use sych_core::config::{
    EffectiveConfig, PolicyFiles, PolicyInput, ReviewProfile, RuleException, ServerConfig,
    ServerInput, policy_diff,
};
use sych_core::db_schema::DbSchemaConfig;
use sych_core::{
    Candidate, Coverage, Evidence, EvidenceManifest, ExternalInputManifest, FileChange,
    PolicySnapshot, PolicySource, Recommendation, ReviewPlan, ReviewResult, RunFacts,
    SchemaVersion, SnapshotHeadKind, SourceSide, Timestamp, is_migration_path, plan, redact,
};
use sych_core::{deploy, effects};
use sych_storage::{RunHandle, StoreConfig};
use tokio::sync::watch;

use crate::args::{ExitPolicy, ModelSelection, Switch};
use crate::backend::{self, ResolveError, Routing};
use crate::errors::{EXIT_INCOMPLETE, EXIT_VIOLATION, Failure};
use crate::fake::{FakeResponses, Scripted};
use crate::render::{PolicyReview, Report};
use crate::{ci, config, semantic, snapshot, storage};

mod checkpoints;
mod db_schema;
mod outcome;
mod request;
mod schema;
mod session;

pub use request::Request;

/// Strict mode: a significant violation (10) outranks incomplete scope (11).
pub fn exit_code(policy: ExitPolicy, result: &ReviewResult) -> ExitCode {
    match policy {
        ExitPolicy::Strict if result.recommendation == Recommendation::AttentionRequired => {
            ExitCode::from(EXIT_VIOLATION)
        }
        ExitPolicy::Strict if result.coverage != Coverage::CompleteForScope => {
            ExitCode::from(EXIT_INCOMPLETE)
        }
        _ => ExitCode::SUCCESS,
    }
}

/// Effective config from the server config and the policy at the target tip, or at
/// `--policy-ref` for an explicit run; the head's policy files are returned only for the diff.
fn trusted_config(
    args: &Request,
    snap: &LocalSnapshot,
) -> anyhow::Result<(EffectiveConfig, PolicyFiles, PolicyFiles, PolicySource)> {
    let server = config::read_server(args.config.as_deref())?;
    let (ref_name, commit, trusted) = match &args.policy_ref {
        Some(rev) => {
            let (commit, files) = snap.policy_files_at(rev)?;
            (rev.clone(), commit, files)
        }
        None => (
            args.comparison.base.clone(),
            snap.snapshot.target_tip.clone(),
            snap.policy_files(SourceSide::TargetTip)?,
        ),
    };
    let head = snap.policy_files(SourceSide::Head)?;
    let server_origin = args
        .config
        .as_ref()
        .map_or_else(|| "<engine defaults>".into(), |p| p.display().to_string());
    let effective = EffectiveConfig::build(
        &ServerInput {
            origin: &server_origin,
            text: server.as_deref(),
        },
        &PolicyInput {
            origin: &format!("{ref_name}@{commit}"),
            files: &trusted,
        },
    )
    .map_err(|errors| Failure::config(&errors))
    .with_context(|| format!("trusted policy at `{ref_name}` is invalid"))?;
    Ok((
        effective,
        trusted,
        head,
        PolicySource::TrustedRef { ref_name, commit },
    ))
}

fn migration_changed(file: &FileChange) -> bool {
    [&file.old, &file.new]
        .into_iter()
        .flatten()
        .any(|side| is_migration_path(side.path.as_str()))
}

/// Facts, derived and external evidence, and the CI task gaps of a run.
type Collected = (RunFacts, Vec<Evidence>, Vec<Evidence>, Vec<String>);

/// Semantic (optional) and syntax facts of the snapshot with the evidence derived from them.
/// Imported CI reports come back as external evidence.
fn derived_facts(
    args: &Request,
    snap: &LocalSnapshot,
    effective: &EffectiveConfig,
    now: Timestamp,
) -> anyhow::Result<Collected> {
    let mut ci = ci::import(snap, &args.ci_facts, args.ci_manifest.as_deref())?;
    ci.extend(args.ci_imported.iter().cloned());
    // Off unless the server allows profiles; a refused or failed check is a task gap.
    let profiles = args
        .execution
        .as_ref()
        .unwrap_or(&effective.server.execution);
    let (executed, refused) = if args.comparison.fork {
        (
            Vec::new(),
            execution::refused_for_fork(profiles, &args.checks),
        )
    } else {
        execution::run_profiles(snap, profiles, &args.checks)
    };
    ci.extend(executed);
    let ci_gaps = args
        .ci_gaps
        .iter()
        .cloned()
        .chain(refused.iter().map(ToString::to_string))
        .collect();
    let (facts, lint, architecture) = match args.semantic {
        Switch::On => (
            semantic::collect_facts(snap, &effective.server.tooling)?,
            semantic::collect_lint(snap, &effective.server.tooling)?,
            semantic::collect_architecture(
                snap,
                &effective.server.tooling,
                &effective.repository.config.architecture,
                &effective.repository.config.review.fixture_paths,
            )?,
        ),
        Switch::Off => (Vec::new(), Vec::new(), Vec::new()),
    };
    let effects = &effective.repository.effects;
    let syntax = syntax::collect(snap, effects);
    let ledger = effects::ledger(
        &snap.snapshot.id,
        &snap.manifest,
        &syntax,
        &facts,
        effects::bindings(effects, &snap.manifest),
    );
    let (db_schema, db_derived, db_external) =
        db_schema::collect_evidence(args, snap, effective, &ci, now)?;
    let derived = facts
        .iter()
        .map(|f| f.to_evidence(&snap.snapshot, now))
        .chain(syntax.iter().map(|f| f.to_evidence(&snap.snapshot, now)))
        .chain(ledger.to_evidence(&snap.snapshot, now).transpose())
        .chain(lint.iter().map(|f| f.to_evidence(&snap.snapshot, now)))
        .chain(architecture.iter().map(|f| f.to_evidence(&snap.snapshot, now)))
        .chain(db_derived.into_iter().map(Ok))
        // The MIG obligations cite it; they exist only when a migration path changed.
        .chain(snap.manifest.files.iter().any(migration_changed).then(|| {
            deploy::matrix(effective.repository.deployment.as_ref())
                .to_evidence(&snap.snapshot, now)
        }))
        .collect::<Result<Vec<_>, _>>();
    let external = ci
        .iter()
        .map(|f| f.to_evidence(&snap.snapshot, now))
        .chain(db_external.into_iter().map(Ok))
        .collect::<Result<Vec<_>, _>>();
    let hashed = derived.and_then(|d| external.map(|e| (d, e)));
    let (derived, external) =
        hashed.map_err(|err| Failure::no_result(format!("cannot hash facts: {err}")))?;
    let facts = RunFacts {
        schema_version: SchemaVersion,
        semantic: facts,
        syntax,
        ci,
        ledger,
        lint,
        architecture,
        db_schema,
        schema_refs: schema::schema_refs(snap),
    };
    Ok((facts, derived, external, ci_gaps))
}

/// The routing of `--models`, resolved with its credentials; `None` for `fake`.
fn model_profile(
    args: &Request,
    fake_responses: bool,
    server: &ServerConfig,
) -> anyhow::Result<Option<Routing>> {
    let models = ModelSelection::for_review(args.models.as_ref(), server.default_models.as_deref());
    let selected = match &models {
        ModelSelection::Fake => return Ok(None),
        ModelSelection::Roles => None,
        ModelSelection::Profile(name) => Some(name.as_str()),
    };
    let config = args
        .config
        .as_deref()
        .ok_or_else(|| Failure::invalid("a real model profile needs --config".to_owned()))?;
    if fake_responses {
        return Err(Failure::invalid("--fake-responses only works with --models fake").into());
    }

    let routing = backend::routing(config, server, selected, args.deep, args.run_mode).map_err(
        |e| match e {
            ResolveError::Invalid(e) => Failure::invalid(redact(&e)),
            ResolveError::Credentials(e) => Failure::permission(e),
        },
    )?;
    Ok(Some(routing))
}

/// Where a review's run lives: a new local run, or a server run a worker claimed.
enum Target {
    Local {
        change_key: String,
        kind: LocalRun,
        local_head: Option<LocalHead>,
    },
    /// Every write carries `run.fence`; the run stops when `cancel` turns true or drops.
    Claimed {
        run: RunHandle,
        cancel: watch::Receiver<bool>,
    },
}

enum LocalRun {
    Evaluation,
    ConfiguredCli,
    DefaultCli,
}

impl Target {
    fn local(args: &Request, kind: LocalRun, local_head: Option<LocalHead>) -> Self {
        Self::Local {
            change_key: format!("{}...{}", args.comparison.base, args.comparison.head),
            kind,
            local_head,
        }
    }

    fn is_local_cli(&self) -> bool {
        matches!(
            self,
            Self::Local {
                kind: LocalRun::ConfiguredCli | LocalRun::DefaultCli,
                ..
            }
        )
    }
}

/// The pinned comparison and the trusted policy a review reads.
pub struct Pinned {
    pub snap: LocalSnapshot,
    pub effective: EffectiveConfig,
    trusted: PolicyFiles,
    head: PolicyFiles,
    source: PolicySource,
}

impl Pinned {
    pub fn policy(&self) -> anyhow::Result<PolicySnapshot> {
        self.effective
            .policy_snapshot(self.source.clone(), Timestamp::now())
            .map_err(|err| Failure::no_result(format!("cannot hash policy: {err}")).into())
    }

    /// Whether the trusted policy names `review.block_on` rules, so a failed run fails its check.
    pub fn blocks(&self) -> bool {
        !self.effective.repository.config.review.block_on.is_empty()
    }
}

/// Commit messages a prompt names at most; the newest come first.
const MAX_INTENT_COMMITS: usize = 20;
/// Bytes of an `--intent-file` read at most; a prompt shows its first 8000 characters.
const MAX_INTENT_FILE_BYTES: u64 = 1 << 20;

/// `--intent-file`, else the commit messages of the change. History that cannot be read leaves
/// no intent rather than failing the review: intent is optional context.
fn intent(args: &Request, snap: &LocalSnapshot) -> anyhow::Result<Option<String>> {
    let Some(path) = &args.intent_file else {
        let messages = snap.commit_messages(MAX_INTENT_COMMITS).ok();
        if args.head_kind == SnapshotHeadKind::Committed {
            return Ok(messages.map(|m| m.join("\n\n")));
        }

        let mut messages = messages.unwrap_or_default();
        messages.insert(
            0,
            "The uncommitted changes have no commit message of their own.".into(),
        );
        return Ok(Some(messages.join("\n\n")));
    };
    let read = fs::metadata(path)
        .and_then(|m| {
            if m.len() > MAX_INTENT_FILE_BYTES {
                Err(io::Error::other("larger than 1 MiB"))
            } else {
                fs::read(path)
            }
        })
        .map_err(|e| Failure::invalid(format!("--intent-file {}: {e}", path.display())))?;
    Ok(Some(String::from_utf8_lossy(&read).into_owned()))
}

/// Pins the comparison and reads the trusted policy at its target tip.
pub fn pin(args: &Request) -> anyhow::Result<Pinned> {
    let snap = snapshot::capture_with_kind(&args.comparison, args.head_kind)
        .context("cannot pin the comparison")?;
    let (effective, trusted, head, source) = trusted_config(args, &snap)?;
    Ok(Pinned {
        snap,
        effective,
        trusted,
        head,
        source,
    })
}

/// Adds server-recorded waivers to their rules as exceptions; statuses stay as assessed.
fn waive(effective: &mut EffectiveConfig, waivers: &[(String, RuleException)]) {
    for (rule, exception) in waivers {
        if let Some(r) = effective
            .repository
            .rules
            .iter_mut()
            .find(|r| &r.id == rule)
        {
            r.exceptions.push(exception.clone());
        }
    }
}

/// Reviews and saves a new run in the local CLI store.
pub fn run_local_cli(mut args: Request, local_head: Option<LocalHead>) -> anyhow::Result<Report> {
    let kind = if args.config.is_some() {
        LocalRun::ConfiguredCli
    } else {
        args.config = config::local_path(None)?;
        LocalRun::DefaultCli
    };
    if let Some(head) = &local_head {
        head.commit().hex().clone_into(&mut args.comparison.head);
    }

    let target = Target::local(&args, kind, local_head);
    execute(&args, None, target).map(|(report, ..)| report)
}

/// Runs an evaluation, returning all candidates and the packets a finished discovery searched.
pub fn run_with_candidates(
    args: &Request,
    store: Option<StoreConfig>,
) -> anyhow::Result<(Report, Vec<Candidate>, Vec<usize>)> {
    execute(args, store, Target::local(args, LocalRun::Evaluation, None))
}

/// Reviews into an existing server run under its lease.
pub fn run_claimed(
    args: &Request,
    store: StoreConfig,
    run: RunHandle,
    cancel: watch::Receiver<bool>,
) -> anyhow::Result<Report> {
    execute(args, Some(store), Target::Claimed { run, cancel }).map(|(report, ..)| report)
}

#[tracing::instrument(name = "review", skip_all, fields(base = %args.comparison.base, head = %args.comparison.head))]
#[allow(clippy::too_many_lines)] // one review in order: pin, facts, plan, analysis, save
fn execute(
    args: &Request,
    store: Option<StoreConfig>,
    target: Target,
) -> anyhow::Result<(Report, Vec<Candidate>, Vec<usize>)> {
    let invalid_fake = |err: String| Failure::invalid(format!("invalid fake responses: {err}"));
    let fake = args
        .fake_responses
        .as_deref()
        .map(FakeResponses::read)
        .transpose()
        .map_err(invalid_fake)?;
    let Pinned {
        snap,
        mut effective,
        trusted,
        head,
        source,
    } = pin(args)?;
    waive(&mut effective, &args.waivers);
    let model = model_profile(args, fake.is_some(), &effective.server)?;
    let store = match (store, &target) {
        (Some(store), _) => store,
        (
            None,
            Target::Local {
                kind: LocalRun::DefaultCli,
                ..
            },
        ) => storage::local_store_config(args.config.as_deref(), &effective.server)
            .context("cannot locate the store")?,
        (None, _) => storage::store_config(args.config.as_deref(), &effective.server)
            .context("cannot locate the store")?,
    };
    if args.policy_ref.is_some() {
        tracing::warn!(source = ?source, "explicit policy revision instead of the target tip");
    }

    let now = Timestamp::now();
    let policy = effective
        .policy_snapshot(source, now)
        .map_err(|err| Failure::no_result(format!("cannot hash policy: {err}")))?;
    let script = match fake {
        Some(fake) => fake.script(&snap, now).map_err(invalid_fake)?,
        None => Scripted::default(),
    };
    let (facts, derived, external, ci_gaps) =
        derived_facts(args, &snap, &effective, now).context("cannot collect facts")?;
    tracing::info!(
        semantic = facts.semantic.len(),
        syntax = facts.syntax.len(),
        ci = facts.ci.len(),
        derived_evidence = derived.len(),
        external_evidence = external.len(),
        "facts collected"
    );
    let plan = plan(
        &snap.snapshot.id,
        &snap.manifest,
        &effective.repository,
        &facts,
    );
    tracing::info!(
        units = plan.scope.units.len(),
        obligations = plan.obligations.len(),
        unreviewed = plan.unreviewed.len(),
        "plan fixed"
    );
    let policy_review = PolicyReview {
        schema_version: SchemaVersion,
        provenance: effective.provenance,
        diff: policy_diff(&effective.repository, &trusted, &head, &effective.server),
    };
    let session = Session {
        snap: &snap,
        policy: &policy,
        policy_review: &policy_review,
        plan: &plan,
        script,
        facts: &facts,
        derived,
        external,
        limits: Limits::from_settings(&effective.repository.config.context),
        model: model.as_ref(),
        budgets: &effective.server.budgets,
        risk_audit: args.risk_audit,
        verify: args.verify,
        expansion_rounds: effective.repository.config.context.expansion_rounds,
        intent: intent(args, &snap)?,
        replay: args.replay.as_deref(),
        ci_gaps: &ci_gaps,
        parent: args.parent,
        language: effective.repository.config.review.language,
        fixture_paths: &effective.repository.config.review.fixture_paths,
        security: effective
            .repository
            .config
            .review
            .profiles
            .contains(&ReviewProfile::Security),
        block_on: &effective.repository.config.review.block_on,
        rules: &effective.repository.rules,
        waivers: &args.waivers,
        schema_config: effective.repository.effective_schema_lint_config(),
        now,
    };
    let saved =
        storage::block_on(session.save(&store, target))?.map_err(|stopped| match stopped {
            Stopped::Failed(err) => Failure::no_result(format!("{err}; no result was saved")),
            Stopped::Cancelled => Failure::Cancelled("cancelled; no result was saved".into()),
        })?;
    let report = Report {
        snapshot: Some(snap.snapshot),
        diff_manifest: Some(snap.manifest),
        policy,
        policy_review,
        external_input_manifest: saved.external,
        evidence_manifest: saved.evidence_manifest,
        evidence: saved.evidence,
        semantic_facts: facts.semantic,
        syntax_facts: facts.syntax,
        ci_facts: facts.ci,
        effect_ledger: facts.ledger,
        lint_facts: facts.lint,
        architecture_facts: facts.architecture,
        db_schema_facts: facts.db_schema,
        context_packets: saved.packets,
        review_result: saved.result,
    };
    Ok((report, saved.candidates, saved.searched))
}

struct Saved {
    external: ExternalInputManifest,
    evidence_manifest: EvidenceManifest,
    evidence: Vec<Evidence>,
    packets: Vec<EvidencePacket>,
    result: ReviewResult,
    /// Every candidate of the model passes, verified or not.
    candidates: Vec<Candidate>,
    /// Packets a finished general discovery searched, by index into `packets`; an
    /// obligation's risk audit looks for that rule only, so it does not count.
    searched: Vec<usize>,
}

enum Stopped {
    Failed(String),
    Cancelled,
}

struct Session<'a> {
    snap: &'a LocalSnapshot,
    policy: &'a PolicySnapshot,
    policy_review: &'a PolicyReview,
    plan: &'a ReviewPlan,
    script: Scripted,
    facts: &'a RunFacts,
    /// Evidence derived in this run from `facts`.
    derived: Vec<Evidence>,
    /// Imported CI reports, frozen as external inputs.
    external: Vec<Evidence>,
    limits: Limits,
    /// The resolved route plan; `None` runs the fake adapter.
    model: Option<&'a Routing>,
    budgets: &'a Budgets,
    risk_audit: bool,
    verify: bool,
    /// `context.expansion_rounds` of the trusted policy.
    expansion_rounds: u32,
    /// `--intent-file`, else the change's commit messages: for the discovery, review and
    /// expansion prompts.
    intent: Option<String>,
    /// `eval measure --replay`: these discoveries are verified again instead of a new search.
    replay: Option<&'a [Discovered]>,
    /// Expected CI reports that are not evidence; each is a task gap.
    ci_gaps: &'a [String],
    /// The parent run whose checkpoints may answer this run's tasks.
    parent: Option<sych_core::RunId>,
    /// `review.language` of the trusted policy.
    language: sych_core::ReviewLanguage,
    fixture_paths: &'a [String],
    /// The `security` profile: added lines are scanned for secrets and hidden text, and
    /// discovery also hunts security defects.
    security: bool,
    /// `review.block_on` of the trusted policy.
    block_on: &'a [String],
    /// The trusted repository rules: a required `ARCH-*` rule makes an architecture finding a
    /// rule violation.
    rules: &'a [Rule],
    /// Server-recorded scoped waivers, also checked against built-in `DB-*` rules.
    waivers: &'a [(String, RuleException)],
    /// Effective trusted schema linter config (inline `.sych/config.yaml` + optional `.sych/schema-lint.yaml`).
    schema_config: DbSchemaConfig,
    now: Timestamp,
}
