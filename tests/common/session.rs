//! Minimal LSP client over a child process's stdio, shared by the stdio
//! integration test and the replay harness (which includes this file by path).

#![allow(dead_code)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

pub struct Session {
    pub child: Child,
    messages: Receiver<Value>,
    next_id: i64,
    timeout: Duration,
}

impl Session {
    /// Spawn `binary` and start reading framed messages from its stdout.
    pub fn start(binary: &str) -> Self {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|e| panic!("cannot start {binary}: {e}"));
        let stdout = child.stdout.take().unwrap();
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Content-Length:") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
                let mut body = vec![0; length.expect("LSP Content-Length header")];
                reader.read_exact(&mut body).unwrap();
                if sender.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        Session {
            child,
            messages,
            next_id: 1,
            timeout: Duration::from_secs(5),
        }
    }

    pub fn send(&mut self, message: Value) {
        let body = serde_json::to_vec(&message).unwrap();
        let stdin = self.child.stdin.as_mut().unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        stdin.write_all(&body).unwrap();
        stdin.flush().unwrap();
    }

    /// Send a notification. `Value::Null` omits the params key, as the
    /// spec requires for `initialized`-style methods without parameters.
    pub fn notify(&mut self, method: &str, params: Value) {
        let mut message = json!({ "jsonrpc": "2.0", "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message);
    }

    /// Send a request and wait for its response, returning it with the
    /// round-trip latency. Panics on an error response.
    pub fn request(&mut self, method: &str, params: Value) -> (Value, Duration) {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        let start = Instant::now();
        self.send(message);
        let response = self.wait(|m| m.get("id") == Some(&json!(id)));
        assert!(
            response.get("error").is_none(),
            "{method} failed: {response}"
        );
        (response, start.elapsed())
    }

    /// Send a request and return its response together with every
    /// notification that arrived before it, for messages whose order
    /// relative to the reply is not guaranteed.
    pub fn request_collect(&mut self, method: &str, params: Value) -> (Value, Vec<Value>) {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message);
        let mut seen = Vec::new();
        let deadline = Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self.messages.recv_timeout(remaining).unwrap();
            if message.get("id") == Some(&json!(id)) {
                assert!(message.get("error").is_none(), "{method} failed: {message}");
                return (message, seen);
            }
            seen.push(message);
        }
    }

    /// Send a request and return its response, error or not.
    pub fn request_raw(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message);
        self.wait(|m| m.get("id") == Some(&json!(id)))
    }

    /// Wait for a diagnostics publication for `uri` at `version` that carries
    /// a semantic diagnostic (one with a code; reader errors have none),
    /// skipping the immediate syntax-only publication. Analysis is debounced.
    pub fn semantic_diagnostics(&mut self, uri: &str, version: i64) -> Value {
        self.wait(|m| {
            m.get("method") == Some(&json!("textDocument/publishDiagnostics"))
                && m["params"]["uri"] == json!(uri)
                && m["params"]["version"] == json!(version)
                && m["params"]["diagnostics"]
                    .as_array()
                    .is_some_and(|d| d.iter().any(|x| x.get("code").is_some()))
        })
    }

    /// A queued diagnostics publication for `uri`, without waiting for one.
    /// Other queued messages are dropped.
    pub fn try_notification(&mut self, method: &str, uri: &str) -> Option<Vec<Value>> {
        while let Ok(message) = self.messages.try_recv() {
            if message.get("method") == Some(&json!(method))
                && message["params"]["uri"] == json!(uri)
            {
                return message["params"]["diagnostics"].as_array().cloned();
            }
        }
        None
    }

    /// Wait until the server logs the worker's fate: ready or unavailable.
    /// Semantic requests sent before that answer from syntax alone.
    pub fn wait_for_worker(&mut self) -> Value {
        self.wait(|m| {
            m.get("method") == Some(&json!("window/logMessage"))
                && m["params"]["message"].as_str().is_some_and(|t| {
                    t.contains("Guile worker ready")
                        || t.contains("worker unavailable")
                        || t.contains("cannot start Guile worker")
                })
        })
    }

    /// Wait for the next notification with `method`, discarding others.
    pub fn notification(&mut self, method: &str) -> (Value, Duration) {
        let start = Instant::now();
        let message = self.wait(|m| m.get("method") == Some(&json!(method)));
        (message, start.elapsed())
    }

    /// Wait for the first message `accept` approves, discarding others.
    pub fn wait_for(&mut self, accept: impl Fn(&Value) -> bool) -> Value {
        self.wait(accept)
    }

    fn wait(&mut self, accept: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self.messages.recv_timeout(remaining).unwrap();
            // SESSION_TRACE=1 prints every message, for debugging a hang.
            if std::env::var_os("SESSION_TRACE").is_some() {
                let text = message.to_string();
                eprintln!("<- {}", &text[..text.len().min(300)]);
            }
            if accept(&message) {
                return message;
            }
            // Server logs are the only explanation for empty semantic results.
            if message.get("method") == Some(&json!("window/logMessage")) {
                eprintln!("server log: {}", message["params"]["message"]);
            }
        }
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Close stdin so the server's serve loop returns after `exit`.
    pub fn close_stdin(&mut self) {
        drop(self.child.stdin.take());
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
