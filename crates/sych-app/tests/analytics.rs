//! Analytics over real HTTP and storage: a person's latest judgement counts once, findings of
//! finished pull requests settle by state, reasons outlive the result, an unknown charge is
//! never zero, the tenant's budget is only for a caller of all of it, and another scope sees
//! none of it.

use std::collections::BTreeSet;
use std::path::Path;

use reqwest::StatusCode;
use serde_json::{Value, json};
use sych_app::render_sarif::sarif;
use sych_app::{render, report_view};
use sych_core::config::PolicyDiff;
use sych_core::effects::EffectLedger;

mod fake_github;
mod http;
mod world;

use http::{call, listen};
use world::database::inspect;
use world::{commit_result, now, world};

async fn get(url: &str, who: &str, path: &str) -> (StatusCode, Value) {
    call(url, "GET", path, Some(who), None, &Value::Null).await
}

async fn judge(url: &str, who: &str, key: &str, finding: i64, judgement: &str) {
    let body = json!({"judgement": judgement, "scope": "finding", "rationale": "seen"});
    let path = format!("/v1/findings/{finding}/feedback");
    let (status, answer) = call(url, "POST", &path, Some(who), Some(key), &body).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
}

#[tokio::test(flavor = "current_thread")]
async fn latest_judgements_and_settled_findings_of_readable_repositories() {
    let w = world(None).await;
    commit_result(&w, 3).await;
    w.engine.lineage_once().await.expect("lineage");
    let url = listen(&w).await;
    let (_, page) = get(&url, "reader", "/v1/findings").await;
    let findings = page["items"].as_array().cloned().unwrap_or_default();
    let id = |i: usize| findings[i]["id"].as_i64().expect("id");

    judge(&url, "writer", "f1", id(0), "FALSE_POSITIVE").await;
    judge(&url, "writer", "f2", id(0), "WONT_FIX").await;
    judge(&url, "lead", "f3", id(1), "USEFUL").await;
    sqlx::query("UPDATE changes SET scm_state = 'MERGED'")
        .execute(&inspect(w.dir.config()).await)
        .await
        .expect("merge");

    let (status, quality) = get(&url, "reader", "/v1/analytics/quality?since=0").await;
    assert_eq!(status, StatusCode::OK, "{quality}");
    let judged: BTreeSet<String> = quality["judgements"]
        .as_array()
        .expect("judgements")
        .iter()
        .map(|j| format!("{} {} {}", j["repository"], j["judgement"], j["count"]))
        .collect();
    assert_eq!(
        judged,
        BTreeSet::from([
            r#""o/r" "USEFUL" 1"#.to_owned(),
            r#""o/r" "WONT_FIX" 1"#.to_owned(),
        ]),
        "a changed mind counts once: {quality}"
    );

    assert_eq!(
        quality["settled"],
        json!([{"key": "DISMISSED", "count": 1}, {"key": "OPEN", "count": 2}]),
        "{quality}"
    );
    let open: BTreeSet<String> = findings[1..]
        .iter()
        .map(|f| f["severity"].to_string())
        .collect();
    let merged = quality["merged_with_open"].as_array().expect("merged");
    assert_eq!(merged.len(), open.len(), "{quality}");
    assert!(
        merged
            .iter()
            .all(|m| m["changes"] == 1 && open.contains(&m["severity"].to_string()))
    );

    let (_, later) = get(&url, "reader", "/v1/analytics/quality?since=99999999999").await;
    assert_eq!(later["judgements"], json!([]));
    let (_, other) = get(
        &url,
        "reader",
        "/v1/analytics/quality?since=0&repository_id=999",
    )
    .await;
    assert_eq!(other["settled"], json!([]), "only the asked repository");
    let (_, recent) = get(&url, "reader", "/v1/analytics/quality").await;
    assert_eq!(recent["settled"], quality["settled"], "30 days by default");
    let (_, nothing) = get(&url, "outsider", "/v1/analytics/quality?since=0").await;
    assert_eq!(
        (
            &nothing["judgements"],
            &nothing["settled"],
            &nothing["merged_with_open"]
        ),
        (&json!([]), &json!([]), &json!([]))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn reasons_outlive_the_result_and_older_runs_are_counted_apart() {
    let w = world(None).await;
    commit_result(&w, 1).await;
    let url = listen(&w).await;

    let (status, honesty) = get(&url, "reader", "/v1/analytics/honesty?since=0").await;
    assert_eq!(status, StatusCode::OK, "{honesty}");
    let runs: i64 = honesty["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .filter_map(|c| c["count"].as_i64())
        .sum();
    assert_eq!(
        (runs, honesty["unrecorded_runs"].as_i64()),
        (1, Some(0)),
        "{honesty}"
    );
    let day = &honesty["coverage_by_day"][0];
    assert_eq!(day["runs"], 1, "{honesty}");
    assert_eq!(day["day"].as_i64().map(|d| d % 86_400), Some(0));
    let (_, summary) = get(&url, "reader", "/v1/summary?since=0").await;
    let coverage = summary["runs_by_coverage"][0]["key"].as_str().unwrap_or("");
    assert!(
        ["COMPLETE_FOR_SCOPE", "PARTIAL", "UNAVAILABLE"].contains(&coverage),
        "coverage as the API spells it: {summary}"
    );

    sqlx::query("DELETE FROM run_reasons")
        .execute(&inspect(w.dir.config()).await)
        .await
        .expect("a run from before the table");
    let (_, older) = get(&url, "reader", "/v1/analytics/honesty?since=0").await;
    assert_eq!(
        (&older["outcomes"], older["unrecorded_runs"].as_i64()),
        (&json!([]), Some(1))
    );
    let (_, nothing) = get(&url, "outsider", "/v1/analytics/honesty?since=0").await;
    assert_eq!(
        (
            nothing["unrecorded_runs"].as_i64(),
            &nothing["coverage_by_day"]
        ),
        (Some(0), &json!([]))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cost_counts_unknown_charges_apart_and_budgets_only_for_the_whole_tenant() {
    let w = world(None).await;
    commit_result(&w, 1).await;
    let run = w.run_ids().await[0].clone();
    let at = now();
    let day = at.strftime("%Y-%m-%d").to_string();
    let pool = inspect(w.dir.config()).await;
    for (id, state, charge) in [("a1", "SETTLED", Some(1200_i64)), ("a2", "UNKNOWN", None)] {
        sqlx::query(
            "INSERT INTO model_attempts (id, tenant_id, run_id, purpose, kind, template, profile,
                 provider, requested_model, billing_kind, enforcement, day, month,
                 reserved_micro_usd, input_tokens_bound, state, charge_micro_usd, input_tokens,
                 output_tokens, created_at)
             VALUES ($1, 'local', $2, 'discovery', 'primary', 't', 'cheap_api', 'openrouter',
                 'm', 'api_usage', 'api_hard', $3, $4, 2000, 100, $5, $6, 10, 5, $7)",
        )
        .bind(id)
        .bind(&run)
        .bind(&day)
        .bind(&day[..7])
        .bind(state)
        .bind(charge)
        .bind(at.as_second())
        .execute(&pool)
        .await
        .expect("attempt");
    }
    let url = listen(&w).await;

    let (status, cost) = get(&url, "reader", "/v1/analytics/cost?since=0").await;
    assert_eq!(status, StatusCode::OK, "{cost}");
    let row = json!([{"key": "discovery", "attempts": 2, "cost_known_micro_usd": 1200,
        "unknown_cost_attempts": 1}]);
    assert_eq!(cost["by_purpose"], row, "{cost}");
    assert_eq!(cost["by_model"][0]["key"], "m");
    assert_eq!(cost["by_repository"][0]["key"], "o/r");
    assert_eq!(cost["top_changes"][0]["cost_known_micro_usd"], 1200);
    assert!(
        cost["budget"].is_null(),
        "a scoped caller sees no tenant budget"
    );

    let (_, tenant) = get(&url, "lead", "/v1/analytics/cost?since=0").await;
    let budget = &tenant["budget"];
    assert_eq!(
        (
            budget["day_spent_micro_usd"].as_i64(),
            budget["month_spent_micro_usd"].as_i64()
        ),
        (Some(3200), Some(3200)),
        "a held reservation counts as the ledger counts it: {tenant}"
    );
    assert_eq!(budget["day_limit_micro_usd"], 3_000_000);
    assert!(budget["month_forecast_micro_usd"].as_i64() >= Some(3200));
    let (_, one) = get(&url, "lead", "/v1/analytics/cost?since=0&repository_id=1").await;
    assert!(
        one["budget"].is_null(),
        "a repository has no budget of its own"
    );

    let (_, nothing) = get(&url, "outsider", "/v1/analytics/cost?since=0").await;
    assert_eq!(
        (&nothing["by_purpose"], &nothing["top_changes"]),
        (&json!([]), &json!([]))
    );

    let (status, latency) = get(&url, "reader", "/v1/analytics/latency?since=0").await;
    assert_eq!(status, StatusCode::OK, "{latency}");
    assert_eq!(latency["run_duration"]["n"], 1, "{latency}");
    assert!(latency["run_duration"]["p50_seconds"].as_i64() >= Some(0));
    assert_eq!(latency["queue_wait"]["n"], 1, "the claimed job: {latency}");
    let (_, hidden) = get(&url, "outsider", "/v1/analytics/latency?since=0").await;
    assert_eq!(
        hidden["run_duration"],
        json!({"n": 0, "p50_seconds": null, "p90_seconds": null})
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_weekly_report_is_json_or_markdown_over_the_callers_repositories() {
    let w = world(None).await;
    commit_result(&w, 1).await;
    let url = listen(&w).await;

    let (status, report) = get(&url, "reader", "/v1/reports/weekly").await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        report["until"]
            .as_i64()
            .zip(report["since"].as_i64())
            .map(|(u, s)| u - s),
        Some(7 * 86_400)
    );
    assert_eq!(report["latency"]["run_duration"]["n"], 1, "{report}");
    assert!(report["cost"]["budget"].is_null(), "a scoped caller");

    let reply = reqwest::Client::new()
        .get(format!("{url}/v1/reports/weekly?format=markdown"))
        .bearer_auth("reader-token-0123456789")
        .send()
        .await
        .expect("send");
    let kind = reply.headers()["content-type"]
        .to_str()
        .unwrap_or("")
        .to_owned();
    let text = reply.text().await.expect("text");
    assert!(kind.starts_with("text/markdown"), "{kind}");
    assert!(text.starts_with("# Sych weekly report"), "{text}");
    assert!(text.contains("- Incomplete coverage:"), "{text}");

    assert_eq!(
        get(&url, "reader", "/v1/reports/weekly?format=pdf").await.0,
        StatusCode::BAD_REQUEST
    );
    let (_, nothing) = get(&url, "outsider", "/v1/reports/weekly").await;
    assert_eq!(nothing["latency"]["run_duration"]["n"], 0);
}

fn markdown_for(result: &sych_core::ReviewResult, repo: &Path) -> String {
    let external = sych_core::ExternalInputManifest::freeze(
        &result.source_snapshot_id,
        &result.policy_snapshot_id,
        &[],
        now(),
    )
    .expect("external");
    let evidence_manifest =
        sych_core::EvidenceManifest::freeze(&external, &[], now()).expect("evidence");
    let report = render::Report {
        snapshot: None,
        diff_manifest: None,
        policy: sych_core::PolicySnapshot {
            schema_version: sych_core::SchemaVersion,
            id: result.policy_snapshot_id.clone(),
            source: sych_core::PolicySource::EngineDefaults,
            rules_hash: sych_core::sha256(b"r"),
            config_hash: sych_core::sha256(b"c"),
            authorization_policy_version: 1,
            created_at: now(),
        },
        policy_review: render::PolicyReview {
            schema_version: sych_core::SchemaVersion,
            provenance: Vec::new(),
            diff: PolicyDiff::default(),
        },
        external_input_manifest: external,
        evidence_manifest,
        evidence: Vec::new(),
        semantic_facts: Vec::new(),
        syntax_facts: Vec::new(),
        ci_facts: Vec::new(),
        effect_ledger: EffectLedger::default(),
        lint_facts: Vec::new(),
        architecture_facts: Vec::new(),
        db_schema_facts: Vec::new(),
        context_packets: Vec::new(),
        review_result: result.clone(),
    };
    let (view, _) = report_view::build(&report, repo, sych_core::Redactor::builtin());
    render::markdown(&report, &view)
}

#[tokio::test(flavor = "current_thread")]
async fn preference_findings_carry_rule_id_into_quality_analytics() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    world::commit_claimed_with(&w, &head, 1, b"policy", |r| {
        r.findings[0].category = sych_core::FindingCategory::Nit;
        r.findings[0].severity = sych_core::Severity::Low;
        r.findings[0].support = sych_core::Support::Preference;
        r.findings[0].rule_id = Some("API-PREF".into());
        r.findings[0].title = "Use domain noun".into();
        r.findings[0].obligation_ids.clear();
    })
    .await;
    w.engine.lineage_once().await.expect("lineage");
    while w.engine.publish_once().await.expect("publish") {}

    let result = w.result(0).await;
    let json_text = render::json(&result).expect("json");
    assert!(
        json_text.contains("\"rule_id\": \"API-PREF\""),
        "{json_text}"
    );
    let sarif_doc = sarif(&result);
    assert_eq!(sarif_doc["runs"][0]["results"][0]["ruleId"], "API-PREF");
    assert!(
        sarif_doc["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .expect("rules")
            .iter()
            .any(|r| r["id"] == "API-PREF"),
        "{sarif_doc}"
    );
    let summary = fake_github::lock(&w.fake).comments[&1].1.clone();
    assert!(
        summary.contains("- Nit · API-PREF: Use domain noun"),
        "{summary}"
    );
    let md_text = markdown_for(&result, &w.origin);
    assert!(md_text.contains("  - rule: API-PREF"), "{md_text}");

    let url = listen(&w).await;

    let (_, page) = get(&url, "reader", "/v1/findings").await;
    let item = &page["items"][0];
    assert_eq!(item["rule_id"], "API-PREF", "{item}");
    let finding_id = item["id"].as_i64().expect("id");

    judge(&url, "lead", "pref-f1", finding_id, "USEFUL").await;
    let (status, quality) = get(&url, "reader", "/v1/analytics/quality?since=0").await;
    assert_eq!(status, StatusCode::OK, "{quality}");
    assert_eq!(
        quality["judgements"],
        json!([{
            "repository": "o/r",
            "rule_id": "API-PREF",
            "category": "NIT",
            "severity": "LOW",
            "judgement": "USEFUL",
            "count": 1
        }]),
        "{quality}"
    );
}
