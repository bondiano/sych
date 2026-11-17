//! The `security` profile on the real CLI: a committed token, a credentials file, a bidi
//! override and an install script in added lines are findings without a model, and the token is
//! never quoted.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

// Split, so this file holds no token for a scanner, Sych included.
const TOKEN: &str = concat!("ghp", "_0123456789abcdefABCDEF0123");

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?} failed");
}

fn sych(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sych"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run sych")
}

/// `target` has one file; HEAD adds a token, a bidi override, an env file and an install script.
fn repo(policy: Option<&str>) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    fs::create_dir_all(d.join("src")).expect("mkdir");
    fs::write(d.join("src/lib.rs"), "pub fn a() {}\n").expect("write");
    if let Some(policy) = policy {
        fs::create_dir_all(d.join(".sych")).expect("mkdir");
        fs::write(d.join(".sych/config.yaml"), policy).expect("write");
    }

    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "base"]);
    git(d, &["branch", "target"]);
    let lib = format!(
        "pub fn a() {{}}\n\npub const T: &str = \"{TOKEN}\";\n\
         pub fn admin(u: &str) -> bool {{ u == \"user\u{202e}\u{2066}// admin\u{2069}\u{2066}\" }}\n"
    );
    fs::write(d.join("src/lib.rs"), lib).expect("write");
    fs::write(d.join(".env"), "DB_PASSWORD=hunter2hunter2\n").expect("write");
    let manifest = "{\n  \"scripts\": {\n    \"postinstall\": \"node setup.js\"\n  }\n}\n";
    fs::write(d.join("package.json"), manifest).expect("write");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "head"]);
    dir
}

fn review(repo: &TempDir) -> (Value, String) {
    let data = TempDir::new().expect("tempdir");
    assert_eq!(sych(data.path(), &["db", "migrate"]).status.code(), Some(0));
    let path = repo.path().to_str().expect("utf-8 path");
    let out = sych(
        data.path(),
        &[
            "review", "--repo", path, "--base", "target", "--models", "fake",
        ],
    );
    let text = String::from_utf8(out.stdout).expect("utf-8");
    let json = serde_json::from_str(&text).unwrap_or_else(|_| panic!("json: {text}"));
    (json, text)
}

fn found(review: &Value) -> Vec<(String, String, usize)> {
    let mut out: Vec<_> = review["review_result"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter_map(|f| {
            let rule = f["rule_id"].as_str()?.to_owned();
            rule.starts_with("SEC-").then(|| {
                let anchors = f["anchors"].as_array().expect("anchors");
                let path = anchors
                    .first()
                    .map_or_else(|| f["scope"].clone(), |a| a["path"].clone());
                (rule, path.as_str().expect("path").to_owned(), anchors.len())
            })
        })
        .collect();
    out.sort();
    out
}

#[test]
fn added_secrets_and_hidden_text_are_findings_and_the_token_is_never_quoted() {
    let (result, text) = review(&repo(None));
    assert_eq!(
        found(&result),
        [
            ("SEC-HIDDEN-TEXT".into(), "src/lib.rs".into(), 1),
            ("SEC-SECRET".into(), ".env".into(), 0),
            ("SEC-SECRET".into(), "src/lib.rs".into(), 1),
            ("SEC-SUPPLY-CHAIN".into(), "package.json".into(), 1),
        ]
    );
    assert!(!text.contains(TOKEN), "the token is quoted: {text}");
    assert_eq!(
        result["review_result"]["recommendation"],
        "ATTENTION_REQUIRED"
    );
}

#[test]
fn without_the_security_profile_nothing_is_scanned() {
    let policy = "schema_version: 1\nreview:\n  profiles: [correctness]\n";
    let (result, _) = review(&repo(Some(policy)));
    assert!(found(&result).is_empty(), "{result}");
}
