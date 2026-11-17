//! `sych serve` stops on SIGTERM, which a container runtime sends: as PID 1 the server would
//! ignore an unhandled one and be killed at the end of its grace period, mid-work.

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn sych(dir: &TempDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sych"));
    let url = format!("sqlite://{}?mode=rwc", dir.path().join("s.db").display());
    cmd.current_dir(dir.path()).env("SYCH_DATABASE_URL", url);
    cmd
}

#[test]
fn sigterm_stops_the_server_cleanly() {
    let dir = TempDir::new().expect("tempdir");
    // A free port, released for the server; another process taking it first only fails the start.
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("port")
        .port();
    let config = format!(
        "schema_version: 1\nmode: personal\nstorage:\n  database_url_env: SYCH_DATABASE_URL\n  \
         artifact_store:\n    kind: local\n    path: \"./artifacts\"\nhttp:\n  bind: \"127.0.0.1:{port}\"\n"
    );
    fs::write(dir.path().join("sych.yaml"), config).expect("config");
    let migrated = sych(&dir)
        .args(["db", "migrate", "--config", "sych.yaml"])
        .output()
        .expect("migrate");
    assert!(migrated.status.success());

    let mut server = sych(&dir)
        .args(["serve", "--config", "sych.yaml"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("serve");
    let started = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the server did not listen"
        );
        thread::sleep(Duration::from_millis(50));
    }

    let sent = Command::new("kill")
        .args(["-TERM", &server.id().to_string()])
        .status()
        .expect("kill");
    assert!(sent.success());
    let stopped = Instant::now();
    let status = loop {
        if let Some(status) = server.try_wait().expect("wait") {
            break status;
        }

        if stopped.elapsed() > Duration::from_secs(20) {
            let _ = server.kill();
            panic!("the server ignored SIGTERM");
        }

        thread::sleep(Duration::from_millis(50));
    };
    assert!(
        status.success(),
        "{status}: killed by the signal, not stopped by it"
    );
}
