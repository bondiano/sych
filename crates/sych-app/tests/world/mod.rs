//! The server under test: loopback GitHub fake, a local origin fetched over `file://`, a
//! migrated store and the fake model, with a per-thread clock.
#![allow(dead_code)]

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};
use sych_adapters::git::{LocalSnapshot, SnapshotRequest};
use sych_adapters::github::webhook::parse;
use sych_adapters::github::{AppKey, Github, Secret};
use sych_app::args::{ModelSelection, Switch};
use sych_app::server::{App, Conn, Counters, Engine, ReviewSettings, binding};
use sych_core::config::{ApiClient, GithubConnection, RunMode};
use sych_core::sha256;
use sych_core::{ReviewResult, RunId, Timestamp};
use sych_core::{SourceSide, decode};
use sych_storage::{Admitted, ArtifactKind, Storage, StoreConfig};
use tempfile::TempDir;

use crate::fake_github::{Shared, lock, pull_json, start, stored_zip};

pub const T0: i64 = 1_800_000_000;

thread_local! {
    /// Each test runs on its own thread, so each has its own clock.
    static CLOCK: Cell<i64> = const { Cell::new(T0) };
}

pub fn now() -> Timestamp {
    Timestamp::from_second(CLOCK.with(Cell::get)).expect("timestamp")
}

pub fn advance(seconds: i64) {
    CLOCK.with(|c| c.set(c.get() + seconds));
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

pub fn commit(origin: &Path, file: &str, text: &str) -> String {
    let path = origin.join(file);
    fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    fs::write(path, text).expect("write");
    git(origin, &["add", "."]);
    git(origin, &["commit", "-q", "-m", file]);
    git(origin, &["rev-parse", "HEAD"])
}

pub fn app_key() -> AppKey {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("app.pem");
    let ok = Command::new("ssh-keygen")
        .args(["-q", "-t", "rsa", "-b", "2048", "-m", "PEM", "-N", "", "-f"])
        .arg(&path)
        .status()
        .expect("ssh-keygen");
    assert!(ok.success());
    AppKey::from_pem(1, &fs::read_to_string(path).expect("pem")).expect("key")
}

/// API clients of every world: `(name, role, repositories)`, token = `<name>-token-0123456789`.
pub const CLIENTS: &[(&str, &str, &str)] = &[
    ("writer", "writer", "*"),
    ("reader", "reader", "gh/o/r"),
    ("outsider", "writer", "gh/x/y"),
    ("lead", "maintainer", "*"),
];

fn clients() -> Vec<(ApiClient, Secret)> {
    CLIENTS
        .iter()
        .map(|(name, role, repo)| {
            let client = serde_yaml_ng::from_value(
                serde_yaml_ng::to_value(json!({"name": name,
                "token_env": format!("{}_TOKEN", name.to_uppercase()), "role": role,
                "repositories": [repo]}))
                .expect("value"),
            )
            .expect("client");
            (client, Secret::new(format!("{name}-token-0123456789")))
        })
        .collect()
}

pub struct World {
    pub dir: TempDir,
    pub origin: PathBuf,
    pub base: String,
    pub fake: Shared,
    pub engine: Arc<Engine>,
    pub app: Arc<App>,
}

/// `policy` is the `.sych/config.yaml` of the target branch, if any.
pub async fn world(policy: Option<&str>) -> World {
    world_with(policy, None).await
}

/// The trusted config: engine defaults, or one metered route to the model at `endpoint`.
fn server_config(dir: &Path, endpoint: Option<&str>) -> PathBuf {
    // `CARGO_PKG_NAME` is set for every test process; the fake model ignores the key.
    let models = endpoint.map_or_else(String::new, |endpoint| {
        format!(
            "models:
  providers:
    openrouter:
      adapter: openrouter
      endpoint: \"{endpoint}\"
      api_key_env: CARGO_PKG_NAME
      allowed_modes: [personal_service]
  profiles:
    cheap_api:
      provider: openrouter
      model: deepseek/deepseek-v4.1-flash
      billing_kind: api_usage
      enforcement: api_hard
      upstreams: [deepseek]
      reasoning_effort: low
  roles:
    reviewer: cheap_api
  price_catalog_file: prices.json
"
        )
    });
    fs::write(
        dir.join("prices.json"),
        r#"{"schema_version":1,"revision":"test","prices":{"deepseek/deepseek-v4.1-flash":{"input_per_mtok":300000,"output_per_mtok":1200000}}}"#,
    )
    .expect("prices");
    let path = dir.join("sych.yaml");
    fs::write(
        &path,
        format!("schema_version: 1\nmode: personal\n{models}"),
    )
    .expect("config");
    path
}

/// [`world`] whose server reviews through `models.roles` against the model at `endpoint`.
pub async fn world_with(policy: Option<&str>, endpoint: Option<&str>) -> World {
    let dir = tempfile::tempdir().expect("tempdir");
    let origin = dir.path().join("git/o/r.git");
    fs::create_dir_all(&origin).expect("origin");
    git(&origin, &["init", "-q", "-b", "main"]);
    if let Some(policy) = policy {
        commit(&origin, ".sych/config.yaml", policy);
    }

    let base = commit(&origin, "src/lib.rs", "pub fn a() {}\n");
    let (url, fake) = start().await;
    let config: GithubConnection = serde_yaml_ng::from_value(
        serde_yaml_ng::to_value(json!({
            "id": "gh", "app_id": 1, "private_key_file_env": "KEY", "webhook_secret_env": "HOOK",
            "repositories": ["o/r"], "bot_login": "sych[bot]", "api_base_url": url, "publish": true,
            "git_base_url": format!("file://{}", dir.path().join("git").display()),
        "artifact_hosts": ["127.0.0.1"],
        "ci_reports": [{"report": "tests", "workflow": ".github/workflows/ci.yml", "job": "test",
            "artifact": "reports", "path": "junit.xml", "checkout_manifest": "sych-ci.json"}]
        }))
        .expect("value"),
    )
    .expect("connection");
    let store = StoreConfig {
        database: dir.path().join("sych.db"),
        artifacts: dir.path().join("artifacts"),
    };
    Storage::migrate(&store).await.expect("migrate");
    let config_path = server_config(dir.path(), endpoint);
    let app = Arc::new(App {
        storage: Storage::open(&store).await.expect("open"),
        github: BTreeMap::from([("gh".into(), (config.clone(), Secret::new("s".into())))]),
        clients: clients(),
        metrics_allow: vec![IpAddr::from([127, 0, 0, 1])],
        counters: Counters::default(),
        engine: OnceLock::new(),
        artifacts: dir.path().join("artifacts"),
        now,
    });
    let engine = Arc::new(Engine {
        app: Arc::clone(&app),
        conns: BTreeMap::from([(
            "gh".into(),
            Conn {
                gh: Github::new(&url, app_key()).expect("client"),
                binding: binding(&config),
                config,
            },
        )]),
        store,
        review: ReviewSettings {
            config: config_path,
            models: if endpoint.is_some() {
                ModelSelection::Roles
            } else {
                ModelSelection::Fake
            },
            semantic: Switch::Off,
            mode: RunMode::PersonalService,
        },
        owner: "w1".into(),
        lease_seconds: 60,
        // Real time against the test clock: preparation and analysis renew the lease often.
        heartbeat: Duration::from_millis(10),
        public_base_url: None,
        now,
    });
    let _ = app.engine.set(Arc::clone(&engine));
    World {
        dir,
        origin,
        base,
        fake,
        engine,
        app,
    }
}

impl World {
    pub fn open_pull(&self, head: &str, base: &str, draft: bool) {
        let mut p = pull_json(7, head, base);
        p["draft"] = json!(draft);
        lock(&self.fake).pulls.insert(7, p);
    }

    pub fn branch(&self, file: &str) -> String {
        git(&self.origin, &["checkout", "-q", "-B", "pr", &self.base]);
        let head = commit(&self.origin, file, "pub fn b() {}\n");
        git(&self.origin, &["checkout", "-q", "main"]);
        head
    }

    pub async fn deliver(&self, delivery: &str, event: &str, payload: &Value) -> Admitted {
        let body = payload.to_string();
        let event = parse(event, body.as_bytes(), "sych[bot]").expect("parse");
        let conn = &self.engine.conns["gh"];
        self.app
            .storage
            .admit(
                &conn.binding,
                delivery,
                &sha256(body.as_bytes()),
                &event,
                true,
                now(),
            )
            .await
            .expect("admit")
    }

    pub async fn pr_event(&self, delivery: &str, action: &str, head: &str) -> Admitted {
        let mut payload = json!({"action": action, "sender": {"login": "dev"},
            "installation": {"id": 9}, "repository": {"id": 42, "full_name": "o/r"},
            "pull_request": {"number": 7, "draft": false, "merged": false,
                "head": {"sha": head, "repo": {"full_name": "o/r"}},
                "base": {"repo": {"full_name": "o/r"}}}});
        if action == "edited" {
            payload["changes"] = json!({"base": {"ref": {"from": "old"}}});
        }

        self.deliver(delivery, "pull_request", &payload).await
    }

    pub async fn comment(&self, delivery: &str, login: &str, body: &str) -> Admitted {
        let payload = json!({"action": "created", "sender": {"login": login},
            "installation": {"id": 9}, "repository": {"id": 42, "full_name": "o/r"},
            "issue": {"number": 7, "pull_request": {}}, "comment": {"body": body}});
        self.deliver(delivery, "issue_comment", &payload).await
    }

    pub async fn ci(&self, delivery: &str, head: &str) -> Admitted {
        let payload = json!({"action": "completed", "sender": {"login": "github-actions"},
            "installation": {"id": 9}, "repository": {"id": 42, "full_name": "o/r"},
            "workflow_run": {"id": 555, "head_sha": head, "pull_requests": [{"number": 7}]}});
        self.deliver(delivery, "workflow_run", &payload).await
    }

    pub async fn schedule(&self, after: i64) {
        advance(after);
        self.engine.schedule_once().await.expect("schedule");
    }

    pub async fn events(&self) -> Vec<String> {
        let change = self.app.storage.open_intents().await.expect("intents");
        let id = change.first().map_or(1, |i| i.change_id);
        self.app
            .storage
            .change_events(id)
            .await
            .expect("events")
            .into_iter()
            .map(|(kind, detail, _)| match kind.as_str() {
                "SKIPPED" | "CANCELLED" => format!("{kind}:{detail}"),
                _ => kind,
            })
            .collect()
    }

    /// A completed `ci.yml` run for `head` whose `reports` artifact holds `junit.xml` and, when
    /// `tested` is set, a checkout manifest naming it.
    pub fn publish_ci(
        &self,
        run: u64,
        head: &str,
        conclusion: &str,
        tested: Option<&str>,
        report: &[u8],
    ) {
        let manifest = tested.map(|t| {
            json!({"schema_version": 1, "job": "ci/test", "tested_oid": t, "toolchain": "stable",
                "target": "x86_64-unknown-linux-gnu"})
            .to_string()
        });
        let mut entries: Vec<(&str, &[u8])> = vec![("junit.xml", report)];
        if let Some(m) = &manifest {
            entries.push(("sych-ci.json", m.as_bytes()));
        }

        let mut f = lock(&self.fake);
        f.runs.push(json!({"id": run, "run_attempt": 1, "name": "CI", "path": ".github/workflows/ci.yml",
            "event": "pull_request", "status": "completed", "conclusion": conclusion, "head_sha": head}));
        f.jobs.insert(
            run,
            vec![
                json!({"id": run * 10, "name": "test", "status": "completed",
            "conclusion": conclusion, "head_sha": head}),
            ],
        );
        f.artifacts.insert(
            run,
            vec![json!({"id": run * 100, "name": "reports", "expired": false,
            "size_in_bytes": 1})],
        );
        f.blobs.insert(run * 100, stored_zip(&entries));
    }

    /// Run ids of the change in generation order.
    pub async fn run_ids(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (_, _, run) in self.app.storage.change_events(1).await.expect("events") {
            if let Some(run) = run.filter(|r| !out.contains(r)) {
                out.push(run);
            }
        }
        out
    }

    /// The saved result of the `n`-th run.
    pub async fn result(&self, n: usize) -> ReviewResult {
        let id = &self.run_ids().await[n];
        let run = RunId(id.parse().expect("uuid"));
        self.app
            .storage
            .load_result("local", run)
            .await
            .expect("result")
    }

    pub async fn states(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (_, _, run) in self.app.storage.change_events(1).await.expect("events") {
            if let Some(run) = run.filter(|r| !out.iter().any(|(id, _): &(String, String)| id == r))
            {
                let (state, ..) = self.app.storage.run_row("local", &run).await.expect("run");
                out.push((run, state));
            }
        }
        out.into_iter().map(|(_, s)| s).collect()
    }
}

/// Commits a result with `defects` high findings on the head's new lines under the claimed run.
pub async fn commit_result(w: &World, defects: u32) -> String {
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    commit_claimed(w, &head, defects).await;
    head
}

/// Claims the queued job of `head` and commits a result with `defects` findings under it.
pub async fn commit_claimed(w: &World, head: &str, defects: u32) {
    let head = head.to_owned();
    let s = &w.app.storage;
    let c = s.claim("w", 60, now()).await.expect("claim").expect("job");
    let snap = LocalSnapshot::capture(&SnapshotRequest {
        repo: &w.origin,
        base: &w.base,
        head: &head,
        diff_base: None,
        max_blob_bytes: 1 << 20,
    })
    .expect("snapshot");
    let policy = sych_core::PolicySnapshotId(sha256(b"policy"));
    s.prepare_run(
        &c.fence,
        &c.run,
        &snap.snapshot.id,
        &policy,
        &json!({}),
        None,
        now(),
    )
    .await
    .expect("prepare");
    s.freeze(&c.fence, &c.run.id, &json!({}), now())
        .await
        .expect("freeze");
    s.put_artifact(
        &c.run,
        "diff_manifest",
        ArtifactKind::DiffManifest,
        &snap.manifest,
        now(),
    )
    .await
    .expect("manifest");
    let text = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/core/v1/review-result.json"),
    )
    .expect("fixture");
    let mut result: ReviewResult = decode(&text).expect("result");
    result.run_id = c.run.id;
    let path = "src/b.rs".parse().expect("path");
    let anchor = snap
        .code_ref(SourceSide::Head, &path, 1, 1)
        .expect("code ref");
    let template = result.findings[0].clone();
    result.findings = (0..defects)
        .map(|i| {
            let mut f = template.clone();
            f.fingerprint = sha256(format!("f{i}").as_bytes());
            f.title = format!("Defect {i}");
            f.anchors = vec![anchor.clone()];
            f
        })
        .collect();
    let digest = s
        .put_artifact(
            &c.run,
            "review_result",
            ArtifactKind::ReviewResult,
            &result,
            now(),
        )
        .await
        .expect("result");
    s.complete_run(&c.run, &result, &digest, now())
        .await
        .expect("commit");
}
