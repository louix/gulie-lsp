//! Syntax-level scope analysis: which names a position can see, whether it
//! sits in quoted data, and what the document imports, with modifiers.
//!
//! This is deliberately conservative. It knows the core binding forms and
//! treats anything else as an ordinary application. Macros that bind names
//! in other ways are invisible until the worker can expand them.

use crate::documents::Snapshot;
use crate::syntax::datum::Datum;
use crate::syntax::{Kind, Node};
use tower_lsp::lsp_types::Position;

/// A name bound by syntax at some point in the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub name: String,
    pub kind: BindingKind,
    /// Byte range of the binding occurrence (the name in the binder).
    pub range: std::ops::Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    Procedure,
    Variable,
    /// Lambda parameter or `let`-family binding.
    Local,
    /// `define-syntax` and friends.
    Syntax,
}

/// How a document imports a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub module: Vec<String>,
    /// `#:select` entries as (imported name, local name); `None` imports all.
    pub select: Option<Vec<(String, String)>>,
    pub hide: Vec<String>,
    pub prefix: Option<String>,
}

impl Import {
    pub fn module_datum(&self) -> Datum {
        Datum::List(self.module.iter().map(|s| Datum::symbol(s)).collect())
    }

    /// The local name an exported `name` gets in this document, if visible.
    pub fn local_name(&self, name: &str) -> Option<String> {
        if self.hide.iter().any(|h| h == name) {
            return None;
        }
        let renamed = match &self.select {
            Some(entries) => entries.iter().find(|(from, _)| from == name)?.1.clone(),
            None => name.to_string(),
        };
        Some(match &self.prefix {
            Some(prefix) => format!("{prefix}{renamed}"),
            None => renamed,
        })
    }

    /// The exported name behind a local `name` in this document, if this
    /// import could have produced it.
    pub fn exported_name(&self, local: &str) -> Option<String> {
        let unprefixed = match &self.prefix {
            Some(prefix) => local.strip_prefix(prefix.as_str())?,
            None => local,
        };
        let original = match &self.select {
            Some(entries) => entries.iter().find(|(_, to)| to == unprefixed)?.0.clone(),
            None => unprefixed.to_string(),
        };
        (!self.hide.iter().any(|h| h == &original)).then_some(original)
    }
}

/// The document's imports in order, followed by an implicit `(guile)`.
pub fn imports(snapshot: &Snapshot) -> Vec<Import> {
    let source = &snapshot.text;
    let mut out: Vec<Import> = Vec::new();
    let mut add = |import: Option<Import>| {
        if let Some(import) = import {
            if !out.contains(&import) {
                out.push(import);
            }
        }
    };
    for form in snapshot.tree.root.data() {
        let items: Vec<&Node> = form.data().collect();
        let Some(head) = items.first().filter(|n| n.kind == Kind::Symbol) else {
            continue;
        };
        match head.text(source) {
            "define-module" => {
                let mut rest = items.iter().skip(2);
                while let Some(node) = rest.next() {
                    if node.kind == Kind::Keyword && node.text(source) == "#:use-module" {
                        if let Some(spec) = rest.next() {
                            add(parse_import(spec, source));
                        }
                    }
                }
            }
            "use-modules" => {
                for spec in &items[1..] {
                    add(parse_import(spec, source));
                }
            }
            _ => {}
        }
    }
    add(Some(Import {
        module: vec!["guile".into()],
        select: None,
        hide: Vec::new(),
        prefix: None,
    }));
    out
}

fn symbols(node: &Node, source: &str) -> Option<Vec<String>> {
    node.data()
        .map(|n| (n.kind == Kind::Symbol).then(|| n.text(source).to_string()))
        .collect()
}

/// `(a b)` or `((a b) #:select (x (y . z)) #:hide (w) #:prefix p:)`.
fn parse_import(spec: &Node, source: &str) -> Option<Import> {
    if spec.kind != Kind::List {
        return None;
    }
    let first = spec.data().next()?;
    if first.kind != Kind::List {
        return Some(Import {
            module: symbols(spec, source)?,
            select: None,
            hide: Vec::new(),
            prefix: None,
        });
    }
    let mut import = Import {
        module: symbols(first, source)?,
        select: None,
        hide: Vec::new(),
        prefix: None,
    };
    let mut rest = spec.data().skip(1);
    while let Some(key) = rest.next() {
        if key.kind != Kind::Keyword {
            continue;
        }
        let Some(value) = rest.next() else { break };
        match key.text(source) {
            "#:select" => {
                import.select = Some(
                    value
                        .data()
                        .filter_map(|entry| match entry.kind {
                            Kind::Symbol => {
                                let name = entry.text(source).to_string();
                                Some((name.clone(), name))
                            }
                            Kind::List => {
                                let parts: Vec<&Node> = entry.data().collect();
                                // (from . to)
                                match parts.as_slice() {
                                    [from, dot, to] if dot.text(source) == "." => Some((
                                        from.text(source).to_string(),
                                        to.text(source).to_string(),
                                    )),
                                    _ => None,
                                }
                            }
                            _ => None,
                        })
                        .collect(),
                );
            }
            "#:hide" => import.hide = symbols(value, source).unwrap_or_default(),
            "#:prefix" => {
                if value.kind == Kind::Symbol {
                    import.prefix = Some(value.text(source).to_string());
                }
            }
            _ => {}
        }
    }
    Some(import)
}

/// Ancestors of the innermost node at `offset`, innermost first.
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

/// Whether `offset` lies inside quoted data (quote or quasiquote without an
/// enclosing unquote), where symbols are data rather than references.
pub fn in_quoted_data(snapshot: &Snapshot, offset: usize) -> bool {
    let source = &snapshot.text;
    for node in ancestors(&snapshot.tree.root, offset) {
        if node.kind != Kind::Prefixed {
            continue;
        }
        match node.children.first().map(|p| p.text(source)) {
            Some(",") | Some(",@") | Some("#$") | Some("#$@") | Some("#+") | Some("#+@") => {
                return false
            }
            Some("'") | Some("`") | Some("#~") | Some("#'") | Some("#`") => return true,
            _ => {}
        }
    }
    false
}

fn head_text<'a>(form: &'a Node, source: &'a str) -> Option<&'a str> {
    let head = form.data().next()?;
    (head.kind == Kind::Symbol).then(|| head.text(source))
}

/// Names bound by a definition target: `name`, `(name args...)`, `((name a) b)`.
fn definition_bindings(target: &Node, source: &str, out: &mut Vec<Binding>) {
    match target.kind {
        Kind::Symbol => out.push(Binding {
            name: target.text(source).to_string(),
            kind: BindingKind::Variable,
            range: target.range.clone(),
        }),
        Kind::List => {
            let mut items = target.data();
            if let Some(name) = items.next() {
                if name.kind == Kind::List {
                    definition_bindings(name, source, out);
                } else if name.kind == Kind::Symbol {
                    out.push(Binding {
                        name: name.text(source).to_string(),
                        kind: BindingKind::Procedure,
                        range: name.range.clone(),
                    });
                }
            }
        }
        _ => {}
    }
}

/// Parameters of a lambda list: symbols, skipping `#:optional`-style
/// markers and reading `(name default)` entries.
fn parameter_bindings(params: &Node, source: &str, out: &mut Vec<Binding>) {
    let push = |node: &Node, out: &mut Vec<Binding>| {
        if node.kind == Kind::Symbol && node.text(source) != "." {
            out.push(Binding {
                name: node.text(source).to_string(),
                kind: BindingKind::Local,
                range: node.range.clone(),
            });
        }
    };
    match params.kind {
        Kind::Symbol => push(params, out),
        Kind::List => {
            for item in params.data() {
                match item.kind {
                    Kind::Symbol => push(item, out),
                    Kind::List => {
                        if let Some(name) = item.data().next() {
                            push(name, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Definitions directly inside `forms` (a body), in order.
fn body_definitions<'a>(
    forms: impl Iterator<Item = &'a Node>,
    source: &str,
    out: &mut Vec<Binding>,
) {
    for form in forms {
        if form.kind != Kind::List {
            continue;
        }
        let Some(head) = head_text(form, source) else {
            continue;
        };
        let mut data = form.data().skip(1);
        match head {
            "define" | "define*" | "define-public" | "define-once" | "define-inlinable" => {
                if let Some(target) = data.next() {
                    definition_bindings(target, source, out);
                }
            }
            "define-syntax" | "define-syntax-rule" | "define-macro" => {
                if let Some(target) = data.next() {
                    let name = if target.kind == Kind::List {
                        target.data().next()
                    } else {
                        Some(target)
                    };
                    if let Some(name) = name.filter(|n| n.kind == Kind::Symbol) {
                        out.push(Binding {
                            name: name.text(source).to_string(),
                            kind: BindingKind::Syntax,
                            range: name.range.clone(),
                        });
                    }
                }
            }
            "define-values" => {
                if let Some(target) = data.next() {
                    parameter_bindings(target, source, out);
                }
            }
            "define-record-type" | "define-immutable-record-type" => {
                // (define-record-type <t> (ctor f...) pred (field accessor [modifier]) ...)
                let items: Vec<&Node> = form.data().collect();
                if let Some(ctor) = items.get(2) {
                    if let Some(name) = ctor.data().next() {
                        out.push(Binding {
                            name: name.text(source).into(),
                            kind: BindingKind::Procedure,
                            range: name.range.clone(),
                        });
                    }
                }
                if let Some(pred) = items.get(3).filter(|n| n.kind == Kind::Symbol) {
                    out.push(Binding {
                        name: pred.text(source).into(),
                        kind: BindingKind::Procedure,
                        range: pred.range.clone(),
                    });
                }
                for field in items.iter().skip(4) {
                    for accessor in field.data().skip(1).filter(|n| n.kind == Kind::Symbol) {
                        out.push(Binding {
                            name: accessor.text(source).into(),
                            kind: BindingKind::Procedure,
                            range: accessor.range.clone(),
                        });
                    }
                }
            }
            "begin" => body_definitions(form.data().skip(1), source, out),
            _ => {}
        }
    }
}

/// Every name visible at `offset`: top-level definitions, then the bindings
/// introduced by each enclosing form, innermost last. Later entries shadow
/// earlier ones with the same name.
pub fn visible_bindings(snapshot: &Snapshot, offset: usize) -> Vec<Binding> {
    let source = &snapshot.text;
    // A cursor at the very end lies past every node; treat it as sitting on
    // the last character so the enclosing forms are found.
    let offset = offset.min(source.len().saturating_sub(1));
    let mut out = Vec::new();
    body_definitions(snapshot.tree.root.data(), source, &mut out);
    let chain = ancestors(&snapshot.tree.root, offset);
    // Outermost first so inner bindings land later and shadow.
    for (depth, form) in chain.iter().rev().enumerate() {
        if form.kind != Kind::List {
            continue;
        }
        let Some(head) = head_text(form, source) else {
            continue;
        };
        let items: Vec<&Node> = form.data().collect();
        // The child of this form on the path to the cursor, to know which
        // part of the form the cursor is in.
        let inner = chain.iter().rev().nth(depth + 1).copied();
        match head {
            "lambda" | "lambda*" => {
                if let Some(params) = items.get(1) {
                    parameter_bindings(params, source, &mut out);
                }
                body_definitions(items.iter().skip(2).copied(), source, &mut out);
            }
            "define" | "define*" | "define-public" | "define-inlinable" => {
                if let Some(target) = items.get(1).filter(|t| t.kind == Kind::List) {
                    // Curried: every level's parameters are in scope in the body.
                    let mut level = *target;
                    loop {
                        for param in level.data().skip(1) {
                            parameter_bindings(param, source, &mut out);
                        }
                        match level.data().next() {
                            Some(next) if next.kind == Kind::List => level = next,
                            _ => break,
                        }
                    }
                    body_definitions(items.iter().skip(2).copied(), source, &mut out);
                }
            }
            "let" | "let*" | "letrec" | "letrec*" | "let-syntax" | "letrec-syntax" => {
                let mut index = 1;
                if let Some(name) = items.get(1).filter(|n| n.kind == Kind::Symbol) {
                    // Named let.
                    out.push(Binding {
                        name: name.text(source).into(),
                        kind: BindingKind::Procedure,
                        range: name.range.clone(),
                    });
                    index = 2;
                }
                if let Some(bindings) = items.get(index) {
                    let in_bindings = inner.is_some_and(|n| std::ptr::eq(n, *bindings));
                    for entry in bindings.data() {
                        // Inside the binding list: `let` inits see none of the
                        // bindings, `let*` inits see the earlier ones, and the
                        // letrec family sees all of them.
                        if in_bindings && head != "letrec" && head != "letrec*" {
                            let earlier = entry.range.end <= offset;
                            if head == "let" || !earlier {
                                continue;
                            }
                        }
                        let name = if entry.kind == Kind::List {
                            entry.data().next()
                        } else {
                            Some(entry)
                        };
                        if let Some(name) = name.filter(|n| n.kind == Kind::Symbol) {
                            out.push(Binding {
                                name: name.text(source).into(),
                                kind: BindingKind::Local,
                                range: name.range.clone(),
                            });
                        }
                    }
                }
                body_definitions(items.iter().skip(index + 1).copied(), source, &mut out);
            }
            "let-values" | "let*-values" | "receive" | "do" => {
                if let Some(bindings) = items.get(1) {
                    if head == "receive" {
                        parameter_bindings(bindings, source, &mut out);
                    } else {
                        for entry in bindings.data() {
                            if let Some(formals) = entry.data().next() {
                                parameter_bindings(formals, source, &mut out);
                            }
                        }
                    }
                }
            }
            "match" | "match-lambda" => {} // patterns bind names; left for expansion
            _ => {}
        }
    }
    out
}

pub fn resolve_lexical(snapshot: &Snapshot, offset: usize, name: &str) -> Option<Binding> {
    visible_bindings(snapshot, offset)
        .into_iter()
        .rev()
        .find(|b| b.name == name)
}

pub fn offset_of(snapshot: &Snapshot, position: Position) -> Option<usize> {
    snapshot.offset(position)
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

    fn names(bindings: &[Binding]) -> Vec<&str> {
        bindings.iter().map(|b| b.name.as_str()).collect()
    }

    #[test]
    fn imports_carry_modifiers() {
        let snap = snapshot(
            "(define-module (app main)\n  #:use-module ((app lib util) #:prefix util:)\n  #:use-module ((srfi srfi-1) #:select (fold (filter . keep)))\n  #:use-module ((ice-9 match) #:hide (match-lambda)))\n(use-modules (rnrs))",
        );
        let imports = imports(&snap);
        assert_eq!(imports.len(), 5);
        assert_eq!(imports[0].prefix.as_deref(), Some("util:"));
        assert_eq!(
            imports[0].local_name("string-length*").as_deref(),
            Some("util:string-length*")
        );
        assert_eq!(
            imports[0].exported_name("util:string-length*").as_deref(),
            Some("string-length*")
        );
        assert_eq!(imports[0].exported_name("string-length*"), None);
        assert_eq!(imports[1].local_name("filter").as_deref(), Some("keep"));
        assert_eq!(imports[1].local_name("map"), None);
        assert_eq!(imports[1].exported_name("keep").as_deref(), Some("filter"));
        assert_eq!(imports[2].local_name("match-lambda"), None);
        assert_eq!(imports[2].local_name("match").as_deref(), Some("match"));
        assert_eq!(imports[4].module, vec!["guile"]);
    }

    #[test]
    fn visible_bindings_follow_lexical_structure() {
        let text = "(define top 1)\n(define (outer x #:optional (y 2) . rest)\n  (let ((a x) (b (+ a 1)))\n    (define (inner z) (* z 2))\n    (let loop ((i 0))\n      (lambda (k) k))))";
        let snap = snapshot(text);
        let at = |needle: &str| text.find(needle).unwrap();
        let at_body_end = at("(lambda (k) k)") + 12; // inside the lambda body, on `k`
        let visible = visible_bindings(&snap, at_body_end);
        let visible = names(&visible);
        for expected in [
            "top", "outer", "x", "y", "rest", "a", "b", "inner", "loop", "i", "k",
        ] {
            assert!(
                visible.contains(&expected),
                "missing {expected} in {visible:?}"
            );
        }
        assert!(
            !visible.contains(&"z"),
            "inner's parameter is not visible outside it"
        );
        // Inside a `let` init, the let's own bindings are not visible.
        let in_init = at("(+ a 1)") + 3;
        let visible = visible_bindings(&snap, in_init);
        assert!(!names(&visible).contains(&"b"), "{:?}", names(&visible));
        assert_eq!(
            resolve_lexical(&snap, at_body_end, "x").map(|b| b.kind),
            Some(BindingKind::Local)
        );
        assert_eq!(
            resolve_lexical(&snap, at_body_end, "outer").map(|b| b.kind),
            Some(BindingKind::Procedure)
        );
        assert_eq!(resolve_lexical(&snap, at_body_end, "nope"), None);
    }

    #[test]
    fn quoted_data_is_recognized() {
        let text = "(list 'a `(b ,c ,@(d e) f) #~(g #$h) \"s\")";
        let snap = snapshot(text);
        let at = |needle: &str| text.find(needle).unwrap();
        assert!(in_quoted_data(&snap, at("a")));
        assert!(in_quoted_data(&snap, at("b")));
        assert!(!in_quoted_data(&snap, at("c")));
        assert!(!in_quoted_data(&snap, at("d e") + 1));
        assert!(in_quoted_data(&snap, at("f)")));
        assert!(in_quoted_data(&snap, at("g")));
        assert!(!in_quoted_data(&snap, at("h")));
        assert!(!in_quoted_data(&snap, at("list")));
    }
}
