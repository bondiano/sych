//! Integration tests for `--out-dir`, `--open`, and the automatic `.sych-output/` terminal bundle.

use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::HeaderName;
use axum::http::header::CONTENT_TYPE;
use axum::routing::post;
use clap::Parser;
use serde_json::{Value, json};
use sych_app::args::{Cli, Commands};
use sych_app::commands;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::runtime::Builder;
use tokio::sync::oneshot;

const BASE: &str = "package orders\n\nfunc Save(o Order) error {\n\treturn tx(o)\n}\n";
const HEAD: &str =
    "package orders\n\nfunc Save(o Order) error {\n\tgo publish(o)\n\treturn nil\n}\n";

fn git(dir: &Path, args: &[&str]) -> Output {
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    fs::create_dir_all(d.join("backend/orders")).expect("mkdir");
    fs::write(d.join("backend/orders/store.go"), BASE).expect("write base");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "base"]);
    git(d, &["checkout", "-qb", "feature"]);
    fs::write(d.join("backend/orders/store.go"), HEAD).expect("write head");
    git(d, &["commit", "-q", "-am", "head"]);
    dir
}

fn sych_with_env(cwd: &Path, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sych"));
    cmd.current_dir(cwd)
        .env("XDG_CONFIG_HOME", cwd.join("user-config"))
        .env("XDG_DATA_HOME", cwd.join("user-data"))
        .env("APPDATA", cwd.join("user-config"))
        .env("LOCALAPPDATA", cwd.join("user-data"))
        .env("SYCH_LOG", "error")
        .args(args);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("sych binary")
}

fn head_span_from_prompt(body: &Value) -> (String, u32, u32) {
    let messages = body["messages"].as_array().expect("messages");
    let user = messages
        .iter()
        .find(|m| m["role"] == "user")
        .and_then(|m| m["content"].as_str())
        .expect("user prompt");
    user.lines()
        .filter_map(|line| line.strip_prefix("<<<DATA "))
        .find_map(|line| {
            let mut words = line.split(' ').skip(1);
            let (id, side, place) = (words.next()?, words.next()?, words.next()?);
            let range = place.rsplit_once(':')?.1;
            let (start_s, end_s) = range.split_once('-').unwrap_or((range, range));
            let start: u32 = start_s.parse().ok()?;
            let end: u32 = end_s.parse().ok()?;
            (id.starts_with("ref-") && side == "HEAD").then(|| (id.to_owned(), start, end))
        })
        .expect("HEAD data span")
}

fn mock_verdict(body: &Value) -> Value {
    let (ref_id, start, end) = head_span_from_prompt(body);
    let anchor = json!({ "ref_id": ref_id, "start_line": 4.max(start), "end_line": 5.min(end) });
    json!({
        "verdicts": [{
            "candidate_id": "cand-1",
            "verdict": "CONFIRMED",
            "category": "DEFECT",
            "severity": "MEDIUM",
            "support": "SEMANTIC_ARGUMENT",
            "title": "Detached goroutine drops transactional event",
            "explanation": "Save launches publish asynchronously instead of returning tx(o).",
            "anchors": [anchor],
            "evidence_ids": [],
            "alternatives_checked": ["checked Save body"],
            "suggested_action": "Call tx(o) synchronously and return its error.",
            "validation_expectation": "Save returns tx(o) directly.",
            "limitations": [],
            "suggested_change": {
                "anchor": anchor,
                "replacement": "\treturn tx(o)"
            },
            "unverified_external_assumptions": []
        }],
        "context_requests": []
    })
}

fn mock_assessment(body: &Value) -> Value {
    let (ref_id, start, _) = head_span_from_prompt(body);
    let anchor = json!([{ "ref_id": ref_id, "start_line": start, "end_line": start }]);
    json!({
        "obligation_id": "ob-1",
        "applicability": "APPLIES",
        "proposed_status": "SUPPORTED",
        "criterion_results": [
            { "criterion_id": "c-1", "status": "MET", "anchors": anchor, "evidence_ids": [] },
            { "criterion_id": "c-2", "status": "MET", "anchors": anchor, "evidence_ids": [] }
        ],
        "evidence_ids": [],
        "rationale": "Checked.",
        "limitations": [],
        "missing_inputs": [],
        "not_applicable_basis": null
    })
}

async fn mock_completions(
    State(discovery_calls): State<Arc<AtomicUsize>>,
    Json(body): Json<Value>,
) -> ([(HeaderName, &'static str); 1], String) {
    let schema = body["response_format"]["json_schema"]["name"]
        .as_str()
        .unwrap_or_default();
    let content = match schema {
        "candidate_verdicts" => mock_verdict(&body),
        "obligation_assessment" => mock_assessment(&body),
        _ => {
            let candidates = if discovery_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let (ref_id, start, end) = head_span_from_prompt(&body);
                let anchor =
                    json!({ "ref_id": ref_id, "start_line": 4.max(start), "end_line": 5.min(end) });
                json!([{
                    "category": "DEFECT",
                    "claim": "Save drops transactional publish",
                    "conditions": ["whenever Save runs"],
                    "causal_chain": ["publish runs in a detached goroutine"],
                    "consequence": "order event can be lost",
                    "anchors": [anchor],
                    "evidence_ids": [],
                    "obligation_ids": [],
                    "proposed_severity": "MEDIUM",
                    "disproof_question": "Does Save still call tx(o)?"
                }])
            } else {
                json!([])
            };
            let mut reply = json!({
                "candidates": candidates,
                "context_requests": [],
                "unresolved_questions": []
            });
            if schema == "review" {
                reply["assessments"] = json!([]);
            }

            reply
        }
    };

    let chunk = json!({
        "id": "gen-test-1",
        "model": "fixture/test-model",
        "provider": "Fixture",
        "choices": [{
            "finish_reason": "stop",
            "delta": { "content": content.to_string() }
        }],
        "usage": { "prompt_tokens": 120, "completion_tokens": 60, "cost": 0.0001 }
    });
    (
        [(CONTENT_TYPE, "text/event-stream")],
        format!("data: {chunk}\n\ndata: [DONE]\n\n"),
    )
}

fn spawn_mock_openrouter() -> (u16, oneshot::Sender<()>, thread::JoinHandle<()>) {
    let (addr_tx, addr_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let handle = thread::spawn(move || {
        let rt = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt");
        rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let port = listener.local_addr().expect("addr").port();
            addr_tx.send(port).expect("send port");
            let app = Router::new()
                .route("/chat/completions", post(mock_completions))
                .with_state(Arc::new(AtomicUsize::new(0)));
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stop_rx.await;
                })
                .await
                .expect("serve");
        });
    });
    let port = addr_rx.recv().expect("port");
    (port, stop_tx, handle)
}

fn write_mock_config(cwd: &Path, port: u16) -> PathBuf {
    fs::write(
        cwd.join("prices.json"),
        r#"{"schema_version":1,"revision":"test","prices":{"fixture/test-model":{"input_per_mtok":300000,"output_per_mtok":1200000}}}"#,
    )
    .expect("write prices.json");
    let cfg_path = cwd.join("sych-test.yaml");
    let cfg = format!(
        r"schema_version: 1
mode: personal
models:
  price_catalog_file: prices.json
  providers:
    local_or:
      adapter: openrouter
      endpoint: http://127.0.0.1:{port}
      api_key_env: TEST_OR_KEY
      allowed_modes: [personal_local]
  profiles:
    local:
      provider: local_or
      model: fixture/test-model
      upstreams: [fixture]
      billing_kind: api_usage
      enforcement: api_hard
"
    );
    fs::write(&cfg_path, cfg).expect("write config");
    cfg_path
}

#[cfg(unix)]
fn install_fake_opener(dir: &Path) -> (PathBuf, PathBuf, String) {
    let bin = dir.join("fake-bin");
    fs::create_dir_all(&bin).expect("fake-bin");
    let log = dir.join("opened.log");
    let script = "#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$FAKE_OPEN_LOG\"\n";
    for tool in ["open", "xdg-open"] {
        let p = bin.join(tool);
        fs::write(&p, script).expect("write opener");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let orig = env::var("PATH").unwrap_or_default();
    let combined = format!("{}:{orig}", bin.display());
    (bin, log, combined)
}

#[test]
fn review_and_show_out_dir_write_byte_identical_bundle_and_apply_patch() {
    let repo = repo();
    let cwd = repo.path();
    let (port, stop_tx, server_handle) = spawn_mock_openrouter();
    let cfg_path = write_mock_config(cwd, port);

    let out1 = cwd.join("bundle-review");
    let review = sych_with_env(
        cwd,
        &[
            "review",
            "--config",
            cfg_path.to_str().expect("utf-8"),
            "--models",
            "local",
            "--semantic",
            "off",
            "--out-dir",
            out1.to_str().expect("utf-8"),
        ],
        &[("TEST_OR_KEY", "test-secret")],
    );
    let _ = stop_tx.send(());
    server_handle.join().expect("join server");

    assert!(
        review.status.success(),
        "review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );
    assert!(
        review.stdout.is_empty(),
        "non-TTY --out-dir leaves stdout empty"
    );

    let entries: Vec<_> = fs::read_dir(&out1)
        .expect("read out1")
        .map(|e| e.expect("entry").file_name().into_string().expect("utf-8"))
        .collect();
    assert_eq!(entries.len(), 1, "one run directory in out1: {entries:?}");
    let run_id = &entries[0];
    let run_dir1 = out1.join(run_id);

    for name in [
        "report.html",
        "fix-brief.md",
        "review.json",
        "review.sarif",
        "fixes.patch",
    ] {
        assert!(run_dir1.join(name).is_file(), "missing {name}");
    }

    let out2 = cwd.join("bundle-show");
    let show = sych_with_env(
        cwd,
        &[
            "show",
            "--run",
            run_id,
            "--config",
            cfg_path.to_str().expect("utf-8"),
            "--out-dir",
            out2.to_str().expect("utf-8"),
        ],
        &[("TEST_OR_KEY", "test-secret")],
    );
    assert!(
        show.status.success(),
        "show failed: {}",
        String::from_utf8_lossy(&show.stderr)
    );
    let run_dir2 = out2.join(run_id);

    for name in [
        "review.json",
        "fix-brief.md",
        "fixes.patch",
        "report.html",
        "review.sarif",
    ] {
        let a = fs::read(run_dir1.join(name)).expect("read from review bundle");
        let b = fs::read(run_dir2.join(name)).expect("read from show bundle");
        assert_eq!(
            a, b,
            "{name} must be byte-identical between review and show"
        );
    }

    let brief = fs::read_to_string(run_dir1.join("fix-brief.md")).expect("brief");
    assert!(
        brief.contains("handoff@2")
            && brief.contains("Detached goroutine drops transactional event"),
        "{brief}"
    );
    git(
        cwd,
        &[
            "apply",
            "--check",
            run_dir1.join("fixes.patch").to_str().expect("utf-8"),
        ],
    );
}

#[test]
fn out_dir_and_open_reject_conflicting_flags_with_exit_two() {
    let repo = repo();
    let cwd = repo.path();
    for bad in [
        &["review", "--out-dir", "out", "--format", "json"][..],
        &["review", "--out-dir", "out", "--output", "r.json"][..],
        &["review", "--open", "--format", "html"][..],
        &["review", "--open", "--output", "r.html"][..],
        &[
            "show",
            "--run",
            "0192f000-0000-7000-8000-000000000001",
            "--out-dir",
            "out",
            "--format",
            "json",
        ][..],
        &[
            "show",
            "--run",
            "0192f000-0000-7000-8000-000000000001",
            "--out-dir",
            "out",
            "--output",
            "r.json",
        ][..],
        &[
            "show",
            "--run",
            "0192f000-0000-7000-8000-000000000001",
            "--open",
        ][..],
    ] {
        let out = sych_with_env(cwd, bad, &[]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "expected exit 2 for {bad:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn tty_bare_review_writes_sych_output_with_gitignore_and_leaves_git_clean() {
    let repo = repo();
    let cwd = repo.path();
    let cli = Cli::parse_from([
        "sych",
        "review",
        "--repo",
        cwd.to_str().expect("utf-8"),
        "--models",
        "fake",
        "--semantic",
        "off",
    ]);
    let Commands::Review(args) = cli.command else {
        panic!("expected review command");
    };

    let code = commands::review::run_with_tty(&args, true).expect("tty review");
    assert_eq!(code, ExitCode::SUCCESS);

    let default_dir = cwd.join(".sych-output");
    let gitignore = default_dir.join(".gitignore");
    assert_eq!(fs::read_to_string(&gitignore).expect("gitignore"), "*\n");

    let run_dirs: Vec<_> = fs::read_dir(&default_dir)
        .expect("read .sych-output")
        .filter_map(|e| {
            let e = e.expect("entry");
            e.file_type().expect("ft").is_dir().then(|| e.path())
        })
        .collect();
    assert_eq!(run_dirs.len(), 1);
    let run_dir = &run_dirs[0];
    for name in ["report.html", "fix-brief.md", "review.json", "review.sarif"] {
        assert!(run_dir.join(name).is_file(), "missing {name}");
    }
    assert!(
        !run_dir.join("fixes.patch").exists(),
        "fixes.patch is omitted when there are no suggested changes"
    );

    fs::write(&gitignore, "*\n# keep custom comment\n").expect("custom gitignore");
    let status = git(cwd, &["status", "--porcelain"]);
    assert!(
        status.stdout.is_empty(),
        "git status must ignore .sych-output: {}",
        String::from_utf8_lossy(&status.stdout)
    );

    let worktree_cli = Cli::parse_from([
        "sych",
        "review",
        "--repo",
        cwd.to_str().expect("utf-8"),
        "--worktree",
        "--models",
        "fake",
        "--semantic",
        "off",
    ]);
    let Commands::Review(wt_args) = worktree_cli.command else {
        panic!("expected review command");
    };
    let wt_code = commands::review::run_with_tty(&wt_args, true).expect("tty worktree review");
    assert_eq!(wt_code, ExitCode::SUCCESS);
    assert_eq!(
        fs::read_to_string(&gitignore).expect("gitignore preserved"),
        "*\n# keep custom comment\n"
    );
}

#[cfg(unix)]
#[test]
fn open_flag_invokes_platform_opener_only_when_passed() {
    let repo = repo();
    let cwd = repo.path();
    let (_bin, log, path_env) = install_fake_opener(cwd);
    let out_dir = cwd.join("out");

    let no_open = sych_with_env(
        cwd,
        &[
            "review",
            "--models",
            "fake",
            "--semantic",
            "off",
            "--out-dir",
            out_dir.to_str().expect("utf-8"),
        ],
        &[
            ("PATH", &path_env),
            ("FAKE_OPEN_LOG", log.to_str().expect("utf-8")),
        ],
    );
    assert!(no_open.status.success());
    assert!(!log.exists(), "without --open, no browser opener may run");

    let with_open = sych_with_env(
        cwd,
        &[
            "review",
            "--models",
            "fake",
            "--semantic",
            "off",
            "--out-dir",
            out_dir.to_str().expect("utf-8"),
            "--open",
        ],
        &[
            ("PATH", &path_env),
            ("FAKE_OPEN_LOG", log.to_str().expect("utf-8")),
        ],
    );
    assert!(
        with_open.status.success(),
        "{}",
        String::from_utf8_lossy(&with_open.stderr)
    );
    let opened = fs::read_to_string(&log).expect("opener log");
    let lines: Vec<_> = opened.lines().collect();
    assert_eq!(lines.len(), 1, "{opened}");
    assert!(lines[0].ends_with("/report.html"), "{opened}");
    assert!(Path::new(lines[0]).is_absolute(), "{opened}");
}
