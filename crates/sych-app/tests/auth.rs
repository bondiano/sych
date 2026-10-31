//! Browser sessions over real HTTP: a person signs in through the App's GitHub authorization
//! and reads only the repositories GitHub grants them; every change they make is confirmed
//! with GitHub now and needs the UI's origin; a client exchanges its token for a session;
//! logout and expiry end sessions; every action lands in the audit log.

use reqwest::header::HeaderMap;
use reqwest::redirect::Policy;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

mod fake_github;
mod http;
mod world;

use fake_github::lock;
use http::listen;
use world::{PUBLIC_BASE, World, advance, commit_result, now, world};

/// One request; `origin` and `key` only when given. Returns the status, headers and body.
async fn send(
    url: &str,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    origin: Option<&str>,
    body: &Value,
) -> (StatusCode, HeaderMap, Value) {
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .build()
        .expect("client");
    let mut req = client.request(method.clone(), format!("{url}{path}"));
    if let Some(cookie) = cookie {
        req = req.header("cookie", cookie);
    }

    if let Some(origin) = origin {
        req = req.header("origin", origin);
    }

    if method == Method::POST && path.starts_with("/v1/") {
        req = req.header("idempotency-key", format!("{path}-{}", now().as_second()));
    }

    if !body.is_null() {
        req = req.json(body);
    }

    let reply = req.send().await.expect("send");
    let (status, headers) = (reply.status(), reply.headers().clone());
    (status, headers, reply.json().await.unwrap_or(Value::Null))
}

/// The value a `Set-Cookie` header gives cookie `name`, with its attributes.
fn set_cookie(headers: &HeaderMap, name: &str) -> Option<(String, String)> {
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| {
            let (pair, attributes) = v.split_once(';').unwrap_or((v, ""));
            let value = pair.strip_prefix(&format!("{name}="))?;
            Some((value.to_owned(), attributes.to_owned()))
        })
}

/// Signs `login` in through the fake GitHub; returns the `Cookie` header of the session.
async fn sign_in(url: &str, login: &str) -> String {
    let (status, headers, _) = send(
        url,
        Method::GET,
        "/auth/github/gh/login",
        None,
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let (state, _) = set_cookie(&headers, "sych_oauth_state").expect("state cookie");
    let path = format!("/auth/github/gh/callback?code=code-{login}&state={state}");
    let cookie = format!("sych_oauth_state={state}");
    let (status, headers, _) =
        send(url, Method::GET, &path, Some(&cookie), None, &Value::Null).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{login} signs in");
    assert_eq!(headers["location"], "/ui/");
    let (token, attributes) = set_cookie(&headers, "sych_session").expect("session cookie");
    assert!(
        attributes.contains("HttpOnly") && attributes.contains("Secure"),
        "{attributes}"
    );
    format!("sych_session={token}")
}

async fn reviewed() -> (World, String) {
    let w = world(None).await;
    commit_result(&w, 2).await;
    w.engine.lineage_once().await.expect("lineage");
    let url = listen(&w).await;
    (w, url)
}

fn grant(w: &World, login: &str, level: &str) {
    lock(&w.fake).permissions.insert(login.into(), level.into());
}

/// Actions in the audit log as the maintainer client of every repository reads them.
async fn audited(url: &str) -> Vec<(String, String)> {
    let (_, page) = http::call(url, "GET", "/v1/audit", Some("lead"), None, &Value::Null).await;
    page["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            (
                e["action"].as_str().unwrap_or_default().to_owned(),
                e["actor"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_person_reads_only_what_github_grants_them() {
    let (w, url) = reviewed().await;
    let (_, _, sign_ins) = send(&url, Method::GET, "/auth/github", None, None, &Value::Null).await;
    assert_eq!(
        sign_ins,
        json!([{"connection_id": "gh", "login_url": "/auth/github/gh/login"}])
    );
    let (status, headers, _) = send(
        &url,
        Method::GET,
        "/auth/github/gh/login",
        None,
        None,
        &Value::Null,
    )
    .await;
    let location = headers["location"].to_str().expect("location");
    assert_eq!(status, StatusCode::FOUND);
    assert!(
        location.contains("/login/oauth/authorize?client_id=Iv1.test"),
        "{location}"
    );
    assert!(
        location.contains("redirect_uri=https%3A%2F%2Fsych.test%2Fauth%2Fgithub%2Fgh%2Fcallback")
    );

    let forged = "/auth/github/gh/callback?code=code-alice&state=forged";
    let (status, _, _) = send(
        &url,
        Method::GET,
        forged,
        Some("sych_oauth_state=real"),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a state this browser does not hold"
    );

    grant(&w, "alice", "read");
    let alice = sign_in(&url, "alice").await;
    let (_, _, me) = send(
        &url,
        Method::GET,
        "/v1/me",
        Some(&alice),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(
        (me["actor"].as_str(), me["kind"].as_str()),
        (Some("github:alice"), Some("user"))
    );
    assert_eq!(me["repositories"][0]["role"], "reader");
    let (_, _, changes) = send(
        &url,
        Method::GET,
        "/v1/changes",
        Some(&alice),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(changes["items"].as_array().map(Vec::len), Some(1));

    let bob = sign_in(&url, "bob").await;
    let (_, _, me) = send(&url, Method::GET, "/v1/me", Some(&bob), None, &Value::Null).await;
    assert_eq!(
        me["repositories"],
        json!([]),
        "no access on GitHub, nothing here"
    );
    let (_, _, changes) = send(
        &url,
        Method::GET,
        "/v1/changes",
        Some(&bob),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(changes["items"], json!([]));
    let (status, _, _) = send(
        &url,
        Method::GET,
        "/v1/changes/1",
        Some(&bob),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        audited(&url)
            .await
            .contains(&("SESSION_STARTED".into(), "github:alice".into()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_persons_change_is_confirmed_with_github_now_from_the_ui_origin() {
    let (w, url) = reviewed().await;
    grant(&w, "alice", "triage");
    let alice = sign_in(&url, "alice").await;
    let fb = json!({"judgement": "USEFUL", "scope": "finding", "rationale": "real"});
    for origin in [None, Some("https://elsewhere.test")] {
        let (status, _, _) = send(
            &url,
            Method::POST,
            "/v1/findings/1/feedback",
            Some(&alice),
            origin,
            &fb,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "origin {origin:?}");
    }
    let (status, _, _) = send(
        &url,
        Method::POST,
        "/v1/findings/1/feedback",
        Some(&alice),
        Some(PUBLIC_BASE),
        &fb,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    grant(&w, "alice", "read");
    let (status, _, body) = send(
        &url,
        Method::POST,
        "/v1/findings/2/feedback",
        Some(&alice),
        Some(PUBLIC_BASE),
        &fb,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "GitHub took triage away: {body}"
    );

    // GitHub reports a maintainer as `write` in `permission`; `role_name` keeps maintain.
    grant(&w, "carol", "maintain");
    let carol = sign_in(&url, "carol").await;
    let waiver = json!({"change_id": 1, "rule_id": "EFFECT-01", "paths": ["src/**"], "reason": "window",
        "owner": "backend", "expires_at": "2027-06-01T00:00:00Z", "until": null});
    let (status, _, body) = send(
        &url,
        Method::POST,
        "/v1/waivers",
        Some(&carol),
        Some(PUBLIC_BASE),
        &waiver,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let log = audited(&url).await;
    for entry in [
        ("FINDING_FEEDBACK", "github:alice"),
        ("WAIVER_CREATED", "github:carol"),
    ] {
        assert!(
            log.contains(&(entry.0.into(), entry.1.into())),
            "{entry:?} in {log:?}"
        );
    }
    assert_eq!(
        log.iter().filter(|(a, _)| a == "FINDING_FEEDBACK").count(),
        1,
        "the refused one is not"
    );
}

/// A session cookie of the configured client `name`.
async fn client_session(url: &str, name: &str) -> String {
    let reply = reqwest::Client::new()
        .post(format!("{url}/auth/token"))
        .bearer_auth(format!("{name}-token-0123456789"))
        .send()
        .await
        .expect("token");
    let (token, _) = set_cookie(reply.headers(), "sych_session").expect("session");
    format!("sych_session={token}")
}

async fn me(url: &str, cookie: &str) -> (StatusCode, Value) {
    let (status, _, body) =
        send(url, Method::GET, "/v1/me", Some(cookie), None, &Value::Null).await;
    (status, body)
}

#[tokio::test(flavor = "current_thread")]
async fn a_client_acts_through_its_session_and_every_request_is_audited() {
    let (w, url) = reviewed().await;
    let (status, _, _) = send(&url, Method::POST, "/auth/token", None, None, &Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let writer = client_session(&url, "writer").await;
    let (_, body) = me(&url, &writer).await;
    assert_eq!(
        (body["actor"].as_str(), body["kind"].as_str()),
        (Some("writer"), Some("client"))
    );

    let rerun = "/v1/changes/1/reruns";
    let (status, _, _) = send(
        &url,
        Method::POST,
        rerun,
        Some(&writer),
        Some(PUBLIC_BASE),
        &json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let run = w.run_ids().await.last().cloned().expect("rerun");
    let cancel = format!("/v1/runs/{run}/cancel");
    let (status, _, _) = send(
        &url,
        Method::POST,
        &cancel,
        Some(&writer),
        Some(PUBLIC_BASE),
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let first = w.run_ids().await[0].clone();
    let storage = &w.app.storage;
    storage
        .enqueue_publication("local", &first, "writer", now().as_second())
        .await
        .expect("publish");

    let log = audited(&url).await;
    for action in [
        "RUN_REQUESTED",
        "RUN_CANCELLED",
        "PUBLICATION_REQUESTED",
        "SESSION_STARTED",
    ] {
        assert!(
            log.contains(&(action.into(), "writer".into())),
            "{action} in {log:?}"
        );
    }
    let (status, _) =
        http::call(&url, "GET", "/v1/audit", Some("reader"), None, &Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "the log is for maintainers");
}

#[tokio::test(flavor = "current_thread")]
async fn sessions_end_at_logout_from_the_ui_origin_and_at_expiry() {
    let (w, url) = reviewed().await;
    let writer = client_session(&url, "writer").await;
    let logout = "/auth/logout";
    let (status, _, _) = send(
        &url,
        Method::POST,
        logout,
        Some(&writer),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "logout from another origin");
    let (status, headers, _) = send(
        &url,
        Method::POST,
        logout,
        Some(&writer),
        Some(PUBLIC_BASE),
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cleared = set_cookie(&headers, "sych_session");
    assert!(cleared.is_some_and(|(v, a)| v.is_empty() && a.contains("Max-Age=0")));
    assert_eq!(
        me(&url, &writer).await.0,
        StatusCode::UNAUTHORIZED,
        "the ended session"
    );
    assert!(
        audited(&url)
            .await
            .contains(&("SESSION_ENDED".into(), "writer".into()))
    );

    grant(&w, "alice", "read");
    let alice = sign_in(&url, "alice").await;
    assert_eq!(me(&url, &alice).await.0, StatusCode::OK);
    advance(8 * 3600 + 1);
    assert_eq!(
        me(&url, &alice).await.0,
        StatusCode::UNAUTHORIZED,
        "the expired session"
    );
}
