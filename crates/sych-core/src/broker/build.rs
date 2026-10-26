//! Packet assembly (spec §9.2–9.3): diff and enclosing constructs of both sides, then
//! diagnostics, then depth-1 callers, implementations, tests and references; overlapping
//! spans merge; required, refuting and supporting evidence fill the budget in that order.

use std::collections::{BTreeMap, BTreeSet};

use super::neighbours::Neighbourhood;
use super::observations;
use super::{
    ContextRequest, EvidencePacket, Excluded, Fragment, Link, PacketResolution, PacketUsage,
    Priority, Purpose, QueryRecord, REQUIRED_OVER_BUDGET, Relation,
};
use crate::config::ContextSettings;
use crate::effects::EffectLedger;
use crate::{
    BackendStatus, CiFacts, CodeRef, Completeness, DiffManifest, EvidenceId, FileChange, LintFacts,
    PolicySnapshotId, RepoPath, ReviewPlan, ReviewUnit, RunId, SchemaVersion, SemanticFacts,
    SnapshotReader, SourceSide, SourceSnapshotId, SyntaxFacts, is_secret_path, sha256,
    source_language,
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
    /// shortcut: request counts and span length are the §9.3 defaults, not yet configurable;
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

fn rank(c: Completeness) -> u8 {
    match c {
        Completeness::Unknown => 0,
        Completeness::Partial => 1,
        Completeness::CompleteForQuery => 2,
    }
}

fn side_name(side: SourceSide) -> &'static str {
    match side {
        SourceSide::DiffBase => "base",
        SourceSide::Head => "head",
        SourceSide::TargetTip => "target tip",
    }
}

/// Discovery packets per unit, then verification packets per applicable obligation.
pub fn build_packets(input: &BrokerInput<'_>, reader: &dyn SnapshotReader) -> Vec<EvidencePacket> {
    let mut state = RunState::default();
    let mut packets = Vec::new();
    for unit in &input.plan.scope.units {
        let request = request(
            input,
            &[unit],
            None,
            unit.title.clone(),
            input.limits.unit_tokens,
        );
        packets.push(packet(input, reader, &mut state, request, &[unit], &[]));
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
        packets.push(packet(
            input,
            reader,
            &mut state,
            request,
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
    request: ContextRequest,
    units: &[&ReviewUnit],
    seeds: &[CodeRef],
) -> EvidencePacket {
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
        let Some(file) = file_for(input.manifest, path) else {
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
    let max_files = input.limits.max_files;
    select(
        reader, state, request, candidates, queries, gaps, requests, max_files,
    )
}

/// Source extensions of languages without a semantic backend yet: their callers and
/// references are unknown, not absent.
const UNSUPPORTED_CODE: &[&str] = &[
    "java", "kt", "rb", "php", "cs", "swift", "c", "h", "cc", "cpp", "hpp", "scala", "ex", "exs",
    "sql",
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
        let context = (
            start.saturating_sub(HUNK_CONTEXT_LINES).max(1),
            last.saturating_add(HUNK_CONTEXT_LINES).min(line_count),
        );
        if context.0 <= context.1 {
            out.push(Candidate {
                side,
                path: path.clone(),
                start: context.0,
                end: context.1,
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

struct Merged {
    side: SourceSide,
    path: RepoPath,
    start: u32,
    end: u32,
    priority: Priority,
    reasons: Vec<String>,
    evidence: Vec<EvidenceId>,
    completeness: Completeness,
}

/// Overlapping or adjacent spans of one file and side become one span.
fn merge(mut candidates: Vec<Candidate>) -> Vec<Merged> {
    candidates.sort_by(|a, b| (a.side, &a.path, a.start).cmp(&(b.side, &b.path, b.start)));
    let mut out: Vec<Merged> = Vec::new();
    for c in candidates {
        match out.last_mut() {
            Some(m)
                if m.side == c.side && m.path == c.path && c.start <= m.end.saturating_add(1) =>
            {
                m.end = m.end.max(c.end);
                m.priority = m.priority.min(c.priority);
                if !m.reasons.contains(&c.reason) {
                    m.reasons.push(c.reason);
                }

                m.evidence
                    .extend(c.evidence.filter(|e| !m.evidence.contains(e)));
                if rank(c.completeness) < rank(m.completeness) {
                    m.completeness = c.completeness;
                }
            }
            _ => out.push(Merged {
                side: c.side,
                path: c.path,
                start: c.start,
                end: c.end,
                priority: c.priority,
                reasons: vec![c.reason],
                evidence: c.evidence.into_iter().collect(),
                completeness: c.completeness,
            }),
        }
    }
    out
}

fn fragment_id(r: &CodeRef) -> String {
    let key = format!(
        "{:?}:{}:{}:{}-{}",
        r.side, r.path, r.blob_oid, r.start_line, r.end_line
    );
    sha256(key.as_bytes()).hex()[..12].to_owned()
}

#[expect(
    clippy::too_many_arguments,
    reason = "one call site assembling the packet"
)]
fn select(
    reader: &dyn SnapshotReader,
    state: &mut RunState,
    request: ContextRequest,
    candidates: Vec<Candidate>,
    queries: Vec<QueryRecord>,
    mut gaps: Vec<String>,
    requests: u32,
    max_files: u32,
) -> EvidencePacket {
    let links: Vec<RawLink> = candidates
        .iter()
        .filter_map(|c| {
            c.link
                .clone()
                .map(|(path, line, rel)| (c.side, path, line, c.path.clone(), c.start, rel))
        })
        .collect();
    let mut merged = merge(candidates);
    merged.sort_by_key(|m| m.priority);
    let budget = request.remaining_budget;
    let mut usage = PacketUsage {
        semantic_items: requests,
        ..PacketUsage::default()
    };
    let mut fragments = Vec::new();
    let mut excluded = Vec::new();
    let mut files = BTreeSet::new();
    for m in merged {
        if is_secret_path(m.path.as_str()) {
            gaps.push(format!("SECRET_PATH {} not read", m.path));
            continue;
        }

        let (code_ref, tokens) = match read(reader, &m) {
            Ok(read) => read,
            Err(err) => {
                gaps.push(err);
                continue;
            }
        };
        let exclude = |reason: &str| Excluded {
            code_ref: code_ref.clone(),
            priority: m.priority,
            reason: reason.to_owned(),
        };
        let file = (m.side, m.path.clone());
        let new_file = !state.files.contains(&file);
        if new_file
            && m.priority != Priority::Required
            && state.files.len() >= usize::try_from(max_files).unwrap_or(usize::MAX)
        {
            excluded.push(exclude("LIMIT_MAX_SOURCE_FILES"));
            continue;
        }

        if usage.tokens.saturating_add(tokens) > budget {
            if m.priority == Priority::Required {
                gaps.push(format!(
                    "{REQUIRED_OVER_BUDGET}: {}:{}-{}",
                    m.path, m.start, m.end
                ));
                excluded.push(exclude(REQUIRED_OVER_BUDGET));
            } else {
                // A writer elsewhere that could refute a hypothesis is not dropped silently.
                if m.priority == Priority::Refuting {
                    gaps.push(format!(
                        "REFUTING_OVER_BUDGET: {}:{}-{}",
                        m.path, m.start, m.end
                    ));
                }

                excluded.push(exclude("BUDGET"));
            }

            continue;
        }

        usage.tokens += tokens;
        files.insert(file.clone());
        state.files.insert(file);
        fragments.push(Fragment {
            id: fragment_id(&code_ref),
            code_ref,
            priority: m.priority,
            reason_selected: m.reasons,
            evidence_ids: m.evidence,
            completeness: m.completeness,
            tokens,
            transformation: "none".into(),
        });
    }
    usage.files = u32::try_from(files.len()).unwrap_or(u32::MAX);
    let links = resolve_links(&fragments, links);
    let resolution = if fragments.is_empty() {
        PacketResolution::Unavailable
    } else if gaps.is_empty() {
        PacketResolution::Satisfied
    } else {
        PacketResolution::Partial
    };
    EvidencePacket {
        schema_version: SchemaVersion,
        request,
        fragments,
        links,
        queries,
        excluded,
        gaps,
        usage,
        resolution,
    }
}

/// The exact reference and token bound of a merged span; the reader checks the hash.
fn read(reader: &dyn SnapshotReader, m: &Merged) -> Result<(CodeRef, u32), String> {
    let unreadable =
        |err: crate::ReadError| format!("UNREADABLE {}:{}-{}: {err}", m.path, m.start, m.end);
    let code_ref = reader
        .code_ref(m.side, &m.path, m.start, m.end)
        .map_err(unreadable)?;
    let span = reader.read_span(&code_ref).map_err(unreadable)?;
    Ok((code_ref, u32::try_from(span.text.len()).unwrap_or(u32::MAX)))
}

type RawLink = (SourceSide, RepoPath, u32, RepoPath, u32, Relation);

/// Links between the included fragments that contain each end; self-links are dropped.
fn resolve_links(fragments: &[Fragment], links: Vec<RawLink>) -> Vec<Link> {
    let containing = |side: SourceSide, path: &RepoPath, line: u32| {
        fragments
            .iter()
            .find(|f| {
                f.code_ref.side == side
                    && &f.code_ref.path == path
                    && (f.code_ref.start_line..=f.code_ref.end_line).contains(&line)
            })
            .map(|f| f.id.clone())
    };
    let mut link_set = BTreeMap::new();
    for (side, from_path, from_line, to_path, to_line, relation) in links {
        if let (Some(from), Some(to)) = (
            containing(side, &from_path, from_line),
            containing(side, &to_path, to_line),
        ) && from != to
        {
            link_set.insert(
                (from.clone(), to.clone(), relation as u8),
                Link { from, to, relation },
            );
        }
    }
    link_set.into_values().collect()
}
