//! Planning and prompt-isolation contracts for `RuleKind::Preference`.

use std::collections::BTreeMap;

use sych_core::analysis::{Context, expansion, prompts};
use sych_core::{
    BackendStatus, Completeness, EdgeSet, Language, PathClass, QueryStatus, RunFacts,
    SchemaVersion, SemanticFacts, SourceSide, SymbolFacts, canonical_sha256,
};

use super::{changed, code_ref, planned, policy, snapshot};

const API_PREFERENCE_RULE: &str = "schema_version: 1\nrules:\n  - id: API-PREF\n    version: 2\n    title: Handler names\n    kind: preference\n    required: false\n    owner: api\n    applies_to:\n      paths: [\"src/api/**\"]\n    statement: Name HTTP handlers after the resource verb.\n    rationale: Consistent route tables.\n    contract_refs: [\".sych/contracts/api.md\"]\n    exceptions:\n      - paths: [\"src/api/legacy/**\"]\n        reason: Legacy routes stay until v2.\n        owner: api\n        author: team\n        expires_at: null\n        until: v2\n";

#[test]
fn preference_rules_match_units_without_creating_obligations_or_touching_unrelated_prompts() {
    let with_pref = policy(&[
        (".sych/rules.yaml", API_PREFERENCE_RULE),
        (".sych/contracts/api.md", "# API conventions\nUse nouns.\n"),
    ]);
    let without_pref = policy(&[]);
    let p = planned(
        vec![changed("src/api/orders.rs"), changed("src/other.rs")],
        &with_pref,
        &RunFacts::default(),
    );
    assert_eq!(p.obligations.len(), 0);
    assert_eq!(p.scope.required_obligation_ids.len(), 0);
    assert_eq!(p.preferences.len(), 1);
    let pref = &p.preferences[0];
    let api_unit = p
        .scope
        .units
        .iter()
        .find(|u| {
            u.paths
                .iter()
                .any(|path| path.as_str() == "src/api/orders.rs")
        })
        .expect("api unit");
    assert_eq!(pref.rule_id, "API-PREF");
    assert_eq!(pref.review_unit_ids, [api_unit.id]);
    assert_eq!(pref.contracts.len(), 1);
    assert_eq!(pref.exceptions.len(), 1);

    let ctx = Context::default();
    let api_disc = prompts::discovery(&ctx, &[], &[pref], "src/api/orders.rs");
    assert!(api_disc.task.contains("# Preferences of this change"));
    assert!(
        api_disc
            .task
            .contains("- pref-1: preference API-PREF v2 (Handler names)")
    );
    assert!(
        api_disc
            .task
            .contains("rationale: Consistent route tables.")
    );
    assert!(api_disc.task.contains("contract .sych/contracts/api.md"));

    // Unrelated unit (`src/other.rs`) has empty matched preferences: prompts and hashes stay identical.
    let other_unit = p
        .scope
        .units
        .iter()
        .find(|u| u.paths.iter().any(|path| path.as_str() == "src/other.rs"))
        .expect("other unit");
    let matched_other: Vec<_> = p
        .preferences
        .iter()
        .filter(|pref| pref.review_unit_ids.contains(&other_unit.id))
        .collect();
    let base_plan = planned(
        vec![changed("src/other.rs")],
        &without_pref,
        &RunFacts::default(),
    );
    assert_eq!(matched_other.len(), 0);
    assert_eq!(base_plan.preferences.len(), 0);
    let disc_a = prompts::discovery(&ctx, &[], &matched_other, "src/other.rs");
    assert!(!disc_a.task.contains("# Preferences of this change"));
    assert_eq!(
        canonical_sha256(&prompts::review(
            &ctx,
            &[],
            &matched_other,
            &[],
            "src/other.rs"
        ))
        .expect("hash"),
        canonical_sha256(&prompts::review(&ctx, &[], &[], &[], "src/other.rs")).expect("hash"),
    );
    assert!(
        !expansion::expansion(&ctx, &[], &matched_other, "src/other.rs", &[])
            .task
            .contains("# Preferences of this change")
    );
    assert!(
        !prompts::verify(&ctx, &[], &[], &matched_other)
            .task
            .contains("# Preferences of this change")
    );
}

#[test]
fn preference_rules_apply_through_semantic_links_and_omit_unmatched_preferences() {
    let repo = policy(&[
        (".sych/rules.yaml", API_PREFERENCE_RULE),
        (".sych/contracts/api.md", "# API\n"),
    ]);
    let untouched = planned(vec![changed("src/other.rs")], &repo, &RunFacts::default());
    assert_eq!(untouched.preferences.len(), 0);

    let answered = |refs| EdgeSet {
        status: QueryStatus::Answered,
        completeness: Completeness::Partial,
        refs,
        external: Vec::new(),
        unresolved: Vec::new(),
    };
    let semantic = SemanticFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snapshot(),
        side: SourceSide::Head,
        language: Language::Rust,
        status: BackendStatus::Ready,
        producer: None,
        position_encoding: None,
        build_configuration: BTreeMap::new(),
        query_scope: Vec::new(),
        symbols: vec![SymbolFacts {
            name: "OrderDto".into(),
            kind: "struct".into(),
            container: Vec::new(),
            definition: code_ref("src/shared/dto.rs", 5),
            path_class: PathClass::Production,
            references: answered(vec![code_ref("src/api/orders.rs", 14)]),
            readers: EdgeSet::not_applicable(),
            writers: EdgeSet::not_applicable(),
            callers: answered(Vec::new()),
            calling_constructs: Vec::new(),
            callees: answered(Vec::new()),
            type_definitions: answered(Vec::new()),
            implementations: EdgeSet::not_applicable(),
        }],
        diagnostics: Vec::new(),
        limitations: Vec::new(),
    };
    let linked = planned(
        vec![changed("src/shared/dto.rs")],
        &repo,
        &RunFacts {
            semantic: vec![semantic],
            ..RunFacts::default()
        },
    );
    assert_eq!(linked.preferences.len(), 1);
    assert_eq!(linked.preferences[0].rule_id, "API-PREF");
    assert!(
        linked.preferences[0].limitations.iter().any(|l| l.contains(
            "applies through a semantic link: `OrderDto` is used from src/api/orders.rs:14"
        )),
        "{:?}",
        linked.preferences[0].limitations
    );
}
