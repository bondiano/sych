//! Server actions from the CLI: a configured identity is required, cancelling
//! before commit exits 130 and keeps CANCELLED, a committed run is never rewritten, and a
//! prune keeps history as long as the config's retention says, and the server prunes by itself.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sych_core::Timestamp;
use sych_storage::Database;

mod fake_github;
mod world;

use world::database::inspect;
use world::{World, commit_result, now, world};

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

#[tokio::test(flavor = "current_thread")]
async fn a_prune_follows_the_configured_event_log_retention() {
    let w = world(None).await;
    commit_result(&w, 0).await;
    let dir = w.dir.path();
    let default = config(dir);
    let short = dir.join("short.yaml");
    let text = fs::read_to_string(&default).expect("config");
    fs::write(
        &short,
        text.replacen(
            "storage:\n",
            "storage:\n  retention:\n    change_events_days: 1\n",
            1,
        ),
    )
    .expect("short config");
    // The CLI prunes at the wall clock, so the event is two days before it.
    let at = Timestamp::now().as_second() - 2 * 86_400;
    sqlx::query(
        "INSERT INTO change_events (change_id, run_id, kind, detail, at)
         SELECT id, NULL, 'TEST', 'two days old', $1 FROM changes",
    )
    .bind(at)
    .execute(&inspect(w.dir.config()).await)
    .await
    .expect("event");

    let plan = |cfg: &Path| {
        let cfg = cfg.to_str().expect("utf8");
        sych(&w, &["cache", "prune", "--dry-run", "--config", cfg])
    };
    let (code, out) = plan(&short);
    assert_eq!(code, 0);
    assert!(
        out.contains(
            "Records change_events 1 (logged before storage.retention.change_events_days)"
        ),
        "{out}"
    );
    let (code, out) = plan(&default);
    assert_eq!(code, 0);
    assert!(!out.contains("change_events"), "90 days keep it: {out}");
}

#[tokio::test(flavor = "current_thread")]
async fn the_server_prunes_history_past_the_default_retention() {
    let w = world(None).await;
    commit_result(&w, 0).await;
    let db = inspect(w.dir.config()).await;
    for (days, detail) in [(91, "past retention"), (89, "kept")] {
        sqlx::query(
            "INSERT INTO change_events (change_id, run_id, kind, detail, at)
             SELECT id, NULL, 'TEST', $1, $2 FROM changes",
        )
        .bind(detail)
        .bind(now().as_second() - days * 86_400)
        .execute(&db)
        .await
        .expect("event");
    }

    w.engine.prune_once().await.expect("prune");

    let left: Vec<String> =
        sqlx::query_scalar("SELECT detail FROM change_events WHERE kind = 'TEST'")
            .fetch_all(&db)
            .await
            .expect("events");
    assert_eq!(left, ["kept"]);
}
