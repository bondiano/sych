//! What an effect is and the scopes it runs in, read from the facts around it on its side.

use super::RegistryBinding;
use crate::config::EffectKind;
use crate::{Language, SyntaxFact, SyntaxKind};

/// The effect kind and target of a fact, `None` for shapes that are not effects.
pub(super) fn effect_of(
    fact: &SyntaxFact,
    language: Language,
    registry: &[RegistryBinding],
) -> Option<(EffectKind, String)> {
    let operation = || {
        // Go details read `ExecContext on `tx` (*sql.Tx)` or `os.WriteFile`; Python and TS
        // details are the call text, named by its callee's last segment.
        if language == Language::Go {
            return fact
                .detail
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned();
        }

        let callee = fact.detail.split('(').next().unwrap_or(&fact.detail);
        callee
            .rsplit('.')
            .next()
            .unwrap_or(callee)
            .trim()
            .to_owned()
    };
    let pair = match fact.kind {
        SyntaxKind::DbCall => (EffectKind::DbWrite, operation()),
        SyntaxKind::TxBoundary => (EffectKind::DbWrite, format!("tx:{}", operation())),
        SyntaxKind::WriteCall => (EffectKind::Filesystem, operation()),
        SyntaxKind::NetworkCall | SyntaxKind::MutationCall => (EffectKind::Network, operation()),
        SyntaxKind::InvalidationCall => (EffectKind::Cache, fact.detail.clone()),
        SyntaxKind::RegisteredEffectCall => {
            let kind = registry
                .iter()
                .find(|b| b.symbol == fact.detail)
                .map_or(EffectKind::Other, |b| b.kind);
            (kind, fact.detail.clone())
        }
        // `useEffect with 3 calls`: the hook names the effect, its call count does not.
        SyntaxKind::EffectWithCleanup | SyntaxKind::EffectWithoutCleanup => {
            let hook = fact.detail.split([' ', ':']).next().unwrap_or_default();
            (EffectKind::Subscription, hook.to_owned())
        }
        SyntaxKind::UnpairedListener
        | SyntaxKind::UnpairedTimer
        | SyntaxKind::UnpairedSubscription => (EffectKind::Subscription, fact.detail.clone()),
        SyntaxKind::AsyncEffectGuarded | SyntaxKind::AsyncEffectUnguarded => {
            (EffectKind::Ui, "state set from an async result".into())
        }
        _ => return None,
    };
    Some(pair)
}

fn contains(outer: &SyntaxFact, inner: &SyntaxFact) -> bool {
    outer.code_ref.path == inner.code_ref.path
        && outer.code_ref.start_line <= inner.code_ref.start_line
        && inner.code_ref.end_line <= outer.code_ref.end_line
        && outer != inner
}

/// The innermost fact of `kinds` around `fact`.
fn enclosing<'a>(
    fact: &SyntaxFact,
    facts: &'a [SyntaxFact],
    kinds: &[SyntaxKind],
) -> Option<&'a SyntaxFact> {
    facts
        .iter()
        .filter(|f| kinds.contains(&f.kind) && contains(f, fact))
        .min_by_key(|f| f.code_ref.end_line - f.code_ref.start_line)
}

fn same_symbol<'a>(
    fact: &'a SyntaxFact,
    facts: &'a [SyntaxFact],
) -> impl Iterator<Item = &'a SyntaxFact> {
    facts
        .iter()
        .filter(move |f| f.code_ref.path == fact.code_ref.path && f.symbol == fact.symbol)
}

pub(super) fn lifetime(fact: &SyntaxFact, facts: &[SyntaxFact]) -> String {
    match fact.kind {
        SyntaxKind::EffectWithCleanup => {
            return "cleaned up on dependency change and unmount".into();
        }
        SyntaxKind::EffectWithoutCleanup
        | SyntaxKind::UnpairedListener
        | SyntaxKind::UnpairedTimer
        | SyntaxKind::UnpairedSubscription => return "no cleanup in the effect".into(),
        SyntaxKind::AsyncEffectGuarded => return "stale results are ignored".into(),
        SyntaxKind::AsyncEffectUnguarded => return "stale results are applied".into(),
        _ => {}
    }
    let scopes = [
        SyntaxKind::DetachedSpawn,
        SyntaxKind::AsyncTask,
        SyntaxKind::ResourceScope,
    ];
    match enclosing(fact, facts, &scopes) {
        Some(f) if f.kind == SyntaxKind::DetachedSpawn => format!("detached: {}", f.detail),
        Some(f) if f.kind == SyntaxKind::AsyncTask => format!("task: {}", f.detail),
        Some(f) => f.detail.clone(),
        None => "caller".into(),
    }
}

pub(super) fn transaction(fact: &SyntaxFact, facts: &[SyntaxFact], language: Language) -> String {
    let relevant = matches!(
        fact.kind,
        SyntaxKind::DbCall | SyntaxKind::RegisteredEffectCall
    );
    match (language, relevant, fact.subject.as_deref()) {
        (_, false, _) => "n/a".into(),
        (Language::Go, _, Some(ty)) if ty.contains("Tx") => format!("transaction ({ty})"),
        (Language::Go, _, Some(ty)) => format!("none: {ty} outside any transaction"),
        (Language::Go, _, None) if fact.kind == SyntaxKind::DbCall => {
            "unknown (receiver type unresolved)".into()
        }
        (Language::Go, _, None) => "none (no transaction passed)".into(),
        (Language::Python, _, _) => {
            if let Some(scope) = enclosing(fact, facts, &[SyntaxKind::ResourceScope]).filter(|s| {
                ["atomic", "begin", "transaction"]
                    .iter()
                    .any(|w| s.detail.contains(w))
            }) {
                return format!("transaction ({})", scope.detail);
            }

            same_symbol(fact, facts)
                .find(|f| {
                    f.kind == SyntaxKind::TxBoundary
                        && f.detail.contains("commit")
                        && f.code_ref.start_line > fact.code_ref.end_line
                })
                .map_or_else(
                    || "unknown (Python receivers are untyped)".into(),
                    |c| format!("transaction (committed at line {})", c.code_ref.start_line),
                )
        }
        _ => "n/a".into(),
    }
}

pub(super) fn failure(fact: &SyntaxFact, facts: &[SyntaxFact], language: Language) -> String {
    if enclosing(fact, facts, &[SyntaxKind::DetachedSpawn]).is_some() {
        return "errors are not returned to the caller (detached)".into();
    }

    match language {
        Language::Go => {
            let returns = same_symbol(fact, facts).any(|f| f.kind == SyntaxKind::ReturnsError);
            let rollback = same_symbol(fact, facts)
                .any(|f| f.kind == SyntaxKind::Defer && f.detail.contains("Rollback"));
            match (returns, rollback) {
                (true, true) => "error returned to the caller; deferred rollback".into(),
                (true, false) => "error returned to the caller".into(),
                (false, true) => "error not returned; deferred rollback".into(),
                (false, false) => "error not returned".into(),
            }
        }
        Language::Python => {
            if let Some(swallow) =
                same_symbol(fact, facts).find(|f| f.kind == SyntaxKind::SwallowedException)
            {
                format!(
                    "an except swallows errors at line {}",
                    swallow.code_ref.start_line
                )
            } else if enclosing(fact, facts, &[SyntaxKind::ResourceScope]).is_some() {
                "exceptions propagate after the scope's cleanup".into()
            } else {
                "exceptions propagate".into()
            }
        }
        _ => "unknown".into(),
    }
}
