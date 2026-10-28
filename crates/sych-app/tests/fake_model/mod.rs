//! A loopback `OpenRouter` stand-in for server runs on a real model route: answers every
//! structured request by its schema name with nothing found, and records the names, so a test
//! sees which model calls a run made.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};

pub struct FakeModel {
    /// `…/api/v1`, the provider endpoint.
    pub endpoint: String,
    pub calls: Arc<Mutex<Vec<String>>>,
}

impl FakeModel {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls").clone()
    }
}

fn answer(schema: &str) -> Value {
    let empty = json!({"candidates": [], "context_requests": [], "unresolved_questions": []});
    match schema {
        "review" => {
            let mut both = empty;
            both["assessments"] = json!([]);
            both
        }
        "candidate_verdicts" => json!({"verdicts": [], "context_requests": []}),
        "followup" => json!({"answer": "The caller gets the error.", "anchors": [],
            "hypotheses": [], "context_requests": []}),
        _ => empty,
    }
}

/// One streamed reply: a content chunk with usage, then `[DONE]`.
fn reply(content: &str) -> String {
    let chunk = json!({
        "id": "gen-fake", "model": "deepseek/deepseek-v4.1-flash-20261001", "provider": "DeepSeek",
        "choices": [{ "finish_reason": "stop", "delta": { "content": content } }],
        "usage": { "prompt_tokens": 90, "completion_tokens": 8, "cost": 0.00001 }
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

/// The request body once headers and `content-length` bytes arrived.
fn body(raw: &[u8]) -> Option<&[u8]> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
    let length = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    raw.get(end + 4..end + 4 + length)
}

fn serve(mut socket: TcpStream, calls: &Mutex<Vec<String>>) {
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    let request = loop {
        match socket.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
        }
        if let Some(body) = body(&raw) {
            break serde_json::from_slice::<Value>(body).unwrap_or_default();
        }
    };
    // Other local tools may probe a fresh loopback port; only the client's POST counts.
    if !raw.starts_with(b"POST ") {
        let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n");
        return;
    }

    let schema = request["response_format"]["json_schema"]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let body = reply(&answer(&schema).to_string());
    calls.lock().expect("calls").push(schema);
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes());
}

pub fn start() -> FakeModel {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("http://{}/api/v1", listener.local_addr().expect("addr"));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&calls);
    thread::spawn(move || {
        for socket in listener.incoming().flatten() {
            serve(socket, &seen);
        }
    });
    FakeModel { endpoint, calls }
}
