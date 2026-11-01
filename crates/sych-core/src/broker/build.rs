//! Packet assembly: diff and enclosing constructs of both sides, then
//! diagnostics, then depth-1 callers, implementations, tests and references; overlapping
//! spans merge; required, refuting and supporting evidence fill the budget in that order.

use std::collections::BTreeSet;

use super::neighbours::Neighbourhood;
use super::observations;
use super::select::select;
use super::{ContextRequest, EvidencePacket, Priority, Purpose, Relation};
use crate::config::ContextSettings;
use crate::effects::EffectLedger;
use crate::{
    BackendStatus, CiFacts, CodeRef, Completeness, DiffManifest, EvidenceId, FileChange, LintFacts,
    PolicySnapshotId, RepoPath, ReviewPlan, ReviewUnit, RunId, SemanticFacts, SnapshotReader,
    SourceSide, SourceSnapshotId, SyntaxFacts, source_language,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub unit_tokens: u32,
    pub request_tokens: u32,
    pub max_files: u32,
    pub requests_per_unit: u32,
    pub requests_per_run: u32,
    pub max_span_lines: u32,
}

impl Limits {
    /// shortcut: request counts and span length are the default limits, not yet configurable;
    /// move them into `ContextSettings` when a repository needs other values.
    pub fn from_settings(context: &ContextSettings) -> Self {
        Self {
            unit_tokens: context.initial_tokens_per_unit,
            request_tokens: context.max_tokens_per_request,
            max_files: context.max_source_files,
            requests_per_unit: 24,
            requests_per_run: 160,
            max_span_lines: 160,
        }
    }

    /// Smaller request budgets when the packets would send more than the run's input cap:
    /// each shown span goes out about twice (search, then verification of what it found).
    /// What no longer fits is cut by priority with the usual gaps. Down to a quarter, and never
    /// below the largest required span; past that the run cap itself refuses the rest.
    #[must_use]
    pub fn fit_run(self, packets: &[EvidencePacket], run_input_tokens: u32) -> Option<Self> {
        let demand = 2 * packets
            .iter()
            .map(|p| u64::from(p.usage.tokens))
            .sum::<u64>();
        let cap = u64::from(run_input_tokens);
        if demand <= cap {
            return None;
        }

        // Never below the largest required span: each one must still fit a chunk of its own.
        let largest = packets
            .iter()
            .flat_map(|p| &p.fragments)
            .filter(|f| f.priority == Priority::Required)
            .map(|f| f.tokens)
            .max()
            .unwrap_or(0);
        let scale = |t: u32| {
            u32::try_from(u64::from(t) * cap / demand)
                .unwrap_or(t)
                .max(t / 4)
                .max(largest.min(t))
        };
        Some(Self {
            unit_tokens: scale(self.unit_tokens),
            request_tokens: scale(self.request_tokens),
            ..self
        })
    }
}

pub struct BrokerInput<'a> {
    pub run_id: RunId,
    pub snapshot_id: &'a SourceSnapshotId,
    pub policy_snapshot_id: &'a PolicySnapshotId,
    pub access_scope: &'a str,
    pub plan: &'a ReviewPlan,
    pub manifest: &'a DiffManifest,
    pub facts: &'a [SemanticFacts],
    pub syntax: &'a [SyntaxFacts],
    pub ci: &'a [CiFacts],
    /// A CI report bound to the head says it compiled: analyzer errors are artefacts then.
    pub head_compiled: bool,
    pub ledger: &'a EffectLedger,
    pub lint: &'a [LintFacts],
    pub limits: Limits,
}

/// A span before merging: where it came from and what it is linked to.
#[derive(Clone)]
pub(super) struct Candidate {
    pub side: SourceSide,
    pub path: RepoPath,
    pub start: u32,
    pub end: u32,
    pub priority: Priority,
    pub reason: String,
    pub evidence: Option<EvidenceId>,
    pub completeness: Completeness,
    /// `(seed side, path, line, relation)`: the construct this span is a neighbour of.
    pub link: Option<(RepoPath, u32, Relation)>,
}

#[derive(Default)]
pub(super) struct RunState {
    pub files: BTreeSet<(SourceSide, RepoPath)>,
    pub requests: u32,
}

fn side_name(side: SourceSide) -> &'static str {
    match side {
        SourceSide::DiffBase => "base",
        SourceSide::Head => "head",
        SourceSide::TargetTip => "target tip",
    }
}

/// Discovery packets per unit, then verification packets per applicable obligation: one,
/// or several chunks when its required spans do not fit one request together.
pub fn build_packets(input: &BrokerInput<'_>, reader: &dyn SnapshotReader) -> Vec<EvidencePacket> {
    let mut state = RunState::default();
    let mut packets = Vec::new();
    for unit in &input.plan.scope.units {
        if unit.paths.iter().all(|p| generated(input.manifest, p)) {
            continue;
        }

        let request = request(
            input,
            &[unit],
            None,
            unit.title.clone(),
            input.limits.unit_tokens,
        );
        packets.extend(packet(input, reader, &mut state, &request, &[unit], &[]));
    }
    for planned in &input.plan.obligations {
        let o = &planned.obligation;
        let units: Vec<&ReviewUnit> = input
            .plan
            .scope
            .units
            .iter()
            .filter(|u| o.review_unit_ids.contains(&u.id))
            .collect();
        if units.is_empty() {
            continue;
        }

        let count = u32::try_from(units.len()).unwrap_or(u32::MAX);
        let budget = input
            .limits
            .unit_tokens
            .saturating_mul(count)
            .min(input.limits.request_tokens);
        let mut request = request(input, &units, Some(o.id), o.question.clone(), budget);
        request.purpose = Purpose::Verification;
        request.seed_refs.clone_from(&o.code_refs);
        packets.extend(packet(
            input,
            reader,
            &mut state,
            &request,
            &units,
            &o.code_refs,
        ));
    }
    packets
}

fn request(
    input: &BrokerInput<'_>,
    units: &[&ReviewUnit],
    obligation: Option<crate::ObligationId>,
    question: String,
    budget: u32,
) -> ContextRequest {
    ContextRequest {
        run_id: input.run_id,
        snapshot_id: input.snapshot_id.clone(),
        policy_snapshot_id: input.policy_snapshot_id.clone(),
        review_unit_ids: units.iter().map(|u| u.id).collect(),
        obligation_id: obligation,
        question,
        purpose: Purpose::Discovery,
        seed_refs: Vec::new(),
        requested_operations: [
            "read_span",
            "find_references",
            "find_callers",
            "find_implementations",
        ]
        .map(str::to_owned)
        .to_vec(),
        remaining_budget: budget,
        access_scope_id: input.access_scope.to_owned(),
    }
}

/// Declared unread by the planner (`GENERATED`, `GENERATED_IN_CHANGE`): its hunks are never
/// packed as context.
fn generated(manifest: &DiffManifest, path: &RepoPath) -> bool {
    file_for(manifest, path).is_some_and(FileChange::hidden)
}

pub(super) fn file_for<'a>(manifest: &'a DiffManifest, path: &RepoPath) -> Option<&'a FileChange> {
    manifest.files.iter().find(|f| {
        f.new
            .as_ref()
            .or(f.old.as_ref())
            .is_some_and(|s| &s.path == path)
    })
}

fn packet(
    input: &BrokerInput<'_>,
    reader: &dyn SnapshotReader,
    state: &mut RunState,
    request: &ContextRequest,
    units: &[&ReviewUnit],
    seeds: &[CodeRef],
) -> Vec<EvidencePacket> {
    let mut gaps = Vec::new();
    let mut queries = Vec::new();
    let mut candidates: Vec<Candidate> = seeds
        .iter()
        .map(|r| Candidate {
            side: r.side,
            path: r.path.clone(),
            start: r.start_line,
            end: r.end_line,
            priority: Priority::Required,
            reason: "obligation anchor".into(),
            evidence: None,
            completeness: Completeness::CompleteForQuery,
            link: None,
        })
        .collect();
    let mut requests = 0;
    for path in units.iter().flat_map(|u| &u.paths) {
        let Some(file) = file_for(input.manifest, path).filter(|f| !f.hidden()) else {
            continue;
        };
        for side in [SourceSide::DiffBase, SourceSide::Head] {
            let Some(file_side) = file.side(side) else {
                continue;
            };
            diff_candidates(file, side, &file_side.path, input.limits, &mut candidates);
            let facts = input.facts.iter().find(|f| {
                f.side == side && f.query_scope.iter().any(|p| p == file_side.path.as_str())
            });
            match facts {
                Some(f) if f.status == BackendStatus::Ready => {
                    let mut ctx = Neighbourhood {
                        facts: f,
                        path: &file_side.path,
                        limits: input.limits,
                        state,
                        requests: &mut requests,
                        candidates: &mut candidates,
                        queries: &mut queries,
                        gaps: &mut gaps,
                        syntax: input.syntax,
                        head_compiled: input.head_compiled,
                    };
                    ctx.collect();
                }
                Some(f) => gaps.push(format!(
                    "semantic backend {:?} for {} ({}): {}",
                    f.status,
                    file_side.path,
                    side_name(side),
                    f.limitations.last().map_or("", String::as_str)
                )),
                None if source_language(file_side.path.as_str()).is_some() => gaps.push(format!(
                    "NO_SEMANTIC_FACTS: {} ({})",
                    file_side.path,
                    side_name(side)
                )),
                // Manifests, docs and data have no callers to look for.
                None => {
                    if let Some(ext) = unsupported_code(file_side.path.as_str()) {
                        gaps.push(format!(
                            "NO_SEMANTIC_BACKEND: {} ({}, .{ext})",
                            file_side.path,
                            side_name(side)
                        ));
                    } else if is_markup(file_side.path.as_str()) {
                        gaps.push(format!(
                            "NO_VISUAL_EVIDENCE: {} ({}): rendering and accessibility need browser, \
                             accessibility or visual reports; source and types do not show them",
                            file_side.path,
                            side_name(side)
                        ));
                    }
                }
            }
        }
    }
    for unit_path in units.iter().flat_map(|u| &u.paths) {
        observations::syntax(input.syntax, unit_path, input.manifest, &mut candidates);
        observations::unseen(input.syntax, unit_path, input.manifest, &mut gaps);
        observations::ci(input.ci, unit_path, input.manifest, &mut candidates);
        observations::ledger(
            input.ledger,
            unit_path,
            input.manifest,
            &mut candidates,
            &mut gaps,
        );
        observations::lint(
            input.lint,
            unit_path,
            input.manifest,
            &mut candidates,
            &mut gaps,
        );
    }
    let limits = (input.limits.max_files, input.limits.max_span_lines);
    select(
        reader, state, request, candidates, queries, gaps, requests, limits,
    )
}

/// Source extensions of languages without a semantic backend yet: their callers and
/// references are unknown, not absent.
const UNSUPPORTED_CODE: &[&str] = &[
    "java", "kt", "kts", "scala", "sc", "clj", "cljs", "cljc", "rb", "php", "cs", "swift", "c",
    "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx", "ex", "exs", "gleam", "zig", "r", "R", "sql",
];

fn unsupported_code(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1;
    UNSUPPORTED_CODE.iter().copied().find(|e| *e == ext)
}

fn is_markup(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| matches!(ext, "css" | "scss" | "sass" | "less" | "html" | "htm"))
}

/// Lines around a hunk, as `git diff` shows them; without a semantic backend they are often
/// the only view of the signature or guard next to a change.
const HUNK_CONTEXT_LINES: u32 = 3;

/// Changed lines of one side, split into spans of at most `max_span_lines`, with their
/// surrounding lines (also around a pure deletion's insertion point).
fn diff_candidates(
    file: &FileChange,
    side: SourceSide,
    path: &RepoPath,
    limits: Limits,
    out: &mut Vec<Candidate>,
) {
    let line_count = file.side(side).and_then(|s| s.line_count).unwrap_or(0);
    for hunk in &file.hunks {
        let (start, count) = hunk.on(side);
        let last = start.saturating_add(count.max(1)).saturating_sub(1);
        let before = start.saturating_sub(HUNK_CONTEXT_LINES).max(1);
        let after = last.saturating_add(HUNK_CONTEXT_LINES).min(line_count);
        // Only the lines around the change: the changed lines are required spans of their
        // own, and a context span over them would glue a long change back into one span.
        let context = if count == 0 {
            vec![(before, after)]
        } else {
            vec![(before, start.saturating_sub(1)), (last + 1, after)]
        };
        for (from, to) in context.into_iter().filter(|(from, to)| from <= to) {
            out.push(Candidate {
                side,
                path: path.clone(),
                start: from,
                end: to,
                priority: Priority::Supporting,
                reason: format!("hunk context ({})", side_name(side)),
                evidence: None,
                completeness: Completeness::CompleteForQuery,
                link: None,
            });
        }

        let mut from = start;
        let end = start.saturating_add(count);
        while from < end {
            let to = (from + limits.max_span_lines).min(end) - 1;
            out.push(Candidate {
                side,
                path: path.clone(),
                start: from,
                end: to,
                priority: Priority::Required,
                reason: format!("diff hunk ({})", side_name(side)),
                evidence: None,
                completeness: Completeness::CompleteForQuery,
                link: None,
            });
            from = to + 1;
        }
    }
}
