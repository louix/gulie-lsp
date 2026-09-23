//! End-to-end sessions: cold startup, a missing module, an unsaved helper
//! module, rapid typing, and a prompt shutdown.
//! Needs `guile` on PATH; skipped otherwise.

#[path = "common/session.rs"]
mod session;

use serde_json::{json, Value};
use session::Session;
use std::time::{Duration, Instant};
use tower_lsp::lsp_types::Url;

fn guile_available() -> bool {
    guile_lsp::worker::default_command().is_some()
}

fn open(session: &mut Session, path: &std::path::Path, text: &str) -> String {
    let uri = Url::from_file_path(path).unwrap().to_string();
    session.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "scheme", "version": 1, "text": text } }),
    );
    session.notification("textDocument/publishDiagnostics");
    uri
}

fn hover(session: &mut Session, uri: &str, line: u32, character: u32) -> Option<String> {
    let (reply, _) = session.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } }),
    );
    reply["result"]["contents"]["value"]
        .as_str()
        .map(str::to_string)
}

#[test]
fn sessions_survive_missing_modules_and_read_unsaved_helpers() {
    if !guile_available() {
        eprintln!("no guile on PATH; skipping");
        return;
    }
    let project = std::env::temp_dir().join(format!("guile-lsp-e2e-{}", std::process::id()));
    std::fs::create_dir_all(project.join("lib")).unwrap();
    let helper = "(define-module (lib helper)\n  #:export (greet))\n\n(define (greet name)\n  \"Greet NAME.\"\n  (string-append \"hello \" name))\n";
    std::fs::write(project.join("lib/helper.scm"), helper).unwrap();
    let main =
        "(use-modules (lib helper)\n             (no such module))\n\n(greet \"world\")\n(wave)\n";
    std::fs::write(project.join("main.scm"), main).unwrap();

    let mut session = Session::start(env!("CARGO_BIN_EXE_guile-lsp"));
    session.set_timeout(Duration::from_secs(60));
    let started = Instant::now();
    session.request(
        "initialize",
        json!({ "processId": null, "rootUri": Url::from_file_path(&project).unwrap().to_string(),
                "capabilities": {},
                "initializationOptions": { "guile": { "trust": "trusted" } } }),
    );
    session.notify("initialized", json!({}));

    // Cold start: the first completion answers from syntax before the
    // worker is up, and never waits for it.
    let main_uri = open(&mut session, &project.join("main.scm"), main);
    let (completion, latency) = session.request(
        "textDocument/completion",
        json!({ "textDocument": { "uri": main_uri }, "position": { "line": 3, "character": 3 } }),
    );
    // Nothing lexical or special starts with `gre`, and imported names are
    // not cached yet: an empty list, at once, is the right answer.
    assert!(completion["result"].is_array());
    assert!(
        latency < Duration::from_millis(500),
        "cold completion took {latency:?}"
    );
    let ready = session.wait_for_worker();
    assert!(
        ready["params"]["message"]
            .as_str()
            .unwrap()
            .contains("Guile worker ready"),
        "{ready}"
    );
    eprintln!("worker ready {:?} after initialize", started.elapsed());

    // The local module resolves through the import-inferred root; the
    // missing module resolves to nothing and breaks nothing.
    let greeting = hover(&mut session, &main_uri, 3, 2).unwrap_or_default();
    assert!(greeting.contains("Greet NAME."), "{greeting}");
    assert!(hover(&mut session, &main_uri, 4, 2).is_none());
    let (completion, _) = session.request(
        "textDocument/completion",
        json!({ "textDocument": { "uri": main_uri }, "position": { "line": 4, "character": 2 } }),
    );
    assert!(!completion["result"].as_array().unwrap().is_empty());

    // An unsaved helper: a definition typed into the open helper buffer is
    // found from main.scm without saving.
    let helper_uri = open(&mut session, &project.join("lib/helper.scm"), helper);
    let edited = format!("{helper}\n(define (wave)\n  \"Wave.\"\n  'wave)\n(export wave)\n");
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": helper_uri, "version": 2 }, "contentChanges": [{ "text": edited }] }),
    );
    session.notification("textDocument/publishDiagnostics");
    let (definition, _) = session.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": main_uri }, "position": { "line": 4, "character": 2 } }),
    );
    assert_eq!(
        definition["result"]["uri"],
        json!(helper_uri),
        "{definition}"
    );
    assert_eq!(definition["result"]["range"]["start"]["line"], 7);

    // Rapid typing: twenty edits without waiting, then a request; every
    // publication carries its version and the last one is current.
    for step in 0..20 {
        let text = format!("{main}(display {step})\n");
        session.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": main_uri, "version": 3 + step }, "contentChanges": [{ "text": text }] }),
        );
    }
    let (symbols, latency) = session.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": main_uri } }),
    );
    assert!(symbols["result"].is_array());
    assert!(latency < Duration::from_secs(2), "{latency:?}");
    let last = session.wait_for(|m| {
        m.get("method") == Some(&json!("textDocument/publishDiagnostics"))
            && m["params"]["uri"] == json!(main_uri)
            && m["params"]["version"] == json!(22)
    });
    assert_eq!(last["params"]["diagnostics"], json!([]));

    // Shutdown is prompt even though the worker may be mid-request.
    let started = Instant::now();
    let (shutdown, _) = session.request("shutdown", Value::Null);
    assert_eq!(shutdown["result"], Value::Null);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    session.notify("exit", Value::Null);
    session.close_stdin();
    let _ = std::fs::remove_dir_all(&project);
}
