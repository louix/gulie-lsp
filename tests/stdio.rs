#[path = "common/session.rs"]
mod session;

use serde_json::{json, Value};
use session::Session;
use std::time::{Duration, Instant};
use tower_lsp::lsp_types::Url;

#[test]
fn stdio_lifecycle_and_capabilities() {
    let mut session = Session::start(env!("CARGO_BIN_EXE_guile-lsp"));
    let (response, _) = session.request(
        "initialize",
        json!({ "processId": null, "rootUri": null, "capabilities": {},
                "initializationOptions": { "guile": { "trust": "trusted" } } }),
    );
    assert_eq!(response["result"]["serverInfo"]["name"], "guile-lsp");
    assert_eq!(
        response["result"]["capabilities"]["textDocumentSync"]["change"],
        1
    );
    assert_eq!(response["result"]["capabilities"]["hoverProvider"], true);
    assert_eq!(
        response["result"]["capabilities"]["definitionProvider"],
        true
    );
    assert_eq!(
        response["result"]["capabilities"]["documentSymbolProvider"],
        true
    );

    session.notify("initialized", json!({}));
    if guile_lsp::worker::default_command().is_some() {
        let fate = session.wait_for_worker();
        assert!(
            fate["params"]["message"]
                .as_str()
                .unwrap()
                .contains("ready"),
            "{fate}"
        );
    }
    let document = json!({ "uri": "file:///config.scm" });
    session.notify(
        "textDocument/didOpen",
        json!({ "textDocument": {
            "uri": "file:///config.scm", "languageId": "scheme",
            "version": 1, "text": "(define name \"λ🦀\")"
        }}),
    );
    let (published, _) = session.notification("textDocument/publishDiagnostics");
    assert_eq!(published["params"]["version"], 1);
    assert_eq!(published["params"]["diagnostics"], json!([]));

    session.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": "file:///config.scm", "version": 2 },
            "contentChanges": [{ "text": "(unfinished λ" }]
        }),
    );
    let (published, _) = session.notification("textDocument/publishDiagnostics");
    assert_eq!(published["params"]["version"], 2);
    let diagnostics = published["params"]["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1, "{published}");
    assert_eq!(
        diagnostics[0]["message"],
        "unclosed form; expected a closing delimiter"
    );
    assert_eq!(
        diagnostics[0]["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );

    let (symbols, _) = session.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": document }),
    );
    assert_eq!(symbols["result"], json!([]), "not a definition: {symbols}");

    let (selection, _) = session.request(
        "textDocument/selectionRange",
        json!({ "textDocument": document, "positions": [{ "line": 0, "character": 12 }] }),
    );
    assert_eq!(
        selection["result"][0]["range"],
        json!({ "start": { "line": 0, "character": 12 }, "end": { "line": 0, "character": 13 } })
    );
    assert_eq!(
        selection["result"][0]["parent"]["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );

    let (completion, _) = session.request(
        "textDocument/completion",
        json!({ "textDocument": document, "position": { "line": 0, "character": 3 } }),
    );
    let labels: Vec<&str> = completion["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert_eq!(&labels[..2], ["unless", "unquote"], "{completion}");
    if guile_lsp::worker::default_command().is_some() {
        // Imported names come from cached export listings, fetched in the
        // background after the first request; ask again until they are in.
        let mut unread = None;
        for _ in 0..50 {
            let (completion, _) = session.request(
                "textDocument/completion",
                json!({ "textDocument": document, "position": { "line": 0, "character": 3 } }),
            );
            unread = completion["result"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["label"] == "unread-char")
                .cloned();
            if unread.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(
            unread.map(|i| i["detail"].clone()),
            Some(json!("(guile)")),
            "{completion}"
        );
    }

    // Hover and definition: local definitions answer from the tree; imported
    // bindings go through the Guile worker when one is available.
    session.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": "file:///config.scm", "version": 3 },
            "contentChanges": [{ "text": "(use-modules (srfi srfi-1))\n(define (twice x) (fold + x '()))\n(twice 1)" }]
        }),
    );
    session.notification("textDocument/publishDiagnostics");
    let (hover, _) = session.request(
        "textDocument/hover",
        json!({ "textDocument": document, "position": { "line": 2, "character": 2 } }),
    );
    let value = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(
        value.contains("(define (twice x)") && value.contains("defined in this file at line 2"),
        "{hover}"
    );
    let (definition, _) = session.request(
        "textDocument/definition",
        json!({ "textDocument": document, "position": { "line": 2, "character": 2 } }),
    );
    assert_eq!(
        definition["result"]["range"]["start"],
        json!({ "line": 1, "character": 9 })
    );
    if guile_lsp::worker::default_command().is_some() {
        let (hover, _) = session.request(
            "textDocument/hover",
            json!({ "textDocument": document, "position": { "line": 1, "character": 20 } }),
        );
        let value = hover["result"]["contents"]["value"].as_str().unwrap_or("");
        assert!(
            value.contains("(fold kons knil list1") && value.contains("from `(srfi srfi-1)`"),
            "{hover}"
        );
        let (definition, _) = session.request(
            "textDocument/definition",
            json!({ "textDocument": document, "position": { "line": 1, "character": 20 } }),
        );
        assert!(
            definition["result"]["uri"]
                .as_str()
                .unwrap()
                .ends_with("srfi/srfi-1.scm"),
            "{definition}"
        );
        let (none, _) = session.request(
            "textDocument/hover",
            json!({ "textDocument": document, "position": { "line": 1, "character": 18 } }),
        );
        assert!(none["result"].is_null(), "hover on a paren: {none}");
    } else {
        eprintln!("no guile on PATH; skipping worker-backed hover assertions");
    }

    // Opening a file that declares a module teaches the server its load-path
    // root, so sibling modules resolve without any loadPath setting.
    let util_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/guile/app/lib/util.scm"
    );
    let util_uri = Url::from_file_path(util_path).unwrap().to_string();
    session.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": util_uri, "languageId": "scheme", "version": 1,
                                   "text": std::fs::read_to_string(util_path).unwrap() } }),
    );
    session.notification("textDocument/publishDiagnostics");
    session.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": "file:///config.scm", "version": 4 },
            "contentChanges": [{ "text": "(use-modules (lambda-star))\n(connect \"h\")" }]
        }),
    );
    session.notification("textDocument/publishDiagnostics");
    if guile_lsp::worker::default_command().is_some() {
        let (hover, _) = session.request(
            "textDocument/hover",
            json!({ "textDocument": document, "position": { "line": 1, "character": 3 } }),
        );
        let value = hover["result"]["contents"]["value"].as_str().unwrap_or("");
        assert!(
            value.contains("(connect host [port] #:timeout #:tls? . extra)"),
            "root inference: {hover}"
        );
        assert!(value.contains("from `(lambda-star)`"), "{hover}");
    }
    // Once the defining file is open, its buffer wins over the worker, so
    // unsaved edits are what hover and definition show.
    let star_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/guile/lambda-star.scm"
    );
    let star_uri = Url::from_file_path(star_path).unwrap().to_string();
    session.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": star_uri, "languageId": "scheme", "version": 1,
                                   "text": std::fs::read_to_string(star_path).unwrap() } }),
    );
    session.notification("textDocument/publishDiagnostics");
    let (hover, _) = session.request(
        "textDocument/hover",
        json!({ "textDocument": document, "position": { "line": 1, "character": 3 } }),
    );
    let value = hover["result"]["contents"]["value"].as_str().unwrap_or("");
    assert!(
        value.contains("defined in lambda-star.scm at line 6"),
        "{hover}"
    );
    let (definition, _) = session.request(
        "textDocument/definition",
        json!({ "textDocument": document, "position": { "line": 1, "character": 3 } }),
    );
    assert_eq!(definition["result"]["uri"], json!(star_uri));
    assert_eq!(
        definition["result"]["range"]["start"],
        json!({ "line": 5, "character": 10 })
    );

    // Completion: lexical names, imported names with their module, module
    // paths inside use-modules, and nothing inside quoted data.
    let change = |session: &mut Session, version: i64, text: &str| {
        session.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": "file:///config.scm", "version": version },
                "contentChanges": [{ "text": text }]
            }),
        );
        session.notification("textDocument/publishDiagnostics");
    };
    let complete = |session: &mut Session, line: u32, character: u32| -> Vec<Value> {
        let (reply, _) = session.request(
            "textDocument/completion",
            json!({ "textDocument": document, "position": { "line": line, "character": character } }),
        );
        reply["result"].as_array().cloned().unwrap_or_default()
    };
    change(&mut session, 5, "(define (shout text) (string-append (te");
    let items = complete(&mut session, 0, 39);
    let local: Vec<&Value> = items.iter().filter(|i| i["label"] == "text").collect();
    assert_eq!(local.len(), 1, "{items:?}");
    assert_eq!(local[0]["detail"], "local binding");
    let (hover, _) = session.request(
        "textDocument/hover",
        json!({ "textDocument": document, "position": { "line": 0, "character": 16 } }),
    );
    assert!(
        hover["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("*local binding* introduced at line 1"),
        "{hover}"
    );
    let (definition, _) = session.request(
        "textDocument/definition",
        json!({ "textDocument": document, "position": { "line": 0, "character": 16 } }),
    );
    assert_eq!(
        definition["result"]["range"]["start"],
        json!({ "line": 0, "character": 15 })
    );

    change(&mut session, 6, "(list 'quo");
    assert!(
        complete(&mut session, 0, 10).is_empty(),
        "quoted data offers nothing"
    );

    if guile_lsp::worker::default_command().is_some() {
        change(
            &mut session,
            7,
            "(use-modules ((srfi srfi-1) #:prefix s:))\n(s:fo",
        );
        let items = complete(&mut session, 1, 5);
        let fold: Vec<&Value> = items.iter().filter(|i| i["label"] == "s:fold").collect();
        assert_eq!(fold.len(), 1, "{items:?}");
        assert_eq!(fold[0]["detail"], "(srfi srfi-1)");
        assert_eq!(fold[0]["kind"], 3, "function");
        assert!(
            items.iter().all(|i| i["label"] != "fold"),
            "prefixed import hides the bare name"
        );
        change(
            &mut session,
            9,
            "(use-modules ((srfi srfi-1) #:prefix s:))\n(s:fold",
        );
        let (hover, _) = session.request(
            "textDocument/hover",
            json!({ "textDocument": document, "position": { "line": 1, "character": 3 } }),
        );
        let value = hover["result"]["contents"]["value"].as_str().unwrap_or("");
        assert!(
            value.contains("(s:fold kons knil list1")
                && value.contains("from `(srfi srfi-1)`, exported as `fold`"),
            "{hover}"
        );

        // A module name in an import jumps to the module's file.
        let (definition, _) = session.request(
            "textDocument/definition",
            json!({ "textDocument": document, "position": { "line": 0, "character": 20 } }),
        );
        assert!(
            definition["result"]["uri"]
                .as_str()
                .unwrap_or("")
                .ends_with("/srfi/srfi-1.scm"),
            "{definition}"
        );
        let (hover, _) = session.request(
            "textDocument/hover",
            json!({ "textDocument": document, "position": { "line": 0, "character": 20 } }),
        );
        let value = hover["result"]["contents"]["value"].as_str().unwrap_or("");
        assert!(value.starts_with("**module** `(srfi srfi-1)`"), "{hover}");

        change(&mut session, 10, "(use-modules (srfi srfi-");
        // The module listing is fetched in the background on the first miss.
        let mut items = complete(&mut session, 0, 24);
        for _ in 0..50 {
            if !items.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            items = complete(&mut session, 0, 24);
        }
        let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
        assert!(
            labels.contains(&"srfi-1") && labels.contains(&"srfi-9"),
            "{labels:?}"
        );
        let one = items.iter().find(|i| i["label"] == "srfi-1").unwrap();
        assert_eq!(one["detail"], "(srfi srfi-1)");
    }

    // Semantic diagnostics: arity mistakes on resolved procedures arrive in a
    // debounced publication for the same version; valid calls stay clean.
    if guile_lsp::worker::default_command().is_some() {
        change(&mut session, 11, "(use-modules (srfi srfi-1))\n(define (twice x) (fold + x))\n(fold + 0 '())\n(fold + 0 '() '())\n(map car)\n'(fold)\n(1 2)");
        let published = session.semantic_diagnostics("file:///config.scm", 11);
        let messages: Vec<&str> = published["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["message"].as_str().unwrap())
            .collect();
        assert!(
            messages.contains(&"fold expects at least 3 arguments, got 2"),
            "{messages:?}"
        );
        assert!(messages.contains(&"1 is not a procedure"), "{messages:?}");
        assert!(
            messages.iter().filter(|m| m.contains("fold")).count() == 1,
            "the valid calls, including the two-list case-lambda clause, and the quoted one are clean: {messages:?}"
        );
        assert!(
            messages.contains(&"map expects at least 2 arguments, got 1"),
            "{messages:?}"
        );
        let arity = published["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["message"].as_str().unwrap().starts_with("fold"))
            .unwrap();
        assert_eq!(
            arity["range"]["start"],
            json!({ "line": 1, "character": 19 })
        );
        assert_eq!(arity["severity"], 1);
    }

    session.notify("textDocument/didClose", json!({ "textDocument": document }));
    let (cleared, _) = session.notification("textDocument/publishDiagnostics");
    assert_eq!(cleared["params"]["diagnostics"], json!([]));
    assert!(cleared["params"].get("version").is_none_or(|v| v.is_null()));

    let (shutdown, _) = session.request("shutdown", Value::Null);
    assert_eq!(shutdown.get("result"), Some(&Value::Null));
    session.notify("exit", Value::Null);
    // tower-lsp finishes serving on stdin EOF (or the next message after `exit`),
    // so close the pipe the way a real client does once it is done.
    session.close_stdin();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = session.child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "server did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}
