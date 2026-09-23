//! Import assistance: the edit that makes a module visible to a document.
//! Prefers the document's own conventions: a `#:use-module` clause in a
//! `define-module`, a new spec in an existing `use-modules`, a Guix
//! shorthand (`use-service-modules ssh`) when the file already uses one,
//! and only otherwise a fresh `use-modules` form after the leading comments.

use crate::documents::Snapshot;
use crate::syntax::{Kind, Node};
use tower_lsp::lsp_types::TextEdit;

/// A way to import `module`, with its edit and a label for the action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub title: String,
    pub edit: TextEdit,
}

fn module_text(module: &[String]) -> String {
    format!("({})", module.join(" "))
}

/// Indentation (columns, spaces only) of the line containing `offset`.
fn indent_of(snapshot: &Snapshot, offset: usize) -> usize {
    let line = snapshot.lines.line_of(offset);
    let start = snapshot.lines.line_start(line).unwrap_or(0);
    snapshot.text[start..offset]
        .chars()
        .take_while(|c| *c == ' ')
        .count()
}

/// Column of `offset` on its line.
fn column_of(snapshot: &Snapshot, offset: usize) -> usize {
    let line = snapshot.lines.line_of(offset);
    let start = snapshot.lines.line_start(line).unwrap_or(0);
    snapshot.text[start..offset].chars().count()
}

/// The edit importing `module`, or `None` when the document already does.
pub fn suggest(snapshot: &Snapshot, module: &[String]) -> Option<Suggestion> {
    let source: &str = &snapshot.text;
    if crate::scope::imports(snapshot)
        .iter()
        .any(|i| i.module == module)
    {
        return None;
    }
    let forms: Vec<&Node> = snapshot.tree.root.data().collect();
    let head_of = |form: &Node| -> Option<String> {
        let head = form.data().next()?;
        (head.kind == Kind::Symbol).then(|| head.text(source).to_string())
    };

    // Guix shorthand: (use-service-modules ssh) for (gnu services ssh).
    if module.len() == 3 && module[0] == "gnu" {
        let shorthand = match module[1].as_str() {
            "services" => Some("use-service-modules"),
            "packages" => Some("use-package-modules"),
            "system" => Some("use-system-modules"),
            _ => None,
        };
        if let Some(shorthand) = shorthand {
            if let Some(form) = forms
                .iter()
                .find(|f| head_of(f).as_deref() == Some(shorthand))
            {
                let last = form.data().last()?;
                let insert = last.range.end;
                return Some(Suggestion {
                    title: format!("Add {} to {shorthand}", module[2]),
                    edit: TextEdit {
                        range: snapshot.range(insert..insert),
                        new_text: format!(" {}", module[2]),
                    },
                });
            }
        }
    }

    // A define-module: another #:use-module clause after the last one, or
    // after the module name.
    if let Some(form) = forms
        .iter()
        .find(|f| head_of(f).as_deref() == Some("define-module"))
    {
        let data: Vec<&Node> = form.data().collect();
        let last_use = data
            .windows(2)
            .rev()
            .find(|w| w[0].kind == Kind::Keyword && w[0].text(source) == "#:use-module")
            .map(|w| w[1]);
        let after = last_use.or_else(|| data.get(1).copied())?;
        let indent = match last_use {
            Some(node) => {
                let keyword = data.iter().position(|n| std::ptr::eq(*n, node))? - 1;
                column_of(snapshot, data[keyword].range.start)
            }
            None => indent_of(snapshot, form.range.start) + 2,
        };
        let insert = after.range.end;
        return Some(Suggestion {
            title: format!("Import {} with #:use-module", module_text(module)),
            edit: TextEdit {
                range: snapshot.range(insert..insert),
                new_text: format!(
                    "\n{}#:use-module {}",
                    " ".repeat(indent),
                    module_text(module)
                ),
            },
        });
    }

    // An existing use-modules: a new spec after the last one, aligned with
    // the first spec when the form spans lines.
    if let Some(form) = forms
        .iter()
        .find(|f| head_of(f).as_deref() == Some("use-modules"))
    {
        let data: Vec<&Node> = form.data().collect();
        let (first, last) = (data.get(1), data.last());
        let insert = last.map_or(form.data().next()?.range.end, |n| n.range.end);
        let separator = match first {
            Some(first) if data.len() > 1 => {
                let one_line = snapshot.lines.line_of(first.range.start)
                    == snapshot.lines.line_of(last?.range.start)
                    && data.len() <= 2;
                if one_line {
                    " ".to_string()
                } else {
                    format!("\n{}", " ".repeat(column_of(snapshot, first.range.start)))
                }
            }
            _ => " ".to_string(),
        };
        return Some(Suggestion {
            title: format!("Import {}", module_text(module)),
            edit: TextEdit {
                range: snapshot.range(insert..insert),
                new_text: format!("{separator}{}", module_text(module)),
            },
        });
    }

    // Nothing to extend: a new form before the first datum, after the
    // shebang and leading comments.
    let insert = forms.first().map_or(source.len(), |f| f.range.start);
    Some(Suggestion {
        title: format!("Import {}", module_text(module)),
        edit: TextEdit {
            range: snapshot.range(insert..insert),
            new_text: format!("(use-modules {})\n\n", module_text(module)),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::Documents;
    use crate::format::apply;
    use tower_lsp::lsp_types::{TextDocumentItem, Url};

    fn snapshot(text: &str) -> Snapshot {
        Documents::default().open(TextDocumentItem {
            uri: Url::parse("file:///f.scm").unwrap(),
            language_id: "scheme".into(),
            version: 1,
            text: text.into(),
        })
    }

    fn imported(text: &str, module: &[&str]) -> (String, String) {
        let module: Vec<String> = module.iter().map(|s| s.to_string()).collect();
        let s = suggest(&snapshot(text), &module).expect("a suggestion");
        (s.title, apply(text, std::slice::from_ref(&s.edit)))
    }

    #[test]
    fn extends_the_document_s_own_import_style() {
        let (title, text) = imported(
            "(define-module (app main)\n  #:use-module (srfi srfi-1)\n  #:export (main))\n",
            &["ice-9", "match"],
        );
        assert_eq!(title, "Import (ice-9 match) with #:use-module");
        assert_eq!(
            text,
            "(define-module (app main)\n  #:use-module (srfi srfi-1)\n  #:use-module (ice-9 match)\n  #:export (main))\n"
        );
        let (_, text) = imported("(define-module (app main))\n", &["ice-9", "match"]);
        assert_eq!(
            text,
            "(define-module (app main)\n  #:use-module (ice-9 match))\n"
        );

        let (_, text) = imported(
            "(use-modules (gnu)\n             (gnu services ssh))\n(x)",
            &["gnu", "services", "networking"],
        );
        assert_eq!(
            text,
            "(use-modules (gnu)\n             (gnu services ssh)\n             (gnu services networking))\n(x)"
        );
        let (_, text) = imported("(use-modules (gnu))\n", &["guix", "gexp"]);
        assert_eq!(text, "(use-modules (gnu) (guix gexp))\n");

        let (title, text) = imported(
            "(use-modules (gnu))\n(use-service-modules desktop)\n",
            &["gnu", "services", "ssh"],
        );
        assert_eq!(title, "Add ssh to use-service-modules");
        assert_eq!(
            text,
            "(use-modules (gnu))\n(use-service-modules desktop ssh)\n"
        );

        let (_, text) = imported(
            "#!/usr/bin/env guile\n!#\n;;; script\n(display 1)\n",
            &["ice-9", "format"],
        );
        assert_eq!(
            text,
            "#!/usr/bin/env guile\n!#\n;;; script\n(use-modules (ice-9 format))\n\n(display 1)\n"
        );
        assert!(suggest(
            &snapshot("(use-modules (ice-9 match))"),
            &["ice-9".into(), "match".into()]
        )
        .is_none());
    }
}
