//! Packet filling (spec §9.3): merged spans go into the request budget by priority; a
//! verification packet whose required spans do not fit together is split into chunks.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;

use super::build::{Candidate, RunState};
use super::{
    ContextRequest, EvidencePacket, Excluded, Fragment, Link, PacketResolution, PacketUsage,
    Priority, QueryRecord, REQUIRED_OVER_BUDGET, Relation, SIGNATURE_ONLY,
};
use crate::{
    CodeRef, Completeness, EvidenceId, RepoPath, SchemaVersion, SnapshotReader, SourceSide,
    is_secret_path, sha256,
};

fn rank(c: Completeness) -> u8 {
    match c {
        Completeness::Unknown => 0,
        Completeness::Partial => 1,
        Completeness::CompleteForQuery => 2,
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

/// Overlapping or adjacent spans of one file and side become one span of at most
/// `max_lines`; past that the rest of an overlapping span starts a new one, so a long change
/// stays in pieces that each fit a request.
fn merge(mut candidates: Vec<Candidate>, max_lines: u32) -> Vec<Merged> {
    candidates.sort_by(|a, b| (a.side, &a.path, a.start).cmp(&(b.side, &b.path, b.start)));
    let mut out: Vec<Merged> = Vec::new();
    for mut c in candidates {
        let last = out.last().filter(|m| m.side == c.side && m.path == c.path);
        let touches = last.is_some_and(|m| c.start <= m.end.saturating_add(1));
        let fits = last.is_some_and(|m| c.end <= m.end || c.end - m.start < max_lines);
        if let Some(m) = last.filter(|m| touches && !fits && c.start <= m.end) {
            c.start = m.end + 1;
        }

        match out.last_mut() {
            Some(m) if touches && fits => {
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

fn is_prose(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| matches!(ext, "md" | "mdx" | "markdown" | "txt" | "rst" | "adoc"))
}

/// Packets one obligation's required spans may be split into; the rest is over budget.
const MAX_CHUNKS: usize = 4;

/// The fragments of one packet being filled.
#[derive(Default)]
struct Chunk {
    fragments: Vec<Fragment>,
    tokens: u32,
    files: BTreeSet<(SourceSide, RepoPath)>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "one call site assembling the packet"
)]
pub(super) fn select(
    reader: &dyn SnapshotReader,
    state: &mut RunState,
    request: &ContextRequest,
    candidates: Vec<Candidate>,
    queries: Vec<QueryRecord>,
    mut gaps: Vec<String>,
    requests: u32,
    (max_files, max_span_lines): (u32, u32),
) -> Vec<EvidencePacket> {
    let links: Vec<RawLink> = candidates
        .iter()
        .filter_map(|c| {
            c.link
                .clone()
                .map(|(path, line, rel)| (c.side, path, line, c.path.clone(), c.start, rel))
        })
        .collect();
    let mut merged = merge(candidates, max_span_lines);
    // Code before prose of the same priority: a tight budget drops the notes first.
    merged.sort_by_key(|m| (m.priority, is_prose(m.path.as_str())));
    let budget = request.remaining_budget;
    // A verification packet whose required spans do not fit together is split (spec §9.3);
    // discovery keeps one packet per unit.
    let chunked = request.obligation_id.is_some();
    let mut chunks = vec![Chunk::default()];
    let mut excluded = Vec::new();
    for m in merged {
        if is_secret_path(m.path.as_str()) {
            gaps.push(format!("SECRET_PATH {} not read", m.path));
            continue;
        }

        let (mut code_ref, mut tokens) = match read(reader, &m, m.end) {
            Ok(read) => read,
            Err(err) => {
                gaps.push(err);
                continue;
            }
        };
        let file = (m.side, m.path.clone());
        let new_file = !state.files.contains(&file);
        if new_file
            && m.priority != Priority::Required
            && state.files.len() >= usize::try_from(max_files).unwrap_or(usize::MAX)
        {
            excluded.push(Excluded {
                code_ref,
                priority: m.priority,
                reason: "LIMIT_MAX_SOURCE_FILES".into(),
            });
            continue;
        }

        let fits = |tokens: u32| {
            chunks
                .iter()
                .position(|c| c.tokens.saturating_add(tokens) <= budget)
        };
        let mut room = fits(tokens);
        let mut transformation = "none";
        if room.is_none()
            && let Some((head, head_tokens, i)) = signature(reader, &m, fits)
        {
            excluded.push(Excluded {
                code_ref: mem::replace(&mut code_ref, head),
                priority: m.priority,
                reason: SIGNATURE_ONLY.to_owned(),
            });
            (room, tokens, transformation) = (Some(i), head_tokens, "signature");
        }

        if room.is_none()
            && chunked
            && m.priority == Priority::Required
            && tokens <= budget
            && chunks.len() < MAX_CHUNKS
        {
            chunks.push(Chunk::default());
            room = Some(chunks.len() - 1);
        }

        let Some(chunk) = room.map(|i| &mut chunks[i]) else {
            excluded.push(over_budget(&m, code_ref, &mut gaps));
            continue;
        };

        chunk.tokens += tokens;
        chunk.files.insert(file.clone());
        state.files.insert(file);
        chunk.fragments.push(Fragment {
            id: fragment_id(&code_ref),
            code_ref,
            priority: m.priority,
            reason_selected: m.reasons,
            evidence_ids: m.evidence,
            completeness: m.completeness,
            tokens,
            transformation: transformation.into(),
        });
    }
    // What did not fit stays on its own packet, so the chunks that did fit are still assessed.
    let over = excluded.iter().any(|e| e.reason == REQUIRED_OVER_BUDGET);
    if chunked && over && chunks.iter().any(|c| !c.fragments.is_empty()) {
        chunks.push(Chunk::default());
    }

    let rest = Rest {
        queries,
        excluded,
        gaps,
        semantic_items: requests,
    };
    assemble(request, chunks, &links, rest)
}

/// What the first packet records (queries, semantic items) and the last one keeps (what
/// was left out and the gaps).
struct Rest {
    queries: Vec<QueryRecord>,
    excluded: Vec<Excluded>,
    gaps: Vec<String>,
    semantic_items: u32,
}

fn assemble(
    request: &ContextRequest,
    chunks: Vec<Chunk>,
    links: &[RawLink],
    mut rest: Rest,
) -> Vec<EvidencePacket> {
    let last = chunks.len() - 1;
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            let (excluded, gaps) = if i == last {
                (mem::take(&mut rest.excluded), mem::take(&mut rest.gaps))
            } else {
                (Vec::new(), Vec::new())
            };
            let resolution = if chunk.fragments.is_empty() {
                PacketResolution::Unavailable
            } else if gaps.is_empty() {
                PacketResolution::Satisfied
            } else {
                PacketResolution::Partial
            };
            EvidencePacket {
                schema_version: SchemaVersion,
                request: request.clone(),
                links: resolve_links(&chunk.fragments, links.to_vec()),
                queries: if i == 0 {
                    mem::take(&mut rest.queries)
                } else {
                    Vec::new()
                },
                excluded,
                gaps,
                usage: PacketUsage {
                    tokens: chunk.tokens,
                    files: u32::try_from(chunk.files.len()).unwrap_or(u32::MAX),
                    semantic_items: if i == 0 { rest.semantic_items } else { 0 },
                },
                fragments: chunk.fragments,
                resolution,
            }
        })
        .collect()
}

/// The exact reference and token bound of a merged span; the reader checks the hash.
/// Over budget, a supporting construct keeps its first line, usually the signature,
/// instead of disappearing; the full span is still listed as excluded.
fn signature(
    reader: &dyn SnapshotReader,
    m: &Merged,
    fits: impl Fn(u32) -> Option<usize>,
) -> Option<(CodeRef, u32, usize)> {
    if m.priority != Priority::Supporting || m.end <= m.start {
        return None;
    }

    let (head, tokens) = read(reader, m, m.start).ok()?;
    fits(tokens).map(|i| (head, tokens, i))
}

/// A span with no room left: a required one forces `UNKNOWN`, and a writer elsewhere that
/// could refute a hypothesis is not dropped silently.
fn over_budget(m: &Merged, code_ref: CodeRef, gaps: &mut Vec<String>) -> Excluded {
    let reason = match m.priority {
        Priority::Required => REQUIRED_OVER_BUDGET,
        _ => "BUDGET",
    };
    match m.priority {
        Priority::Required => gaps.push(format!(
            "{REQUIRED_OVER_BUDGET}: {}:{}-{}",
            m.path, m.start, m.end
        )),
        Priority::Refuting => gaps.push(format!(
            "REFUTING_OVER_BUDGET: {}:{}-{}",
            m.path, m.start, m.end
        )),
        Priority::Supporting => {}
    }
    Excluded {
        code_ref,
        priority: m.priority,
        reason: reason.to_owned(),
    }
}

fn read(reader: &dyn SnapshotReader, m: &Merged, end: u32) -> Result<(CodeRef, u32), String> {
    let unreadable =
        |err: crate::ReadError| format!("UNREADABLE {}:{}-{end}: {err}", m.path, m.start);
    let code_ref = reader
        .code_ref(m.side, &m.path, m.start, end)
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
