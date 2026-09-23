//! Fidelity check for the formatter: reindent Emacs-formatted sources and
//! report the lines that would change. Run against a Guix checkout:
//!
//!     cargo run --release --example indent -- --diff 3 gnu/services/*.scm
//!
//! Guix's sources are not uniformly Emacs-indented, so the stricter check
//! reindents them with Emacs first (`scripts/emacs-indent.sh`) and then
//! points `--dir-locals` at the checkout the rules should come from:
//!
//!     cargo run --release --example indent -- --dir-locals GUIX /tmp/emacs-indented/*.scm
//!
//! Every file is read as is, formatted with the Scheme and Guix rules plus
//! any `.dir-locals.el` found above it, and compared line by line. Exit
//! status is non-zero when any file would change, so this doubles as a
//! regression check. Also verifies that formatting is idempotent and that
//! no non-blank content changes.

use guile_lsp::documents::Documents;
use guile_lsp::format::{apply, format, standalone_rules};
use guile_lsp::syntax::{parse, Kind};
use std::path::PathBuf;
use tower_lsp::lsp_types::{TextDocumentItem, Url};

fn tokens(text: &str) -> Vec<String> {
    let tree = parse(text);
    tree.root
        .descendants()
        .filter(|n| n.children.is_empty() && n.kind != Kind::Whitespace)
        .map(|n| n.text(text).trim_end().to_string())
        .collect()
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut diff_lines = 0usize;
    let mut dir_locals: Option<PathBuf> = None;
    if let Some(i) = args.iter().position(|a| a == "--dir-locals") {
        dir_locals = Some(PathBuf::from(&args[i + 1]));
        args.drain(i..i + 2);
    }
    if let Some(i) = args.iter().position(|a| a == "--diff") {
        diff_lines = args[i + 1].parse().expect("--diff N");
        args.drain(i..i + 2);
    }
    let mut total_lines = 0;
    let mut changed_lines = 0;
    let mut changed_files = 0;
    let mut report: Vec<(usize, PathBuf)> = Vec::new();
    for arg in &args {
        let path = PathBuf::from(arg);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                continue;
            }
        };
        let snapshot = Documents::default().open(TextDocumentItem {
            uri: Url::from_file_path(std::fs::canonicalize(&path).unwrap()).unwrap(),
            language_id: "scheme".into(),
            version: 1,
            text: text.clone(),
        });
        let rules = standalone_rules(&snapshot, dir_locals.as_deref().or(path.parent()), true);
        let outcome = format(&snapshot, &rules, None);
        if let Some(stop) = &outcome.stopped {
            eprintln!(
                "{}: stopped at line {}: {}",
                path.display(),
                stop.line + 1,
                stop.message
            );
        }
        let after = apply(&text, &outcome.edits);
        assert_eq!(
            tokens(&text),
            tokens(&after),
            "{}: tokens changed",
            path.display()
        );
        let again = Documents::default().open(TextDocumentItem {
            uri: Url::parse("file:///again.scm").unwrap(),
            language_id: "scheme".into(),
            version: 1,
            text: after.clone(),
        });
        let twice = apply(&after, &format(&again, &rules, None).edits);
        assert_eq!(after, twice, "{}: not idempotent", path.display());

        let mut changed = 0;
        let mut shown = 0;
        for (number, (before, now)) in text.lines().zip(after.lines()).enumerate() {
            total_lines += 1;
            if expand(before.trim_end()) != expand(now.trim_end()) {
                changed += 1;
                if shown < diff_lines {
                    shown += 1;
                    println!("{}:{}", path.display(), number + 1);
                    println!("-{before}");
                    println!("+{now}");
                }
            }
        }
        if changed > 0 {
            changed_files += 1;
            changed_lines += changed;
            report.push((changed, path));
        }
    }
    report.sort_by(|a, b| b.0.cmp(&a.0));
    for (changed, path) in report.iter().take(15) {
        println!("{changed:6} {}", path.display());
    }
    println!(
        "{} files, {} lines; {} files and {} lines would change",
        args.len(),
        total_lines,
        changed_files,
        changed_lines
    );
    if changed_lines > 0 {
        std::process::exit(1);
    }
}

/// Tabs to spaces at eight-column stops, as Emacs shows them.
fn expand(line: &str) -> String {
    let mut out = String::new();
    for ch in line.chars() {
        if ch == '\t' {
            let pad = 8 - out.chars().count() % 8;
            out.extend(std::iter::repeat_n(' ', pad));
        } else {
            out.push(ch);
        }
    }
    out
}
