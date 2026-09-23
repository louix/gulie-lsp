//! Guix adapter: extra hover content for service types and configuration
//! records, obtained through the worker's Guix-specific methods.

use crate::syntax::datum::Datum;
use crate::worker::{Identity, Worker};

/// Whether the environment can answer Guix questions at all.
pub fn active(identity: &Identity) -> bool {
    identity.guix_channels.is_some()
}

/// Extra markdown for a resolved binding, or `None` when the binding is not
/// a Guix form or the adapter cannot answer. Never fails loudly.
pub async fn enrich_hover(
    worker: &Worker,
    identity: &Identity,
    name: &str,
    lookup: &Datum,
) -> Option<String> {
    if !active(identity) {
        return None;
    }
    let binding = lookup.assq("binding")?;
    let module = module_datum(lookup)?;
    let kind = binding.assq("kind").and_then(Datum::as_symbol)?;
    let ty = binding.assq("type").and_then(Datum::as_symbol);
    match (kind, ty) {
        ("variable", Some("<service-type>")) => {
            let summary = worker
                .request("service-type", module_and_name(&module, name))
                .await
                .ok()?;
            Some(service_type_markdown(&summary))
        }
        // A syntactic record constructor: only worth a round trip in Guix.
        ("macro", _) => {
            let fields = worker
                .request("configuration", module_and_name(&module, name))
                .await
                .ok()?;
            configuration_markdown(&fields)
        }
        _ => None,
    }
}

fn module_datum(lookup: &Datum) -> Option<Datum> {
    match lookup.assq("module")? {
        Datum::List(items) if items.first().and_then(Datum::as_symbol) == Some("module") => {
            Some(Datum::List(items[1..].to_vec()))
        }
        Datum::Symbol(single) => Some(Datum::list([Datum::symbol(single)])),
        other => Some(other.clone()),
    }
}

fn module_and_name(module: &Datum, name: &str) -> Datum {
    let module_entry = match module {
        Datum::List(items) => Datum::List(
            std::iter::once(Datum::symbol("module"))
                .chain(items.iter().cloned())
                .collect(),
        ),
        other => Datum::list([Datum::symbol("module"), other.clone()]),
    };
    Datum::list([
        module_entry,
        Datum::Pair(
            Box::new(Datum::symbol("name")),
            Box::new(Datum::symbol(name)),
        ),
    ])
}

pub fn service_type_markdown(summary: &Datum) -> String {
    let mut out = String::from("\n---\n**Guix service type**");
    if let Some(name) = summary.assq("service-name").and_then(Datum::as_symbol) {
        out.push_str(&format!(" `{name}`"));
    }
    out.push('\n');
    if let Some(description) = summary.assq("description").and_then(Datum::as_str) {
        out.push_str(&format!("\n{}\n", description.trim()));
    }
    if let Some(default) = summary.assq("default-value").and_then(Datum::as_symbol) {
        if default != "none" {
            out.push_str(&format!("\nDefault value: `{default}`\n"));
        }
    }
    if let Some(Datum::List(extends)) = summary.assq("extends") {
        let targets: Vec<String> = extends.iter().skip(1).map(|d| format!("`{d}`")).collect();
        if !targets.is_empty() {
            out.push_str(&format!("\nExtends: {}\n", targets.join(", ")));
        }
    }
    out
}

pub fn configuration_markdown(fields: &Datum) -> Option<String> {
    let source = fields.assq("source").and_then(Datum::as_symbol)?;
    let Datum::List(list) = fields.assq("fields")? else {
        return None;
    };
    let mut out = String::from("\n---\n**Guix record fields**");
    out.push_str(match source {
        "runtime" => " (from `define-configuration`)\n\n",
        "source-file" => " (read from the definition)\n\n",
        _ => return None,
    });
    let entries: Vec<&Datum> = list.iter().skip(1).collect();
    out.push_str("| field | default | documentation |\n|---|---|---|\n");
    for field in entries.iter().take(40) {
        let name = field
            .assq("name")
            .map(|n| n.to_string())
            .unwrap_or_default();
        let default = match field.assq("default") {
            Some(Datum::String(text)) => format!("`{text}`"),
            Some(Datum::Boolean(false)) | None => String::new(),
            // `(default (value . 1))` arrives as the bare pair.
            Some(Datum::Pair(key, value)) if key.as_symbol() == Some("value") => {
                format!("`{value}`")
            }
            Some(Datum::Pair(key, ty)) if key.as_symbol() == Some("opaque") => format!("*{ty}*"),
            Some(nested) => match nested.assq("value") {
                Some(value) => format!("`{value}`"),
                None => nested
                    .assq("opaque")
                    .map(|t| format!("*{t}*"))
                    .unwrap_or_default(),
            },
        };
        let required = field.assq("required?") == Some(&Datum::Boolean(true));
        let doc = field
            .assq("documentation")
            .and_then(Datum::as_str)
            .map(|d| d.lines().next().unwrap_or("").replace('|', "\\|"))
            .unwrap_or_default();
        out.push_str(&format!(
            "| `{name}` | {} | {doc} |\n",
            if required {
                "*required*".to_string()
            } else {
                default
            }
        ));
    }
    if entries.len() > 40 {
        out.push_str(&format!("\n… {} more fields\n", entries.len() - 40));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_service_types_and_fields() {
        let summary = Datum::parse_one(
            "((module gnu services ssh) (name . openssh-service-type) (service-name . openssh) (description . \"Run the OpenSSH secure shell (SSH) server.\") (default-value . <openssh-configuration>) (extends shepherd-root pam) (location . #f))",
        )
        .unwrap();
        let markdown = service_type_markdown(&summary);
        assert!(markdown.contains("**Guix service type** `openssh`"));
        assert!(markdown.contains("Run the OpenSSH"));
        assert!(markdown.contains("Default value: `<openssh-configuration>`"));
        assert!(markdown.contains("Extends: `shepherd-root`, `pam`"));

        let fields = Datum::parse_one(
            "((module gnu system) (name . operating-system) (source . source-file) (fields ((name . host-name) (required? . #t) (default . #f)) ((name . timezone) (required? . #f) (default . \"\\\"Etc/UTC\\\"\")) ((name . repeat) (type . integer) (default (value . 1)) (documentation . \"How many|times.\\nMore.\"))))",
        )
        .unwrap();
        let markdown = configuration_markdown(&fields).unwrap();
        assert!(
            markdown.contains("| `host-name` | *required* |  |"),
            "{markdown}"
        );
        assert!(
            markdown.contains("| `timezone` | `\"Etc/UTC\"` |  |"),
            "{markdown}"
        );
        assert!(
            markdown.contains("| `repeat` | `1` | How many\\|times. |"),
            "{markdown}"
        );
        let unavailable = Datum::parse_one("((source . unavailable) (fields . #f))").unwrap();
        assert!(configuration_markdown(&unavailable).is_none());
    }
}

use crate::documents::Snapshot;
use crate::scope::Import;
use crate::syntax::{Kind, Node};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, Location, MarkupContent, MarkupKind,
    Position, Range, Url,
};

/// `(use-service-modules desktop networking)` and its package and system
/// siblings, expanded to plain imports. Only meaningful when the document
/// imports `(gnu)`, which defines those macros.
pub fn shorthand_imports(snapshot: &Snapshot) -> Vec<Import> {
    let source = &snapshot.text;
    let imports_gnu = crate::scope::imports(snapshot)
        .iter()
        .any(|i| i.module == ["gnu"]);
    if !imports_gnu {
        return Vec::new();
    }
    let mut out = Vec::new();
    for form in snapshot.tree.root.data() {
        let mut items = form.data();
        let Some(head) = items.next().filter(|h| h.kind == Kind::Symbol) else {
            continue;
        };
        let middle = match head.text(source) {
            "use-service-modules" => "services",
            "use-package-modules" => "packages",
            "use-system-modules" => "system",
            _ => continue,
        };
        for spec in items {
            // Each spec is a symbol or a list of symbols: (gnu services X ...).
            let tail: Option<Vec<String>> = match spec.kind {
                Kind::Symbol => Some(vec![spec.text(source).to_string()]),
                Kind::List => spec
                    .data()
                    .map(|n| (n.kind == Kind::Symbol).then(|| n.text(source).to_string()))
                    .collect(),
                _ => None,
            };
            if let Some(tail) = tail {
                let mut module = vec!["gnu".to_string(), middle.to_string()];
                module.extend(tail);
                out.push(Import {
                    module,
                    select: None,
                    hide: Vec::new(),
                    prefix: None,
                });
            }
        }
    }
    out
}

/// The cursor's position relative to a record constructor form
/// `(ctor (field value) ...)`. Purely syntactic; whether `ctor` is a record
/// is decided by resolving it through the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordContext {
    /// The constructor symbol as written.
    pub head: String,
    /// Fields already written in the form.
    pub supplied: Vec<String>,
    /// The cursor is directly inside the constructor form, not yet inside a
    /// `(field ...)` list, so a completion must insert the parentheses.
    pub bare: bool,
    /// The partial field name under the cursor.
    pub prefix: String,
    /// The field name the cursor is on, when it sits on a written field name.
    pub field: Option<String>,
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

/// Whether `form` is used with field syntax: every argument other than
/// `except` is a `(name ...)` list. Wrappers such as Guix's `service` are
/// records underneath but used positionally, and must not offer fields.
fn field_syntax(form: &Node, except: Option<&Node>, source: &str) -> bool {
    form.data().skip(1).all(|arg| {
        except.is_some_and(|e| std::ptr::eq(e, arg))
            || (arg.kind == Kind::List
                && arg
                    .data()
                    .next()
                    .is_none_or(|n| n.kind == Kind::Symbol && n.text(source) != "."))
    })
}

/// Field names among `form`'s own children (for a list that swallowed its
/// siblings), not skipping a head.
fn field_names_of_children(form: &Node, source: &str) -> Vec<String> {
    form.data()
        .filter(|f| f.kind == Kind::List)
        .filter_map(|f| f.data().next())
        .filter(|n| n.kind == Kind::Symbol)
        .map(|n| n.text(source).to_string())
        .collect()
}

fn field_names(form: &Node, source: &str) -> Vec<String> {
    form.data()
        .skip(1)
        .filter(|f| f.kind == Kind::List)
        .filter_map(|f| f.data().next())
        .filter(|n| n.kind == Kind::Symbol)
        .map(|n| n.text(source).to_string())
        .collect()
}

pub fn record_context(snapshot: &Snapshot, offset: usize) -> Option<RecordContext> {
    let source = &snapshot.text;
    let last = source.len().checked_sub(1)?;
    // With no token under the cursor, the character before it says which
    // form the cursor is in: for `foo |(bar` that is foo's list, not bar's.
    let probe = offset.saturating_sub(1).min(last);
    let token_at = |at: usize| {
        let node = snapshot.tree.root.node_at(at);
        (node.kind == Kind::Symbol).then_some(node)
    };
    // The token being typed ends at the cursor; the token hovered contains it.
    let token = token_at(probe)
        .filter(|t| t.range.start < offset)
        .or_else(|| token_at(offset.min(last)));
    let prefix = token
        .map(|t| source[t.range.start..offset.min(t.range.end)].to_string())
        .unwrap_or_default();
    // A cursor right after an open paren, possibly across a newline, starts an empty
    // field list. The reader has that paren swallow the sibling forms that
    // follow, so treat the list as empty and its children as written fields.
    if token.is_none() {
        let before = source[..offset].trim_end();
        if before.ends_with('(') {
            let paren = before.len() - 1;
            let chain = ancestors(&snapshot.tree.root, paren);
            let lists: Vec<&Node> = chain
                .iter()
                .copied()
                .filter(|n| n.kind == Kind::List)
                .collect();
            if let (Some(inner), Some(outer)) = (lists.first().copied(), lists.get(1).copied()) {
                if inner.range.start == paren {
                    if let Some(head) = outer
                        .data()
                        .next()
                        .filter(|h| h.kind == Kind::Symbol)
                        .map(|h| h.text(source).to_string())
                    {
                        let is_ctor_head =
                            outer.data().next().is_some_and(|h| std::ptr::eq(h, inner));
                        if !is_ctor_head && field_syntax(outer, Some(inner), source) {
                            let mut supplied = field_names(outer, source);
                            supplied.extend(field_names_of_children(inner, source));
                            return Some(RecordContext {
                                head,
                                supplied,
                                bare: false,
                                prefix: String::new(),
                                field: None,
                            });
                        }
                    }
                }
            }
        }
    }
    let anchor = token.map(|t| t.range.start).unwrap_or(probe);
    let chain = ancestors(&snapshot.tree.root, anchor);
    let lists: Vec<&Node> = chain
        .iter()
        .copied()
        .filter(|n| n.kind == Kind::List)
        .collect();
    let head_of = |form: &Node| {
        form.data()
            .next()
            .filter(|h| h.kind == Kind::Symbol)
            .map(|h| h.text(source).to_string())
    };
    let is_head = |form: &Node| {
        form.data()
            .next()
            .is_some_and(|h| token.is_some_and(|t| std::ptr::eq(h, t)))
    };
    let (inner, outer) = (lists.first().copied(), lists.get(1).copied());
    // Case 1: inside a `(field ...)` list whose parent is the constructor form.
    if let (Some(inner), Some(outer)) = (inner, outer) {
        if let Some(head) = head_of(outer) {
            // The cursor is on the field list's first element or the list is
            // still empty; a value position is not a field-name position.
            let is_first = match inner.data().next() {
                None => true,
                Some(first) => token.is_some_and(|t| std::ptr::eq(first, t)),
            };
            let is_ctor_head = outer.data().next().is_some_and(|h| std::ptr::eq(h, inner));
            if !is_ctor_head && is_first && field_syntax(outer, Some(inner), source) {
                return Some(RecordContext {
                    head,
                    supplied: field_names(outer, source),
                    bare: false,
                    prefix,
                    field: token
                        .filter(|_| is_head(inner))
                        .map(|t| t.text(source).to_string()),
                });
            }
        }
    }
    // Case 2: directly inside the constructor form, after its head.
    if let Some(inner) = inner {
        if let Some(head) = head_of(inner) {
            if !token.is_some_and(|_| is_head(inner)) && field_syntax(inner, None, source) {
                return Some(RecordContext {
                    head,
                    supplied: field_names(inner, source),
                    bare: true,
                    prefix,
                    field: None,
                });
            }
        }
    }
    None
}

/// Whether a `configuration` result describes a record at all.
pub fn is_record(fields: &Datum) -> bool {
    matches!(
        fields.assq("source").and_then(Datum::as_symbol),
        Some("runtime") | Some("source-file")
    )
}

fn field_entries(fields: &Datum) -> Vec<&Datum> {
    match fields.assq("fields") {
        Some(Datum::List(list)) => list.iter().skip(1).collect(),
        _ => Vec::new(),
    }
}

fn field_default(field: &Datum) -> Option<String> {
    match field.assq("default") {
        Some(Datum::String(text)) => Some(text.clone()),
        Some(Datum::Pair(key, value)) if key.as_symbol() == Some("value") => {
            Some(value.to_string())
        }
        Some(Datum::Pair(key, ty)) if key.as_symbol() == Some("opaque") => Some(format!("a {ty}")),
        Some(Datum::Boolean(false)) | None => None,
        Some(nested) => nested
            .assq("value")
            .map(|v| v.to_string())
            .or_else(|| nested.assq("opaque").map(|t| format!("a {t}"))),
    }
}

fn field_required(field: &Datum) -> bool {
    field.assq("required?") == Some(&Datum::Boolean(true))
        || matches!(field.assq("default"), Some(Datum::Pair(key, _)) if key.as_symbol() == Some("required"))
}

/// Field completion items for a record, minus fields already supplied.
pub fn field_items(fields: &Datum, context: &RecordContext) -> Vec<CompletionItem> {
    let record = fields
        .assq("name")
        .map(|n| n.to_string())
        .unwrap_or_default();
    let mut items = Vec::new();
    let mut offer = |name: &str, detail: String, doc: Option<String>| {
        if !name.starts_with(&context.prefix) || context.supplied.iter().any(|s| s == name) {
            return;
        }
        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(detail),
            documentation: doc.map(|value| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                })
            }),
            insert_text: context.bare.then(|| format!("({name} )")),
            ..Default::default()
        });
    };
    for field in field_entries(fields) {
        let Some(name) = field.assq("name").and_then(Datum::as_symbol) else {
            continue;
        };
        let mut detail = format!("field of {record}");
        if field_required(field) {
            detail.push_str(", required");
        } else if let Some(default) = field_default(field) {
            detail.push_str(&format!(", default {default}"));
        }
        if let Some(ty) = field.assq("type").and_then(Datum::as_symbol) {
            detail.push_str(&format!(", {ty}"));
        }
        let doc = field
            .assq("documentation")
            .and_then(Datum::as_str)
            .map(str::to_string);
        offer(name, detail, doc);
    }
    if fields.assq("source").and_then(Datum::as_symbol) == Some("source-file") {
        offer(
            "inherit",
            format!("copy the other fields from an existing {record}"),
            None,
        );
    }
    items
}

pub fn field_hover(fields: &Datum, name: &str) -> Option<String> {
    let record = fields
        .assq("name")
        .map(|n| n.to_string())
        .unwrap_or_default();
    let field = field_entries(fields)
        .into_iter()
        .find(|f| f.assq("name").and_then(Datum::as_symbol) == Some(name))?;
    let mut out = format!("```scheme\n({name} ...)\n```\n*field* of `{record}`");
    if let Some(ty) = field.assq("type").and_then(Datum::as_symbol) {
        out.push_str(&format!(", type `{ty}`"));
    }
    out.push('\n');
    if field_required(field) {
        out.push_str("\nRequired.\n");
    } else if let Some(default) = field_default(field) {
        out.push_str(&format!("\nDefault: `{default}`\n"));
    }
    for (flag, text) in [
        ("thunked?", "thunked"),
        ("delayed?", "delayed"),
        ("innate?", "innate, not inherited"),
    ] {
        if field.assq(flag) == Some(&Datum::Boolean(true)) {
            out.push_str(&format!("\n*{text}*\n"));
        }
    }
    if let Some(doc) = field.assq("documentation").and_then(Datum::as_str) {
        out.push_str(&format!("\n{}\n", doc.trim()));
    }
    let source = match fields.assq("source").and_then(Datum::as_symbol) {
        Some("runtime") => "define-configuration metadata",
        _ => "the record definition",
    };
    if let Some(file) = fields.assq("file").and_then(Datum::as_str) {
        out.push_str(&format!("\nfrom {source} in `{file}`"));
    } else {
        out.push_str(&format!("\nfrom {source}"));
    }
    Some(out)
}

/// Where a field is declared, when the extractor found its form.
pub fn field_location(fields: &Datum, name: &str) -> Option<Location> {
    let field = field_entries(fields)
        .into_iter()
        .find(|f| f.assq("name").and_then(Datum::as_symbol) == Some(name))?;
    let location = field.assq("location")?;
    let file = location.assq("file")?.as_str()?;
    let line = location.assq("line")?.as_integer()? as u32;
    let column = location.assq("column")?.as_integer()? as u32;
    let position = Position::new(line, column);
    Some(Location::new(
        Url::from_file_path(file).ok()?,
        Range::new(position, position),
    ))
}

pub fn configuration_params(module: &Datum, name: &str) -> Datum {
    module_and_name(module, name)
}

pub fn lookup_module(lookup: &Datum) -> Option<Datum> {
    module_datum(lookup)
}

#[cfg(test)]
mod record_tests {
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
    fn shorthand_expands_only_with_gnu() {
        let with = snapshot("(use-modules (gnu))\n(use-service-modules desktop (networking))\n(use-package-modules wm)");
        let modules: Vec<String> = shorthand_imports(&with)
            .iter()
            .map(|i| i.module.join(" "))
            .collect();
        assert_eq!(
            modules,
            [
                "gnu services desktop",
                "gnu services networking",
                "gnu packages wm"
            ]
        );
        let without = snapshot("(use-service-modules desktop)");
        assert!(shorthand_imports(&without).is_empty());
    }

    #[test]
    fn record_context_distinguishes_positions() {
        let text = "(service openssh-service-type\n  (openssh-configuration\n   (x11-forwarding? #t)\n   (po\n   ";
        let snap = snapshot(text);
        let at = |needle: &str| text.find(needle).unwrap() + needle.len();
        let ctx = record_context(&snap, at("(po")).unwrap();
        assert_eq!(ctx.head, "openssh-configuration");
        assert_eq!(ctx.supplied, ["x11-forwarding?", "po"]);
        assert!(!ctx.bare);
        assert_eq!(ctx.prefix, "po");
        assert_eq!(ctx.field.as_deref(), Some("po"));
        // Hovering an existing field name.
        let ctx = record_context(&snap, text.find("x11-forwarding?").unwrap() + 3).unwrap();
        assert_eq!(ctx.field.as_deref(), Some("x11-forwarding?"));
        assert_eq!(ctx.prefix, "x11");
        // Directly inside the constructor form, after the last field.
        let bare = snapshot("(openssh-configuration\n (x11-forwarding? #t)\n ");
        let ctx = record_context(&bare, bare.text.len()).unwrap();
        assert_eq!(ctx.head, "openssh-configuration");
        assert!(ctx.bare);
        assert_eq!(ctx.prefix, "");
        // An open paren before other fields: the reader makes it swallow them,
        // but they still count as written.
        let open =
            snapshot("(openssh-configuration\n (x11-forwarding? #t)\n (\n (authorized-keys 1))");
        let at = open.text.find("(\n").unwrap() + 1;
        let ctx = record_context(&open, at).unwrap();
        assert_eq!(ctx.head, "openssh-configuration");
        assert!(!ctx.bare);
        assert_eq!(ctx.supplied, ["x11-forwarding?", "authorized-keys"]);
        // On the constructor name itself: not a field position.
        let ctx = record_context(&snap, text.find("openssh-configuration").unwrap() + 4);
        assert_eq!(
            ctx, None,
            "the enclosing (service type value) form is positional"
        );
        // A value position inside a field is not a field-name position, and
        // `(x11-forwarding? #t)` itself is positional, so no context at all.
        assert_eq!(record_context(&snap, text.find("#t").unwrap() + 1), None);
        // Positional forms such as (service type value) are not record forms.
        let positional = snapshot("(service openssh-service-type (openssh-configuration (po");
        let ctx = record_context(&positional, positional.text.len()).unwrap();
        assert_eq!(ctx.head, "openssh-configuration");
        assert_eq!(record_context(&positional, 30).map(|c| c.head), None);
    }

    #[test]
    fn constructor_head_in_the_fixture_is_not_a_field_position() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/guix/system.scm"
        ))
        .unwrap();
        let snap = snapshot(&text);
        let offset = snap.offset(Position::new(60, 20)).unwrap();
        assert_eq!(&text[offset..offset + 21], "openssh-configuration");
        let ctx = record_context(&snap, offset);
        assert_eq!(ctx, None, "{ctx:?}");
        // Directly after the name the user is still typing it; from the next
        // line on, the cursor is at a bare field position.
        assert_eq!(record_context(&snap, offset + 21), None);
        let ctx = record_context(&snap, offset + 22).unwrap();
        assert!(ctx.bare);
        assert_eq!(ctx.head, "openssh-configuration");
    }

    #[test]
    fn field_items_and_hover_use_defaults_and_docs() {
        let fields = Datum::parse_one(
            "((module gnu services ssh) (name . openssh-configuration) (source . source-file) (file . \"/g/ssh.scm\") (fields ((name . port-number) (location (file . \"/g/ssh.scm\") (line . 109) (column . 2)) (required? . #f) (default . \"22\") (thunked? . #f)) ((name . x11-forwarding?) (required? . #f) (default . \"#f\")) ((name . authorized-keys) (required? . #t) (default . #f))))",
        )
        .unwrap();
        let ctx = RecordContext {
            head: "openssh-configuration".into(),
            supplied: vec!["x11-forwarding?".into()],
            bare: true,
            prefix: String::new(),
            field: None,
        };
        let items = field_items(&fields, &ctx);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["port-number", "authorized-keys", "inherit"]);
        assert_eq!(items[0].insert_text.as_deref(), Some("(port-number )"));
        assert_eq!(
            items[0].detail.as_deref(),
            Some("field of openssh-configuration, default 22")
        );
        assert_eq!(
            items[1].detail.as_deref(),
            Some("field of openssh-configuration, required")
        );
        let hover = field_hover(&fields, "port-number").unwrap();
        assert!(
            hover.contains("*field* of `openssh-configuration`"),
            "{hover}"
        );
        assert!(hover.contains("Default: `22`"));
        assert!(
            hover.ends_with("from the record definition in `/g/ssh.scm`"),
            "{hover}"
        );
        let location = field_location(&fields, "port-number").unwrap();
        assert_eq!(location.range.start, Position::new(109, 2));
        assert!(field_location(&fields, "x11-forwarding?").is_none());
        assert!(is_record(&fields));
        assert!(!is_record(
            &Datum::parse_one("((source . unavailable) (fields . #f))").unwrap()
        ));
    }
}

/// Indentation rules for Guix forms, from Guix's own `.dir-locals.el`.
pub fn indent_rules() -> impl Iterator<Item = (&'static str, u8)> {
    super::guix_indent::RULES.iter().copied()
}

/// Whether a document is Guix code by its imports: anything under `(gnu ...)`
/// or `(guix ...)`.
pub fn imports_guix(snapshot: &Snapshot) -> bool {
    crate::scope::imports(snapshot)
        .iter()
        .any(|i| matches!(i.module.first().map(String::as_str), Some("gnu" | "guix")))
}

/// Characters Guix's editor setup treats as reader prefixes, so that `#~`,
/// `#$`, and `#+` gexp syntax and `$` in match patterns indent naturally.
pub const INDENT_PREFIXES: &str = "$~+";

/// Field names of a record description from the worker's `configuration`
/// method.
pub fn record_field_names(fields: &Datum) -> Vec<String> {
    field_entries(fields)
        .iter()
        .filter_map(|f| f.assq("name").and_then(Datum::as_symbol))
        .map(str::to_string)
        .collect()
}

/// The record whose fields a wrapper macro takes: `(package/inherit base
/// (field value) ...)` writes `package` fields.
pub fn underlying_record(head: &str) -> Option<&str> {
    head.strip_suffix("/inherit")
}

/// Where a record described by the `configuration` method is defined: its
/// first field with a location, which sits inside the definition.
pub fn record_location(fields: &Datum) -> Option<Location> {
    field_entries(fields).into_iter().find_map(|field| {
        let name = field.assq("name")?.as_symbol()?;
        field_location(fields, name)
    })
}

/// The record a service type's default value is an instance of, from its
/// `service-type` summary: `<openssh-configuration>` gives
/// `openssh-configuration`.
pub fn default_record(summary: &Datum) -> Option<String> {
    let default = summary.assq("default-value")?.as_symbol()?;
    let inner = default.strip_prefix('<')?.strip_suffix('>')?;
    Some(inner.to_string())
}
