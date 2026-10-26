//! Minimal synchronous LSP client over stdio (JSON-RPC with `Content-Length` framing).
//!
//! The server runs with a cleared environment: only the variables a toolchain needs to find
//! itself are passed, so host, SCM and model credentials never reach it.

use std::env;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const PASSED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "TMPDIR",
    "LANG",
];

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("cannot start language server: {0}")]
    Spawn(io::Error),
    #[error("language server exited")]
    Exited,
    #[error("{0} timed out")]
    Timeout(String),
    #[error("{method} failed: {message}")]
    Response { method: String, message: String },
    #[error("language server io: {0}")]
    Io(#[from] io::Error),
}

pub struct LspServer {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    next_id: i64,
    /// Notifications seen while waiting, except progress and log chatter.
    pub notifications: Vec<(String, Value)>,
}

/// Reads one framed message; `None` at end of stream.
fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }

        let line = line.trim_end();
        if line.is_empty() {
            break;
        }

        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length.ok_or_else(|| io::Error::other("frame without Content-Length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

impl LspServer {
    /// `extra` overrides the passed-through variables (e.g. a Sych-owned `CARGO_HOME`).
    pub fn spawn(
        program: &Path,
        args: &[&str],
        cwd: &Path,
        extra: &[(String, String)],
    ) -> Result<Self, LspError> {
        let mut command = Command::new(program);
        command.args(args).current_dir(cwd).env_clear();
        for name in PASSED_ENV {
            if let Some(value) = env::var_os(name) {
                command.env(name, value);
            }
        }
        command.envs(extra.iter().map(|(k, v)| (k, v)));
        tracing::debug!(program = %program.display(), ?args, "starting language server");
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(LspError::Spawn)?;
        let stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let (tx, incoming) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(Some(message)) = read_message(&mut stdout) {
                if tx.send(message).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            incoming,
            next_id: 0,
            notifications: Vec::new(),
        })
    }

    pub fn notify(&mut self, method: &str, params: &Value) -> Result<(), LspError> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        Ok(write_message(&mut self.stdin, &message)?)
    }

    pub fn request(
        &mut self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        write_message(&mut self.stdin, &message)?;
        let response = self.wait(timeout, method, |m| m.get("id") == Some(&json!(id)))?;
        match response.get("error") {
            Some(error) => Err(LspError::Response {
                method: method.to_owned(),
                message: error["message"].as_str().unwrap_or("error").to_owned(),
            }),
            None => Ok(response.get("result").cloned().unwrap_or(Value::Null)),
        }
    }

    /// Waits for a notification `method` whose params satisfy `accept`.
    pub fn wait_notification(
        &mut self,
        method: &str,
        timeout: Duration,
        accept: impl Fn(&Value) -> bool,
    ) -> Result<Value, LspError> {
        self.wait(timeout, method, |m| {
            m.get("id").is_none() && m["method"] == method && accept(&m["params"])
        })
        .map(|m| m["params"].clone())
    }

    /// Answers server requests and records notifications until `done` matches a message.
    fn wait(
        &mut self,
        timeout: Duration,
        what: &str,
        done: impl Fn(&Value) -> bool,
    ) -> Result<Value, LspError> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let message = match self.incoming.recv_timeout(left) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => return Err(LspError::Timeout(what.to_owned())),
                Err(RecvTimeoutError::Disconnected) => return Err(LspError::Exited),
            };
            if done(&message) {
                return Ok(message);
            }

            let method = message["method"].as_str().unwrap_or_default().to_owned();
            match message.get("id") {
                // A server request: Sych offers no client features, so every answer is empty.
                Some(id) if !method.is_empty() => {
                    let result = match method.as_str() {
                        "workspace/configuration" => {
                            let items = message["params"]["items"].as_array().map_or(0, Vec::len);
                            Value::Array(vec![Value::Null; items])
                        }
                        _ => Value::Null,
                    };
                    let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
                    write_message(&mut self.stdin, &reply)?;
                }
                Some(_) => {}
                None if method.starts_with("$/") || method.starts_with("window/") => {}
                None => self.notifications.push((method, message["params"].clone())),
            }
        }
    }

    pub fn shutdown(mut self) {
        let quick = Duration::from_secs(5);
        if self.request("shutdown", &Value::Null, quick).is_ok() {
            let _ = self.notify("exit", &Value::Null);
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LspServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buffer = Vec::new();
        let message = json!({"jsonrpc": "2.0", "id": 1, "result": "привет"});
        write_message(&mut buffer, &message).expect("write");
        buffer.extend_from_slice(b"Content-Type: x\r\nContent-Length: 2\r\n\r\n{}");
        let mut reader = io::Cursor::new(buffer);
        assert_eq!(read_message(&mut reader).expect("read"), Some(message));
        assert_eq!(read_message(&mut reader).expect("read"), Some(json!({})));
        assert_eq!(read_message(&mut reader).expect("eof"), None);
    }
}
