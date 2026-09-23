//! The Guix adapter against a real Guix, through the server. Skipped with a
//! note when `guix` is not on PATH.

#[path = "common/session.rs"]
mod session;

use serde_json::{json, Value};
use session::Session;
use tower_lsp::lsp_types::Url;

fn guix_available() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    if std::env::split_paths(&path).any(|dir| dir.join("guix").is_file()) {
        true
    } else {
        eprintln!("no guix on PATH; skipping Guix adapter test");
        false
    }
}

fn hover(session: &mut Session, uri: &str, line: u32, character: u32) -> String {
    let (reply, _) = session.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } }),
    );
    reply["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("no hover at {line}:{character}: {reply}"))
        .to_string()
}

#[test]
fn hover_and_definition_through_guix_repl() {
    if !guix_available() {
        return;
    }
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/guix");
    let path = format!("{root}/system.scm");
    let uri = Url::from_file_path(&path).unwrap().to_string();
    let mut session = Session::start(env!("CARGO_BIN_EXE_guile-lsp"));
    session.set_timeout(std::time::Duration::from_secs(90));
    session.request(
        "initialize",
        json!({ "processId": null, "rootUri": Url::from_file_path(root).unwrap().to_string(),
                "capabilities": {},
                "initializationOptions": { "guile": {
                    "command": ["guix", "repl", "--"], "trust": "trusted", "deadlineMs": 60000 } } }),
    );
    session.notify("initialized", json!({}));
    let ready = session.wait_for_worker();
    assert!(
        ready["params"]["message"]
            .as_str()
            .unwrap()
            .contains("with Guix"),
        "{ready}"
    );
    session.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "scheme", "version": 1,
                                   "text": std::fs::read_to_string(&path).unwrap() } }),
    );
    session.notification("textDocument/publishDiagnostics");

    // (service openssh-service-type ...) on line 60 (1-based).
    let service = hover(&mut session, &uri, 59, 20);
    assert!(
        service.contains("**Guix service type** `openssh`"),
        "{service}"
    );
    assert!(
        service.contains("Default value: `<openssh-configuration>`"),
        "{service}"
    );
    assert!(service.contains("Extends: `shepherd-root`"), "{service}");

    // (operating-system ...) on line 17: fields resolved through the (gnu) umbrella.
    let os = hover(&mut session, &uri, 16, 3);
    assert!(os.contains("from `(gnu)`"), "{os}");
    assert!(os.contains("| `host-name` | *required* |"), "{os}");
    assert!(os.contains("| `timezone` | `\"Etc/UTC\"` |"), "{os}");

    // (thermald-configuration (adaptive? #t)) on line 53.
    let thermald = hover(&mut session, &uri, 52, 24);
    assert!(thermald.contains("| `adaptive?` | `#f` |"), "{thermald}");

    // A service type from the fixture's own module, found via the rootUri.
    let local = hover(&mut session, &uri, 66, 20);
    assert!(
        local.contains("**Guix service type** `my-hello`"),
        "{local}"
    );
    assert!(local.contains("Greet someone at boot time."), "{local}");
    let (definition, _) = session.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 66, "character": 20 } }),
    );
    assert!(
        definition["result"]["uri"]
            .as_str()
            .unwrap_or("")
            .ends_with("fixtures/guix/my/services.scm"),
        "{definition}"
    );
    assert_eq!(definition["result"]["range"]["start"]["line"], 35);

    // The type of a service type is its configuration record.
    let (record, _) = session.request(
        "textDocument/typeDefinition",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 59, "character": 20 } }),
    );
    assert!(
        record["result"]["uri"]
            .as_str()
            .unwrap_or("")
            .ends_with("gnu/services/ssh.scm"),
        "{record}"
    );
    assert!(
        record["result"]["range"]["start"]["line"].as_u64().unwrap() > 100,
        "{record}"
    );

    // A shorthand import symbol jumps to the module file.
    let (definition, _) = session.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 13, "character": 24 } }),
    );
    assert!(
        definition["result"]["uri"]
            .as_str()
            .unwrap_or("")
            .ends_with("gnu/services/desktop.scm"),
        "{definition}"
    );

    // Completion inside the config: imported Guix names with their modules.
    let (completion, _) = session.request(
        "textDocument/completion",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 60, "character": 20 } }),
    );
    let items = completion["result"].as_array().unwrap();
    assert!(
        items.len() > 1000,
        "{} items: {:?}",
        items.len(),
        items
            .iter()
            .map(|i| i["label"].as_str().unwrap())
            .collect::<Vec<_>>()
    );
    let ssh = items
        .iter()
        .find(|i| i["label"] == "openssh-service-type")
        .expect("openssh-service-type offered");
    assert_eq!(ssh["detail"], "(gnu services ssh)");
    let os = items
        .iter()
        .find(|i| i["label"] == "operating-system")
        .expect("operating-system offered");
    assert_eq!(os["detail"], "(gnu)");
    assert_eq!(os["kind"], 14, "macro completes as a keyword");

    // Record fields: hover and definition on a written field, completion of
    // the remaining fields at a new field position, shorthand imports.
    let field_hover = hover(&mut session, &uri, 61, 24);
    assert!(
        field_hover.contains("*field* of `openssh-configuration`"),
        "{field_hover}"
    );
    assert!(field_hover.contains("Default: `#f`"), "{field_hover}");
    assert!(
        field_hover.contains("from the record definition in `"),
        "{field_hover}"
    );
    let (definition, _) = session.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 61, "character": 24 } }),
    );
    assert!(
        definition["result"]["uri"]
            .as_str()
            .unwrap_or("")
            .ends_with("gnu/services/ssh.scm"),
        "{definition}"
    );
    assert_eq!(definition["result"]["range"]["start"]["line"], 127);

    let tlp_hover = hover(&mut session, &uri, 57, 24);
    assert!(tlp_hover.contains("type `maybe-boolean`"), "{tlp_hover}");
    assert!(
        tlp_hover.contains("Enable CPU turbo boost feature on AC mode."),
        "{tlp_hover}"
    );

    let mut edited: Vec<String> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    edited.insert(62, "                    (po".into());
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": edited.join("\n") }] }),
    );
    session.notification("textDocument/publishDiagnostics");
    let (completion, _) = session.request(
        "textDocument/completion",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 62, "character": 23 } }),
    );
    let labels: Vec<&str> = completion["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["port-number"], "{labels:?}");
    let port = &completion["result"][0];
    assert_eq!(port["kind"], 5, "field");
    assert!(
        port["detail"].as_str().unwrap().contains("default 22"),
        "{port}"
    );
    edited[62] = "                    (".into();
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": edited.join("\n") }] }),
    );
    session.notification("textDocument/publishDiagnostics");
    let (completion, _) = session.request(
        "textDocument/completion",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 62, "character": 21 } }),
    );
    let labels: Vec<&str> = completion["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert!(
        labels.contains(&"port-number") && labels.contains(&"inherit"),
        "{labels:?}"
    );
    assert!(
        !labels.contains(&"x11-forwarding?") && !labels.contains(&"authorized-keys"),
        "supplied fields are suppressed: {labels:?}"
    );

    // (use-service-modules desktop networking) brings gnome-desktop-service-type.
    edited.remove(62);
    edited.insert(65, "                    (gnome-".into());
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 4 }, "contentChanges": [{ "text": edited.join("\n") }] }),
    );
    session.notification("textDocument/publishDiagnostics");
    let (completion, _) = session.request(
        "textDocument/completion",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 65, "character": 27 } }),
    );
    let gnome = completion["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["label"] == "gnome-desktop-service-type")
        .expect("shorthand import");
    assert_eq!(gnome["detail"], "(gnu services desktop)");

    // Record diagnostics: an unknown field is an error, a complete form
    // without a required field is a warning, and inherit silences it.
    edited.remove(65);
    edited.insert(62, "                    (nonsense 1)".into());
    edited.push("(file-system (mount-point \"/\"))".into());
    edited.push("(file-system (inherit x) (type \"ext4\"))".into());
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 5 }, "contentChanges": [{ "text": edited.join("\n") }] }),
    );
    let published = session.semantic_diagnostics(&uri, 5);
    let messages: Vec<&str> = published["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["message"].as_str().unwrap())
        .collect();
    assert!(
        messages.contains(&"openssh-configuration has no field nonsense"),
        "{messages:?}"
    );
    assert!(
        messages.contains(&"file-system is missing required fields device, type"),
        "{messages:?}"
    );
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.contains("file-system"))
            .count(),
        1,
        "inherit silences the missing-field check: {messages:?}"
    );
    assert!(
        !messages.iter().any(|m| m.contains("operating-system")),
        "a valid config form stays clean: {messages:?}"
    );

    let (shutdown, _) = session.request("shutdown", Value::Null);
    assert_eq!(shutdown["result"], Value::Null);
    session.notify("exit", Value::Null);
    session.close_stdin();
}

#[test]
fn guix_home_records_and_import_assistance() {
    if !guix_available() {
        return;
    }
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/guix");
    let path = format!("{root}/home.scm");
    let uri = Url::from_file_path(&path).unwrap().to_string();
    let mut session = Session::start(env!("CARGO_BIN_EXE_guile-lsp"));
    session.set_timeout(std::time::Duration::from_secs(120));
    session.request(
        "initialize",
        json!({ "processId": null, "rootUri": Url::from_file_path(root).unwrap().to_string(),
                "capabilities": {},
                "initializationOptions": { "guile": {
                    "command": ["guix", "repl", "--"], "trust": "trusted", "deadlineMs": 60000 } } }),
    );
    session.notify("initialized", json!({}));
    session.wait_for_worker();
    let text = std::fs::read_to_string(&path).unwrap();
    session.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "scheme", "version": 1, "text": text } }),
    );
    session.notification("textDocument/publishDiagnostics");

    // Home records and service types resolve like system ones.
    let bash = hover(&mut session, &uri, 14, 22);
    assert!(hover_lists_fields(&bash), "{bash}");
    let field = hover(&mut session, &uri, 15, 22);
    assert!(
        field.contains("*field* of `home-bash-configuration`"),
        "{field}"
    );
    let service = hover(&mut session, &uri, 13, 25);
    assert!(
        service.contains("**Guix service type** `home-bash`"),
        "{service}"
    );
    let he = hover(&mut session, &uri, 10, 3);
    assert!(
        he.contains("| `services` |") && he.contains("from `(gnu home)`"),
        "{he}"
    );

    // Validation classifies it as a home environment.
    let (reply, _) = session.request(
        "workspace/executeCommand",
        json!({ "command": "guile.validate", "arguments": [uri] }),
    );
    assert_eq!(reply["result"]["started"], true);
    let outcome = session.wait_for(|m| {
        m.get("method") == Some(&json!("window/showMessage"))
            && m["params"]["message"]
                .as_str()
                .is_some_and(|t| t.starts_with("home.scm:"))
    });
    let message = outcome["params"]["message"].as_str().unwrap();
    assert!(
        message.contains("home environment with 15 services"),
        "{message}"
    );

    // Import assistance searches modules the worker has loaded, so touch
    // the ssh module first; then drop its import and ask for a code action
    // on the service type: the import comes back as a quick fix.
    let ssh = hover(&mut session, &uri, 18, 25);
    assert!(ssh.contains("home-openssh"), "{ssh}");
    let without = text.replace("             (gnu home services ssh)\n", "");
    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": without }] }),
    );
    session.notification("textDocument/publishDiagnostics");
    let line = without
        .lines()
        .position(|l| l.contains("home-openssh-service-type"))
        .unwrap() as u32;
    let (actions, _) = session.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri },
                "range": { "start": { "line": line, "character": 20 }, "end": { "line": line, "character": 20 } },
                "context": { "diagnostics": [] } }),
    );
    let titles: Vec<&str> = actions["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        ["Import (gnu home services ssh) for home-openssh-service-type"],
        "{actions}"
    );
    let edit = &actions["result"][0]["edit"]["changes"][&uri][0];
    assert_eq!(edit["newText"], "\n             (gnu home services ssh)");
    assert_eq!(edit["range"]["start"]["line"], 7);

    // A name the document already imports gets no action.
    let (none, _) = session.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri },
                "range": { "start": { "line": 13, "character": 25 }, "end": { "line": 13, "character": 25 } },
                "context": { "diagnostics": [] } }),
    );
    assert!(none["result"].is_null(), "{none}");

    let (shutdown, _) = session.request("shutdown", Value::Null);
    assert_eq!(shutdown["result"], Value::Null);
    session.notify("exit", Value::Null);
    session.close_stdin();
}

fn hover_lists_fields(text: &str) -> bool {
    text.contains("home-bash-configuration") && text.contains("| `guix-defaults?` |")
}
