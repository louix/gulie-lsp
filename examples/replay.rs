//! Request replay harness: drives the built server over stdio with a
//! scripted editing session and reports round-trip latency percentiles.
//!
//!   cargo build --release
//!   cargo run --release --example replay -- fixtures/guix/system.scm [steps]
//!
//! The session opens an empty document, then simulates typing the file in
//! `steps` increments. After each edit it waits for the diagnostics
//! notification and issues completion, document symbol, and selection range
//! requests at the edit point, timing every response.
//! Set GUILE_LSP_BIN to test a binary other than the one next to this example.

#[path = "../tests/common/session.rs"]
mod session;

use serde_json::{json, Value};
use session::Session;
use std::time::{Duration, Instant};

fn server_binary() -> String {
    std::env::var("GUILE_LSP_BIN").unwrap_or_else(|_| {
        let mut path = std::env::current_exe().unwrap();
        path.pop(); // examples/
        path.pop(); // profile dir
        path.push("guile-lsp");
        path.to_string_lossy().into_owned()
    })
}

struct Stat {
    name: &'static str,
    samples: Vec<Duration>,
}

impl Stat {
    fn new(name: &'static str) -> Self {
        Stat {
            name,
            samples: Vec::new(),
        }
    }

    fn percentile(&self, p: f64) -> Duration {
        let mut sorted = self.samples.clone();
        sorted.sort();
        let index = ((sorted.len() - 1) as f64 * p).round() as usize;
        sorted[index]
    }

    fn report(&self) {
        println!(
            "| {:<24} | {:>5} | {:>8.2} | {:>8.2} | {:>8.2} |",
            self.name,
            self.samples.len(),
            self.percentile(0.5).as_secs_f64() * 1000.0,
            self.percentile(0.95).as_secs_f64() * 1000.0,
            self.samples.iter().max().unwrap().as_secs_f64() * 1000.0
        );
    }
}

/// Position (line, UTF-16 column) of the end of `text`.
fn end_position(text: &str) -> Value {
    let line = text.matches('\n').count();
    let last = text.rsplit('\n').next().unwrap_or("");
    let character: usize = last.chars().map(char::len_utf16).sum();
    json!({ "line": line, "character": character })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .unwrap_or_else(|| "fixtures/guix/system.scm".into());
    let steps: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(50);
    let text = std::fs::read_to_string(&path).unwrap();
    let uri = format!("file://{}", std::fs::canonicalize(&path).unwrap().display());

    let mut session = Session::start(&server_binary());
    session.set_timeout(Duration::from_secs(120));
    // REPLAY_GUIX=1 runs a trusted Guix worker and waits for it, so the
    // numbers include import warming and the worker's memory.
    let guix = std::env::var_os("REPLAY_GUIX").is_some();
    let options = if guix {
        json!({ "guile": { "command": ["guix", "repl", "--"], "trust": "trusted" } })
    } else {
        Value::Null
    };
    let (_, init) = session.request(
        "initialize",
        json!({ "processId": null, "rootUri": null, "capabilities": {}, "initializationOptions": options }),
    );
    session.notify("initialized", json!({}));
    let worker_start = Instant::now();
    if guix {
        let ready = session.wait_for_worker();
        println!(
            "worker after {:.2} s: {}",
            worker_start.elapsed().as_secs_f64(),
            ready["params"]["message"].as_str().unwrap_or("")
        );
    }

    let mut open = Stat::new("didOpen -> diagnostics");
    let mut change = Stat::new("didChange -> diagnostics");
    let mut completion = Stat::new("completion");
    let mut symbols = Stat::new("documentSymbol");
    let mut selection = Stat::new("selectionRange");

    // Boundaries at char positions so every intermediate text is valid UTF-8.
    let boundaries: Vec<usize> = {
        let chars: Vec<usize> = text
            .char_indices()
            .map(|(i, _)| i)
            .chain([text.len()])
            .collect();
        (1..=steps)
            .map(|step| chars[(chars.len() - 1) * step / steps])
            .collect()
    };

    session.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "scheme", "version": 1, "text": "" } }),
    );
    open.samples
        .push(session.notification("textDocument/publishDiagnostics").1);

    let mut diagnostics_at_end = 0;
    for (step, &end) in boundaries.iter().enumerate() {
        let partial = &text[..end];
        session.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": step + 2 },
                "contentChanges": [{ "text": partial }]
            }),
        );
        let (published, latency) = session.notification("textDocument/publishDiagnostics");
        change.samples.push(latency);
        diagnostics_at_end = published["params"]["diagnostics"].as_array().unwrap().len();
        let position = end_position(partial);
        let document = json!({ "uri": uri });
        completion.samples.push(
            session
                .request(
                    "textDocument/completion",
                    json!({ "textDocument": document, "position": position }),
                )
                .1,
        );
        symbols.samples.push(
            session
                .request(
                    "textDocument/documentSymbol",
                    json!({ "textDocument": document }),
                )
                .1,
        );
        selection.samples.push(
            session
                .request(
                    "textDocument/selectionRange",
                    json!({ "textDocument": document, "positions": [position] }),
                )
                .1,
        );
    }

    // Memory before shutdown, while the worker is still a child.
    let memory = peak_rss_kib(session.child.id());
    let (_, shutdown) = session.request("shutdown", Value::Null);
    session.notify("exit", Value::Null);

    println!(
        "replayed {} ({} bytes) in {} steps; initialize {:.2} ms, shutdown {:.2} ms, {} diagnostics at the end",
        path,
        text.len(),
        steps,
        init.as_secs_f64() * 1000.0,
        shutdown.as_secs_f64() * 1000.0,
        diagnostics_at_end
    );
    for (name, pid, hwm) in &memory {
        println!(
            "peak RSS of {name} (pid {pid}): {:.1} MiB",
            *hwm as f64 / 1024.0
        );
    }
    println!();
    println!("| request                  |     n |  p50 ms |  p95 ms |   max ms |");
    println!("|--------------------------|-------|---------|---------|----------|");
    for stat in [&open, &change, &completion, &symbols, &selection] {
        stat.report();
    }
}

/// Peak resident set (VmHWM, KiB) of `root` and its descendants, from
/// /proc, so the worker shows up beside the server.
fn peak_rss_kib(root: u32) -> Vec<(String, u32, u64)> {
    let mut all: Vec<(u32, u32, String, u64)> = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        let field = |name: &str| -> Option<String> {
            status
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .map(|v| v.trim().to_string())
        };
        let ppid = field("PPid:").and_then(|p| p.parse().ok()).unwrap_or(0);
        let name = field("Name:").unwrap_or_default();
        let hwm = field("VmHWM:")
            .and_then(|v| v.split_whitespace().next()?.parse().ok())
            .unwrap_or(0);
        all.push((pid, ppid, name, hwm));
    }
    let mut out = Vec::new();
    let mut queue = vec![root];
    while let Some(target) = queue.pop() {
        for (pid, ppid, name, hwm) in &all {
            if *pid == target && !out.iter().any(|(_, p, _)| p == pid) {
                out.push((name.clone(), *pid, *hwm));
            }
            if *ppid == target {
                queue.push(*pid);
            }
        }
    }
    out
}
