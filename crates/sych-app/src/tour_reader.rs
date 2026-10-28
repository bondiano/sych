//! `sych tour show|reveal|complete|dismiss`: reading a stored tour (spec §21.5/21.8, roadmap
//! M5.2). No model is called; code comes from the run's pinned blobs, a newer head marks the
//! tour `STALE` without replacing its code, and progress is the reader's own session, never the
//! artifact, the recommendation, obligations or an approval, and never published.

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use serde::Serialize;
use sych_adapters::git::{Git, LocalSnapshot};
use sych_core::analysis::followup::Exchange;
use sych_core::tour::{self, SessionState, TourArtifact, TourSession, TourStep};
use sych_core::{ComparisonMode, RunId, SnapshotReader, SourceSnapshot, Timestamp, TourId};
use sych_storage::{Storage, StorageError, TENANT_LOCAL, tour_role};

use crate::args::{Comparison, TourCommands, TourRef};
use crate::errors::Failure;
use crate::output::emit_as;
use crate::storage::{block_on, load_store_config, saved_report};
use crate::{followup, snapshot};

/// The only reader of a local store.
pub(crate) const LOCAL_ACTOR: &str = "local";

#[derive(Serialize)]
struct Shown {
    /// Answers not revealed in this session are blanked.
    tour: TourArtifact,
    session: TourSession,
    /// The tour is of an older revision or the repository moved on; its code is still the run's.
    stale: bool,
    /// Locations whose source was pruned or garbage-collected.
    source_unavailable: Vec<String>,
    /// This reader's own questions and replies, read back without a model.
    exchanges: Vec<Exchange>,
}

#[derive(Serialize)]
struct Revealed<'a> {
    step: &'a str,
    question: &'a str,
    reveal: &'a str,
    session: &'a TourSession,
}

pub(crate) fn run_id(text: &str) -> anyhow::Result<RunId> {
    Ok(RunId(text.parse().map_err(|_| {
        Failure::invalid(format!("`{text}` is not a run id"))
    })?))
}

/// The tour and this reader's session (a new one when absent), read from the store.
pub(crate) async fn load(
    storage: &Storage,
    run: RunId,
    r: &TourRef,
) -> anyhow::Result<(TourArtifact, TourSession)> {
    let id = match &r.tour {
        Some(t) => TourId(
            t.parse()
                .map_err(|_| Failure::invalid(format!("`{t}` is not a tour id")))?,
        ),
        None => storage
            .latest_tour(TENANT_LOCAL, run)
            .await?
            .ok_or_else(|| {
                Failure::invalid("the run has no stored tour; generate one with `sych tour`")
            })?,
    };
    let tour: TourArtifact = match storage
        .load_artifact(TENANT_LOCAL, run, &tour_role(id))
        .await
    {
        Err(StorageError::ArtifactUnavailable(_)) => {
            return Err(
                Failure::no_result("the tour artifact was pruned; generate a new one").into(),
            );
        }
        loaded => loaded?,
    };
    let session = storage
        .tour_session(TENANT_LOCAL, id, LOCAL_ACTOR)
        .await?
        .unwrap_or_else(|| TourSession::start(id, LOCAL_ACTOR));
    Ok((tour, session))
}

/// Opens the store, applies `change` to the session and saves it.
fn with_session<T>(
    r: &TourRef,
    change: impl FnOnce(&TourArtifact, TourSession) -> anyhow::Result<(TourSession, T)>,
) -> anyhow::Result<(TourArtifact, TourSession, T)> {
    let run = run_id(&r.run)?;
    let store = load_store_config(r.config.as_deref())?;
    block_on(async {
        let storage = Storage::open(&store).await?;
        let (tour, session) = load(&storage, run, r).await?;
        let (session, value) = change(&tour, session)?;
        storage
            .save_tour_session(TENANT_LOCAL, run, &session, Timestamp::now())
            .await?;
        Ok((tour, session, value))
    })?
}

pub fn run(action: &TourCommands) -> anyhow::Result<ExitCode> {
    match action {
        TourCommands::Show(a) => show(&a.tour, &a.repo, a.brief),
        TourCommands::Reveal(a) => reveal(&a.tour, &a.step),
        TourCommands::Complete(r) => finish(r, SessionState::Completed),
        TourCommands::Dismiss(r) => finish(r, SessionState::Dismissed),
    }
}

/// The run's snapshot re-read from `repo` at its pinned commits, when it still can be.
pub(crate) fn pinned(
    report_snapshot: Option<&SourceSnapshot>,
    repo: &Path,
) -> Option<LocalSnapshot> {
    let saved = report_snapshot?;
    snapshot::capture(&Comparison {
        repo: repo.to_owned(),
        base: saved.target_tip.hex().to_owned(),
        head: saved.head.hex().to_owned(),
        diff_base: (saved.comparison == ComparisonMode::ExplicitDiffBase)
            .then(|| saved.diff_base.hex().to_owned()),
    })
    .ok()
    .filter(|s| s.snapshot.id == saved.id)
}

fn show(r: &TourRef, repo: &Path, brief: bool) -> anyhow::Result<ExitCode> {
    let report = saved_report(r.config.as_deref(), &r.run)?;
    let (t, session, ()) = with_session(r, |_, s| Ok((s, ())))?;
    let store = load_store_config(r.config.as_deref())?;
    let exchanges = block_on(async {
        let storage = Storage::open(&store).await?;
        anyhow::Ok(
            storage
                .exchanges(TENANT_LOCAL, t.tour_id, LOCAL_ACTOR)
                .await?,
        )
    })??;
    let snap = pinned(report.snapshot.as_ref(), repo);
    let head = Git::open(repo)
        .and_then(|g| g.stdout(["rev-parse", "HEAD"]))
        .map(|o| String::from_utf8_lossy(&o).trim().to_owned());
    let moved = match (&report.snapshot, head) {
        (Some(saved), Ok(head)) => saved.head.hex() != head,
        _ => false,
    };
    let stale = moved || tour::stale(&t, report.review_result.review_revision);
    let reader = snap.as_ref().map(|s| s as &dyn SnapshotReader);
    let mut source_unavailable = Vec::new();
    let mut text = String::new();
    let _ = writeln!(
        text,
        "# Tour {} of result {} (revision {}){}\n\nCoverage {:?}; critical findings {}; required UNKNOWN {}. Session {:?}.\n",
        t.tour_id,
        t.result_id,
        t.review_revision,
        if stale {
            " — STALE: the change moved on; this is the code the review saw"
        } else {
            ""
        },
        t.overview.coverage,
        t.overview.critical_findings.len(),
        t.overview.important_unknowns.len(),
        session.state,
    );
    for step in &t.steps {
        if brief {
            let _ = writeln!(text, "- {}: {}", step.id, step.title);
        } else {
            write_step(&mut text, step, reader, &mut source_unavailable);
        }
    }
    if !brief && !exchanges.is_empty() {
        let _ = writeln!(text, "## Your questions (private, not evidence)\n");
        for e in &exchanges {
            let reply = followup::reply_text(&e.reply);
            let _ = writeln!(
                text,
                "- {} {:?}: {}\n  {reply}",
                e.step_id, e.kind, e.question
            );
        }
    }

    let out = Shown {
        tour: tour::for_session(t, &session),
        session,
        stale,
        source_unavailable,
        exchanges,
    };
    emit_as(&out, || text, &r.out)?;
    Ok(ExitCode::SUCCESS)
}

/// One stop in Markdown; a location whose blob is gone is listed, not dropped.
pub fn write_step(
    text: &mut String,
    step: &TourStep,
    reader: Option<&dyn SnapshotReader>,
    unavailable: &mut Vec<String>,
) {
    let _ = writeln!(
        text,
        "## {}: {}\n\n{}\n\n{}\n",
        step.id, step.title, step.purpose, step.explanation
    );
    let code = match reader {
        Some(reader) => tour::render(step, reader),
        None => step.code_refs.iter().map(|r| (r.clone(), None)).collect(),
    };
    for (r, code) in code {
        let at = format!("{}:{}-{}", r.path, r.start_line, r.end_line);
        if let Some(code) = code {
            let _ = writeln!(text, "`{at}`\n```\n{code}```\n");
        } else {
            let _ = writeln!(
                text,
                "`{at}`: source unavailable (pruned or garbage-collected)\n"
            );
            unavailable.push(at);
        }
    }
    for e in &step.example_snippets {
        let _ = writeln!(
            text,
            "{} (not from the repository):\n```\n{}\n```\n",
            e.label, e.text
        );
    }
    if let Some(q) = &step.question {
        let _ = writeln!(text, "> {:?}: {}\n", q.kind, q.text);
    }
}

fn reveal(r: &TourRef, step: &str) -> anyhow::Result<ExitCode> {
    let (_, session, (question, answer)) = with_session(r, |t, s| {
        let q = tour::question(t, step).map_err(Failure::invalid)?.clone();
        Ok((s.reveal(step), (q.text, q.reveal)))
    })?;
    let out = Revealed {
        step,
        question: &question,
        reveal: &answer,
        session: &session,
    };
    emit_as(&out, || format!("> {question}\n\n{answer}\n"), &r.out)?;
    Ok(ExitCode::SUCCESS)
}

fn finish(r: &TourRef, to: SessionState) -> anyhow::Result<ExitCode> {
    let (_, session, ()) =
        with_session(r, |_, s| Ok((s.finish(to).map_err(Failure::invalid)?, ())))?;
    emit_as(
        &session,
        || {
            format!(
                "Session {:?}: personal progress only; the review and its approval are unchanged.\n",
                session.state
            )
        },
        &r.out,
    )?;
    Ok(ExitCode::SUCCESS)
}
