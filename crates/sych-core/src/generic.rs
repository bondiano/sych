//! The generic rule pack shipped with the engine (spec §11.4). A generic rule is enabled by
//! its review profile and activates only on the changes it is about; a repository rule with
//! the same id replaces it. Domain rules are bound to their inputs after planning: migration
//! rules to the deployment model (`migration::bind`), TF-CHANGE to a Terraform plan and
//! K8S-ROLLOUT to rendered manifests; a missing input keeps them `UNKNOWN`, never excluded.

use crate::config::{Acceptance, AppliesTo, Effect, ReviewProfile, Rule, RuleKind, RuleSeverity};
use crate::effects::{EffectLedger, EffectMatch, LedgerEntry};
use crate::{EvidenceKind, FileChange, RepoPath, RunFacts, SyntaxKind, glob_match};

/// What a change must touch for the rule to activate.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Trigger {
    Paths(&'static [&'static str]),
    /// A path listed in `implementation_refs` of the effect registry, or an effect of the
    /// ledger that is not preserved.
    Effects,
    /// Changed syntax shapes of these kinds, or a ledger effect whose lifetime or failure
    /// behaviour changed.
    Lifetime(&'static [SyntaxKind]),
    /// Changed tests or CI gates, or a newly skipped test or suppressed lint.
    TestIntegrity,
}

pub(crate) struct GenericRule {
    pub rule: Rule,
    pub profile: ReviewProfile,
    pub trigger: Trigger,
}

/// The rule whose subject is the tests themselves, so test files are its evidence.
pub const TEST_INTEGRITY: &str = "TEST-INTEGRITY";
const MIGRATION: &[&str] = &["**/migrations/**", "**/migrate/**", "**/*.sql"];
const TEST_PATHS: &[&str] = &[
    "**/tests/**",
    "**/test/**",
    "**/*_test.*",
    "**/*.test.*",
    "**/*.spec.*",
    ".github/workflows/**",
    "**/.gitlab-ci.yml",
];

fn rule(id: &str, title: &str, severity: RuleSeverity, statement: &str, criteria: &[&str]) -> Rule {
    Rule {
        id: id.into(),
        version: 1,
        title: title.into(),
        kind: RuleKind::Invariant,
        required: true,
        severity_on_violation: Some(severity),
        owner: "engine".into(),
        applies_to: AppliesTo::default(),
        statement: statement.into(),
        rationale: Some("generic rule shipped with Sych (spec §11.4)".into()),
        contract_refs: Vec::new(),
        acceptance: Some(Acceptance {
            evidence_kinds: vec![
                EvidenceKind::Source,
                EvidenceKind::Semantic,
                EvidenceKind::Test,
            ],
            criteria: criteria.iter().map(|c| (*c).to_owned()).collect(),
        }),
        exceptions: Vec::new(),
    }
}

/// Ids of the shipped generic rules, e.g. for labelled cases that expect their obligations.
pub fn generic_rule_ids() -> Vec<String> {
    pack().into_iter().map(|g| g.rule.id).collect()
}

/// A path the migration rules activate on.
pub fn is_migration_path(path: &str) -> bool {
    MIGRATION.iter().any(|g| glob_match(g, path))
}

pub(crate) fn pack() -> Vec<GenericRule> {
    use ReviewProfile::{Correctness, Infrastructure, MigrationCompatibility, SideEffects};
    use RuleSeverity::{High, Medium};
    let generic = |rule, profile, trigger| GenericRule {
        rule,
        profile,
        trigger,
    };
    let migration = |id, title, statement| {
        generic(
            rule(id, title, High, statement, &[statement]),
            MigrationCompatibility,
            Trigger::Paths(MIGRATION),
        )
    };
    vec![
        generic(
            rule(
                "EFFECT-01",
                "Required effects are kept",
                High,
                "A changed path keeps the target, condition, multiplicity and order of its registered effects.",
                &[
                    "Every registered effect of the changed path still happens with the same target, condition, multiplicity and order.",
                ],
            ),
            SideEffects,
            Trigger::Effects,
        ),
        generic(
            rule(
                "EFFECT-02",
                "Failure, cancellation and lifetime behaviour is kept",
                Medium,
                "Errors, cancellation and resource lifetime keep the behaviour the code relied on before the change.",
                &[
                    "Errors are still propagated or handled as before.",
                    "Spawned work, listeners, timers and subscriptions are still owned, cancelled or cleaned up.",
                ],
            ),
            SideEffects,
            Trigger::Lifetime(&[
                SyntaxKind::SwallowedException,
                SyntaxKind::DiscardedResult,
                SyntaxKind::DetachedSpawn,
                SyntaxKind::GuardAcrossAwait,
                SyntaxKind::BlockingInAsync,
                SyntaxKind::EffectWithoutCleanup,
                SyntaxKind::UnpairedListener,
                SyntaxKind::UnpairedTimer,
                SyntaxKind::UnpairedSubscription,
                SyntaxKind::AsyncEffectUnguarded,
            ]),
        ),
        migration(
            "MIG-COMPAT",
            "Rollout states stay compatible",
            "Every reachable rollout state of the schema, API or event migration stays compatible.",
        ),
        migration(
            "MIG-RESTART",
            "Backfills survive a restart",
            "A repeated or resumed backfill or batch loses no data.",
        ),
        migration(
            "MIG-CONTRACT",
            "Old contracts are removed only when unused",
            "An old contract is removed only after its required use has stopped.",
        ),
        migration(
            "MIG-RECOVERY",
            "New persistent data can be recovered",
            "A new form of persistent data has a working recovery path.",
        ),
        generic(
            rule(
                "TF-CHANGE",
                "Infrastructure replacement is intended",
                High,
                "A resource replacement or deletion matches the accepted intent.",
                &[
                    "Every replaced or deleted resource holds no data, or the change keeps its data \
                   (create_before_destroy, prevent_destroy, a snapshot/restore or moved block).",
                ],
            ),
            Infrastructure,
            Trigger::Paths(&["**/*.tf", "**/*.tfvars"]),
        ),
        generic(
            rule(
                "K8S-ROLLOUT",
                "Rollout order keeps dependencies available",
                High,
                "The workload rollout order keeps its dependencies available.",
                &["The rollout order and dependency availability agree."],
            ),
            Infrastructure,
            Trigger::Paths(&["**/k8s/**", "**/helm/**", "**/charts/**"]),
        ),
        generic(
            rule(
                TEST_INTEGRITY,
                "Important checks are not weakened",
                Medium,
                "A change to assertions, skips, fixtures or CI gates does not quietly weaken an important check.",
                &[
                    "No assertion, test or CI gate that guarded behaviour was removed, skipped or loosened without an equivalent check.",
                ],
            ),
            Correctness,
            Trigger::TestIntegrity,
        ),
    ]
}

fn any_glob(globs: &[&str], path: &RepoPath) -> bool {
    globs.iter().any(|g| glob_match(g, path.as_str()))
}

/// Ledger entries of these paths that `keep` selects.
fn ledger_hit(
    ledger: &EffectLedger,
    paths: &mut dyn Iterator<Item = &RepoPath>,
    keep: impl Fn(&LedgerEntry) -> bool,
) -> bool {
    let paths: Vec<&RepoPath> = paths.collect();
    ledger
        .entries
        .iter()
        .any(|e| keep(e) && paths.contains(&&e.path))
}

/// Whether `files` (old and new paths) activate `trigger`.
pub(crate) fn activated(
    trigger: Trigger,
    files: &[&FileChange],
    effects: &[Effect],
    facts: &RunFacts,
) -> bool {
    let syntax = &facts.syntax;
    let paths = || {
        files
            .iter()
            .flat_map(|f| f.old.iter().chain(&f.new))
            .map(|s| &s.path)
    };
    let changed_shape = |kinds: &[SyntaxKind]| {
        syntax.iter().flat_map(|d| &d.facts).any(|f| {
            f.in_change && kinds.contains(&f.kind) && paths().any(|p| *p == f.code_ref.path)
        })
    };
    match trigger {
        Trigger::Paths(globs) => paths().any(|p| any_glob(globs, p)),
        Trigger::Effects => {
            paths().any(|p| effects.iter().any(|e| e.implementation_refs.contains(p)))
                || ledger_hit(&facts.ledger, &mut paths(), |e| {
                    e.matched != EffectMatch::Preserved
                })
        }
        Trigger::Lifetime(kinds) => {
            changed_shape(kinds)
                || ledger_hit(&facts.ledger, &mut paths(), |e| {
                    e.notes
                        .iter()
                        .any(|n| n.starts_with("lifetime") || n.starts_with("failure"))
                })
        }
        Trigger::TestIntegrity => {
            paths().any(|p| any_glob(TEST_PATHS, p))
                || changed_shape(&[SyntaxKind::TestSkipped, SyntaxKind::LintSuppressed])
        }
    }
}
