//! Syntax-only editor features computed from a [`Snapshot`]. None of these
//! wait for the Guile worker; they answer from the tolerant tree alone.

use crate::documents::Snapshot;
use crate::syntax::position::Position as BytePosition;
use crate::syntax::{Kind, Node};
use std::ops::Range as ByteRange;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, DocumentSymbol, Position,
    Range, SelectionRange, SymbolKind,
};

const SOURCE: &str = "guile-lsp";

/// Core forms offered by completion even before any worker metadata exists.
const SPECIAL_FORMS: &[&str] = &[
    "and",
    "begin",
    "case",
    "cond",
    "define",
    "define*",
    "define-module",
    "define-public",
    "define-record-type",
    "define-syntax",
    "define-syntax-rule",
    "define-values",
    "delay",
    "do",
    "else",
    "export",
    "if",
    "lambda",
    "lambda*",
    "let",
    "let*",
    "let-values",
    "letrec",
    "letrec*",
    "quasiquote",
    "quote",
    "set!",
    "syntax-case",
    "syntax-rules",
    "unless",
    "unquote",
    "use-modules",
    "when",
    "with-syntax",
];

impl Snapshot {
    pub fn range(&self, bytes: ByteRange<usize>) -> Range {
        Range::new(self.position(bytes.start), self.position(bytes.end))
    }

    pub fn position(&self, offset: usize) -> Position {
        let BytePosition { line, character } = self.lines.position(&self.text, offset);
        Position::new(line, character)
    }

    pub fn offset(&self, position: Position) -> Option<usize> {
        self.lines.offset(
            &self.text,
            BytePosition {
                line: position.line,
                character: position.character,
            },
        )
    }
}

pub fn diagnostics(snapshot: &Snapshot) -> Vec<Diagnostic> {
    snapshot
        .tree
        .errors
        .iter()
        .map(|error| Diagnostic {
            range: snapshot.range(error.range.clone()),
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some(SOURCE.into()),
            message: error.message.into(),
            ..Default::default()
        })
        .collect()
}

/// The innermost name of a definition target: `name`, `(name args)`, or the
/// curried `((name a) b)`.
fn definition_name(target: &Node) -> Option<&Node> {
    match target.kind {
        Kind::Symbol => Some(target),
        Kind::List => definition_name(target.data().next()?),
        _ => None,
    }
}

fn is_definer(head: &str) -> bool {
    head.starts_with("define")
}

fn symbol_kind(head: &str, target: &Node) -> SymbolKind {
    match head {
        "define-module" => SymbolKind::MODULE,
        "define-syntax" | "define-syntax-rule" | "define-macro" => SymbolKind::OPERATOR,
        "define-record-type" | "define-immutable-record-type" => SymbolKind::STRUCT,
        _ if target.kind == Kind::List => SymbolKind::FUNCTION,
        _ => SymbolKind::VARIABLE,
    }
}

fn record_fields(snapshot: &Snapshot, form: &Node) -> Vec<DocumentSymbol> {
    // (define-record-type <type> (ctor ...) pred (field accessor [modifier]) ...)
    form.data()
        .skip(4)
        .filter(|field| field.kind == Kind::List)
        .filter_map(|field| {
            let name = field.data().next()?;
            (name.kind == Kind::Symbol)
                .then(|| symbol(snapshot, name, field, SymbolKind::FIELD, None, Vec::new()))
        })
        .collect()
}

#[allow(deprecated)] // DocumentSymbol::deprecated must be initialized.
fn symbol(
    snapshot: &Snapshot,
    name: &Node,
    form: &Node,
    kind: SymbolKind,
    detail: Option<String>,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    DocumentSymbol {
        name: name.text(&snapshot.text).to_string(),
        detail,
        kind,
        tags: None,
        deprecated: None,
        range: snapshot.range(form.range.clone()),
        selection_range: snapshot.range(name.range.clone()),
        children: (!children.is_empty()).then_some(children),
    }
}

fn collect_symbols(snapshot: &Snapshot, forms: &[&Node], out: &mut Vec<DocumentSymbol>) {
    for form in forms {
        if form.kind != Kind::List {
            continue;
        }
        let mut data = form.data();
        let (Some(head), Some(target)) = (data.next(), data.next()) else {
            continue;
        };
        let head_text = head.text(&snapshot.text);
        let target_name = if head.kind == Kind::Symbol && is_definer(head_text) {
            match head_text {
                // (define-module (app main) ...) names the module by its list.
                "define-module" if target.kind == Kind::List => Some(target),
                _ => definition_name(target),
            }
        } else {
            None
        };
        match target_name {
            Some(name) => {
                let kind = symbol_kind(head_text, target);
                let children = if kind == SymbolKind::STRUCT {
                    record_fields(snapshot, form)
                } else {
                    let body: Vec<&Node> = data.collect();
                    let mut children = Vec::new();
                    collect_symbols(snapshot, &body, &mut children);
                    children
                };
                let detail = (kind != SymbolKind::MODULE).then(|| head_text.to_string());
                out.push(symbol(snapshot, name, form, kind, detail, children));
            }
            None => {
                // Not a definition: definitions nested in `begin`, `let`, and
                // similar bodies still surface, one level up.
                let inner: Vec<&Node> = form.data().collect();
                collect_symbols(snapshot, &inner, out);
            }
        }
    }
}

pub fn document_symbols(snapshot: &Snapshot) -> Vec<DocumentSymbol> {
    let forms: Vec<&Node> = snapshot.tree.root.data().collect();
    let mut out = Vec::new();
    collect_symbols(snapshot, &forms, &mut out);
    out
}

/// Ancestors of the node at `offset`, innermost first, excluding the root.
pub fn path_to(root: &Node, offset: usize) -> Vec<&Node> {
    let mut path = Vec::new();
    let mut node = root;
    while let Some(child) = node
        .children
        .iter()
        .find(|child| child.range.start <= offset && offset < child.range.end)
    {
        path.push(child);
        node = child;
    }
    path.reverse();
    path
}

pub fn selection_range(snapshot: &Snapshot, position: Position) -> Option<SelectionRange> {
    let offset = snapshot.offset(position)?;
    let offset = offset.min(snapshot.text.len().saturating_sub(1));
    let mut chain = None;
    for node in path_to(&snapshot.tree.root, offset).into_iter().rev() {
        // Delimiters and whitespace are not useful selections on their own.
        if matches!(
            node.kind,
            Kind::Open | Kind::Close | Kind::Whitespace | Kind::Prefix
        ) {
            continue;
        }
        chain = Some(SelectionRange {
            range: snapshot.range(node.range.clone()),
            parent: chain.map(Box::new),
        });
    }
    chain.or_else(|| {
        Some(SelectionRange {
            range: snapshot.range(0..snapshot.text.len()),
            parent: None,
        })
    })
}

/// The symbol token being typed at `offset`, if any.
fn symbol_prefix(snapshot: &Snapshot, offset: usize) -> Option<(&Node, &str)> {
    let path = path_to(&snapshot.tree.root, offset.saturating_sub(1));
    let node = path.first()?;
    (node.kind == Kind::Symbol && node.range.start < offset).then(|| {
        let end = offset.min(node.range.end);
        (*node, &snapshot.text[node.range.start..end])
    })
}

pub fn completions(snapshot: &Snapshot, position: Position) -> Vec<CompletionItem> {
    let Some(offset) = snapshot.offset(position) else {
        return Vec::new();
    };
    let (current, prefix) = match symbol_prefix(snapshot, offset) {
        Some((node, prefix)) => (Some(node.range.clone()), prefix),
        None => (None, ""),
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut items = Vec::new();
    for form in SPECIAL_FORMS {
        if form.starts_with(prefix) && seen.insert(*form) {
            items.push(CompletionItem {
                label: form.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..Default::default()
            });
        }
    }
    for node in snapshot.tree.root.descendants() {
        if node.kind != Kind::Symbol || Some(&node.range) == current.as_ref() {
            continue;
        }
        let text = node.text(&snapshot.text);
        if text.starts_with(prefix) && text != prefix && seen.insert(text) {
            items.push(CompletionItem {
                label: text.to_string(),
                kind: Some(CompletionItemKind::TEXT),
                detail: Some("in this file".into()),
                ..Default::default()
            });
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::Documents;
    use tower_lsp::lsp_types::{TextDocumentItem, Url};

    fn snapshot(text: &str) -> Snapshot {
        Documents::default().open(TextDocumentItem {
            uri: Url::parse("file:///t.scm").unwrap(),
            language_id: "scheme".into(),
            version: 1,
            text: text.into(),
        })
    }

    fn fixture(path: &str) -> Snapshot {
        snapshot(
            &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/").to_string() + path)
                .unwrap(),
        )
    }

    fn names(symbols: &[DocumentSymbol]) -> Vec<(String, SymbolKind)> {
        symbols.iter().map(|s| (s.name.clone(), s.kind)).collect()
    }

    #[test]
    fn diagnostics_map_reader_errors_to_utf16_ranges() {
        let snapshot = snapshot("(define λ \"🦀");
        let diagnostics = diagnostics(&snapshot);
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(
            diagnostics[0].message,
            "unclosed form; expected a closing delimiter"
        );
        assert_eq!(
            diagnostics[0].range,
            Range::new(Position::new(0, 0), Position::new(0, 1))
        );
        assert_eq!(diagnostics[1].message, "unterminated string");
        assert_eq!(
            diagnostics[1].range,
            Range::new(Position::new(0, 10), Position::new(0, 13))
        );
    }

    #[test]
    fn document_symbols_cover_definition_styles() {
        let snapshot = fixture("fixtures/guile/lambda-star.scm");
        let symbols = document_symbols(&snapshot);
        assert_eq!(
            names(&symbols),
            vec![
                ("(lambda-star)".to_string(), SymbolKind::MODULE),
                ("connect".to_string(), SymbolKind::FUNCTION),
                ("send-all".to_string(), SymbolKind::FUNCTION),
                ("curried".to_string(), SymbolKind::FUNCTION),
            ]
        );
        assert_eq!(symbols[1].detail.as_deref(), Some("define*"));
        assert_eq!(symbols[1].selection_range.start, Position::new(5, 10));

        let records = document_symbols(&fixture("fixtures/guile/records.scm"));
        let point = records.iter().find(|s| s.name == "<point>").unwrap();
        assert_eq!(point.kind, SymbolKind::STRUCT);
        assert_eq!(
            names(point.children.as_ref().unwrap()),
            vec![
                ("x".to_string(), SymbolKind::FIELD),
                ("y".to_string(), SymbolKind::FIELD)
            ]
        );

        let macros = document_symbols(&fixture("fixtures/guile/macros.scm"));
        assert_eq!(
            names(&macros)[1..],
            [
                ("swap!".to_string(), SymbolKind::OPERATOR),
                ("unless*".to_string(), SymbolKind::OPERATOR),
                ("while".to_string(), SymbolKind::OPERATOR)
            ]
        );

        let shadowing = document_symbols(&fixture("fixtures/guile/shadowing.scm"));
        let outer = shadowing.iter().find(|s| s.name == "outer").unwrap();
        assert_eq!(
            names(outer.children.as_ref().unwrap()),
            vec![("inner".to_string(), SymbolKind::FUNCTION)]
        );
    }

    #[test]
    fn symbols_survive_incomplete_input_and_guix_forms() {
        let symbols = document_symbols(&fixture("fixtures/guix/my/services.scm"));
        let found = names(&symbols);
        assert!(found.contains(&("my-hello-configuration".to_string(), SymbolKind::VARIABLE)));
        assert!(found.contains(&("my-hello-service-type".to_string(), SymbolKind::VARIABLE)));
        assert!(found.contains(&("<my-mirror>".to_string(), SymbolKind::VARIABLE)));
        let incomplete = document_symbols(&fixture("fixtures/guile/incomplete.scm"));
        assert_eq!(
            names(&incomplete),
            vec![
                ("(incomplete)".to_string(), SymbolKind::MODULE),
                ("partial".to_string(), SymbolKind::FUNCTION)
            ]
        );
    }

    #[test]
    fn selection_ranges_grow_outward_from_the_token() {
        let snapshot = snapshot("(a (b \"λ🦀\" c))");
        let crab = Position::new(0, 8);
        let selection = selection_range(&snapshot, crab).unwrap();
        let mut ranges = vec![selection.range];
        let mut parent = selection.parent;
        while let Some(p) = parent {
            ranges.push(p.range);
            parent = p.parent;
        }
        assert_eq!(
            ranges,
            vec![
                Range::new(Position::new(0, 6), Position::new(0, 11)),
                Range::new(Position::new(0, 3), Position::new(0, 14)),
                Range::new(Position::new(0, 0), Position::new(0, 15)),
            ]
        );
        // On whitespace between forms, the selection starts at the enclosing list.
        let outer = selection_range(&snapshot, Position::new(0, 2)).unwrap();
        assert_eq!(
            outer.range,
            Range::new(Position::new(0, 0), Position::new(0, 15))
        );
        assert!(outer.parent.is_none());
    }

    #[test]
    fn completion_offers_special_forms_and_file_symbols_by_prefix() {
        let snapshot = snapshot("(define (double-it x) (* x 2))\n(defi");
        let items = completions(&snapshot, Position::new(1, 5));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "define",
                "define*",
                "define-module",
                "define-public",
                "define-record-type",
                "define-syntax",
                "define-syntax-rule",
                "define-values"
            ]
        );
        let items = completions(&snapshot, Position::new(1, 2));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"double-it"));
        assert!(labels.contains(&"delay"));
        assert!(
            !labels.contains(&"defi"),
            "the token being typed is not offered"
        );
        assert!(completions(&snapshot, Position::new(9, 0)).is_empty());
    }
}
