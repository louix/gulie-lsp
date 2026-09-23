//! Formatting through the server: whole documents, ranges, and malformed
//! input, plus idempotence and token preservation over every fixture.

#[path = "common/session.rs"]
mod session;

use guile_lsp::documents::Documents;
use guile_lsp::format::{apply, format, Rules};
use guile_lsp::syntax::{parse, Kind};
use serde_json::{json, Value};
use session::Session;
use tower_lsp::lsp_types::{TextDocumentItem, TextEdit, Url};

fn edits(reply: &Value) -> Vec<TextEdit> {
    serde_json::from_value(reply["result"].clone()).expect("text edits")
}

fn tokens(text: &str) -> Vec<String> {
    parse(text)
        .root
        .descendants()
        .filter(|n| n.children.is_empty() && n.kind != Kind::Whitespace)
        .map(|n| n.text(text).trim_end().to_string())
        .collect()
}

fn open(session: &mut Session, path: &str, version: i64) -> (String, String) {
    let uri = Url::from_file_path(path).unwrap().to_string();
    let text = std::fs::read_to_string(path).unwrap();
    session.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "scheme", "version": version, "text": text } }),
    );
    session.notification("textDocument/publishDiagnostics");
    (uri, text)
}

#[test]
fn formats_documents_and_ranges_through_the_server() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/guile");
    let mut session = Session::start(env!("CARGO_BIN_EXE_guile-lsp"));
    session.set_timeout(std::time::Duration::from_secs(60));
    let (init, _) = session.request(
        "initialize",
        json!({ "processId": null, "rootUri": Url::from_file_path(root).unwrap().to_string(),
                "capabilities": {},
                "initializationOptions": { "guile": { "command": ["false"], "indent": { "my-with": 1 } } } }),
    );
    assert_eq!(
        init["result"]["capabilities"]["documentFormattingProvider"],
        true
    );
    assert_eq!(
        init["result"]["capabilities"]["documentRangeFormattingProvider"],
        true
    );
    session.notify("initialized", json!({}));

    // The whole messy fixture becomes the expected one, and stays there.
    let messy = format!("{root}/format/messy.scm");
    let (uri, text) = open(&mut session, &messy, 1);
    let (reply, _) = session.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 2, "insertSpaces": true } }),
    );
    let formatted = apply(&text, &edits(&reply));
    let expected = std::fs::read_to_string(format!("{root}/format/expected.scm")).unwrap();
    assert_eq!(formatted, expected);
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": formatted }] }),
    );
    session.notification("textDocument/publishDiagnostics");
    let (reply, _) = session.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 2, "insertSpaces": true } }),
    );
    assert_eq!(reply["result"], json!([]), "formatting is idempotent");

    // Range formatting touches only the requested lines, but reads the
    // lines above them.
    let ranged = "(define (f)\n      (let ((x 1))\nx\ny))\n(my-with a\nb)\n";
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": ranged }] }),
    );
    session.notification("textDocument/publishDiagnostics");
    let (reply, _) = session.request(
        "textDocument/rangeFormatting",
        json!({ "textDocument": { "uri": uri },
                "range": { "start": { "line": 2, "character": 0 }, "end": { "line": 3, "character": 0 } },
                "options": { "tabSize": 2, "insertSpaces": true } }),
    );
    assert_eq!(
        apply(ranged, &edits(&reply)),
        "(define (f)\n      (let ((x 1))\n        x\ny))\n(my-with a\nb)\n"
    );
    // The user's own rule from initializationOptions applies.
    let (reply, _) = session.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 2, "insertSpaces": true } }),
    );
    assert!(apply(ranged, &edits(&reply)).ends_with("(my-with a\n  b)\n"));

    // Malformed input: lines before the first reader error are formatted,
    // the rest untouched, and the client is told why.
    let broken = "(define x\n1)\n(define (f)\n  (let ((y 2)\ny))\n";
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 4 }, "contentChanges": [{ "text": broken }] }),
    );
    session.notification("textDocument/publishDiagnostics");
    let (reply, seen) = session.request_collect(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 2, "insertSpaces": true } }),
    );
    assert_eq!(
        apply(broken, &edits(&reply)),
        "(define x\n  1)\n(define (f)\n  (let ((y 2)\ny))\n"
    );
    // The explanation may precede the reply.
    let message = match seen
        .into_iter()
        .find(|m| m.get("method") == Some(&json!("window/showMessage")))
    {
        Some(message) => message,
        None => session.notification("window/showMessage").0,
    };
    let text = message["params"]["message"].as_str().unwrap();
    assert!(
        text.starts_with("Formatted only up to line 3:") && text.contains("unclosed form"),
        "{text}"
    );

    let (shutdown, _) = session.request("shutdown", Value::Null);
    assert_eq!(shutdown["result"], Value::Null);
    session.notify("exit", Value::Null);
    session.close_stdin();
}

#[test]
fn fixtures_format_stably_without_changing_tokens() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");
    let mut rules = Rules::scheme();
    rules.extend(guile_lsp::adapters::guix::indent_rules());
    rules.treat_as_prefixes(guile_lsp::adapters::guix::INDENT_PREFIXES);
    let mut checked = 0;
    let mut pending = vec![std::path::PathBuf::from(root)];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "scm") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let snapshot = |text: &str| {
                Documents::default().open(TextDocumentItem {
                    uri: Url::parse("file:///f.scm").unwrap(),
                    language_id: "scheme".into(),
                    version: 1,
                    text: text.into(),
                })
            };
            let once = apply(&text, &format(&snapshot(&text), &rules, None).edits);
            let twice = apply(&once, &format(&snapshot(&once), &rules, None).edits);
            assert_eq!(once, twice, "{}: not idempotent", path.display());
            assert_eq!(
                tokens(&text),
                tokens(&once),
                "{}: tokens changed",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked > 10, "only {checked} fixtures checked");
}

fn run_format(args: &[&str], cwd: &std::path::Path, stdin: &str) -> (i32, String, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_guile-lsp"))
        .arg("format")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn command_line_checks_and_rewrites_files() {
    let dir = std::env::temp_dir().join(format!("guile-lsp-format-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    let messy = "(define (f)\nx)\n";
    let tidy = "(define (f)\n  x)\n";
    std::fs::write(dir.join("sub/a.scm"), messy).unwrap();
    std::fs::write(dir.join("b.scm"), tidy).unwrap();
    std::fs::write(dir.join(".hidden/c.scm"), messy).unwrap();
    std::fs::write(dir.join("notes.txt"), messy).unwrap();

    let (status, stdout, _) = run_format(&["--check", "."], &dir, "");
    assert_eq!((status, stdout.as_str()), (1, "./sub/a.scm\n"));
    assert_eq!(
        std::fs::read_to_string(dir.join("sub/a.scm")).unwrap(),
        messy
    );

    let (status, stdout, stderr) = run_format(&["."], &dir, "");
    assert_eq!((status, stdout.as_str(), stderr.as_str()), (0, "", ""));
    assert_eq!(
        std::fs::read_to_string(dir.join("sub/a.scm")).unwrap(),
        tidy
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(".hidden/c.scm")).unwrap(),
        messy
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
        messy
    );
    assert_eq!(run_format(&["--check", "."], &dir, "").0, 0);

    // A reader error formats the lines before it and fails.
    std::fs::write(dir.join("broken.scm"), "(define (g)\ny)\n(oops\n").unwrap();
    let (status, _, stderr) = run_format(&["broken.scm"], &dir, "");
    assert_eq!(status, 1);
    assert!(stderr.starts_with("broken.scm:3: "), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.join("broken.scm")).unwrap(),
        "(define (g)\n  y)\n(oops\n"
    );

    // Rules come from the nearest .dir-locals.el above a relative path.
    std::fs::write(
        dir.join(".dir-locals.el"),
        "((scheme-mode . ((eval . (put 'with-thing 'scheme-indent-function 1)))))",
    )
    .unwrap();
    std::fs::write(dir.join("sub/d.scm"), "(with-thing x\ny)\n").unwrap();
    assert_eq!(run_format(&["d.scm"], &dir.join("sub"), "").0, 0);
    assert_eq!(
        std::fs::read_to_string(dir.join("sub/d.scm")).unwrap(),
        "(with-thing x\n  y)\n"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn command_line_formats_standard_input() {
    let cwd = std::env::temp_dir();
    let source =
        "(use-modules ((gnu system) #:prefix sys.))\n(sys.operating-system\n(host-name \"h\"))\n";
    let (status, stdout, _) = run_format(&["-"], &cwd, source);
    assert_eq!(status, 0);
    assert_eq!(
        stdout,
        "(use-modules ((gnu system) #:prefix sys.))\n(sys.operating-system\n  (host-name \"h\"))\n"
    );
    assert_eq!(run_format(&["--check", "-"], &cwd, source).0, 1);
    assert_eq!(run_format(&["--check", "-"], &cwd, &stdout).0, 0);
    // Without a Guix import the record rule comes only with --guix.
    let plain = "(operating-system\n(host-name \"h\"))\n";
    assert_eq!(
        run_format(&["-"], &cwd, plain).1,
        "(operating-system\n (host-name \"h\"))\n"
    );
    assert_eq!(
        run_format(&["--guix", "-"], &cwd, plain).1,
        "(operating-system\n  (host-name \"h\"))\n"
    );
    assert_eq!(run_format(&[], &cwd, "").0, 2);
    assert_eq!(run_format(&["-", "x.scm"], &cwd, "").0, 2);
    assert_eq!(run_format(&["--bogus", "x.scm"], &cwd, "").0, 2);
}
