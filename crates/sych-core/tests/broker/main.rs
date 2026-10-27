//! `ContextBroker` scenarios (roadmap M1.2): what a packet contains, what it leaves out and why.

use std::collections::BTreeMap;

use sych_core::broker::{
    BrokerInput, EvidencePacket, Limits, OpStatus, Operation, Ops, PacketResolution, Priority,
    REQUIRED_OVER_BUDGET, Relation, SymbolRef, build_packets, context_gaps, context_limited,
};
use sych_core::config::{PolicyFiles, RepositoryPolicy, ServerConfig};
use sych_core::effects::EffectLedger;
use sych_core::{
    Assessment, BackendStatus, ChangeStatus, CodeRef, Completeness, ContentKind, Coverage,
    DEFAULT_ATTENTION_THRESHOLD, DiffManifest, EdgeSet, ExecutionOutcome, FileChange, FileSide,
    GitHashAlgorithm, GitObjectId, Hunk, Language, LintFacts, ObligationStatus, PathClass,
    PolicySnapshotId, QueryStatus, ReadError, ReduceInput, RepoPath, ReviewPlan, RunFacts, RunId,
    SchemaVersion, SemanticFacts, SnapshotReader, SourceSide, SourceSnapshotId, SourceSpan,
    SymbolFacts, SyntaxFacts, line_range, plan, reduce, sha256,
};

mod chunks;
mod ledger;
mod lint;
mod syntax;

const STORE: &str = "pub struct Store;\n\nimpl Store {\n    pub fn save(&self) -> Result<(), Error> {\n        self.tx(|tx| {\n            tx.insert()?;\n            tx.append_event()\n        })\n    }\n}\n";
const API: &str = "fn handle(store: &Store) {\n    store.save().unwrap();\n}\n";
const TX: &str = "impl Store {\n    fn tx<T>(&self, f: impl FnOnce(&Tx) -> T) -> T {\n        f(&Tx::begin())\n    }\n}\n";
const TEST: &str = "#[test]\nfn saves() {\n    Store.save().unwrap();\n}\n";

const RULES: &str = r#"schema_version: 1
rules:
  - id: EFFECT-01
    version: 1
    title: Saved order publishes its event
    kind: invariant
    required: true
    severity_on_violation: high
    owner: backend
    applies_to:
      paths: ["src/**"]
    statement: A stored order publishes order-created in the same transaction.
"#;

fn oid(c: char) -> GitObjectId {
    GitObjectId::new(GitHashAlgorithm::Sha1, &c.to_string().repeat(40)).expect("oid")
}

fn snapshot() -> SourceSnapshotId {
    SourceSnapshotId(sha256(b"broker fixture"))
}

fn path(p: &str) -> RepoPath {
    p.parse().expect("path")
}

/// Head-side files held in memory, addressed like a pinned snapshot.
struct Reader(BTreeMap<RepoPath, (GitObjectId, &'static str)>);

impl Reader {
    fn new() -> Self {
        Self(BTreeMap::from([
            (path("src/store.rs"), (oid('b'), STORE)),
            (path("src/api.rs"), (oid('c'), API)),
            (path("tests/store.rs"), (oid('d'), TEST)),
            (path("src/tx.rs"), (oid('e'), TX)),
        ]))
    }
}

impl SnapshotReader for Reader {
    fn read_span(&self, r: &CodeRef) -> Result<SourceSpan, ReadError> {
        let fresh = self.code_ref(r.side, &r.path, r.start_line, r.end_line)?;
        if &fresh != r {
            return Err(ReadError::HashMismatch(r.path.to_string()));
        }

        let text = line_range(self.0[&r.path].1, r.start_line, r.end_line).expect("range");
        Ok(SourceSpan {
            reference: fresh,
            text: text.to_owned(),
        })
    }

    fn code_ref(
        &self,
        side: SourceSide,
        p: &RepoPath,
        start: u32,
        end: u32,
    ) -> Result<CodeRef, ReadError> {
        let (oid, text) = self
            .0
            .get(p)
            .ok_or_else(|| ReadError::Unavailable(p.to_string()))?;
        let span = line_range(text, start, end).ok_or(ReadError::LineRange {
            start,
            end,
            line_count: 0,
        })?;
        Ok(CodeRef {
            snapshot_id: snapshot(),
            side,
            path: p.clone(),
            blob_oid: oid.clone(),
            start_line: start,
            end_line: end,
            content_sha256: sha256(span.as_bytes()),
        })
    }

    fn search(
        &self,
        side: SourceSide,
        literal: &str,
        _: &[RepoPath],
        limit: usize,
    ) -> Result<Vec<CodeRef>, ReadError> {
        let mut hits = Vec::new();
        for (p, (_, text)) in &self.0 {
            for (i, line) in text.lines().enumerate() {
                if line.contains(literal) {
                    let n = u32::try_from(i + 1).expect("line");
                    hits.push(self.code_ref(side, p, n, n)?);
                }
            }
        }
        hits.truncate(limit);
        Ok(hits)
    }

    fn diff(&self, _: &SourceSnapshotId) -> Result<DiffManifest, ReadError> {
        unreachable!("not used by the broker")
    }
}

fn manifest() -> DiffManifest {
    DiffManifest {
        schema_version: SchemaVersion,
        method: "fixture".into(),
        diff_base: oid('a'),
        head: oid('e'),
        files: vec![FileChange {
            status: ChangeStatus::Added,
            old: None,
            new: Some(FileSide {
                path: path("src/store.rs"),
                mode: "100644".into(),
                oid: oid('b'),
                content: ContentKind::Text,
                line_count: Some(10),
            }),
            similarity: None,
            generated: false,
            hunks: vec![Hunk {
                old_start: 0,
                old_lines: 0,
                new_start: 6,
                new_lines: 2,
            }],
            limitations: Vec::new(),
        }],
        omissions: Vec::new(),
    }
}

fn answered(refs: Vec<CodeRef>) -> EdgeSet {
    EdgeSet {
        status: QueryStatus::Answered,
        completeness: Completeness::Partial,
        refs,
        external: Vec::new(),
        unresolved: Vec::new(),
    }
}

fn facts(reader: &Reader) -> SemanticFacts {
    let at = |p: &str, s, e| {
        reader
            .code_ref(SourceSide::Head, &path(p), s, e)
            .expect("ref")
    };
    SemanticFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snapshot(),
        side: SourceSide::Head,
        language: Language::Rust,
        status: BackendStatus::Ready,
        producer: None,
        position_encoding: Some("utf-8".into()),
        build_configuration: BTreeMap::new(),
        query_scope: vec!["src/store.rs".into()],
        symbols: vec![SymbolFacts {
            name: "save".into(),
            kind: "method".into(),
            container: vec!["impl Store".into()],
            definition: at("src/store.rs", 4, 9),
            path_class: PathClass::Production,
            references: answered(vec![at("src/api.rs", 2, 2), at("tests/store.rs", 3, 3)]),
            readers: EdgeSet::not_applicable(),
            writers: EdgeSet::not_applicable(),
            callers: answered(vec![at("src/api.rs", 2, 2)]),
            calling_constructs: vec![at("src/api.rs", 1, 3)],
            callees: answered(vec![at("src/tx.rs", 2, 4)]),
            type_definitions: answered(vec![at("src/store.rs", 1, 1)]),
            implementations: EdgeSet::not_applicable(),
        }],
        diagnostics: Vec::new(),
        limitations: Vec::new(),
    }
}

fn run(limits: Limits, facts: &[SemanticFacts]) -> (ReviewPlan, Vec<EvidencePacket>) {
    run_with(limits, facts, &[])
}

fn run_with(
    limits: Limits,
    facts: &[SemanticFacts],
    syntax: &[SyntaxFacts],
) -> (ReviewPlan, Vec<EvidencePacket>) {
    run_full(limits, facts, syntax, &EffectLedger::default(), &[])
}

fn run_full(
    limits: Limits,
    facts: &[SemanticFacts],
    syntax: &[SyntaxFacts],
    ledger: &EffectLedger,
    lint: &[LintFacts],
) -> (ReviewPlan, Vec<EvidencePacket>) {
    run_on(&manifest(), limits, facts, syntax, ledger, lint)
}

pub fn run_on(
    manifest: &DiffManifest,
    limits: Limits,
    facts: &[SemanticFacts],
    syntax: &[SyntaxFacts],
    ledger: &EffectLedger,
    lint: &[LintFacts],
) -> (ReviewPlan, Vec<EvidencePacket>) {
    let mut files = PolicyFiles::default();
    files
        .files
        .insert(path(".sych/rules.yaml"), RULES.as_bytes().to_vec());
    let policy = RepositoryPolicy::parse(&files, &ServerConfig::engine_default()).expect("policy");
    let plan = plan(&snapshot(), manifest, &policy, &RunFacts::default());
    let policy_id = PolicySnapshotId(sha256(b"policy"));
    let input = BrokerInput {
        run_id: RunId(uuid::Uuid::from_u128(1)),
        snapshot_id: &snapshot(),
        policy_snapshot_id: &policy_id,
        access_scope: "local:fixture",
        plan: &plan,
        manifest,
        facts,
        syntax,
        ci: &[],
        ledger,
        lint,
        limits,
    };
    let packets = build_packets(&input, &Reader::new());
    (plan, packets)
}

fn limits() -> Limits {
    Limits {
        unit_tokens: 6000,
        request_tokens: 14_000,
        max_files: 80,
        requests_per_unit: 24,
        requests_per_run: 160,
        max_span_lines: 160,
    }
}

#[test]
fn construct_absorbs_the_hunk_and_links_caller_and_test() {
    let reader = Reader::new();
    let (_, packets) = run(limits(), &[facts(&reader)]);
    let discovery = &packets[0];
    assert_eq!(
        packets.len(),
        2,
        "one discovery and one verification packet"
    );
    assert_eq!(
        discovery.resolution,
        PacketResolution::Satisfied,
        "{:?}",
        discovery.gaps
    );
    let construct = &discovery.fragments[0];
    assert_eq!(construct.priority, Priority::Required);
    // The construct (4-9) absorbs the hunk (6-7) and its three context lines on each side.
    assert_eq!(
        (construct.code_ref.start_line, construct.code_ref.end_line),
        (3, 10)
    );
    assert!(
        construct
            .reason_selected
            .iter()
            .any(|r| r.starts_with("diff hunk"))
    );
    assert!(
        construct
            .reason_selected
            .iter()
            .any(|r| r.contains("enclosing construct `save`"))
    );
    let relations: Vec<Relation> = discovery.links.iter().map(|l| l.relation).collect();
    assert!(
        relations.contains(&Relation::CalledBy)
            && relations.contains(&Relation::Calls)
            && relations.contains(&Relation::TestedBy),
        "{relations:?}"
    );
    let caller = discovery
        .fragments
        .iter()
        .find(|f| f.code_ref.path == path("src/api.rs"))
        .expect("caller");
    assert_eq!(
        (caller.code_ref.start_line, caller.code_ref.end_line),
        (1, 3)
    );
    assert!(
        caller
            .reason_selected
            .iter()
            .any(|r| r == "construct calling `save`"),
        "{:?}",
        caller.reason_selected
    );
    let wrapper = discovery
        .fragments
        .iter()
        .find(|f| f.code_ref.path == path("src/tx.rs"))
        .expect("callee");
    assert_eq!(wrapper.reason_selected, ["called by `save`"]);
    assert!(
        discovery
            .fragments
            .iter()
            .any(|f| f.reason_selected.iter().any(|r| r == "type used by `save`")),
        "{:?}",
        discovery.fragments
    );
    assert_eq!(discovery.usage.files, 4);
    assert_eq!(
        discovery.usage.semantic_items, 4,
        "the not-applicable query costs nothing"
    );
}

#[test]
fn diff_without_facts_is_partial_with_a_named_gap() {
    let (_, packets) = run(limits(), &[]);
    let discovery = &packets[0];
    assert_eq!(discovery.resolution, PacketResolution::Partial);
    assert_eq!(discovery.fragments.len(), 1);
    assert_eq!(
        (
            discovery.fragments[0].code_ref.start_line,
            discovery.fragments[0].code_ref.end_line
        ),
        (3, 10)
    );
    assert!(
        discovery.fragments[0]
            .reason_selected
            .iter()
            .any(|r| r == "hunk context (head)")
    );
    assert_eq!(discovery.gaps, ["NO_SEMANTIC_FACTS: src/store.rs (head)"]);
}

#[test]
fn supporting_context_yields_to_the_budget_first() {
    let reader = Reader::new();
    let (_, roomy) = run(limits(), &[facts(&reader)]);
    let construct = roomy[0].fragments[0].tokens;
    // Exactly the construct fits; callers and tests do not.
    let tight = Limits {
        unit_tokens: construct,
        request_tokens: construct,
        ..limits()
    };
    let (_, packets) = run(tight, &[facts(&reader)]);
    let discovery = &packets[0];
    assert_eq!(discovery.fragments.len(), 1);
    assert!(
        discovery
            .excluded
            .iter()
            .all(|e| e.reason == "BUDGET" && e.priority == Priority::Supporting)
    );
    assert!(!discovery.excluded.is_empty());
}

#[test]
fn required_guard_over_budget_makes_the_obligation_unknown() {
    let reader = Reader::new();
    let starved = Limits {
        unit_tokens: 10,
        request_tokens: 10,
        ..limits()
    };
    let (plan, packets) = run(starved, &[facts(&reader)]);
    let verification = &packets[1];
    assert!(
        verification
            .excluded
            .iter()
            .any(|e| e.reason == REQUIRED_OVER_BUDGET)
    );
    assert_eq!(verification.resolution, PacketResolution::Unavailable);
    let supported = Assessment {
        rule_id: "EFFECT-01".into(),
        status: ObligationStatus::Supported,
        evidence_ids: Vec::new(),
        limitations: Vec::new(),
    };
    let assessments = context_limited(&plan, &packets, vec![supported]);
    assert_eq!(assessments.len(), 1);
    assert!(matches!(
        &assessments[0].status,
        ObligationStatus::Unknown { reason_code, .. } if reason_code == "CONTEXT_LIMIT"
    ));

    // Without any rule the unread discovery span still keeps coverage partial.
    let gaps = context_gaps(&packets);
    assert!(gaps.iter().all(|g| g.starts_with(REQUIRED_OVER_BUDGET)) && !gaps.is_empty());
    let mut bare = plan;
    bare.obligations.clear();
    let coverage = |task_gaps: &[String]| {
        reduce(&ReduceInput {
            plan: &bare,
            assessments: &[],
            evidence: &[],
            findings: &[],
            candidates: &[],
            task_gaps,
            questions: &[],
            outcome: ExecutionOutcome::PlanExecuted,
            attention_threshold: DEFAULT_ATTENTION_THRESHOLD,
            now: "2026-10-25T00:00:00Z".parse().expect("timestamp"),
        })
        .coverage
    };
    assert_eq!(coverage(&gaps), Coverage::Partial);
    assert_eq!(coverage(&[]), Coverage::CompleteForScope);
}

#[test]
fn operations_answer_from_facts_and_stay_partial() {
    let reader = Reader::new();
    let facts = [facts(&reader)];
    let ops = Ops {
        facts: &facts,
        evidence: &[],
        reader: &reader,
    };
    let symbol = SymbolRef {
        side: SourceSide::Head,
        path: path("src/store.rs"),
        name: "save".into(),
    };
    let callers = ops.run(&Operation::FindCallers {
        symbol: symbol.clone(),
        limit: 10,
    });
    assert_eq!(
        callers.status,
        OpStatus::Partial,
        "an answered edge never proves absence"
    );
    assert_eq!(callers.refs.len(), 1);
    let other = SymbolRef {
        name: "load".into(),
        ..symbol
    };
    assert_eq!(
        ops.run(&Operation::FindReferences {
            symbol: other,
            limit: 10
        })
        .status,
        OpStatus::Unsupported
    );
    let hits = ops.run(&Operation::SearchSource {
        query: "unwrap".into(),
        paths: Vec::new(),
        side: SourceSide::Head,
        limit: 1,
    });
    assert_eq!((hits.status, hits.refs.len()), (OpStatus::Partial, 1));
    let bad = ops.run(&Operation::SearchSource {
        query: "a\nb".into(),
        paths: Vec::new(),
        side: SourceSide::Head,
        limit: 5,
    });
    assert_eq!(bad.status, OpStatus::Failed);
    let wire: Result<Operation, _> =
        serde_json::from_str(r#"{"op": "read_span", "code_ref": null, "shell": "rm -rf /"}"#);
    assert!(wire.is_err(), "unknown fields and operations are rejected");
}

/// `facts` plus the changed file's `Store` type, which `src/tx.rs` writes and `src/api.rs` reads.
fn with_access(reader: &Reader) -> SemanticFacts {
    let at = |p: &str, s, e| {
        reader
            .code_ref(SourceSide::Head, &path(p), s, e)
            .expect("ref")
    };
    let mut facts = facts(reader);
    let mut store = facts.symbols[0].clone();
    store.name = "Store".into();
    store.kind = "struct".into();
    store.container = Vec::new();
    store.definition = at("src/store.rs", 1, 1);
    store.writers = answered(vec![at("src/tx.rs", 3, 3)]);
    store.readers = answered(vec![at("src/api.rs", 1, 1)]);
    for edge in [
        &mut store.references,
        &mut store.callers,
        &mut store.callees,
        &mut store.type_definitions,
    ] {
        *edge = EdgeSet::not_applicable();
    }
    store.calling_constructs = Vec::new();
    facts.symbols.push(store);
    facts
}

#[test]
fn a_writer_elsewhere_refutes_and_is_never_cut_silently() {
    let reader = Reader::new();
    let facts = [with_access(&reader)];
    let (_, packets) = run(limits(), &facts);
    let discovery = &packets[0];
    let writer = discovery
        .fragments
        .iter()
        .find(|f| f.code_ref.path == path("src/tx.rs"))
        .expect("writer");
    assert_eq!(writer.priority, Priority::Refuting);
    assert!(writer.reason_selected.iter().any(|r| r == "writes `Store`"));
    let relations: Vec<Relation> = discovery.links.iter().map(|l| l.relation).collect();
    assert!(
        relations.contains(&Relation::WrittenBy) && relations.contains(&Relation::ReadBy),
        "{relations:?}"
    );

    let required: u32 = discovery
        .fragments
        .iter()
        .filter(|f| f.priority == Priority::Required)
        .map(|f| f.tokens)
        .sum();
    let tight = Limits {
        unit_tokens: required,
        ..limits()
    };
    let (_, packets) = run(tight, &facts);
    let discovery = &packets[0];
    assert!(
        discovery
            .gaps
            .iter()
            .any(|g| g.starts_with("REFUTING_OVER_BUDGET: src/tx.rs:")),
        "{:?}",
        discovery.gaps
    );
    assert_eq!(discovery.resolution, PacketResolution::Partial);
}
