//! `sych ask`: a bounded follow-up on one stop of a stored tour. A
//! comprehension question calls the model under the session's own `budgets.followup_session`,
//! with the stop's code and at most one round of typed context queries; a defect claim in the
//! answer goes through the regular verifier and stays in the session. A fact is the reader's
//! own statement; a risk decision is a repository owner's waiver, which a local session cannot
//! create. Nothing here changes the tour, the result or its obligations, or is published.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::mem;
use std::path::Path;
use std::process::ExitCode;

use sych_core::analysis::followup::{self, Exchange, FOLLOWUP, FollowupOutput, Reply};
use sych_core::analysis::{Context, VerifyOutput, context_asks, prompts, validate_verdicts};
use sych_core::broker::Ops;
use sych_core::config::{Role, RunMode};
use sych_core::model::{AttemptRecord, Ledger, Limits, ModelProvider, Prompt, Purpose, Router};
use sych_core::tour::{QuestionKind, TourArtifact, TourStep};
use sych_core::{
    GENERATED_IN_CHANGE, MicroUsd, RepoPath, ReviewResult, RunId, SchemaVersion, SnapshotReader,
    Timestamp, VerificationStatus,
};
use sych_storage::{RunLedger, Storage, TENANT_LOCAL};

use crate::args::{AskArguments, AskKind, ModelSelection};
use crate::backend::{self, ResolveError};
use crate::errors::Failure;
use crate::output::emit_as;
use crate::render::Report;
use crate::storage::{block_on, load_store_config, saved_report};
use crate::{config, tour_reader};

/// Spans one context query may add, as in analysis.
const MAX_REFS_PER_ANSWER: usize = 3;

/// The two calls a follow-up makes.
trait Model {
    async fn followup(&self, prompt: &Prompt) -> Result<FollowupOutput, String>;
    async fn verify(&self, prompt: &Prompt) -> Result<VerifyOutput, String>;
}

struct Routed<'a, P, L> {
    router: &'a Router<'a, P, L>,
    attempts: RefCell<Vec<AttemptRecord>>,
}

impl<P: ModelProvider, L: Ledger> Model for Routed<'_, P, L> {
    async fn followup(&self, prompt: &Prompt) -> Result<FollowupOutput, String> {
        let outcome = self
            .router
            .structured::<FollowupOutput>(Role::Reviewer, prompt, "followup")
            .await;
        self.attempts.borrow_mut().extend(outcome.attempts);
        outcome.value.map_err(|e| format!("{e:?}"))
    }

    async fn verify(&self, prompt: &Prompt) -> Result<VerifyOutput, String> {
        let outcome = self
            .router
            .structured::<VerifyOutput>(Role::Verifier, prompt, "candidate_verdicts")
            .await;
        self.attempts.borrow_mut().extend(outcome.attempts);
        outcome.value.map_err(|e| format!("{e:?}"))
    }
}

/// The answer to `question`, or why the first call gave none (a spent session budget among
/// them). Later calls that fail leave a limitation instead.
async fn answer(
    model: &impl Model,
    (tour, step): (&TourArtifact, &TourStep),
    result: &ReviewResult,
    ops: &Ops<'_>,
    question: &str,
) -> Result<Reply, String> {
    let reader = ops.reader;
    let mut context = Context::new(result.language);
    for r in &step.code_refs {
        context.add_span(r, "the stop's code", reader);
    }
    let mut limitations = Vec::new();
    let ask = |context: &Context| followup::prompt(tour, step, result, context, question);
    let mut out = model.followup(&ask(&context)).await?;
    let (asks, rejected) = context_asks(mem::take(&mut out.context_requests));
    limitations.extend(rejected);
    if !asks.is_empty() {
        for a in &asks {
            limitations.extend(a.answer(ops, &mut context, MAX_REFS_PER_ANSWER).limitations);
        }
        match model.followup(&ask(&context)).await {
            Ok(second) => out = second,
            Err(e) => limitations.push(format!("the answered context was not used: {e}")),
        }
        if !out.context_requests.is_empty() {
            limitations.push("further context requests: one round per question".into());
        }
    }

    let code_refs = context.anchors(&out.anchors, reader, &mut limitations);
    let key = format!("{FOLLOWUP}:{}:{}:{question}", tour.tour_id, step.id);
    let (candidates, dropped) = followup::hypotheses(&out, &key, step, &context, reader);
    limitations.extend(dropped);
    let hypotheses = if candidates.is_empty() {
        candidates
    } else {
        match model
            .verify(&prompts::verify(&context, &candidates, &[]))
            .await
        {
            Ok(v) => validate_verdicts(&v, candidates, &context, reader, &[], &key).candidates,
            Err(e) => candidates
                .into_iter()
                .map(|mut c| {
                    c.verification_status = VerificationStatus::Unresolved;
                    c.limitations.push(format!("not verified: {e}"));
                    c
                })
                .collect(),
        }
    };
    Ok(Reply::Explanation {
        text: out.answer,
        code_refs,
        hypotheses,
        limitations,
        attempts: 0,
        cost_micro_usd: 0,
    })
}

/// One line of a reply, for listings.
pub fn reply_text(reply: &Reply) -> String {
    match reply {
        Reply::Explanation { text, .. } | Reply::HumanStatement { text } => text.clone(),
        Reply::RiskDecision { waiver_id, rule_id } => {
            format!("waiver {waiver_id} for {rule_id}; the review's statuses are unchanged")
        }
    }
}

fn markdown(e: &Exchange) -> String {
    let mut text = format!("> {:?} on {}: {}\n\n", e.kind, e.step_id, e.question);
    match &e.reply {
        Reply::HumanStatement { text: s } => {
            let _ = writeln!(
                text,
                "Recorded as your statement ({}, {}); it is not evidence and changes no requirement.\n\n{s}",
                e.actor, e.at
            );
        }
        Reply::RiskDecision { .. } => {
            let _ = writeln!(text, "{}", reply_text(&e.reply));
        }
        Reply::Explanation {
            text: answer,
            code_refs,
            hypotheses,
            limitations,
            attempts,
            cost_micro_usd,
        } => {
            let _ = writeln!(text, "{answer}\n");
            for r in code_refs {
                let _ = writeln!(text, "- `{}:{}-{}`", r.path, r.start_line, r.end_line);
            }
            for h in hypotheses {
                let _ = writeln!(
                    text,
                    "- hypothesis {:?}: {} (not a finding of the review; a rerun reviews again)",
                    h.verification_status, h.claim
                );
            }
            for l in limitations {
                let _ = writeln!(text, "- limitation: {l}");
            }
            let _ = writeln!(
                text,
                "\n_{attempts} attempts, {cost_micro_usd} micro-USD of the session's follow-up budget; generated and may be wrong._"
            );
        }
    }
    text
}

/// The step of `tour` a question names.
pub fn step<'a>(tour: &'a TourArtifact, id: &str) -> Result<&'a TourStep, String> {
    tour.steps
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("the tour has no step `{id}`"))
}

/// A new exchange of `actor`'s session on `step`.
pub fn exchange(
    (tour, step): (&TourArtifact, &TourStep),
    actor: &str,
    (kind, question): (QuestionKind, &str),
    at: Timestamp,
    reply: Reply,
) -> Exchange {
    Exchange {
        schema_version: SchemaVersion,
        id: uuid::Uuid::now_v7().to_string(),
        tour_id: tour.tour_id,
        step_id: step.id.clone(),
        actor: actor.to_owned(),
        kind,
        question: question.to_owned(),
        at,
        reply,
    }
}

pub fn run(args: &AskArguments) -> anyhow::Result<ExitCode> {
    let r = &args.tour;
    let run = tour_reader::run_id(&r.run)?;
    let kind = match args.kind {
        AskKind::Comprehension => QuestionKind::Comprehension,
        AskKind::FactRequest => QuestionKind::FactRequest,
        AskKind::RiskDecision => {
            return Err(Failure::permission(
                "a risk decision is a scoped waiver a repository owner accepts on the server \
                 (`POST /v1/runs/:id/tours/:tour/questions` or `POST /v1/waivers`) or in the \
                 accepted `.sych/rules.yaml`; a local session cannot create one",
            )
            .into());
        }
    };
    let store = load_store_config(r.config.as_deref())?;
    let report = saved_report(r.config.as_deref(), &r.run)?;
    let actor = tour_reader::LOCAL_ACTOR;
    let exchange = block_on(async {
        let storage = Storage::open(&store).await?;
        let (tour, _) = tour_reader::load(&storage, run, r).await?;
        let step = step(&tour, &args.step).map_err(Failure::invalid)?;
        let reply = if kind == QuestionKind::FactRequest {
            Reply::HumanStatement {
                text: args.question.clone(),
            }
        } else {
            let config = r.config.as_deref().ok_or_else(|| {
                Failure::invalid("a question needs --config with the model profiles")
            })?;
            let reader =
                tour_reader::pinned(report.snapshot.as_ref(), &args.repo).ok_or_else(|| {
                    Failure::no_result(
                        "the run's source is unavailable (pruned, or --repo is not the \
                         reviewed repository)",
                    )
                })?;
            let asker = Asker {
                config,
                models: &args.models,
                mode: RunMode::PersonalLocal,
                storage: &storage,
                tenant: TENANT_LOCAL,
                actor,
            };
            explain(&asker, run, (&tour, step), &report, &reader, &args.question).await?
        };
        let exchange = exchange(
            (&tour, step),
            actor,
            (kind, &args.question),
            Timestamp::now(),
            reply,
        );
        storage.add_exchange(TENANT_LOCAL, run, &exchange).await?;
        anyhow::Ok(exchange)
    })??;
    emit_as(&exchange, || markdown(&exchange), &r.out)?;
    Ok(ExitCode::SUCCESS)
}

/// Who asks, and where the model profiles and the store are: the CLI's local reader or a
/// server caller.
pub struct Asker<'a> {
    pub config: &'a Path,
    pub models: &'a ModelSelection,
    pub mode: RunMode,
    pub storage: &'a Storage,
    pub tenant: &'a str,
    pub actor: &'a str,
}

/// A comprehension answer under the asker's own follow-up budget; `reader` is the run's
/// pinned snapshot.
pub async fn explain(
    asker: &Asker<'_>,
    run: RunId,
    (tour, step): (&TourArtifact, &TourStep),
    report: &Report,
    reader: &dyn SnapshotReader,
    question: &str,
) -> anyhow::Result<Reply> {
    let selected = match asker.models {
        ModelSelection::Fake => {
            return Err(Failure::invalid("a question needs a model profile").into());
        }
        ModelSelection::Roles => None,
        ModelSelection::Profile(name) => Some(name.as_str()),
    };
    let server = config::server(Some(asker.config))?;
    let routing = backend::routing(asker.config, &server, selected, false, asker.mode).map_err(
        |e| match e {
            ResolveError::Invalid(e) | ResolveError::Credentials(e) => Failure::invalid(e),
        },
    )?;
    let handle = asker.storage.run_handle(asker.tenant, run).await?;
    let ledger = RunLedger::new(asker.storage, &handle, Timestamp::now)
        .scoped(followup::scope(tour.tour_id, asker.actor));
    let limits = Limits::new(&server.budgets, Purpose::Followup);
    let router = routing.router(
        &ledger,
        run,
        Purpose::Followup,
        limits,
        None,
        BTreeMap::new(),
    );
    let model = Routed {
        router: &router,
        attempts: RefCell::new(Vec::new()),
    };
    // The run's own account of what was generated: the diff manifest may be gone by now.
    let generated: Vec<RepoPath> = report
        .review_result
        .excluded_or_unreviewed
        .iter()
        .filter(|g| matches!(g.reason.as_str(), "GENERATED" | GENERATED_IN_CHANGE))
        .map(|g| g.path.clone())
        .collect();
    let ops = Ops {
        facts: &report.semantic_facts,
        evidence: &report.evidence,
        reader,
        generated: &generated,
    };
    let mut reply = answer(&model, (tour, step), &report.review_result, &ops, question)
        .await
        .map_err(|e| {
            Failure::no_result(format!(
                "no answer within the session's follow-up budget and routes: {e}"
            ))
        })?;
    let spent = model.attempts.into_inner();
    if let Reply::Explanation {
        attempts,
        cost_micro_usd,
        ..
    } = &mut reply
    {
        *attempts = spent.len();
        *cost_micro_usd = spent
            .iter()
            .filter_map(|a| a.charge)
            .map(|MicroUsd(c)| c)
            .sum();
    }

    Ok(reply)
}

#[cfg(test)]
mod tests;
