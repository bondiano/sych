//! Snapshot, scripted provider and engine set-up shared by the analysis suite.
#![allow(dead_code)] // each suite uses some of them

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::{env, fs};

use serde_json::{Value, json};
use sych_adapters::git::{LocalSnapshot, SnapshotRequest};
use sych_adapters::openrouter::CAPABILITIES;
use sych_app::analysis::{Engine, Outcome};
use sych_core::analysis::packet_evidence;
use sych_core::broker::{BrokerInput, Limits as BrokerLimits, build_packets};
use sych_core::config::{BillingKind, Budgets, Enforcement, RepositoryPolicy, ServerConfig};
use sych_core::effects::EffectLedger;
use sych_core::model::{
    Checkpoint, Denied, ErrorKind, Finish, Hop, Ledger, LedgerError, Limits, ModelProvider,
    ModelReply, ModelRequest, ObservedUsage, PriceCard, ProviderError, Purpose, REVIEW_ROLES,
    Reservation, Route, RoutePlan, Router, Settlement, Speaker,
};
use sych_core::{
    AttemptId, MicroUsd, ObligationStatus, PolicySnapshotId, Redactor, RunFacts, RunId,
    Sha256Digest, SourceSide, Timestamp, plan, sha256,
};
use sych_storage::{Partition, Storage, StoreConfig, TENANT_LOCAL};
use tempfile::TempDir;

pub const RULES: &str = "schema_version: 1\nrules:\n  - id: ORDER-01\n    version: 1\n    title: Missing items total zero\n    kind: invariant\n    required: true\n    severity_on_violation: high\n    owner: backend\n    applies_to:\n      paths: [\"src/**\"]\n    statement: total() returns 0 when items is undefined.\n";
pub const BASE: &str = "export function total(items) {\n  if (!items) {\n    return 0;\n  }\n  return items.reduce(sum, 0);\n}\n";
pub const SAFE: &str =
    "export function total(items) {\n  return (items ?? []).reduce(sum, 0);\n}\n";

pub fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .env_clear()
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}");
}

pub fn repo(second_unit: bool) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    fs::create_dir_all(d.join(".sych")).expect("dir");
    fs::create_dir_all(d.join("src")).expect("dir");
    // With a second unit the rule covers both, so its verification packet spans both.
    let rules = if second_unit {
        RULES.replace("[\"src/**\"]", "[\"src/**\", \"lib/**\"]")
    } else {
        RULES.to_owned()
    };
    fs::write(d.join(".sych/rules.yaml"), rules).expect("rules");
    fs::write(d.join("src/order.ts"), BASE).expect("base");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "base"]);
    fs::write(d.join("src/order.ts"), SAFE).expect("head");
    if second_unit {
        fs::create_dir_all(d.join("lib")).expect("dir");
        fs::write(
            d.join("lib/format.ts"),
            "export const money = (n) => `${n} ₽`;\n",
        )
        .expect("lib");
    }

    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "head"]);
    dir
}

/// Answers by schema name; counts every request.
pub struct Scripted {
    pub answer: fn(&ModelRequest) -> Result<ModelReply, ProviderError>,
    pub calls: RefCell<Vec<String>>,
}

impl ModelProvider for Scripted {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply, ProviderError> {
        self.calls.borrow_mut().push(request.schema_name.clone());
        (self.answer)(request)
    }
}

pub fn reply(text: &str) -> ModelReply {
    ModelReply {
        text: text.to_owned(),
        finish: Finish::Complete,
        resolved_model: Some("fixture/model-1".into()),
        upstream_provider: Some("Fixture".into()),
        generation_id: Some("gen-1".into()),
        usage: ObservedUsage {
            input_tokens: Some(100),
            output_tokens: Some(50),
            reasoning_tokens: None,
        },
        reported_cost: Some(MicroUsd(10)),
        billing_source: None,
        allowance: None,
        list_cost: None,
    }
}

/// `ref-N` and its first line from the data fence labels of the user message.
pub fn first_ref(request: &ModelRequest, side: &str) -> (String, u32) {
    let user = &request
        .messages
        .iter()
        .find(|m| m.speaker == Speaker::User)
        .expect("user")
        .content;
    user.lines()
        .filter_map(|l| l.strip_prefix("<<<DATA "))
        .find_map(|l| {
            let mut words = l.split(' ').skip(1);
            let (id, s, place) = (words.next()?, words.next()?, words.next()?);
            let start = place.rsplit_once(':')?.1.split('-').next()?.parse().ok()?;
            (id.starts_with("ref-") && s == side).then(|| (id.to_owned(), start))
        })
        .expect("a handed-out span")
}

pub fn supported(request: &ModelRequest) -> Value {
    let (id, line) = first_ref(request, "HEAD");
    let anchor = json!([{"ref_id": id, "start_line": line, "end_line": line}]);
    json!({
        "obligation_id": "ob-1", "applicability": "APPLIES", "proposed_status": "SUPPORTED",
        "criterion_results": [{"criterion_id": "c-1", "status": "MET", "anchors": anchor, "evidence_ids": []}],
        "evidence_ids": ["ev-1"], "rationale": "`?? []` keeps the zero total", "limitations": [],
        "missing_inputs": [], "not_applicable_basis": null
    })
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "scripted answers share the provider signature"
)]
pub fn supported_and_empty(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let empty = json!({"candidates": [], "context_requests": [], "unresolved_questions": []});
    Ok(reply(
        &match request.schema_name.as_str() {
            "obligation_assessment" => supported(request),
            "review" => {
                let mut both = empty;
                both["assessments"] = json!([supported(request)]);
                both
            }
            _ => empty,
        }
        .to_string(),
    ))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "scripted answers share the provider signature"
)]
pub fn prose(_: &ModelRequest) -> Result<ModelReply, ProviderError> {
    Ok(reply("Looks fine to me."))
}

pub fn timeout(_: &ModelRequest) -> Result<ModelReply, ProviderError> {
    Err(ProviderError {
        kind: ErrorKind::Timeout,
        dispatched: true,
        message: "no answer in 90 s".into(),
        retry_after_seconds: None,
    })
}

pub async fn run(
    answer: fn(&ModelRequest) -> Result<ModelReply, ProviderError>,
    budgets: Budgets,
) -> (Outcome, Vec<String>) {
    run_on(repo(false), answer, budgets, 14_000, true).await
}

pub async fn run_on(
    repo: TempDir,
    answer: fn(&ModelRequest) -> Result<ModelReply, ProviderError>,
    budgets: Budgets,
    batch_tokens: u32,
    verify: bool,
) -> (Outcome, Vec<String>) {
    let (outcome, calls, _) = run_reusing(
        repo.path(),
        answer,
        budgets,
        batch_tokens,
        verify,
        BTreeMap::new(),
    )
    .await;
    (outcome, calls)
}

/// The storage ledger, also keeping every checkpoint the router hands it.
pub struct Recorder<'a> {
    pub storage: &'a Storage,
    pub kept: RefCell<Vec<Checkpoint>>,
}

impl Ledger for Recorder<'_> {
    async fn reserve(
        &self,
        reservation: &Reservation,
        limits: &Limits,
    ) -> Result<Result<(), Denied>, LedgerError> {
        self.storage.reserve(reservation, limits).await
    }

    async fn settle(
        &self,
        attempt: AttemptId,
        settlement: &Settlement,
        at: Timestamp,
    ) -> Result<(), LedgerError> {
        self.storage.settle(attempt, settlement, at).await
    }

    async fn checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), LedgerError> {
        self.kept.borrow_mut().push(checkpoint.clone());
        Ok(())
    }
}

/// [`run_on`] in a new run that may reuse `reuse`; also returns the run's checkpoints.
pub async fn run_reusing(
    repo: &Path,
    answer: fn(&ModelRequest) -> Result<ModelReply, ProviderError>,
    budgets: Budgets,
    batch_tokens: u32,
    verify: bool,
    reuse: BTreeMap<Sha256Digest, Checkpoint>,
) -> (Outcome, Vec<String>, Vec<Checkpoint>) {
    let snap = LocalSnapshot::capture(&SnapshotRequest {
        repo,
        base: "HEAD~1",
        head: "HEAD",
        diff_base: None,
        max_blob_bytes: 1 << 20,
    })
    .expect("snapshot");
    let files = snap
        .policy_files(SourceSide::TargetTip)
        .expect("policy files");
    let policy = RepositoryPolicy::parse(&files, &ServerConfig::engine_default()).expect("policy");
    let facts = RunFacts::default();
    let plan = plan(&snap.snapshot.id, &snap.manifest, &policy, &facts);
    let store_dir = tempfile::tempdir().expect("store");
    let store = StoreConfig {
        database: store_dir.path().join("sych.db"),
        artifacts: store_dir.path().join("artifacts"),
    };
    Storage::migrate(&store).await.expect("migrate");
    let storage = Storage::open(&store).await.expect("open");
    let policy_id = PolicySnapshotId(sha256(b"policy"));
    let handle = storage
        .begin_run(
            &Partition {
                tenant_id: TENANT_LOCAL.into(),
                access_scope: snap.snapshot.repository.clone(),
            },
            "fixture",
            &snap.snapshot.id,
            &policy_id,
            Timestamp::now(),
        )
        .await
        .expect("run");
    let packets = build_packets(
        &BrokerInput {
            run_id: handle.id,
            snapshot_id: &snap.snapshot.id,
            policy_snapshot_id: &policy_id,
            access_scope: &snap.snapshot.repository,
            plan: &plan,
            manifest: &snap.manifest,
            facts: &[],
            syntax: &[],
            ci: &[],
            ledger: &EffectLedger::default(),
            lint: &[],
            limits: BrokerLimits::from_settings(&policy.config.context),
        },
        &snap,
    );
    let packet_evidence: Vec<_> = packets
        .iter()
        .map(|p| packet_evidence(p, &snap.snapshot, Timestamp::now()).expect("hash"))
        .collect();
    let provider = Scripted {
        answer,
        calls: RefCell::new(Vec::new()),
    };
    let ledger = Recorder {
        storage: &storage,
        kept: RefCell::default(),
    };
    let mut router = router(&provider, &ledger, handle.id, &budgets);
    router.reuse = reuse;
    let engine = Engine {
        router,
        snap: &snap,
        plan: &plan,
        packets: &packets,
        packet_evidence: &packet_evidence,
        facts: &facts,
        evidence: &packet_evidence,
        known: Vec::new(),
        batch_tokens,
        risk_audit: true,
        verify,
        parallel: budgets
            .analysis
            .max_parallel_calls
            .map_or(1, |n| usize::try_from(n).unwrap_or(1)),
        language: policy.config.review.language,
    };
    let outcome = engine.run().await;
    drop(engine);
    (
        outcome,
        provider.calls.into_inner(),
        ledger.kept.into_inner(),
    )
}

/// The scripted profile for every review role, with no fallback.
pub fn router<'a, L: Ledger>(
    provider: &'a Scripted,
    storage: &'a L,
    run_id: RunId,
    budgets: &Budgets,
) -> Router<'a, Scripted, L> {
    Router {
        hops: BTreeMap::from([(
            "fixture".to_owned(),
            Ok(Hop {
                provider,
                route: Route {
                    profile: "fixture".into(),
                    provider: "fixture".into(),
                    adapter: "scripted".into(),
                    model: "fixture/model".into(),
                    billing: BillingKind::ApiUsage,
                    enforcement: Enforcement::ApiHard,
                    capabilities: CAPABILITIES,
                },
                price: Some(PriceCard {
                    input_per_mtok: 300_000,
                    output_per_mtok: 1_200_000,
                }),
                fixed_fee: None,
            }),
        )]),
        plan: plan_routes("fixture"),
        on: Vec::new(),
        cooldown_seconds: 0,
        ledger: storage,
        tenant_id: TENANT_LOCAL.into(),
        run_id,
        purpose: Purpose::Analysis,
        limits: Limits::new(budgets, Purpose::Analysis),
        deadline: None,
        now: Timestamp::now,
        redactor: Redactor::builtin(),
        cooldowns: RefCell::default(),
        reuse: BTreeMap::new(),
        models_revision: "fixture".into(),
        eligible: None,
    }
}

/// Every review role on the one scripted profile, no fallback.
pub fn plan_routes(profile: &str) -> RoutePlan {
    RoutePlan {
        chains: REVIEW_ROLES
            .iter()
            .map(|&r| (r, vec![profile.to_owned()]))
            .collect(),
        revision: "fixture".into(),
    }
}

pub fn reason(status: &ObligationStatus) -> &str {
    match status {
        ObligationStatus::Unknown { reason_code, .. } => reason_code,
        ObligationStatus::Supported => "SUPPORTED",
        ObligationStatus::Violated => "VIOLATED",
        ObligationStatus::NotApplicable { .. } => "NOT_APPLICABLE",
    }
}
