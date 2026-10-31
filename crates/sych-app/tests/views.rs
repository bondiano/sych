//! Dashboard reads over real HTTP and storage: lists and details limited to readable
//! repositories, id cursors, waiver revocation that the next review honours, totals and usage
//! that never turn an unknown charge into zero, and a pruned result reported as such.

use reqwest::StatusCode;
use serde_json::{Value, json};

mod fake_github;
mod http;
mod world;

use http::{call, listen};
use world::database::inspect;
use world::{World, commit_result, now, world};

async fn get(url: &str, who: &str, path: &str) -> (StatusCode, Value) {
    call(url, "GET", path, Some(who), None, &Value::Null).await
}

/// One reviewed pull request with `defects` open findings; returns the server's URL.
async fn reviewed(defects: u32) -> (World, String) {
    let w = world(None).await;
    commit_result(&w, defects).await;
    w.engine.lineage_once().await.expect("lineage");
    let url = listen(&w).await;
    (w, url)
}

#[tokio::test(flavor = "current_thread")]
async fn lists_show_only_readable_repositories_and_page_by_id() {
    let (_w, url) = reviewed(3).await;

    let (status, repos) = get(&url, "reader", "/v1/repositories").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(repos[0]["full_name"], "o/r");
    assert_eq!(get(&url, "outsider", "/v1/repositories").await.1, json!([]));

    let (_, changes) = get(&url, "reader", "/v1/changes").await;
    let change = &changes["items"][0];
    assert_eq!(
        (change["number"].as_i64(), change["open_findings"].as_i64()),
        (Some(7), Some(3))
    );
    assert_eq!(change["latest_run"]["state"], "COMPLETED");
    assert!(changes["next_before"].is_null());
    let (_, hidden) = get(&url, "outsider", "/v1/changes").await;
    assert_eq!(hidden["items"], json!([]), "another scope lists nothing");
    let id = change["id"].as_i64().expect("id");
    let (status, _) = get(&url, "outsider", &format!("/v1/changes/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, detail) = get(&url, "reader", &format!("/v1/changes/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["runs"].as_array().map(Vec::len), Some(1));
    assert!(detail["events"].as_array().is_some_and(|e| !e.is_empty()));

    let (_, first) = get(&url, "reader", "/v1/findings?limit=2").await;
    let before = first["next_before"].as_i64().expect("a second page");
    let (_, second) = get(
        &url,
        "reader",
        &format!("/v1/findings?limit=2&before={before}"),
    )
    .await;
    assert!(second["next_before"].is_null());
    let mut ids: Vec<i64> = [&first, &second]
        .iter()
        .flat_map(|p| p["items"].as_array().cloned().unwrap_or_default())
        .map(|f| f["id"].as_i64().expect("id"))
        .collect();
    ids.dedup();
    assert_eq!(
        ids.len(),
        3,
        "three findings over two pages, none twice: {ids:?}"
    );
    let top = &first["items"][0];
    assert_eq!(
        (top["severity"].as_str(), top["state"].as_str()),
        (Some("HIGH"), Some("OPEN"))
    );
    assert!(
        top["title"]
            .as_str()
            .is_some_and(|t| t.starts_with("Defect"))
    );
    assert_eq!(
        get(&url, "outsider", "/v1/findings").await.1["items"],
        json!([])
    );

    for bad in ["/v1/findings?limit=0", "/v1/changes?limit=201"] {
        assert_eq!(
            get(&url, "reader", bad).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn feedback_reaches_a_finding_by_its_listed_id() {
    let (_w, url) = reviewed(1).await;
    let (_, page) = get(&url, "reader", "/v1/findings").await;
    let id = page["items"][0]["id"].as_i64().expect("id");
    let (status, _) = get(&url, "outsider", &format!("/v1/findings/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let fb =
        json!({"judgement": "FALSE_POSITIVE", "scope": "finding", "rationale": "guarded upstream"});
    let path = format!("/v1/findings/{id}/feedback");
    let (status, _) = call(&url, "POST", &path, Some("writer"), Some("f1"), &fb).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, detail) = get(&url, "reader", &format!("/v1/findings/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["summary"]["state"], "DISMISSED");
    assert_eq!(detail["feedback"][0]["actor"], "writer");
    let last = detail["history"].as_array().and_then(|h| h.last()).cloned();
    assert_eq!(
        last.map(|e| (e["to_state"].clone(), e["actor"].clone())),
        Some((json!("DISMISSED"), json!("writer")))
    );
    assert!(detail["finding"]["explanation"].is_string(), "{detail}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_revoked_waiver_stops_applying_and_lists_by_state() {
    let (w, url) = reviewed(1).await;
    let waiver = json!({"change_id": 1, "rule_id": "EFFECT-01", "paths": ["src/**"], "reason": "window",
        "owner": "backend", "expires_at": "2027-06-01T00:00:00Z", "until": null});
    let (status, created) = call(
        &url,
        "POST",
        "/v1/waivers",
        Some("lead"),
        Some("w1"),
        &waiver,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["waiver_id"].as_i64().expect("id");
    let at = now().as_second();
    assert_eq!(
        w.app
            .storage
            .active_waivers(1, at)
            .await
            .expect("active")
            .len(),
        1
    );
    let listed = |state: &'static str| {
        let url = url.clone();
        async move {
            let (_, list) = get(&url, "reader", &format!("/v1/waivers?state={state}")).await;
            list.as_array().map_or(0, Vec::len)
        }
    };
    assert_eq!(listed("active").await, 1);

    let path = format!("/v1/waivers/{id}/revoke");
    let (status, _) = call(
        &url,
        "POST",
        &path,
        Some("writer"),
        Some("r1"),
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "revoking needs a maintainer");
    let (status, revoked) = call(&url, "POST", &path, Some("lead"), Some("r1"), &Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revoked["revoked_at"], json!(at));
    assert!(
        w.app
            .storage
            .active_waivers(1, at)
            .await
            .expect("active")
            .is_empty()
    );
    assert_eq!((listed("active").await, listed("revoked").await), (0, 1));
    let (_, again) = call(&url, "POST", &path, Some("lead"), Some("r1"), &Value::Null).await;
    assert_eq!(
        (again["reused"].as_bool(), again["revoked_at"].as_i64()),
        (Some(true), Some(at))
    );
    assert_eq!(
        get(&url, "reader", "/v1/waivers?state=gone").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test(flavor = "current_thread")]
async fn totals_and_usage_count_unknown_charges_apart_and_only_readable_runs() {
    let (w, url) = reviewed(2).await;
    let run = w.run_ids().await[0].clone();
    let day = now().strftime("%Y-%m-%d").to_string();
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
        .bind(now().as_second())
        .execute(&pool)
        .await
        .expect("attempt");
    }
    sqlx::query("INSERT INTO subscription_fees VALUES ('local', 'pi', $1, 20000000, 0, 0)")
        .bind(&day[..7])
        .execute(&pool)
        .await
        .expect("fee");

    let (_, attempts) = get(&url, "reader", &format!("/v1/runs/{run}/attempts")).await;
    assert_eq!(attempts.as_array().map(Vec::len), Some(2));
    assert!(
        attempts[1]["charge_micro_usd"].is_null(),
        "unknown stays unknown"
    );
    let (status, _) = get(&url, "outsider", &format!("/v1/runs/{run}/attempts")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, summary) = get(&url, "reader", "/v1/summary?since=0").await;
    assert_eq!(
        summary["runs_by_state"],
        json!([{"key": "COMPLETED", "count": 1}])
    );
    assert_eq!(
        summary["finding_events"],
        json!([{"key": "OPEN", "count": 2}])
    );
    assert_eq!(
        (
            summary["cost_known_micro_usd"].as_i64(),
            summary["unknown_cost_attempts"].as_i64()
        ),
        (Some(1200), Some(1))
    );
    let (_, nothing) = get(&url, "outsider", "/v1/summary?since=0").await;
    assert_eq!(
        (
            nothing["runs_by_state"].clone(),
            nothing["cost_known_micro_usd"].as_i64()
        ),
        (json!([]), Some(0))
    );

    let range = format!("/v1/usage?from={day}&to={day}");
    let (_, usage) = get(&url, "writer", &range).await;
    assert_eq!(usage["days"][0]["cost_known_micro_usd"], 1200);
    assert_eq!(usage["days"][0]["unknown_cost_attempts"], 1);
    assert_eq!(
        usage["subscription_fees"][0]["amount_micro_usd"],
        20_000_000
    );
    let (_, narrow) = get(&url, "reader", &range).await;
    assert_eq!(narrow["days"][0]["attempts"], 2);
    assert_eq!(
        narrow["subscription_fees"],
        json!([]),
        "fees are the whole tenant's"
    );
    let (status, _) = get(&url, "reader", "/v1/usage?from=2027-13-01").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "current_thread")]
async fn a_pruned_result_is_reported_and_its_finding_keeps_its_history() {
    let (w, url) = reviewed(1).await;
    let run = w.run_ids().await[0].clone();
    sqlx::query("DELETE FROM artifacts WHERE sha256 IN (SELECT result_sha256 FROM runs)")
        .execute(&inspect(w.dir.config()).await)
        .await
        .expect("prune");

    let (status, body) = get(&url, "reader", &format!("/v1/runs/{run}/result")).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("RESULT_PRUNED"))
    );
    let (_, page) = get(&url, "reader", "/v1/findings").await;
    let id = page["items"][0]["id"].as_i64().expect("id");
    let (status, detail) = get(&url, "reader", &format!("/v1/findings/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(detail["finding"].is_null());
    assert_eq!(detail["history"][0]["to_state"], "OPEN");
}
