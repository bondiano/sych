//! The server loop: a publication that becomes due while an analysis runs is published then,
//! not after the analysis ends.

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use sych_core::RunId;
use tokio::sync::watch;
use tokio::task::{self, LocalSet};
use tokio::time;

mod fake_github;
mod world;

use world::{now, world_with};

#[tokio::test(flavor = "current_thread")]
async fn a_publication_does_not_wait_for_an_analysis_in_progress() {
    // A model endpoint that accepts every connection and never answers: the analysis waits.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("http://{}/api/v1", listener.local_addr().expect("addr"));
    let accepted = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&accepted);
    thread::spawn(move || {
        let mut held = Vec::new();
        for socket in listener.incoming().flatten() {
            held.push(socket);
            seen.store(true, Ordering::Relaxed);
        }
    });
    let w = world_with(None, Some(&endpoint)).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    // With checks on, as a checks-enabled connection sets them, so a failure check can be due.
    let storage = &w.app.storage;
    let mut inputs = storage
        .latest(1)
        .await
        .expect("latest")
        .expect("run")
        .inputs;
    (inputs.checks, inputs.publish) = (true, true);
    storage
        .generate(1, &inputs, now().as_second() + 600, 3, now())
        .await
        .expect("generate");
    let run = storage
        .latest(1)
        .await
        .expect("latest")
        .expect("run")
        .run_id;

    let (stop, stopped) = watch::channel(false);
    LocalSet::new()
        .run_until(async {
            let running = task::spawn_local(Arc::clone(&w.engine).run(stopped));
            while !accepted.load(Ordering::Relaxed) {
                time::sleep(Duration::from_millis(10)).await;
            }

            // Due now, while the worker waits on the model: the run's failure check.
            let handle = storage
                .run_handle("local", RunId(run.parse().expect("run id")))
                .await
                .expect("run");
            storage
                .enqueue_failure_check(&handle, now().as_second())
                .await
                .expect("enqueue")
                .expect("an operation");
            let started = Instant::now();
            let published = loop {
                let ops = storage.operations(1).await.expect("ops");
                if ops.iter().all(|o| o.state != "PENDING") {
                    break true;
                }

                if started.elapsed() > Duration::from_secs(10) {
                    break false;
                }

                time::sleep(Duration::from_millis(50)).await;
            };
            let _ = stop.send(true);
            running.abort();
            assert!(published, "the publication waited for the analysis");
        })
        .await;
}
