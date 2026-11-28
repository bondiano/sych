//! Context-request filtering, fixture-claim filtering, and verifier-assist searches derived
//! from discovery candidates and the effect ledger.

use super::{AskOperation, ContextAsk, Discovered, MAX_CONTEXT_ASKS, RejectedDraft};
use crate::effects::{EffectMatch, LedgerEntry};
use crate::{Candidate, RepoPath, SourceSide, glob_match};

/// Searches for what could refute candidates the verifier left open without asking: the
/// identifiers each claim names in backticks (two per candidate, four in all), where a guard
/// or validator elsewhere is most likely found.
pub fn refuting_asks(open: &[Candidate]) -> Vec<ContextAsk> {
    let identifier = |t: &&str| {
        (3..=64).contains(&t.len())
            && t.chars().any(char::is_alphabetic)
            && t.chars().all(|c| c.is_alphanumeric() || "_:.-".contains(c))
    };
    let mut queries: Vec<&str> = Vec::new();
    for candidate in open {
        let named = candidate
            .claim
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(identifier);
        for query in named
            .filter(|q| !queries.contains(q))
            .take(2)
            .collect::<Vec<_>>()
        {
            queries.push(query);
        }
    }
    queries
        .into_iter()
        .take(4)
        .map(|query| ContextAsk {
            question: format!("where else is `{query}` set, checked or validated?"),
            operation: AskOperation::SearchSource {
                side: SourceSide::Head,
                query: query.to_owned(),
            },
        })
        .collect()
}

/// Where the change put an effect the ledger saw leave a file the candidates point at: a HEAD
/// search for its target's last name (3 to 64 bytes, like `refuting_asks`; four in all, registered
/// effects first), since the ledger reads only the changed files and a wrapper, worker or
/// middleware outside them may now run it. A moved effect needs no search: the ledger already
/// found its new place among the changed files.
pub fn effect_asks(ledger: &[LedgerEntry], candidates: &[Candidate]) -> Vec<ContextAsk> {
    let mut targets: Vec<(&str, &RepoPath)> = Vec::new();
    // Registered effects first: a transaction's own BeginTx or Commit must not crowd out the
    // outbox write the registry names.
    let mut entries: Vec<&LedgerEntry> = ledger.iter().collect();
    entries.sort_by_key(|e| e.registry_id.is_none());
    for entry in entries {
        let lost = matches!(
            entry.matched,
            EffectMatch::Removed | EffectMatch::Unresolved
        );
        let at = |path: &RepoPath| {
            entry
                .base
                .iter()
                .chain(&entry.head)
                .any(|o| &o.code_ref.path == path)
        };
        let pointed = candidates
            .iter()
            .flat_map(|c| &c.anchors)
            .any(|a| at(&a.path));
        // `example.com/shop/orders.Store.AppendEvent` is called as `AppendEvent`.
        let name = entry
            .target
            .rsplit(['.', '/', ':'])
            .next()
            .unwrap_or_default();
        if lost
            && pointed
            && (3..=64).contains(&name.len())
            && !targets.iter().any(|(n, _)| *n == name)
        {
            targets.push((name, &entry.path));
        }
    }
    targets
        .into_iter()
        .take(4)
        .map(|(name, path)| ContextAsk {
            question: format!(
                "effect ledger: where does `{name}`, gone from `{path}`, happen now?"
            ),
            operation: AskOperation::SearchSource {
                side: SourceSide::Head,
                query: name.to_owned(),
            },
        })
        .collect()
}

/// `found` without the candidates anchored only in `fixture_paths` (labelled data such as eval
/// cases), each kept among the refused drafts with the reason.
pub fn drop_fixture_claims(mut found: Discovered, fixture_paths: &[String]) -> Discovered {
    if fixture_paths.is_empty() {
        return found;
    }

    let in_fixture = |c: &Candidate| {
        !c.anchors.is_empty()
            && c.anchors
                .iter()
                .all(|a| fixture_paths.iter().any(|g| glob_match(g, a.path.as_str())))
    };
    let (fixture, kept): (Vec<Candidate>, Vec<Candidate>) =
        found.candidates.into_iter().partition(in_fixture);
    found.candidates = kept;
    found
        .rejected
        .extend(fixture.into_iter().map(|c| RejectedDraft {
            claim: c.claim,
            problems: vec!["anchored only in labelled fixture data (review.fixture_paths)".into()],
        }));
    found
}

/// The asks of one answer that are answered, at most `MAX_CONTEXT_ASKS`, and why the rest
/// are not.
pub fn context_asks(requests: Vec<ContextAsk>) -> (Vec<ContextAsk>, Vec<String>) {
    let mut asks = Vec::new();
    let mut rejected = Vec::new();
    for (i, ask) in requests.into_iter().enumerate() {
        if i >= MAX_CONTEXT_ASKS {
            rejected.push(format!(
                "`{}`: more than {MAX_CONTEXT_ASKS} context requests",
                ask.question
            ));
        } else if ask.question.trim().is_empty() {
            rejected.push("context request without a question".into());
        } else {
            asks.push(ask);
        }
    }
    (asks, rejected)
}
