//! `sych doctor --probe-model` against a loopback `OpenRouter` stub: the gateway,
//! ledger and adapter together, with no external API.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::{self, JoinHandle};

use serde_json::Value;
use tempfile::TempDir;

/// One streamed reply: a single content chunk with usage, then `[DONE]`.
fn reply(content: &str) -> String {
    let chunk = serde_json::json!({
        "id": "gen-probe", "model": "deepseek/deepseek-v4.1-flash-20261001", "provider": "DeepSeek",
        "choices": [{ "finish_reason": "stop", "delta": { "content": content } }],
        "usage": { "prompt_tokens": 90, "completion_tokens": 8, "cost": 0.00014 }
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

/// Headers and as many body bytes as `content-length` announces, counted in bytes.
fn request_complete(raw: &[u8]) -> bool {
    let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
    let length = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    raw.len() - end - 4 >= length
}

/// Answers each client request with the next body; returns the raw requests.
fn stub(bodies: Vec<String>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("http://{}/api/v1", listener.local_addr().expect("addr"));
    let handle = thread::spawn(move || {
        let mut seen = Vec::new();
        let mut bodies = bodies.into_iter();
        let mut next = bodies.next();
        while let Some(body) = &next {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut raw = Vec::new();
            let mut buf = [0u8; 8192];
            while let Ok(n) = socket.read(&mut buf) {
                raw.extend_from_slice(&buf[..n]);
                if n == 0 || request_complete(&raw) {
                    break;
                }
            }
            // Other local tools may probe a fresh loopback port; only the client's POST counts.
            if !raw.starts_with(b"POST ") {
                let _ = socket.write_all(
                    b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                );
                continue;
            }

            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).expect("write");
            seen.push(String::from_utf8_lossy(&raw).into_owned());
            next = bodies.next();
        }
        seen
    });
    (endpoint, handle)
}

fn config(dir: &Path, endpoint: &str) -> PathBuf {
    let path = dir.join("sych.yaml");
    fs::write(
        &path,
        format!(
            "schema_version: 1
mode: personal
models:
  providers:
    openrouter:
      adapter: openrouter
      endpoint: \"{endpoint}\"
      api_key_env: SYCH_TEST_OPENROUTER_KEY
      allowed_modes: [personal_local]
  profiles:
    cheap_api:
      provider: openrouter
      model: deepseek/deepseek-v4.1-flash
      billing_kind: api_usage
      enforcement: api_hard
      upstreams: [deepseek]
  price_catalog_file: prices.json
"
        ),
    )
    .expect("config");
    fs::write(
        dir.join("prices.json"),
        r#"{"schema_version":1,"revision":"test-2026-10","prices":{"deepseek/deepseek-v4.1-flash":{"input_per_mtok":300000,"output_per_mtok":1200000}}}"#,
    )
    .expect("prices");
    path
}

fn sych(config: &Path, args: &[&str], key: bool) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sych"));
    cmd.args(args)
        .arg("--config")
        .arg(config)
        .env_remove("SYCH_TEST_OPENROUTER_KEY");
    if key {
        cmd.env("SYCH_TEST_OPENROUTER_KEY", "sk-test-not-real");
    }

    cmd.output().expect("sych")
}

fn probe(dir: &TempDir, endpoint: &str, key: bool) -> (Output, Value) {
    let cfg = config(dir.path(), endpoint);
    assert!(sych(&cfg, &["db", "migrate"], key).status.success());
    let repo = dir.path().to_str().expect("utf-8");
    let out = sych(
        &cfg,
        &["doctor", "--repo", repo, "--probe-model", "cheap_api"],
        key,
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json = stdout
        .find("{\n")
        .and_then(|i| serde_json::from_str(&stdout[i..]).ok())
        .unwrap_or(Value::Null);
    (out, json)
}

fn init_git(dir: &Path) {
    let ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .expect("git");
    assert!(ok.success());
}

#[test]
fn probe_records_route_usage_and_charge() {
    let dir = TempDir::new().expect("tempdir");
    init_git(dir.path());
    let (endpoint, server) = stub(vec![reply(r#"{"echo":"owl-probe"}"#)]);
    let (out, report) = probe(&dir, &endpoint, true);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(report["valid"], true);
    assert_eq!(report["echo_matches"], true);
    assert_eq!(report["route"]["upstreams"][0], "deepseek");
    assert_eq!(report["route"]["price_revision"], "test-2026-10");
    let attempt = &report["attempts"][0];
    assert_eq!(attempt["charge_state"], "SETTLED");
    assert_eq!(attempt["charge"], 140);
    assert_eq!(attempt["upstream_provider"], "DeepSeek");
    assert_eq!(attempt["generation_id"], "gen-probe");
    assert!(attempt["reserved"].as_u64().expect("reserved") >= 140);
    let requests = server.join().expect("stub");
    assert!(requests[0].contains("\"allow_fallbacks\":false"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("sk-test-not-real"));
}

#[test]
fn invalid_output_gets_one_repair_then_fails() {
    let dir = TempDir::new().expect("tempdir");
    init_git(dir.path());
    let (endpoint, server) = stub(vec![reply("owl-probe"), reply(r#"{"word":"owl-probe"}"#)]);
    let (out, report) = probe(&dir, &endpoint, true);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(report["failure"]["failure"], "SCHEMA");
    let kinds: Vec<_> = report["attempts"]
        .as_array()
        .expect("attempts")
        .iter()
        .map(|a| a["kind"].clone())
        .collect();
    assert_eq!(kinds, ["INITIAL", "REPAIR"]);
    assert_eq!(server.join().expect("stub").len(), 2);
}

#[test]
fn missing_key_is_a_credentials_error_without_a_request() {
    let dir = TempDir::new().expect("tempdir");
    init_git(dir.path());
    let (out, _) = probe(&dir, "http://127.0.0.1:9/api/v1", false);
    assert_eq!(out.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&out.stderr).contains("SYCH_TEST_OPENROUTER_KEY"));
}
