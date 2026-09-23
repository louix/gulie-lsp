//! Explicit Guix configuration validation through the server. Skipped with
//! a note when `guix` is not on PATH.

#[path = "common/session.rs"]
mod session;

use serde_json::{json, Value};
use session::Session;
use std::time::Duration;
use tower_lsp::lsp_types::Url;

fn guix_available() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    if std::env::split_paths(&path).any(|dir| dir.join("guix").is_file()) {
        true
    } else {
        eprintln!("no guix on PATH; skipping validation test");
        false
    }
}

fn open(session: &mut Session, path: &str, text: &str) -> String {
    let uri = Url::from_file_path(path).unwrap().to_string();
    session.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "scheme", "version": 1, "text": text } }),
    );
    session.notification("textDocument/publishDiagnostics");
    uri
}

fn validate(session: &mut Session, uri: &str) -> Value {
    let (reply, _) = session.request(
        "workspace/executeCommand",
        json!({ "command": "guile.validate", "arguments": [uri] }),
    );
    reply
}

/// The next `window/showMessage` for a validation outcome.
fn outcome(session: &mut Session) -> (i64, String) {
    let (message, _) = session.notification("window/showMessage");
    (
        message["params"]["type"].as_i64().unwrap(),
        message["params"]["message"].as_str().unwrap().to_string(),
    )
}

fn validation_processes() -> usize {
    let output = std::process::Command::new("pgrep")
        .args(["-f", "guile-lsp-worker.*validate.scm"])
        .output()
        .expect("pgrep");
    String::from_utf8_lossy(&output.stdout).lines().count()
}

#[test]
fn validates_saved_configurations_and_cancels_slow_ones() {
    if !guix_available() {
        return;
    }
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/guix");
    let system = format!("{root}/system.scm");
    let scratch = std::env::temp_dir().join(format!("guile-lsp-validate-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let mut session = Session::start(env!("CARGO_BIN_EXE_guile-lsp"));
    session.set_timeout(Duration::from_secs(120));
    let (init, _) = session.request(
        "initialize",
        // The repository is the workspace root, as in an editor; the fixture's
        // own directory must be found from its `(my services)` import.
        json!({ "processId": null, "rootUri": Url::from_file_path(env!("CARGO_MANIFEST_DIR")).unwrap().to_string(),
                "capabilities": {},
                "initializationOptions": { "guile": {
                    "command": ["guix", "repl", "--"], "trust": "trusted",
                    "validateTimeoutMs": 8000 } } }),
    );
    assert_eq!(
        init["result"]["capabilities"]["executeCommandProvider"]["commands"],
        json!(["guile.validate", "guile.validate.cancel"])
    );
    session.notify("initialized", json!({}));
    session.wait_for_worker();

    // The saved fixture: an operating system whose service graph folds.
    let text = std::fs::read_to_string(&system).unwrap();
    let uri = open(&mut session, &system, &text);
    let reply = validate(&mut session, &uri);
    assert_eq!(reply["result"]["started"], true, "{reply}");
    let (kind, message) = outcome(&mut session);
    assert_eq!(kind, 3, "info: {message}");
    assert!(
        message.contains("operating system \"fixture\" with")
            && message.contains("no derivation computed"),
        "{message}"
    );
    // Diagnostics precede the message; the validation set is empty here.

    // An unsaved edit is refused with an explanation, before anything runs.
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": format!("{text}\n;; edited") }] }),
    );
    session.notification("textDocument/publishDiagnostics");
    let refused = session.request_raw(
        "workspace/executeCommand",
        json!({ "command": "guile.validate", "arguments": [uri] }),
    );
    let error = refused["error"]["message"].as_str().unwrap_or("");
    assert!(
        error.starts_with("Save system.scm before validating"),
        "{refused}"
    );
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": text }] }),
    );
    session.notification("textDocument/publishDiagnostics");

    // A broken copy: the unknown field is reported at its form, labelled
    // with the document version, and dropped by the next edit.
    let broken_path = scratch.join("broken.scm");
    let broken = text.replace("(x11-forwarding? #t)", "(x11-forwarding? #t) (nonsense 1)");
    std::fs::write(&broken_path, &broken).unwrap();
    let broken_uri = open(&mut session, broken_path.to_str().unwrap(), &broken);
    validate(&mut session, &broken_uri);
    // Diagnostics are published before the summary message.
    let published = session.wait_for(|m| {
        m.get("method") == Some(&json!("textDocument/publishDiagnostics"))
            && m["params"]["uri"] == json!(broken_uri)
            && m["params"]["diagnostics"]
                .as_array()
                .is_some_and(|d| d.iter().any(|x| x["source"] == "guix validation"))
    });
    let (kind, message) = outcome(&mut session);
    assert_eq!(kind, 1, "error: {message}");
    assert!(
        message.contains("extraneous field initializers (nonsense)"),
        "{message}"
    );
    assert_eq!(published["params"]["version"], 1);
    let diagnostics = published["params"]["diagnostics"].as_array().unwrap();
    let validation: Vec<&Value> = diagnostics
        .iter()
        .filter(|d| d["source"] == "guix validation")
        .collect();
    assert_eq!(validation.len(), 1, "{diagnostics:?}");
    assert_eq!(validation[0]["range"]["start"]["line"], 60);
    assert_eq!(validation[0]["range"]["start"]["character"], 19);
    assert!(
        validation[0]["range"]["end"]["line"].as_u64().unwrap() > 60,
        "spans the form"
    );
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": broken_uri, "version": 2 }, "contentChanges": [{ "text": format!("{broken}\n") }] }),
    );
    let published = session.wait_for(|m| {
        m.get("method") == Some(&json!("textDocument/publishDiagnostics"))
            && m["params"]["uri"] == json!(broken_uri)
            && m["params"]["version"] == json!(2)
    });
    assert_eq!(published["params"]["version"], 2);
    assert!(
        !published["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["source"] == "guix validation"),
        "validation diagnostics are invalidated by an edit"
    );

    // Its buffer now differs from disk, which would block validating anything.
    session.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": broken_uri } }),
    );
    session.notification("textDocument/publishDiagnostics");
    // A configuration that never finishes: cancel kills the process; a
    // second run hits the time limit.
    let slow_path = scratch.join("slow.scm");
    let slow = "(use-modules (gnu))\n(let loop () (loop))\n";
    std::fs::write(&slow_path, slow).unwrap();
    let slow_uri = open(&mut session, slow_path.to_str().unwrap(), slow);
    // Without an argument the command applies to the document touched last.
    let (reply, _) = session.request(
        "workspace/executeCommand",
        json!({ "command": "guile.validate", "arguments": [] }),
    );
    assert_eq!(reply["result"]["started"], true, "{reply}");
    std::thread::sleep(Duration::from_secs(3));
    assert!(validation_processes() >= 1, "validation process is running");
    let (reply, _) = session.request(
        "workspace/executeCommand",
        json!({ "command": "guile.validate.cancel", "arguments": [slow_uri] }),
    );
    assert_eq!(reply["result"]["cancelled"], true);
    // The confirmation message may arrive before the reply, so it is not
    // waited for here; the process going away is the real check.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(validation_processes(), 0, "process killed on cancel");
    validate(&mut session, &slow_uri);
    let (kind, message) = outcome(&mut session);
    assert_eq!(kind, 1);
    assert!(message.contains("exceeded 8 s"), "{message}");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(validation_processes(), 0, "process killed on timeout");

    let (shutdown, _) = session.request("shutdown", Value::Null);
    assert_eq!(shutdown["result"], Value::Null);
    session.notify("exit", Value::Null);
    session.close_stdin();
    let _ = std::fs::remove_dir_all(&scratch);
}
