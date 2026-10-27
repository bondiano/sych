//! The review pipeline: snapshot, trusted policy, facts, plan, context packets, model or fake
//! analysis, reduce and a durable result.

use std::process::ExitCode;

use anyhow::Context;

use sych_adapters::git::LocalSnapshot;
use sych_adapters::{execution, syntax};
use sych_core::broker::{EvidencePacket, Limits};
use sych_core::config::Budgets;
use sych_core::config::{
    EffectiveConfig, PolicyFiles, PolicyInput, RuleException, ServerConfig, ServerInput,
    policy_diff,
};
use sych_core::{
    Candidate, CiFacts, Coverage, Evidence, EvidenceManifest, ExternalInputManifest, FileChange,
    PolicySnapshot, PolicySource, Recommendation, ReviewPlan, ReviewResult, RunFacts,
    SchemaVersion, SourceSide, Timestamp, is_migration_path, plan, redact,
};
use sych_core::{deploy, effects};
use sych_storage::{RunHandle, StoreConfig};
use tokio::sync::watch;

use crate::args::{ExitPolicy, ModelSelection, ReviewArguments, Switch};
use crate::backend::{self, ResolveError, Routing};
use crate::errors::{EXIT_INCOMPLETE, EXIT_VIOLATION, Failure};
use crate::fake::{FakeResponses, Scripted};
use crate::render::{PolicyReview, Report};
use crate::{ci, config, semantic, snapshot, storage};

mod checkpoints;
mod outcome;
mod schema;
mod session;

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
    args: &ReviewArguments,
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

/// Semantic (optional) and syntax facts of the snapshot with the evidence derived from them.
/// Imported CI reports come back as external evidence.
fn derived_facts(
    args: &ReviewArguments,
    executed: Vec<CiFacts>,
    snap: &LocalSnapshot,
    effective: &EffectiveConfig,
    now: Timestamp,
) -> anyhow::Result<(RunFacts, Vec<Evidence>, Vec<Evidence>)> {
    let mut ci = ci::import(snap, &args.ci_facts, args.ci_manifest.as_deref())?;
    ci.extend(args.ci_imported.iter().cloned());
    ci.extend(executed);
    let (facts, lint) = match args.semantic {
        Switch::On => (
            semantic::collect_facts(snap, &effective.server.tooling)?,
            semantic::collect_lint(snap)?,
        ),
        Switch::Off => (Vec::new(), Vec::new()),
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
    let derived = facts
        .iter()
        .map(|f| f.to_evidence(&snap.snapshot, now))
        .chain(syntax.iter().map(|f| f.to_evidence(&snap.snapshot, now)))
        .chain(ledger.to_evidence(&snap.snapshot, now).transpose())
        .chain(lint.iter().map(|f| f.to_evidence(&snap.snapshot, now)))
        // The MIG obligations cite it; they exist only when a migration path changed.
        .chain(snap.manifest.files.iter().any(migration_changed).then(|| {
            deploy::matrix(effective.repository.deployment.as_ref())
                .to_evidence(&snap.snapshot, now)
        }))
        .collect::<Result<Vec<_>, _>>();
    let external = ci
        .iter()
        .map(|f| f.to_evidence(&snap.snapshot, now))
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
        schema_refs: schema::schema_refs(snap),
    };
    Ok((facts, derived, external))
}

/// The routing of `--models`, resolved with its credentials; `None` for `fake`.
fn model_profile(
    args: &ReviewArguments,
    fake_responses: bool,
    server: &ServerConfig,
) -> anyhow::Result<Option<Routing>> {
    let selected = match &args.models {
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
pub enum Target {
    Local {
        change_key: String,
    },
    /// Every write carries `run.fence`; the run stops when `cancel` turns true or drops.
    Claimed {
        run: RunHandle,
        cancel: watch::Receiver<bool>,
    },
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
}

/// Pins the comparison and reads the trusted policy at its target tip.
pub fn pin(args: &ReviewArguments) -> anyhow::Result<Pinned> {
    let snap = snapshot::capture(&args.comparison).context("cannot pin the comparison")?;
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

/// Runs and saves one review. `store` overrides the configured storage (eval runs).
pub fn run(args: &ReviewArguments, store: Option<StoreConfig>) -> anyhow::Result<Report> {
    run_with_candidates(args, store).map(|(report, _)| report)
}

/// [`run`], also returning every model candidate, verified or not (evaluation baselines).
pub fn run_with_candidates(
    args: &ReviewArguments,
    store: Option<StoreConfig>,
) -> anyhow::Result<(Report, Vec<Candidate>)> {
    let change_key = format!("{}...{}", args.comparison.base, args.comparison.head);
    execute(args, store, Target::Local { change_key })
}

/// Reviews into an existing server run under its lease (spec §8.2).
pub fn run_claimed(
    args: &ReviewArguments,
    store: StoreConfig,
    run: RunHandle,
    cancel: watch::Receiver<bool>,
) -> anyhow::Result<Report> {
    execute(args, Some(store), Target::Claimed { run, cancel }).map(|(report, _)| report)
}

#[tracing::instrument(name = "review", skip_all, fields(base = %args.comparison.base, head = %args.comparison.head))]
fn execute(
    args: &ReviewArguments,
    store: Option<StoreConfig>,
    target: Target,
) -> anyhow::Result<(Report, Vec<Candidate>)> {
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
    let store = match store {
        Some(store) => store,
        None => storage::store_config(args.config.as_deref(), &effective.server)
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
    // Off unless the server allows profiles; a refused or failed check is a task gap.
    let (executed, refused) = execution::run_profiles(&snap, &effective.server.execution);
    let ci_gaps: Vec<String> = args
        .ci_gaps
        .iter()
        .cloned()
        .chain(refused.iter().map(ToString::to_string))
        .collect();
    let (facts, derived, external) =
        derived_facts(args, executed, &snap, &effective, now).context("cannot collect facts")?;
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
        ci_gaps: &ci_gaps,
        parent: args.parent,
        language: effective.repository.config.review.language,
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
        context_packets: saved.packets,
        review_result: saved.result,
    };
    Ok((report, saved.candidates))
}

struct Saved {
    external: ExternalInputManifest,
    evidence_manifest: EvidenceManifest,
    evidence: Vec<Evidence>,
    packets: Vec<EvidencePacket>,
    result: ReviewResult,
    /// Every candidate of the model passes, verified or not.
    candidates: Vec<Candidate>,
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
    /// Expected CI reports that are not evidence; each is a task gap.
    ci_gaps: &'a [String],
    /// The parent run whose checkpoints may answer this run's tasks.
    parent: Option<sych_core::RunId>,
    /// `review.language` of the trusted policy.
    language: sych_core::ReviewLanguage,
    now: Timestamp,
}
