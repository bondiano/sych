//! Loopback HTTP stubs and stand-in helpers shared by the adapter fixtures; each suite uses
//! some of them.
#![allow(dead_code)]

use std::env;
use std::path::PathBuf;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

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

/// Serves one canned HTTP response under `base` and returns the raw request it received.
pub async fn serve_once(
    base: &'static str,
    status: &'static str,
    body: &'static str,
) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let endpoint = format!("http://{}{base}", listener.local_addr().expect("addr"));
    let handle = tokio::spawn(async move {
        // Other local tools may probe a fresh loopback port; only the client's POST counts.
        loop {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = socket.read(&mut buf).await {
                raw.extend_from_slice(&buf[..n]);
                if n == 0 || request_complete(&raw) {
                    break;
                }
            }
            if !raw.starts_with(b"POST ") {
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await;
                continue;
            }

            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.expect("write");
            return String::from_utf8_lossy(&raw).into_owned();
        }
    });
    (endpoint, handle)
}

/// Answers each POST with the next `(status, body)`; returns the raw requests in order.
pub async fn serve_seq(
    base: &'static str,
    replies: Vec<(&'static str, String)>,
) -> (String, String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let root = format!("http://{}", listener.local_addr().expect("addr"));
    let endpoint = format!("{root}{base}");
    let handle = tokio::spawn(async move {
        let mut seen = Vec::new();
        let mut replies = replies.into_iter();
        let mut next = replies.next();
        while let Some((status, body)) = &next {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = socket.read(&mut buf).await {
                raw.extend_from_slice(&buf[..n]);
                if n == 0 || request_complete(&raw) {
                    break;
                }
            }
            if !raw.starts_with(b"POST ") {
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await;
                continue;
            }

            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.expect("write");
            seen.push(String::from_utf8_lossy(&raw).into_owned());
            next = replies.next();
        }
        seen
    });
    (root, endpoint, handle)
}

/// The first `sh` on the test's `PATH`, for stand-in shebangs: on NixOS `/bin/sh` is a wrapper
/// that loads the system environment back into an emptied one, which would hide what the
/// adapter actually passes.
pub fn host_sh() -> PathBuf {
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("sh"))
        .find(|sh| sh.is_file())
        .expect("a POSIX sh on PATH")
}
