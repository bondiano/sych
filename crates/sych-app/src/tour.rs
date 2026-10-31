//! `sych tour`: a guided tour of a saved run. The plan is
//! deterministic, the model writes the prose of its stops, and the artifact is validated
//! against the result and the re-captured snapshot before it is stored, never rewritten.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::future::Future;
use std::process::ExitCode;

use anyhow::Context as _;
use serde::Serialize;
use sych_adapters::git::LocalSnapshot;
use sych_core::analysis::tour_prompt::{self, TourDraft};
use sych_core::config::{Role, RunMode};
use sych_core::model::{AttemptRecord, DataBlock, Limits, Prompt, Purpose};
use sych_core::tour::{self, TourArtifact, TourEvent, TourPlan, TourState};
use sych_core::{ComparisonMode, MicroUsd, ReviewResult, RunId, SnapshotReader, Timestamp};
use sych_storage::{ArtifactKind, RunLedger, Storage, TENANT_LOCAL, tour_role};

use crate::args::{Comparison, ModelSelection, TourArguments};
use crate::backend::{self, ResolveError};
use crate::errors::Failure;
use crate::output::emit_as;
use crate::storage::{block_on, load_store_config, saved_report};
use crate::{config, snapshot, tour_reader};

/// Code shown to the guide; the rest of a long stop is cut, the reader still renders it all.
const MAX_SPAN_CHARS: usize = 6_000;

#[derive(Serialize)]
pub struct TourOutput {
    pub state: TourState,
    pub tour: Option<TourArtifact>,
    pub problems: Vec<String>,
    pub attempts: usize,
    pub cost_micro_usd: u64,
}

/// The stops' code from the pinned blobs, as data blocks for the guide.
fn spans(plan: &TourPlan, reader: &dyn SnapshotReader) -> Vec<DataBlock> {
    plan.stops
        .iter()
        .flat_map(|s| s.code_refs.iter().map(move |r| (s, r)))
        .filter_map(|(s, r)| {
            let span = reader.read_span(r).ok()?;
            Some(DataBlock {
                label: format!("{} {}:{}-{}", s.id, r.path, r.start_line, r.end_line),
                content: span.text.chars().take(MAX_SPAN_CHARS).collect(),
            })
        })
        .collect()
}

/// Generates until a valid tour or the attempt cap: an invalid tour gets one
/// regeneration told what was wrong; `call` is the model.
pub async fn generate<F, Fut>(
    plan: &TourPlan,
    result: &ReviewResult,
    reader: &dyn SnapshotReader,
    provenance: &str,
    mut call: F,
) -> (TourState, Option<TourArtifact>, Vec<String>)
where
    F: FnMut(Prompt) -> Fut,
    Fut: Future<Output = Result<TourDraft, String>>,
{
    let blocks = spans(plan, reader);
    let mut state = tour::next(&TourState::NotRequested, TourEvent::Requested);
    let mut problems = Vec::new();
    while let TourState::Generating { .. } = state {
        let prompt = tour_prompt::prompt(plan, result, blocks.clone(), &problems);
        let draft = match call(prompt).await {
            Ok(draft) => draft,
            Err(e) => return (tour::next(&state, TourEvent::Error), None, vec![e]),
        };
        state = tour::next(&state, TourEvent::Generated);
        let (artifact, mut found) =
            tour_prompt::assemble(plan, result, draft, provenance.to_owned());
        found.extend(tour::validate(&artifact, result, reader));
        if found.is_empty() {
            return (
                tour::next(&state, TourEvent::Valid),
                Some(artifact),
                Vec::new(),
            );
        }

        problems = found;
        state = tour::next(&state, TourEvent::Invalid);
    }
    (state, None, problems)
}

fn markdown(out: &TourOutput, reader: &LocalSnapshot) -> String {
    let mut text = String::new();
    let Some(t) = &out.tour else {
        let _ = writeln!(text, "Tour {:?}: {}", out.state, out.problems.join("; "));
        return text;
    };
    let _ = writeln!(
        text,
        "# Tour of result {} (revision {})\n\nCoverage {:?}; critical findings {}; required UNKNOWN {}.\n",
        t.result_id,
        t.review_revision,
        t.overview.coverage,
        t.overview.critical_findings.len(),
        t.overview.important_unknowns.len()
    );
    for step in &t.steps {
        tour_reader::write_step(&mut text, step, Some(reader), &mut Vec::new());
    }
    let _ = writeln!(
        text,
        "_{} attempts, {} micro-USD; explanations are generated and may be wrong; statuses are the review's._",
        out.attempts, out.cost_micro_usd
    );
    text
}

fn cost(attempts: &[AttemptRecord]) -> u64 {
    attempts
        .iter()
        .filter_map(|a| a.charge)
        .map(|MicroUsd(c)| c)
        .sum()
}

pub fn run(args: &TourArguments) -> anyhow::Result<ExitCode> {
    let selected = match &args.models {
        ModelSelection::Fake => return Err(Failure::invalid("a tour needs a model profile").into()),
        ModelSelection::Roles => None,
        ModelSelection::Profile(name) => Some(name.as_str()),
    };
    let (Some(run), Some(config)) = (&args.run, &args.config) else {
        return Err(Failure::invalid("a tour needs --run and --config").into());
    };
    let effective = config::effective(Some(config), Some(&args.repo))?;
    let report = saved_report(Some(config), run)?;
    let result = report.review_result;
    let saved = report
        .snapshot
        .ok_or_else(|| Failure::invalid("the run's source snapshot was pruned; review again"))?;
    let snap = snapshot::capture(&Comparison {
        repo: args.repo.clone(),
        base: saved.target_tip.hex().to_owned(),
        head: saved.head.hex().to_owned(),
        diff_base: (saved.comparison == ComparisonMode::ExplicitDiffBase)
            .then(|| saved.diff_base.hex().to_owned()),
    })?;
    if snap.snapshot.id != result.source_snapshot_id {
        return Err(Failure::invalid("--repo is not the repository this run reviewed").into());
    }

    let server = &effective.server;
    let routing = backend::routing(config, server, selected, false, RunMode::PersonalLocal)
        .map_err(|e| match e {
            ResolveError::Invalid(e) | ResolveError::Credentials(e) => Failure::invalid(e),
        })?;
    let plan = tour::plan(
        &result,
        effective.repository.config.tour.target_steps as usize,
    );
    let run: RunId = result.run_id;
    let store = load_store_config(Some(config))?;
    let out = block_on(async {
        let storage = Storage::open(&store).await?;
        let handle = storage.run_handle(TENANT_LOCAL, run).await?;
        let ledger = RunLedger::new(&storage, &handle, Timestamp::now);
        let limits = Limits::new(&server.budgets, Purpose::Tour);
        let router = routing.router(&ledger, run, Purpose::Tour, limits, None, BTreeMap::new());
        let attempts = RefCell::new(Vec::new());
        let provenance = format!("{:?}", routing.plan);
        let (state, tour, problems) = generate(&plan, &result, &snap, &provenance, |prompt| {
            let (router, attempts) = (&router, &attempts);
            async move {
                let outcome = router
                    .structured::<TourDraft>(Role::Reviewer, &prompt, "tour")
                    .await;
                attempts.borrow_mut().extend(outcome.attempts);
                outcome.value.map_err(|e| format!("{e:?}"))
            }
        })
        .await;
        if let Some(t) = &tour {
            storage
                .put_artifact(
                    &handle,
                    &tour_role(t.tour_id),
                    ArtifactKind::Tour,
                    t,
                    Timestamp::now(),
                )
                .await?;
        }

        let attempts = attempts.into_inner();
        anyhow::Ok(TourOutput {
            state,
            tour,
            problems,
            attempts: attempts.len(),
            cost_micro_usd: cost(&attempts),
        })
    })?
    .context("cannot generate the tour")?;
    emit_as(&out, || markdown(&out, &snap), &args.out)?;
    Ok(if out.tour.is_some() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use sych_core::analysis::tour_prompt::{DraftStep, TourDraft};
    use sych_core::{
        CodeRef, DiffManifest, ReadError, RepoPath, SourceSide, SourceSnapshotId, SourceSpan,
    };

    use super::*;

    struct NoCode;

    impl SnapshotReader for NoCode {
        fn read_span(&self, r: &CodeRef) -> Result<SourceSpan, ReadError> {
            Err(ReadError::UnknownSnapshot(r.path.to_string()))
        }

        fn code_ref(
            &self,
            _: SourceSide,
            p: &RepoPath,
            _: u32,
            _: u32,
        ) -> Result<CodeRef, ReadError> {
            Err(ReadError::UnknownSnapshot(p.to_string()))
        }

        fn search(
            &self,
            _: SourceSide,
            _: &str,
            _: &[RepoPath],
            _: usize,
        ) -> Result<Vec<CodeRef>, ReadError> {
            Ok(Vec::new())
        }

        fn diff(&self, s: &SourceSnapshotId) -> Result<DiffManifest, ReadError> {
            Err(ReadError::UnknownSnapshot(s.0.to_string()))
        }
    }

    fn result() -> ReviewResult {
        let text = include_str!("../../../fixtures/review/violated.bundle.json");
        let bundle: serde_json::Value = serde_json::from_str(text).expect("bundle");
        serde_json::from_value(bundle["review_result"].clone()).expect("result")
    }

    fn draft(stop: &str) -> TourDraft {
        TourDraft {
            intent: "orders publish their event".into(),
            steps: vec![DraftStep {
                stop_id: stop.into(),
                title: "The lost event".into(),
                purpose: "Why the order event is not published".into(),
                explanation: "The rule requires the event in the transaction.".into(),
                question: None,
                example_snippets: Vec::new(),
            }],
        }
    }

    fn run(
        answers: &[Result<&str, &str>],
    ) -> (TourState, Option<TourArtifact>, Vec<String>, usize) {
        let r = result();
        let plan = tour::plan(&r, 5);
        let calls = Cell::new(0);
        let (state, t, problems) = block_on(generate(&plan, &r, &NoCode, "fake", |_| {
            let answer = answers[calls.get()];
            calls.set(calls.get() + 1);
            async move { answer.map(draft).map_err(str::to_owned) }
        }))
        .expect("runtime");
        (state, t, problems, calls.get())
    }

    #[test]
    fn an_invalid_draft_gets_one_regeneration() {
        let (state, t, _, calls) = run(&[Ok("stop-9"), Ok("stop-1")]);
        assert_eq!((state, calls), (TourState::Ready, 2));
        let t = t.expect("tour");
        assert_eq!(t.steps[0].id, "stop-1");
        assert_eq!(
            t.overview,
            tour::plan(&result(), 5).overview,
            "overview from the result"
        );

        let (state, t, problems, calls) = run(&[Ok("stop-9"), Ok("stop-8")]);
        assert!(matches!(state, TourState::Failed { .. }) && t.is_none() && calls == 2);
        assert!(
            problems.iter().any(|p| p.contains("stop-8 is not a stop")),
            "{problems:?}"
        );

        let (state, _, problems, calls) = run(&[Err("quota")]);
        assert!(matches!(state, TourState::Failed { .. }) && calls == 1);
        assert_eq!(problems, ["quota"]);
    }
}
