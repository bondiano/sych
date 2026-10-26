//! Ledger construction (spec §12.2): effects of each side keyed by construct, kind and
//! target; a key missing on head is looked for elsewhere (a move) before it is called
//! removed, and kept keys are compared by scope, guard, multiplicity and order.

use std::collections::{BTreeMap, BTreeSet};

use super::scope::{effect_of, failure, lifetime, transaction};
use super::{
    BindingStatus, EffectLedger, EffectMatch, ImplementationRef, LedgerDependency, LedgerEntry,
    Occurrence, RegistryBinding, Resolution,
};
use crate::config::{Effect, EffectKind};
use crate::{
    DiffManifest, Language, RepoPath, SchemaVersion, SemanticFacts, SourceSide, SourceSnapshotId,
    SyntaxFacts, SyntaxKind,
};

/// Each registry entry bound to its implementation refs as this diff leaves them.
pub fn bindings(effects: &[Effect], manifest: &DiffManifest) -> Vec<RegistryBinding> {
    effects
        .iter()
        .map(|e| {
            let implementation: Vec<ImplementationRef> = e
                .implementation_refs
                .iter()
                .map(|path| {
                    let file = manifest
                        .files
                        .iter()
                        .find(|f| f.old.iter().chain(&f.new).any(|s| &s.path == path));
                    let oid = |side: SourceSide| {
                        file.and_then(|f| f.side(side))
                            .filter(|s| &s.path == path)
                            .map(|s| s.oid.clone())
                    };
                    ImplementationRef {
                        path: path.clone(),
                        base_oid: oid(SourceSide::DiffBase),
                        head_oid: oid(SourceSide::Head),
                        changed: file.is_some(),
                    }
                })
                .collect();
            let status = if implementation.is_empty() {
                BindingStatus::Unbound
            } else if implementation.iter().any(|i| i.changed) {
                BindingStatus::ImplementationChanged
            } else {
                BindingStatus::Current
            };
            RegistryBinding {
                registry_id: e.id.clone(),
                symbol: e.matcher.symbol.clone(),
                kind: e.effect_kind,
                durable_with_transaction: e.guarantees.durable_with_transaction,
                implementation,
                contract_refs: e.contract_refs.clone(),
                status,
            }
        })
        .collect()
}

type Key = (RepoPath, Option<String>, EffectKind, String);
type Field = (&'static str, fn(&Occurrence) -> String);
/// A key with the first line it occupies on base and on head.
type Placed<'a> = (&'a Key, u32, u32);

struct Slot {
    shape: SyntaxKind,
    language: Language,
    base: Vec<Occurrence>,
    head: Vec<Occurrence>,
    dynamic: bool,
}

/// The name a changed file goes by: its head path, or its base path when deleted.
fn canonical(manifest: &DiffManifest, side: SourceSide, path: &RepoPath) -> RepoPath {
    manifest
        .files
        .iter()
        .find(|f| f.side(side).is_some_and(|s| &s.path == path))
        .and_then(|f| f.new.as_ref().or(f.old.as_ref()))
        .map_or_else(|| path.clone(), |s| s.path.clone())
}

fn collect(
    manifest: &DiffManifest,
    syntax: &[SyntaxFacts],
    registry: &[RegistryBinding],
) -> BTreeMap<Key, Slot> {
    let mut slots: BTreeMap<Key, Slot> = BTreeMap::new();
    for doc in syntax {
        for fact in &doc.facts {
            let Some((kind, target)) = effect_of(fact, doc.language, registry) else {
                continue;
            };
            let path = canonical(manifest, doc.side, &fact.code_ref.path);
            let dynamic = doc.facts.iter().any(|f| {
                f.in_change
                    && f.symbol == fact.symbol
                    && f.code_ref.path == fact.code_ref.path
                    && matches!(
                        f.kind,
                        SyntaxKind::DynamicDispatch | SyntaxKind::MonkeyPatch
                    )
            });
            let occurrence = Occurrence {
                code_ref: fact.code_ref.clone(),
                symbol: fact.symbol.clone(),
                payload: fact.detail.clone(),
                guard: fact.guard.clone(),
                transaction_scope: transaction(fact, &doc.facts, doc.language),
                lifetime_scope: lifetime(fact, &doc.facts),
                failure_behavior: failure(fact, &doc.facts, doc.language),
                in_change: fact.in_change,
            };
            let slot = slots
                .entry((path, fact.symbol.clone(), kind, target))
                .or_insert_with(|| Slot {
                    shape: fact.kind,
                    language: doc.language,
                    base: Vec::new(),
                    head: Vec::new(),
                    dynamic: false,
                });
            slot.dynamic |= dynamic;
            match doc.side {
                SourceSide::DiffBase => slot.base.push(occurrence),
                _ => slot.head.push(occurrence),
            }
        }
    }
    slots
}

/// Differences of two non-empty sides of one key, as notes. A move compares only the
/// scopes the effect carries with it: guard and failure handling belong to the construct, and
/// the old construct still holds them around the new call.
fn compare(base: &[Occurrence], head: &[Occurrence], moved: bool) -> Vec<String> {
    let mut notes = Vec::new();
    if base.len() != head.len() {
        notes.push(format!("multiplicity {} → {}", base.len(), head.len()));
    }

    let field = |o: &[Occurrence], f: fn(&Occurrence) -> String| -> BTreeSet<String> {
        o.iter().map(f).collect()
    };
    let fields: [Field; 4] = [
        ("transaction scope", |o| o.transaction_scope.clone()),
        ("lifetime", |o| o.lifetime_scope.clone()),
        ("failure behaviour", |o| o.failure_behavior.clone()),
        ("guard", |o| {
            o.guard.clone().unwrap_or_else(|| "none".into())
        }),
    ];
    for (name, get) in fields.into_iter().take(if moved { 2 } else { 4 }) {
        let (b, h) = (field(base, get), field(head, get));
        if b != h {
            let join = |s: BTreeSet<String>| s.into_iter().collect::<Vec<_>>().join(" | ");
            notes.push(format!("{name}: {} → {}", join(b), join(h)));
        }
    }
    notes
}

/// Whether head semantic facts show `to` called from the file the effect left.
fn call_edge(semantic: &[SemanticFacts], to: Option<&str>, from_path: &RepoPath) -> bool {
    // gopls names a method `(*Store).Create`, the syntax facts `Store.Create`.
    let last = |s: &str| s.rsplit('.').next().unwrap_or(s).to_owned();
    let Some(name) = to.map(last) else {
        return false;
    };
    semantic
        .iter()
        .filter(|f| f.side == SourceSide::Head)
        .flat_map(|f| &f.symbols)
        .filter(|s| last(&s.name) == name)
        .flat_map(|s| s.callers.refs.iter().chain(&s.calling_constructs))
        .any(|r| &r.path == from_path)
}

/// The base and head order of keys in each construct; keys present on both sides whose
/// relative order differs.
fn reordered(slots: &BTreeMap<Key, Slot>) -> BTreeSet<Key> {
    let mut by_construct: BTreeMap<(RepoPath, Option<String>), Vec<Placed<'_>>> = BTreeMap::new();
    for (key, slot) in slots {
        if let (Some(b), Some(h)) = (slot.base.first(), slot.head.first()) {
            by_construct
                .entry((key.0.clone(), key.1.clone()))
                .or_default()
                .push((key, b.code_ref.start_line, h.code_ref.start_line));
        }
    }
    let mut out = BTreeSet::new();
    for keys in by_construct.values() {
        let order = |pick: fn(&Placed<'_>) -> u32| {
            let mut sorted = keys.clone();
            sorted.sort_by_key(pick);
            sorted
                .into_iter()
                .map(|(k, _, _)| k.clone())
                .collect::<Vec<_>>()
        };
        let (base, head) = (order(|k| k.1), order(|k| k.2));
        if base != head {
            out.extend(
                base.into_iter()
                    .zip(head)
                    .filter(|(b, h)| b != h)
                    .flat_map(|(b, h)| [b, h]),
            );
        }
    }
    out
}

const REMOVED: &str = "gone from the changed code; no alternative mechanism (wrapper, middleware, \
                       subscription, trigger) was found in the changed files, and none is ruled \
                       out outside them";
const DETACHED_DELIVERY: &str = "delivery no longer runs inside the transaction: a crash or \
                                 shutdown after commit can lose it unless a recovery path (relay, \
                                 retry, reconciliation) exists; that path is not yet established";

/// Keys only on base matched to the same effect only on head in another construct: each pair
/// leaves `slots` as one moved (or moved and changed) entry.
fn moved(
    slots: &mut BTreeMap<Key, Slot>,
    semantic: &[SemanticFacts],
    registry: &[RegistryBinding],
) -> Vec<LedgerEntry> {
    // A key only on base looks for the same effect only on head in another construct.
    let removed: Vec<Key> = slots
        .iter()
        .filter(|(_, s)| s.head.is_empty())
        .map(|(k, _)| k.clone())
        .collect();
    let mut moves: Vec<(Key, Key)> = Vec::new();
    for from in removed {
        let to = slots
            .iter()
            .find(|(k, s)| {
                s.base.is_empty()
                    && k.2 == from.2
                    && k.3 == from.3
                    && !moves.iter().any(|(_, t)| t == *k)
            })
            .map(|(k, _)| k.clone());
        if let Some(to) = to {
            moves.push((from, to));
        }
    }
    let mut entries = Vec::new();
    for (from, to) in &moves {
        let (Some(old), Some(new)) = (slots.remove(from), slots.remove(to)) else {
            continue;
        };
        let proven = call_edge(semantic, to.1.as_deref(), &from.0);
        let mut notes = vec![format!(
            "moved from {}{} to {}{}",
            from.0,
            from.1
                .as_deref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default(),
            to.0,
            to.1.as_deref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default()
        )];
        if !proven {
            notes.push("the call edge from the old construct to the new one is not proven".into());
        }

        let scope_notes = compare(&old.base, &new.head, true);
        let slot = Slot {
            base: old.base,
            dynamic: old.dynamic || new.dynamic,
            ..new
        };
        let matched = if scope_notes.is_empty() {
            EffectMatch::Moved
        } else {
            EffectMatch::Changed
        };
        notes.extend(scope_notes);
        let resolution = if proven && !slot.dynamic {
            Resolution::Resolved
        } else {
            Resolution::Partial
        };
        entries.push(entry(
            to.clone(),
            slot,
            matched,
            resolution,
            notes,
            registry,
        ));
    }
    entries
}

/// The ledger of `syntax` against the bound registry; `semantic` proves move edges.
pub fn ledger(
    snapshot: &SourceSnapshotId,
    manifest: &DiffManifest,
    syntax: &[SyntaxFacts],
    semantic: &[SemanticFacts],
    registry: Vec<RegistryBinding>,
) -> EffectLedger {
    let mut slots = collect(manifest, syntax, &registry);
    let reordered = reordered(&slots);
    let mut entries = moved(&mut slots, semantic, &registry);
    for (key, slot) in slots {
        let (matched, mut notes) = match (slot.base.is_empty(), slot.head.is_empty()) {
            (false, true) => (EffectMatch::Removed, vec![REMOVED.to_owned()]),
            (true, false) => (EffectMatch::Added, Vec::new()),
            _ => {
                let mut notes = compare(&slot.base, &slot.head, false);
                if reordered.contains(&key) {
                    notes.push("order relative to the construct's other effects changed".into());
                }

                let matched = if notes.is_empty() {
                    EffectMatch::Preserved
                } else {
                    EffectMatch::Changed
                };
                (matched, notes)
            }
        };
        let resolution = if matches!(matched, EffectMatch::Removed | EffectMatch::Added) {
            Resolution::Partial
        } else {
            Resolution::Resolved
        };
        notes.retain(|n| !n.is_empty());
        entries.push(entry(key, slot, matched, resolution, notes, &registry));
    }
    entries.retain(|e| {
        e.matched != EffectMatch::Preserved
            || !e.notes.is_empty()
            || e.base.iter().chain(&e.head).any(|o| o.in_change)
    });
    entries.sort_by(|a, b| {
        (&a.path, &a.symbol, &a.effect_id).cmp(&(&b.path, &b.symbol, &b.effect_id))
    });
    let dependencies = dependencies(&entries, &registry);
    EffectLedger {
        schema_version: SchemaVersion,
        source_snapshot_id: Some(snapshot.clone()),
        entries,
        registry,
        dependencies,
        limitations: vec![
            "effects are read from syntax facts of the changed files: effects in unchanged \
             files, behind dynamic dispatch or in generated code are not in the ledger, and an \
             empty ledger is not proof that no effect changed"
                .into(),
        ],
    }
}

fn entry(
    key: Key,
    slot: Slot,
    matched: EffectMatch,
    resolution: Resolution,
    mut notes: Vec<String>,
    registry: &[RegistryBinding],
) -> LedgerEntry {
    let (path, symbol, kind, target) = key;
    let binding = (slot.shape == SyntaxKind::RegisteredEffectCall)
        .then(|| registry.iter().find(|b| b.symbol == target))
        .flatten();
    let mut matched = matched;
    let mut resolution = resolution;
    let durable = binding
        .filter(|b| b.carries_guarantees())
        .and_then(|b| b.durable_with_transaction);
    if let Some(b) = binding.filter(|b| !b.carries_guarantees()) {
        notes.push(format!(
            "registry `{}` is not carried: {:?}; its guarantees are checked again",
            b.registry_id, b.status
        ));
        resolution = resolution.max(Resolution::Partial);
    }

    let durable_claim =
        durable == Some(true) || binding.is_some_and(|b| b.durable_with_transaction == Some(true));
    if durable_claim
        && slot.head.iter().any(|o| {
            !o.transaction_scope.starts_with("transaction")
                || o.lifetime_scope.starts_with("detached")
        })
        && slot
            .base
            .iter()
            .any(|o| o.transaction_scope.starts_with("transaction"))
    {
        notes.push(DETACHED_DELIVERY.into());
        if matched == EffectMatch::Preserved || matched == EffectMatch::Moved {
            matched = EffectMatch::Changed;
        }
    }

    if slot.dynamic
        || slot
            .base
            .iter()
            .chain(&slot.head)
            .any(|o| o.transaction_scope.starts_with("unknown (receiver"))
    {
        resolution = Resolution::DynamicUnknown;
        notes.push("a receiver, callee or registration in this construct is not resolved".into());
    } else if slot.language == Language::Python {
        resolution = resolution.max(Resolution::Partial);
    }

    if resolution == Resolution::DynamicUnknown && matched == EffectMatch::Preserved {
        matched = EffectMatch::Unresolved;
    }

    let kind_name = serde_json::to_value(kind).unwrap_or_default();
    LedgerEntry {
        effect_id: format!(
            "{}:{target}@{path}#{}",
            kind_name.as_str().unwrap_or_default(),
            symbol.as_deref().unwrap_or("<module>")
        ),
        kind,
        target,
        shape: slot.shape,
        language: slot.language,
        path,
        symbol,
        registry_id: binding.map(|b| b.registry_id.clone()),
        durable_with_transaction: durable,
        contract_refs: binding.map(|b| b.contract_refs.clone()).unwrap_or_default(),
        base: slot.base,
        head: slot.head,
        matched,
        resolution,
        notes,
    }
}

fn dependencies(entries: &[LedgerEntry], registry: &[RegistryBinding]) -> Vec<LedgerDependency> {
    let mut out: BTreeSet<LedgerDependency> = entries
        .iter()
        .flat_map(|e| e.base.iter().chain(&e.head))
        .map(|o| LedgerDependency {
            side: o.code_ref.side,
            path: o.code_ref.path.clone(),
            blob_oid: o.code_ref.blob_oid.clone(),
        })
        .collect();
    for i in registry.iter().flat_map(|b| &b.implementation) {
        for (side, oid) in [
            (SourceSide::DiffBase, &i.base_oid),
            (SourceSide::Head, &i.head_oid),
        ] {
            if let Some(oid) = oid {
                out.insert(LedgerDependency {
                    side,
                    path: i.path.clone(),
                    blob_oid: oid.clone(),
                });
            }
        }
    }
    out.into_iter().collect()
}
