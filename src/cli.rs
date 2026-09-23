//! `guile-lsp format`: the server's formatter from the command line, for
//! pre-commit hooks, CI, and editors that pipe a buffer through a command.
//!
//! There is no worker here, so Guix rules apply when a file imports
//! `(gnu ...)` or `(guix ...)`, or with `--guix`, and `.dir-locals.el` is
//! the nearest one at or above the file (or the working directory for
//! standard input).

use crate::documents::{Documents, Snapshot};
use crate::format::{apply, format, standalone_rules};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::{TextDocumentItem, Url};

pub const FORMAT_USAGE: &str = "\
usage: guile-lsp format [--check] [--guix] PATH...
       guile-lsp format [--check] [--guix] -

Reindent Scheme files in place. A directory stands for every .scm file
under it, skipping hidden directories; `-` formats standard input to
standard output.

  --check  change nothing; list the files that would change
  --guix   apply Guix's indentation rules even to files that do not import
           (gnu ...) or (guix ...)

Exit status: 0 when everything is formatted, 1 when --check found files to
change or a file has a reader error (lines from the error on are left
alone), 2 on usage or I/O errors.";

/// Run `guile-lsp format` with the arguments after `format`, returning the
/// exit status.
pub fn format_command(args: &[String]) -> i32 {
    let mut check = false;
    let mut guix = false;
    let mut stdin = false;
    let mut paths = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "--guix" => guix = true,
            "-h" | "--help" => {
                println!("{FORMAT_USAGE}");
                return 0;
            }
            "-" => stdin = true,
            flag if flag.starts_with('-') => return usage(&format!("unknown option {flag}")),
            path => paths.push(PathBuf::from(path)),
        }
    }
    match (stdin, paths.is_empty()) {
        (true, false) => usage("`-` cannot be combined with paths"),
        (false, true) => usage("no files given"),
        (true, true) => format_stdin(check, guix),
        (false, false) => format_paths(&paths, check, guix),
    }
}

fn usage(problem: &str) -> i32 {
    eprintln!("guile-lsp format: {problem}\n\n{FORMAT_USAGE}");
    2
}

/// Formatted text of one document, and whether a reader error stopped it.
fn formatted(text: String, path: Option<&Path>, guix: bool, name: &str) -> (String, bool) {
    let absolute = path.and_then(|p| std::fs::canonicalize(p).ok());
    let uri = absolute
        .as_ref()
        .and_then(|p| Url::from_file_path(p).ok())
        .unwrap_or_else(|| Url::parse("file:///stdin.scm").unwrap());
    let snapshot: Snapshot = Documents::default().open(TextDocumentItem {
        uri,
        language_id: "scheme".into(),
        version: 1,
        text: text.clone(),
    });
    let dir = match &absolute {
        Some(p) => p.parent().map(Path::to_path_buf),
        None => std::env::current_dir().ok(),
    };
    let rules = standalone_rules(&snapshot, dir.as_deref(), guix);
    let outcome = format(&snapshot, &rules, None);
    if let Some(stop) = &outcome.stopped {
        eprintln!(
            "{name}:{}: {}; formatted only the lines before it",
            stop.line + 1,
            stop.message
        );
    }
    (apply(&text, &outcome.edits), outcome.stopped.is_some())
}

fn format_stdin(check: bool, guix: bool) -> i32 {
    let mut text = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut text) {
        eprintln!("guile-lsp format: standard input: {e}");
        return 2;
    }
    let (after, stopped) = formatted(text.clone(), None, guix, "<stdin>");
    if check {
        return i32::from(stopped || after != text);
    }
    let mut stdout = std::io::stdout().lock();
    if let Err(e) = stdout
        .write_all(after.as_bytes())
        .and_then(|()| stdout.flush())
    {
        eprintln!("guile-lsp format: standard output: {e}");
        return 2;
    }
    i32::from(stopped)
}

fn format_paths(paths: &[PathBuf], check: bool, guix: bool) -> i32 {
    let mut files = Vec::new();
    let mut status = 0;
    for path in paths {
        if path.is_dir() {
            if let Err(e) = scheme_files(path, &mut files) {
                eprintln!("{}: {e}", path.display());
                status = 2;
            }
        } else {
            files.push(path.clone());
        }
    }
    for file in &files {
        let name = file.display().to_string();
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("{name}: {e}");
                status = 2;
                continue;
            }
        };
        let (after, stopped) = formatted(text.clone(), Some(file), guix, &name);
        if stopped {
            status = status.max(1);
        }
        if after == text {
            continue;
        }
        if check {
            println!("{name}");
            status = status.max(1);
        } else if let Err(e) = std::fs::write(file, after) {
            eprintln!("{name}: {e}");
            status = 2;
        }
    }
    status
}

/// Every `.scm` file under `dir`, sorted, skipping hidden directories.
fn scheme_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    entries.sort();
    for path in entries {
        let hidden = path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'));
        if path.is_dir() {
            if !hidden {
                scheme_files(&path, out)?;
            }
        } else if path.extension().is_some_and(|e| e == "scm") {
            out.push(path);
        }
    }
    Ok(())
}
