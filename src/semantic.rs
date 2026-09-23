//! Bridges between the syntax tree and the worker: what to ask, and how to
//! present what comes back. Nothing here talks to the worker itself.

use crate::documents::Snapshot;
use crate::features::document_symbols;
use crate::syntax::datum::Datum;
use crate::syntax::{Kind, Node};
use tower_lsp::lsp_types::{DocumentSymbol, Location, Position, Range, Url};

/// The symbol under or immediately before `position`.
pub fn symbol_at(snapshot: &Snapshot, position: Position) -> Option<(&Node, &str)> {
    let offset = snapshot.offset(position)?;
    let candidates = [offset, offset.saturating_sub(1)];
    candidates.into_iter().find_map(|at| {
        let node = snapshot.tree.root.node_at(at);
        (node.kind == Kind::Symbol && at < snapshot.text.len())
            .then(|| (node, node.text(&snapshot.text)))
    })
}

/// Module names the document imports, from `define-module` and `use-modules`
/// forms, followed by `(guile)`. Own-module definitions are handled from the
/// syntax tree instead, so unsaved buffers never need loading.
pub fn module_context(snapshot: &Snapshot) -> Vec<Datum> {
    let mut modules: Vec<Datum> = Vec::new();
    let mut add = |datum: Datum| {
        if datum
            .as_list()
            .is_some_and(|items| items.iter().all(|i| i.as_symbol().is_some()))
            && !modules.contains(&datum)
        {
            modules.push(datum);
        }
    };
    let source = &snapshot.text;
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
                            if let Some(name) = module_spec_name(spec, source) {
                                add(name);
                            }
                        }
                    }
                }
            }
            "use-modules" => {
                for spec in &items[1..] {
                    if let Some(name) = module_spec_name(spec, source) {
                        add(name);
                    }
                }
            }
            _ => {}
        }
    }
    add(Datum::list([Datum::symbol("guile")]));
    modules
}

/// `(a b)` or `((a b) #:select ...)` -> the module name datum.
fn module_spec_name(spec: &Node, source: &str) -> Option<Datum> {
    if spec.kind != Kind::List {
        return None;
    }
    let first = spec.data().next()?;
    let name_node = if first.kind == Kind::List {
        first
    } else {
        spec
    };
    let names: Option<Vec<Datum>> = name_node
        .data()
        .map(|n| (n.kind == Kind::Symbol).then(|| Datum::symbol(n.text(source))))
        .collect();
    names.map(Datum::List)
}

pub fn local_definition(snapshot: &Snapshot, name: &str) -> Option<DocumentSymbol> {
    fn find(symbols: &[DocumentSymbol], name: &str) -> Option<DocumentSymbol> {
        symbols.iter().find_map(|s| {
            if s.name == name {
                Some(s.clone())
            } else {
                s.children.as_deref().and_then(|c| find(c, name))
            }
        })
    }
    find(&document_symbols(snapshot), name)
}

/// A definition of `name` in another open document whose declared module this
/// document imports. Unsaved edits are visible this way; the worker only
/// sees files on disk.
pub fn definition_in_open_documents<'a>(
    mut documents: impl Iterator<Item = (&'a Url, &'a Snapshot)>,
    snapshot: &Snapshot,
    name: &str,
) -> Option<(Url, DocumentSymbol)> {
    let imports = module_context(snapshot);
    documents.find_map(|(uri, other)| {
        let declared = crate::roots::declared_module(other)?;
        let declared = Datum::List(declared.iter().map(|s| Datum::symbol(s)).collect());
        imports
            .contains(&declared)
            .then(|| local_definition(other, name).map(|d| (uri.clone(), d)))?
    })
}

/// The worker `lookup` parameter sets for a local `name`, honouring import
/// modifiers: a prefixed or renamed import maps `name` back to the exported
/// name in its module. Usually one set; several when imports disagree.
pub fn lookup_params(snapshot: &Snapshot, name: &str) -> Vec<Datum> {
    lookup_params_for(&crate::scope::imports(snapshot), name)
}

/// As [`lookup_params`], for an explicit import list (which may include
/// adapter-provided imports).
pub fn lookup_params_for(imports: &[crate::scope::Import], name: &str) -> Vec<Datum> {
    let mut groups: Vec<(String, Vec<Datum>)> = Vec::new();
    for import in imports {
        let Some(exported) = import.exported_name(name) else {
            continue;
        };
        match groups.iter_mut().find(|(n, _)| *n == exported) {
            Some((_, modules)) => modules.push(import.module_datum()),
            None => groups.push((exported, vec![import.module_datum()])),
        }
    }
    groups
        .into_iter()
        .map(|(exported, modules)| {
            Datum::list([
                Datum::Pair(
                    Box::new(Datum::symbol("name")),
                    Box::new(Datum::symbol(&exported)),
                ),
                Datum::list(std::iter::once(Datum::symbol("modules")).chain(modules)),
            ])
        })
        .collect()
}

pub fn hover_markdown(name: &str, lookup: &Datum) -> String {
    let binding = lookup.assq("binding");
    let module = module_name(lookup);
    let kind = binding
        .and_then(|b| b.assq("kind"))
        .and_then(Datum::as_symbol)
        .unwrap_or("binding");
    let mut out = String::new();
    match kind {
        "procedure" => {
            out.push_str("```scheme\n");
            // A case-lambda shows every clause, one per line.
            let clauses: Vec<&Datum> = binding.map(arity_clauses).unwrap_or_default();
            if clauses.is_empty() {
                out.push_str(&signature(name, binding.and_then(|b| b.assq("arguments"))));
            } else {
                for clause in clauses {
                    out.push_str(&signature(name, Some(clause)));
                    out.push('\n');
                }
                out.pop();
            }
            out.push_str("\n```\n");
        }
        "macro" => out.push_str(&format!("```scheme\n({name} ...)\n```\n*syntax*\n")),
        "record-type" => {
            let fields = binding
                .and_then(|b| b.assq("fields"))
                .map(|f| f.to_string())
                .unwrap_or_default();
            out.push_str(&format!(
                "```scheme\n{name}\n```\n*record type* with fields {fields}\n"
            ));
        }
        _ => {
            let ty = binding
                .and_then(|b| b.assq("type"))
                .map(|t| t.to_string())
                .unwrap_or_default();
            out.push_str(&format!("```scheme\n{name}\n```\n*variable* {ty}\n"));
        }
    }
    if let Some(doc) = binding
        .and_then(|b| b.assq("documentation"))
        .and_then(Datum::as_str)
    {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out.push_str(&format!("\nfrom `{module}`"));
    // A prefixed or renamed import: say what the module calls it.
    if let Some(exported) = binding
        .and_then(|b| b.assq("name"))
        .and_then(Datum::as_symbol)
    {
        if exported != name {
            out.push_str(&format!(", exported as `{exported}`"));
        }
    }
    out
}

/// The argument clauses of a procedure binding: several for a case-lambda,
/// one otherwise, from `arities` when present or `arguments` as a fallback.
/// A single clause comes back bare from `assq`, so clauses are recognised by
/// their leading `required` entry.
pub fn arity_clauses(binding: &Datum) -> Vec<&Datum> {
    fn is_clause(datum: &Datum) -> bool {
        matches!(datum, Datum::List(items) if items.first().and_then(Datum::as_list).and_then(|e| e.first()).and_then(Datum::as_symbol) == Some("required")
            || items.first().is_some_and(|e| matches!(e, Datum::Pair(k, _) if k.as_symbol() == Some("required") || k.as_symbol() == Some("required-count"))))
    }
    match binding.assq("arities") {
        Some(single) if is_clause(single) => vec![single],
        Some(Datum::List(items)) => items
            .iter()
            .filter(|d| d.as_symbol() != Some("arities"))
            .filter(|d| is_clause(d))
            .collect(),
        _ => binding.assq("arguments").into_iter().collect(),
    }
}

/// The module a `lookup` result came from, as `(srfi srfi-1)`.
fn module_name(lookup: &Datum) -> String {
    match lookup.assq("module") {
        // `(module srfi srfi-1)` is returned whole; drop the key.
        Some(Datum::List(items)) if items.first().and_then(Datum::as_symbol) == Some("module") => {
            Datum::List(items[1..].to_vec()).to_string()
        }
        // A one-component module `(module x)` comes back as the bare symbol.
        Some(Datum::Symbol(single)) => format!("({single})"),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// `(name a b [opt] #:key . rest)` from the worker's arguments alist.
fn signature(name: &str, arguments: Option<&Datum>) -> String {
    let mut parts = vec![name.to_string()];
    let Some(arguments) = arguments else {
        return format!("({name} ...)");
    };
    // `assq` yields the bare value for `(required host)` but the whole entry
    // for `(required a b c)`; accept both.
    let names = |key: &str| -> Vec<String> {
        match arguments.assq(key) {
            Some(Datum::List(items)) if items.first().and_then(Datum::as_symbol) == Some(key) => {
                items.iter().skip(1).map(|d| d.to_string()).collect()
            }
            Some(Datum::List(items)) => items.iter().map(|d| d.to_string()).collect(),
            Some(single) => vec![single.to_string()],
            None => Vec::new(),
        }
    };
    parts.extend(names("required"));
    parts.extend(names("optional").into_iter().map(|o| format!("[{o}]")));
    parts.extend(names("keyword").into_iter().map(|k| format!("#:{k}")));
    if arguments.assq("allow-other-keys?") == Some(&Datum::Boolean(true)) {
        parts.push("#:allow-other-keys".into());
    }
    if let Some(Datum::Symbol(rest)) = arguments.assq("rest") {
        parts.push(format!(". {rest}"));
    }
    if arguments.assq("required-count").is_some() {
        // Primitive without names: report counts only.
        let required = arguments
            .assq("required-count")
            .and_then(Datum::as_integer)
            .unwrap_or(0);
        let optional = arguments
            .assq("optional-count")
            .and_then(Datum::as_integer)
            .unwrap_or(0);
        parts.extend((0..required).map(|i| format!("arg{}", i + 1)));
        parts.extend((0..optional).map(|i| format!("[arg{}]", required + i + 1)));
        if arguments.assq("rest?") == Some(&Datum::Boolean(true)) {
            parts.push(". rest".into());
        }
    }
    format!("({})", parts.join(" "))
}

/// The module name under `position` when it is written as an import:
/// a spec of `use-modules`, a `#:use-module` clause, a symbol of a
/// `use-service-modules`-style shorthand, or the module of `@`/`@@`.
pub fn module_at(snapshot: &Snapshot, position: Position) -> Option<Vec<String>> {
    let source: &str = &snapshot.text;
    let offset = crate::scope::offset_of(snapshot, position)?;
    let path = crate::features::path_to(&snapshot.tree.root, offset);
    let head_name = |form: &Node| -> Option<&str> {
        let head = form.data().next()?;
        (head.kind == Kind::Symbol).then(|| head.text(source))
    };
    let symbols = |list: &Node| -> Option<Vec<String>> {
        let items: Vec<&Node> = list.data().collect();
        (!items.is_empty() && items.iter().all(|n| n.kind == Kind::Symbol))
            .then(|| items.iter().map(|n| n.text(source).to_string()).collect())
    };
    // Innermost first: the list of symbols the cursor is in, and its parents.
    for (depth, node) in path.iter().enumerate() {
        if node.kind != Kind::List {
            continue;
        }
        let Some(name) = symbols(node) else { continue };
        let parents: Vec<&Node> = path[depth + 1..].to_vec();
        // (use-modules (a b) ((a b) #:select ...)) and (@ (a b) name)
        let mut import_form = None;
        for parent in parents.iter().take(2) {
            match head_name(parent) {
                Some("use-modules" | "@" | "@@") => {
                    import_form = Some(*parent);
                    break;
                }
                Some("define-module") => {
                    // A clause list right after #:use-module, possibly wrapped.
                    let data: Vec<&Node> = parent.data().collect();
                    let target = if parents.first().is_some_and(|p| std::ptr::eq(*p, *parent)) {
                        *node
                    } else {
                        parents[0]
                    };
                    let index = data.iter().position(|n| std::ptr::eq(*n, target))?;
                    if index > 0
                        && data[index - 1].kind == Kind::Keyword
                        && data[index - 1].text(source) == "#:use-module"
                    {
                        import_form = Some(*parent);
                    }
                    break;
                }
                _ => {}
            }
        }
        if import_form.is_some() {
            return Some(name);
        }
    }
    // (use-service-modules desktop networking): the symbol under the cursor.
    let symbol = path.first().filter(|n| n.kind == Kind::Symbol)?;
    let form = path.get(1)?;
    let middle = match head_name(form)? {
        "use-service-modules" => "services",
        "use-package-modules" => "packages",
        "use-system-modules" => "system",
        _ => return None,
    };
    if std::ptr::eq(form.data().next()?, *symbol) {
        return None;
    }
    Some(vec![
        "gnu".to_string(),
        middle.to_string(),
        symbol.text(source).to_string(),
    ])
}

pub fn module_params(module: &[String]) -> Datum {
    Datum::list([Datum::Pair(
        Box::new(Datum::symbol("module")),
        Box::new(Datum::list(module.iter().map(|m| Datum::symbol(m)))),
    )])
}
/// The definition location the worker reported, if it has a file.
pub fn location_from_lookup(lookup: &Datum) -> Option<Location> {
    let location = lookup.assq("binding")?.assq("location")?;
    let file = location.assq("file")?.as_str()?;
    let line = location.assq("line")?.as_integer()? as u32;
    let column = location.assq("column")?.as_integer()? as u32;
    let uri = Url::from_file_path(file).ok()?;
    let position = Position::new(line, column);
    Some(Location::new(uri, Range::new(position, position)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::Documents;
    use tower_lsp::lsp_types::TextDocumentItem;

    fn snapshot(text: &str) -> Snapshot {
        Documents::default().open(TextDocumentItem {
            uri: Url::parse("file:///t.scm").unwrap(),
            language_id: "scheme".into(),
            version: 1,
            text: text.into(),
        })
    }

    #[test]
    fn module_context_reads_both_import_styles() {
        let snapshot = snapshot(
            "(define-module (app main)\n  #:use-module ((app lib util) #:prefix util:)\n  #:use-module (srfi srfi-1)\n  #:export (run))\n(use-modules (ice-9 match) ((rnrs) #:select (assert)))\n(define (run) (fold + 0 '()))",
        );
        let modules: Vec<String> = module_context(&snapshot)
            .iter()
            .map(|m| m.to_string())
            .collect();
        assert_eq!(
            modules,
            [
                "(app lib util)",
                "(srfi srfi-1)",
                "(ice-9 match)",
                "(rnrs)",
                "(guile)"
            ]
        );
        let (node, name) = symbol_at(&snapshot, Position::new(5, 16)).unwrap();
        assert_eq!(name, "fold");
        assert_eq!(node.kind, Kind::Symbol);
        assert_eq!(
            symbol_at(&snapshot, Position::new(5, 18)).map(|s| s.1),
            Some("fold")
        );
        assert!(
            symbol_at(&snapshot, Position::new(5, 13)).is_none(),
            "whitespace"
        );
        assert_eq!(
            local_definition(&snapshot, "run").map(|s| s.selection_range.start),
            Some(Position::new(5, 9))
        );
        assert!(local_definition(&snapshot, "fold").is_none());
        let params: Vec<String> = lookup_params(&snapshot, "fold")
            .iter()
            .map(|d| d.to_string())
            .collect();
        assert_eq!(
            params,
            ["((name . fold) (modules (srfi srfi-1) (ice-9 match) (guile)))"],
            "neither the util: prefix nor the rnrs #:select can produce a bare `fold`"
        );
        let params: Vec<String> = lookup_params(&snapshot, "util:string-length*")
            .iter()
            .map(|d| d.to_string())
            .collect();
        assert_eq!(
            params,
            [
                "((name . string-length*) (modules (app lib util)))",
                "((name . util:string-length*) (modules (srfi srfi-1) (ice-9 match) (guile)))"
            ],
            "the prefixed import comes first; plain imports still get a chance"
        );
    }

    #[test]
    fn hover_renders_signatures_and_locations() {
        let lookup = Datum::parse_one(
            "((module srfi srfi-1) (binding (name . fold) (kind . procedure) (arguments (required kons knil list1) (optional) (keyword) (allow-other-keys? . #f) (rest . #f)) (documentation . \"Fold.\") (location (file . \"/g/srfi-1.scm\") (line . 450) (column . 2))))",
        )
        .unwrap();
        let markdown = hover_markdown("fold", &lookup);
        assert!(
            markdown.starts_with("```scheme\n(fold kons knil list1)\n```\n"),
            "{markdown}"
        );
        assert!(markdown.contains("Fold."));
        assert!(markdown.ends_with("from `(srfi srfi-1)`"));
        let location = location_from_lookup(&lookup).unwrap();
        assert_eq!(location.uri.path(), "/g/srfi-1.scm");
        assert_eq!(location.range.start, Position::new(450, 2));

        let keyworded = Datum::parse_one(
            "((module x) (binding (kind . procedure) (arguments (required host) (optional port) (keyword timeout tls?) (allow-other-keys? . #f) (rest . extra))))",
        )
        .unwrap();
        assert!(hover_markdown("connect", &keyworded)
            .contains("(connect host [port] #:timeout #:tls? . extra)"));
        let primitive = Datum::parse_one("((module guile) (binding (kind . procedure) (arguments (required-count . 2) (optional-count . 0) (rest? . #f))))").unwrap();
        assert!(hover_markdown("cons", &primitive).contains("(cons arg1 arg2)"));
        let clauses = Datum::parse_one("((module srfi srfi-1) (binding (kind . procedure) (arguments (required f l)) (arities ((required f l) (optional) (keyword) (rest . #f)) ((required f l1) (optional) (keyword) (rest . rest)))))").unwrap();
        assert!(
            hover_markdown("map", &clauses)
                .starts_with("```scheme\n(map f l)\n(map f l1 . rest)\n```\n"),
            "{}",
            hover_markdown("map", &clauses)
        );
        let macro_binding = Datum::parse_one(
            "((module guile) (binding (kind . macro) (documentation . #f) (location . #f)))",
        )
        .unwrap();
        assert!(hover_markdown("when", &macro_binding).contains("*syntax*"));
        assert!(location_from_lookup(&macro_binding).is_none());
    }

    #[test]
    fn module_names_are_recognised_in_import_forms() {
        let text = "(define-module (app main)\n  #:use-module (srfi srfi-1)\n  #:use-module ((ice-9 match) #:select (match))\n  #:export (main))\n(use-modules (gnu) ((guix gexp) #:prefix g:))\n(use-service-modules desktop networking)\n(define x (@ (gnu packages base) coreutils))\n(list 1 2)";
        let snapshot = snapshot(text);
        let at = |line: u32, character: u32| {
            module_at(&snapshot, Position::new(line, character)).map(|m| m.join(" "))
        };
        assert_eq!(at(1, 20).as_deref(), Some("srfi srfi-1"));
        assert_eq!(at(2, 22).as_deref(), Some("ice-9 match"));
        assert_eq!(at(4, 15).as_deref(), Some("gnu"));
        assert_eq!(at(4, 27).as_deref(), Some("guix gexp"));
        assert_eq!(at(5, 24).as_deref(), Some("gnu services desktop"));
        assert_eq!(at(5, 33).as_deref(), Some("gnu services networking"));
        assert_eq!(at(6, 20).as_deref(), Some("gnu packages base"));
        assert_eq!(at(0, 18).as_deref(), None, "the module being defined");
        assert_eq!(at(3, 12), None, "an export list");
        assert_eq!(at(7, 7), None, "ordinary data");
        assert_eq!(at(5, 5), None, "the shorthand's own name");
    }
}
