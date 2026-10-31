//! Server actions from the CLI: a configured identity is required, cancelling
//! before commit exits 130 and keeps CANCELLED, a committed run is never rewritten.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sych_storage::Database;

mod fake_github;
mod world;

use world::{World, commit_result, world};

fn config(dir: &Path) -> PathBuf {
    let path = dir.join("cli.yaml");
    fs::write(
        &path,
        "schema_version: 1\nmode: personal\nstorage:\n  database_url_env: CLI_DB\n  artifact_store:\n    \
         kind: local\n    path: artifacts\nhttp:\n  api_clients:\n    - name: writer\n      token_env: WRITER_TOKEN\n      \
         role: writer\n      repositories: [\"*\"]\n    - name: reader\n      token_env: READER_TOKEN\n      role: reader\n      \
         repositories: [\"*\"]\n",
    )
    .expect("config");
    path
}

fn sych(w: &World, args: &[&str]) -> (i32, String) {
    let database = match &w.dir.config().database {
        Database::Sqlite(path) => path.as_os_str(),
        Database::Postgres(url) => OsStr::new(url),
    };
    let out = Command::new(env!("CARGO_BIN_EXE_sych"))
        .args(args)
        .env("CLI_DB", database)
        .output()
        .expect("sych");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_exits_130_before_commit_and_keeps_a_committed_result() {
    let w = world(None).await;
    commit_result(&w, 0).await;
    let dir = w.dir.path();
    let cfg = config(dir);
    let cfg = cfg.to_str().expect("utf8");
    let committed = w.run_ids().await[0].clone();
    let (code, out) = sych(
        &w,
        &[
            "cancel", "--config", cfg, "--run", &committed, "--as", "writer",
        ],
    );
    assert_eq!(
        (code, out.trim()),
        (0, format!("run {committed} is COMPLETED").as_str())
    );

    w.comment("d-9", "dev", "/sych rerun").await;
    fake_github::lock(&w.fake)
        .permissions
        .insert("dev".into(), "write".into());
    w.schedule(1).await;
    let active = w.run_ids().await.last().expect("run").clone();
    let (code, _) = sych(
        &w,
        &[
            "cancel", "--config", cfg, "--run", &active, "--as", "reader",
        ],
    );
    assert_eq!(code, 4, "a reader may not cancel");
    let (code, _) = sych(
        &w,
        &[
            "cancel", "--config", cfg, "--run", &active, "--as", "nobody",
        ],
    );
    assert_eq!(code, 4);
    let (code, out) = sych(
        &w,
        &[
            "cancel", "--config", cfg, "--run", &active, "--as", "writer",
        ],
    );
    assert_eq!(
        (code, out.trim()),
        (130, format!("run {active} is CANCELLED").as_str())
    );
    let (code, _) = sych(&w, &["review", "--repo", "."]);
    assert_eq!(code, 2, "--base or --change is required");
}
