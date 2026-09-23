//! Completion assembly: lexical names, imported names from worker export
//! listings, module paths inside import forms, and the special-form list.
//! Everything here is pure; `main.rs` supplies the worker data.

use crate::documents::Snapshot;
use crate::scope::{self, BindingKind, Import};
use crate::syntax::datum::Datum;
use crate::syntax::{Kind, Node};
use std::collections::BTreeSet;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, MarkupContent, MarkupKind,
};

/// Core forms offered even before any worker metadata exists.
pub const SPECIAL_FORMS: &[&str] = &[
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

/// What the cursor is completing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Context {
    /// Inside quoted data: nothing to complete.
    Quoted,
    /// Inside a module spec of `use-modules` or `#:use-module`: the
    /// components typed before the cursor, and the partial last one.
    ModulePath { typed: Vec<String>, prefix: String },
    /// An ordinary reference position with the partial symbol.
    Reference { prefix: String },
}

fn ancestors(root: &Node, offset: usize) -> Vec<&Node> {
    let mut path = Vec::new();
    let mut node = root;
    while let Some(child) = node
        .children
        .iter()
        .find(|c| c.range.start <= offset && offset < c.range.end)
    {
        path.push(child);
        node = child;
    }
    path.reverse();
    path
}

/// The symbol token ending at or containing `offset`, with the text before
/// the cursor.
fn partial_symbol(snapshot: &Snapshot, offset: usize) -> Option<(&Node, String)> {
    let at = offset.checked_sub(1)?;
    let node = snapshot.tree.root.node_at(at);
    (node.kind == Kind::Symbol && node.range.start < offset).then(|| {
        let end = offset.min(node.range.end);
        (node, snapshot.text[node.range.start..end].to_string())
    })
}

pub fn context(snapshot: &Snapshot, offset: usize) -> Context {
    let source = &snapshot.text;
    let (token, prefix) = match partial_symbol(snapshot, offset) {
        Some((node, prefix)) => (Some(node), prefix),
        None => (None, String::new()),
    };
    let probe = token
        .map(|t| t.range.start)
        .unwrap_or(offset.saturating_sub(1));
    let chain = ancestors(
        &snapshot.tree.root,
        probe.min(source.len().saturating_sub(1)),
    );
    // A module spec is a list directly under `use-modules`, or the list that
    // follows a `#:use-module` keyword under `define-module`. A spec with
    // modifiers keeps the name in its first sub-list.
    let lists: Vec<&Node> = chain
        .iter()
        .copied()
        .filter(|n| n.kind == Kind::List)
        .collect();
    if let (Some(list), Some(parent)) = (lists.first(), lists.get(1)) {
        let head_of = |node: &Node| {
            node.data()
                .next()
                .filter(|h| h.kind == Kind::Symbol)
                .map(|h| h.text(source).to_string())
        };
        let is_spec = match head_of(parent).as_deref() {
            Some("use-modules") => true,
            Some("define-module") => {
                let mut previous: Option<&Node> = None;
                let mut found = false;
                for child in parent.data() {
                    if std::ptr::eq(child, *list) {
                        found = previous.is_some_and(|p| {
                            p.kind == Kind::Keyword && p.text(source) == "#:use-module"
                        });
                        break;
                    }
                    previous = Some(child);
                }
                found
            }
            // `((a b) #:select ...)`: the name list inside a spec.
            _ => {
                let first_in_parent = parent.data().next().is_some_and(|f| std::ptr::eq(f, *list));
                let grand_head = lists.get(2).and_then(|g| head_of(g));
                first_in_parent
                    && matches!(
                        grand_head.as_deref(),
                        Some("use-modules") | Some("define-module")
                    )
            }
        };
        if is_spec
            && list
                .data()
                .next()
                .is_none_or(|first| first.kind == Kind::Symbol)
        {
            let typed = list
                .data()
                .take_while(|n| n.range.end <= probe && Some(*n) != token)
                .filter(|n| n.kind == Kind::Symbol)
                .map(|n| n.text(source).to_string())
                .collect();
            return Context::ModulePath { typed, prefix };
        }
    }
    if scope::in_quoted_data(snapshot, probe) {
        return Context::Quoted;
    }
    Context::Reference { prefix }
}

fn item(
    label: String,
    kind: CompletionItemKind,
    detail: Option<String>,
    doc: Option<String>,
) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(kind),
        detail,
        documentation: doc.map(|value| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            })
        }),
        ..Default::default()
    }
}

/// Lexical and file-level names, innermost first, deduplicated.
pub fn lexical_items(
    snapshot: &Snapshot,
    offset: usize,
    prefix: &str,
    seen: &mut BTreeSet<String>,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for binding in scope::visible_bindings(snapshot, offset).into_iter().rev() {
        if !binding.name.starts_with(prefix)
            || binding.name == prefix
            || !seen.insert(binding.name.clone())
        {
            continue;
        }
        let (kind, detail) = match binding.kind {
            BindingKind::Procedure => (CompletionItemKind::FUNCTION, "defined in this file"),
            BindingKind::Variable => (CompletionItemKind::VARIABLE, "defined in this file"),
            BindingKind::Local => (CompletionItemKind::VARIABLE, "local binding"),
            BindingKind::Syntax => (CompletionItemKind::KEYWORD, "syntax in this file"),
        };
        items.push(item(binding.name, kind, Some(detail.into()), None));
    }
    items
}

pub fn special_form_items(prefix: &str, seen: &mut BTreeSet<String>) -> Vec<CompletionItem> {
    SPECIAL_FORMS
        .iter()
        .filter(|f| f.starts_with(prefix) && seen.insert(f.to_string()))
        .map(|f| item(f.to_string(), CompletionItemKind::KEYWORD, None, None))
        .collect()
}

/// Items for one import from the worker's `exports` result, applying the
/// import's modifiers.
pub fn imported_items(
    import: &Import,
    exports: &Datum,
    prefix: &str,
    seen: &mut BTreeSet<String>,
) -> Vec<CompletionItem> {
    let module = import.module_datum().to_string();
    let Some(Datum::List(bindings)) = exports.assq("bindings") else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for binding in bindings.iter().skip(1) {
        let Some(name) = binding.assq("name").and_then(Datum::as_symbol) else {
            continue;
        };
        let Some(local) = import.local_name(name) else {
            continue;
        };
        if !local.starts_with(prefix) || !seen.insert(local.clone()) {
            continue;
        }
        let kind = match binding.assq("kind").and_then(Datum::as_symbol) {
            Some("procedure") => CompletionItemKind::FUNCTION,
            Some("macro") => CompletionItemKind::KEYWORD,
            Some("record-type") => CompletionItemKind::STRUCT,
            _ => CompletionItemKind::VARIABLE,
        };
        let doc = binding
            .assq("documentation")
            .and_then(Datum::as_str)
            .map(|d| d.lines().next().unwrap_or("").to_string());
        items.push(item(local, kind, Some(module.clone()), doc));
    }
    items
}

/// Next module-path components after `typed`, from the worker's module list.
pub fn module_path_items(modules: &Datum, typed: &[String], prefix: &str) -> Vec<CompletionItem> {
    let Some(Datum::List(names)) = modules.assq("modules") else {
        return Vec::new();
    };
    // A component can be both a module and a prefix of longer ones
    // (`srfi-9` and `srfi-9 gnu`): report it as a module then.
    let mut next_components: std::collections::BTreeMap<String, (bool, bool)> = Default::default();
    for name in names.iter().skip(1) {
        let Some(components) = name.as_list() else {
            continue;
        };
        let components: Vec<&str> = components.iter().filter_map(Datum::as_symbol).collect();
        if components.len() <= typed.len() || !components.iter().zip(typed).all(|(c, t)| c == t) {
            continue;
        }
        let next = components[typed.len()];
        if !next.starts_with(prefix) {
            continue;
        }
        let entry = next_components
            .entry(next.to_string())
            .or_insert((false, false));
        if components.len() == typed.len() + 1 {
            entry.0 = true;
        } else {
            entry.1 = true;
        }
    }
    next_components
        .into_iter()
        .map(|(next, (complete, deeper))| {
            let full = typed
                .iter()
                .map(String::as_str)
                .chain([next.as_str()])
                .collect::<Vec<_>>()
                .join(" ");
            let detail = match (complete, deeper) {
                (true, true) => format!("({full}) and more modules under it"),
                (true, false) => format!("({full})"),
                _ => format!("{full} modules…"),
            };
            item(
                next,
                if complete {
                    CompletionItemKind::MODULE
                } else {
                    CompletionItemKind::FOLDER
                },
                Some(detail),
                None,
            )
        })
        .collect()
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

    #[test]
    fn detects_module_path_and_quoted_contexts() {
        let text = "(use-modules (srfi srf) ((ice-9 ma) #:select (x)))\n(define-module (m) #:use-module (gnu pa))\n(list 'quo (fo";
        let snap = snapshot(text);
        let at = |needle: &str| text.find(needle).unwrap() + needle.len();
        assert_eq!(
            context(&snap, at("srf)") - 1),
            Context::ModulePath {
                typed: vec!["srfi".into()],
                prefix: "srf".into()
            }
        );
        assert_eq!(
            context(&snap, at("ice-9 ma")),
            Context::ModulePath {
                typed: vec!["ice-9".into()],
                prefix: "ma".into()
            }
        );
        assert_eq!(
            context(&snap, at("gnu pa")),
            Context::ModulePath {
                typed: vec!["gnu".into()],
                prefix: "pa".into()
            }
        );
        assert_eq!(
            context(&snap, at("(srfi ")),
            Context::ModulePath {
                typed: vec!["srfi".into()],
                prefix: String::new()
            }
        );
        assert_eq!(context(&snap, at("'quo")), Context::Quoted);
        assert_eq!(
            context(&snap, at("(fo")),
            Context::Reference {
                prefix: "fo".into()
            }
        );
        assert_eq!(
            context(&snap, at("(list ")),
            Context::Reference {
                prefix: String::new()
            }
        );
    }

    #[test]
    fn imported_and_module_items_apply_modifiers() {
        let import = Import {
            module: vec!["srfi".into(), "srfi-1".into()],
            select: None,
            hide: vec!["fold".into()],
            prefix: Some("s:".into()),
        };
        let exports = Datum::parse_one("((module srfi srfi-1) (bindings ((name . fold) (kind . procedure)) ((name . filter) (kind . procedure) (documentation . \"Keep some.\\nMore.\")) ((name . first) (kind . macro))))").unwrap();
        let mut seen = BTreeSet::new();
        let items = imported_items(&import, &exports, "s:f", &mut seen);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["s:filter", "s:first"]);
        assert_eq!(items[0].detail.as_deref(), Some("(srfi srfi-1)"));
        assert!(
            matches!(&items[0].documentation, Some(Documentation::MarkupContent(m)) if m.value == "Keep some.")
        );
        assert_eq!(items[1].kind, Some(CompletionItemKind::KEYWORD));

        let modules = Datum::parse_one(
            "((modules (srfi srfi-1) (srfi srfi-9 gnu) (srfi srfi-9) (ice-9 match)))",
        )
        .unwrap();
        let items = module_path_items(&modules, &["srfi".into()], "srfi-");
        let labels: Vec<(String, Option<CompletionItemKind>)> =
            items.iter().map(|i| (i.label.clone(), i.kind)).collect();
        assert_eq!(
            labels,
            [
                ("srfi-1".to_string(), Some(CompletionItemKind::MODULE)),
                ("srfi-9".to_string(), Some(CompletionItemKind::MODULE))
            ]
        );
        assert_eq!(
            items[1].detail.as_deref(),
            Some("(srfi srfi-9) and more modules under it")
        );
        let top = module_path_items(&modules, &[], "");
        assert_eq!(
            top.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            ["ice-9", "srfi"]
        );
        assert_eq!(top[0].kind, Some(CompletionItemKind::FOLDER));
    }
}
