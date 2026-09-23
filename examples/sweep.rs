//! False-positive sweep: open each FILE through the server with a trusted
//! worker, wait for semantic analysis, and print every semantic diagnostic.
//! Known-valid code should print nothing.
//!
//!   cargo run --release --example sweep -- [--guix] FILE...
//!
//! `--guix` uses `guix repl --` as the worker; otherwise `guile` from PATH.

#[path = "../tests/common/session.rs"]
mod session;

use serde_json::{json, Value};
use session::Session;
use std::time::{Duration, Instant};
use tower_lsp::lsp_types::Url;

fn server_binary() -> String {
    std::env::var("GUILE_LSP_BIN").unwrap_or_else(|_| {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("guile-lsp");
        path.to_string_lossy().into_owned()
    })
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let guix = args.iter().any(|a| a == "--guix");
    args.retain(|a| a != "--guix");
    let command = if guix {
        json!(["guix", "repl", "--"])
    } else {
        json!(["guile"])
    };
    let mut session = Session::start(&server_binary());
    session.set_timeout(Duration::from_secs(120));
    // SWEEP_ROOT names a workspace root, so a fixture tree can resolve its own
    // modules.
    let root_uri = std::env::var("SWEEP_ROOT")
        .ok()
        .and_then(|r| Url::from_file_path(std::fs::canonicalize(r).ok()?).ok())
        .map(|u| json!(u.to_string()))
        .unwrap_or(Value::Null);
    session.request(
        "initialize",
        json!({ "processId": null, "rootUri": root_uri, "capabilities": {},
                "initializationOptions": { "guile": { "command": command, "trust": "trusted", "deadlineMs": 60000 } } }),
    );
    session.notify("initialized", json!({}));
    session.wait_for_worker();
    let mut total = 0;
    for (index, path) in args.iter().enumerate() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let uri = format!("file://{}", std::fs::canonicalize(path).unwrap().display());
        let start = Instant::now();
        session.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "scheme", "version": 1, "text": text } }),
        );
        // The syntax publication arrives first; the debounced analysis
        // always publishes a second time once the worker is ready, even
        // with nothing to report, so wait for that one.
        let (_, _) = session.notification("textDocument/publishDiagnostics");
        let (published, _) = session.notification("textDocument/publishDiagnostics");
        let diagnostics: Vec<Value> = published["params"]["diagnostics"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let semantic: Vec<&Value> = diagnostics
            .iter()
            .filter(|d| d.get("code").is_some())
            .collect();
        let syntax = diagnostics.len() - semantic.len();
        println!(
            "[{}/{}] {} ({:.1}s): {} syntax, {} semantic",
            index + 1,
            args.len(),
            path,
            start.elapsed().as_secs_f64(),
            syntax,
            semantic.len()
        );
        for d in &semantic {
            println!(
                "    {}:{} [{}] {}",
                d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1,
                d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1,
                d["code"].as_str().unwrap_or(""),
                d["message"].as_str().unwrap_or("")
            );
        }
        total += semantic.len();
        session.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        );
        session.notification("textDocument/publishDiagnostics");
    }
    println!("{total} semantic diagnostics in {} files", args.len());
    session.request("shutdown", Value::Null);
    session.notify("exit", Value::Null);
}
