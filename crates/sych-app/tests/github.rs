//! The GitHub adapter against the loopback fake: App JWT → installation token
//! scoped to one repository and cached, normalized reads and error mapping v1.

use std::fs;
use std::process::Command;

use serde_json::{Value, json};
use sych_adapters::github::{AppKey, Github, capabilities};
use sych_app::server::smoke::smoke;
use sych_core::scm::ScmErrorKind;

mod fake_github;

use fake_github::{TOKEN, lock, pull_json, start};

const NOW: i64 = 1_800_000_000;

/// A fresh App key from `ssh-keygen`, as GitHub issues it (PKCS#1 PEM).
pub fn app_key() -> AppKey {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("app.pem");
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "rsa", "-b", "2048", "-m", "PEM", "-N", "", "-f"])
        .arg(&path)
        .status()
        .expect("ssh-keygen is needed for App key fixtures");
    assert!(status.success());
    AppKey::from_pem(1, &fs::read_to_string(path).expect("pem")).expect("key")
}

#[tokio::test(flavor = "current_thread")]
async fn installation_token_is_scoped_cached_and_capabilities_recorded() {
    let (url, fake) = start().await;
    let gh = Github::new(&url, app_key()).expect("client");
    let installation = gh.installation("o/r", NOW).await.expect("installation");
    assert!(
        capabilities(&installation.permissions)
            .missing(false)
            .is_empty()
    );
    let first = gh.token(installation.id, "o/r", NOW).await.expect("token");
    let again = gh
        .token(installation.id, "o/r", NOW + 60)
        .await
        .expect("token");
    assert_eq!(first.expose(), TOKEN);
    assert_eq!(again, first);
    let s = lock(&fake);
    assert_eq!(s.tokens_issued, 1, "the cached token is reused");
    assert!(
        s.log
            .iter()
            .any(|l| l.ends_with(r#"{"repositories":["r"]}"#)),
        "{:?}",
        s.log
    );
}

#[tokio::test(flavor = "current_thread")]
async fn pull_state_and_files_come_from_the_api() {
    let (url, fake) = start().await;
    let files: Vec<Value> = (0..250)
        .map(|i| json!({"filename": format!("f{i}")}))
        .collect();
    {
        let mut s = lock(&fake);
        s.pulls.insert(7, pull_json(7, "abc", "def"));
        s.files.insert(7, files);
        s.files.insert(
            8,
            (0..3001)
                .map(|i| json!({"filename": i.to_string()}))
                .collect(),
        );
    }
    let gh = Github::new(&url, app_key()).expect("client");
    let token = gh.token(9, "o/r", NOW).await.expect("token");
    let pull = gh.pull("o/r", 7, &token).await.expect("pull");
    assert_eq!(
        (pull.head_sha.as_str(), pull.base_sha.as_str()),
        ("abc", "def")
    );
    assert!(pull.open && !pull.fork("o/r"));
    assert_eq!(
        gh.pull_files("o/r", 7, &token).await.expect("files").len(),
        250
    );
    let cut = gh
        .pull_files("o/r", 8, &token)
        .await
        .expect_err("over the limit");
    assert_eq!(cut.kind, ScmErrorKind::IncompleteDiff);
}

#[tokio::test(flavor = "current_thread")]
async fn errors_map_without_guessing() {
    let (url, fake) = start().await;
    let gh = Github::new(&url, app_key()).expect("client");
    let token = gh.token(9, "o/r", NOW).await.expect("token");
    lock(&fake).fail_next.insert(
        "/pulls/1".into(),
        (
            403,
            json!({"message": "Resource not accessible by integration"}),
        ),
    );
    let forbidden = gh.pull("o/r", 1, &token).await.expect_err("403");
    assert_eq!(
        forbidden.kind,
        ScmErrorKind::Unauthorized,
        "403 is not unsupported"
    );
    lock(&fake)
        .fail_next
        .insert("/pulls/1".into(), (429, json!({"message": "slow down"})));
    let limited = gh.pull("o/r", 1, &token).await.expect_err("429");
    assert_eq!(limited.kind, ScmErrorKind::RateLimited);
    assert_eq!(limited.retry_after_seconds, Some(7));
    let missing = gh.pull("o/r", 404, &token).await.expect_err("404");
    assert_eq!(missing.kind, ScmErrorKind::NotFound);
    assert!(
        !format!("{missing:?}").contains(TOKEN),
        "no token in errors"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn scm_smoke_writes_reads_finds_and_deletes_its_own_comment() {
    let (url, fake) = start().await;
    let gh = Github::new(&url, app_key()).expect("client");
    let report = smoke(&gh, "o/r", 7, "sych[bot]", "n1", NOW)
        .await
        .expect("smoke");
    assert!(report.found_by_marker && report.deleted, "{report:?}");
    assert_eq!(report.marker, "sych:smoke:n1");
    assert_eq!(
        report
            .revisions
            .iter()
            .map(|r| r.revision)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(report.missing_capabilities.is_empty());
    let s = lock(&fake);
    assert!(s.comments.is_empty(), "the smoke comment is removed");
    let writes: Vec<&String> = s
        .log
        .iter()
        .filter(|l| {
            l.starts_with("POST /repos") || l.starts_with("PATCH") || l.starts_with("DELETE")
        })
        .collect();
    assert_eq!(writes.len(), 3, "{writes:?}");
}
