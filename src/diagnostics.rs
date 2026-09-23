//! Conservative checks that need the worker's metadata. Each fires only on
//! positively identified forms; unknown information means no report.
//! Forms inside quoted or staged code (quote, quasiquote, gexps) are never
//! checked. Heads bound lexically or defined in the document are not checked
//! against imported metadata, since the local binding wins.

use crate::adapters::guix;
use crate::documents::Snapshot;
use crate::resolver::{Resolved, Resolver};
use crate::scope::{self, Import};
use crate::syntax::datum::Datum;
use crate::syntax::{Kind, Node};
use std::collections::{BTreeMap, BTreeSet};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

const SOURCE: &str = "guile-lsp";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub head: String,
    pub head_range: std::ops::Range<usize>,
    pub positional: usize,
    pub keywords: Vec<(String, std::ops::Range<usize>)>,
    /// The form is closed; unclosed forms are still being typed.
    pub complete: bool,
    /// Every argument is a `(name ...)` list: a candidate record form.
    pub field_syntax: bool,
    pub fields: Vec<(String, std::ops::Range<usize>)>,
    pub range: std::ops::Range<usize>,
    /// Head of the enclosing call form, when this form is one of its direct
    /// arguments: `(bootloader ...)` inside `(operating-system ...)` may be
    /// a field entry rather than a call.
    pub parent_head: Option<String>,
}

fn is_closed(form: &Node) -> bool {
    form.children.last().is_some_and(|c| c.kind == Kind::Close)
}

/// What a position in a form holds, so that binding lists, patterns,
/// formals, and clause lists are not mistaken for calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// An expression: a list here is a call whose arguments are slots of
    /// its own head.
    Expr,
    /// Not code: skipped entirely (formals, module names, record specs).
    Skip,
    /// A list that is not a call but whose elements are expressions, such
    /// as a `cond` clause or a `modify-services` clause.
    Clause,
    /// `((name init ...) ...)`: names are skipped, inits are expressions.
    Bindings,
    /// A clause whose first element is a pattern or formals and whose rest
    /// are expressions: `match` and `case` clauses, `case-lambda` clauses.
    PatternClause,
    /// `guard`'s `(variable clause ...)`.
    GuardSpec,
}

/// The slot of the 1-based argument `index` of a form headed by `head`.
fn argument_slot(head: &str, index: usize, args: &[&Node]) -> Slot {
    use Slot::*;
    match head {
        "define-module"
        | "use-modules"
        | "export"
        | "re-export"
        | "import"
        | "@"
        | "@@"
        | "use-service-modules"
        | "use-package-modules"
        | "use-system-modules"
        | "define-record-type"
        | "define-record-type*"
        | "define-configuration"
        | "define-configuration/no-serialization"
        | "define-condition-type"
        | "define-enumerate-type"
        | "define-json-mapping"
        | "syntax-rules"
        | "syntax-case"
        | "define-syntax-rule" => Skip,
        "define"
        | "define*"
        | "define-public"
        | "define-inlinable"
        | "define-method"
        | "define-generic"
        | "define-gexp-compiler"
        | "define-values"
        | "lambda"
        | "lambda*"
        | "receive"
        | "eval-when" => {
            if index == 1 {
                Skip
            } else {
                Expr
            }
        }
        "let" | "match-let" => {
            let named = args.first().is_some_and(|a| a.kind == Kind::Symbol);
            if index == if named { 2 } else { 1 } {
                Bindings
            } else {
                Expr
            }
        }
        "let*"
        | "letrec"
        | "letrec*"
        | "let-values"
        | "let*-values"
        | "and-let*"
        | "parameterize"
        | "fluid-let"
        | "with-fluids"
        | "let-syntax"
        | "letrec-syntax"
        | "syntax-parameterize"
        | "match-let*"
        | "with-syntax"
        | "with-parameters" => {
            if index == 1 {
                Bindings
            } else {
                Expr
            }
        }
        "do" => match index {
            1 => Bindings,
            2 => Clause,
            _ => Expr,
        },
        "mlet" | "mlet*" => {
            if index == 2 {
                Bindings
            } else {
                Expr
            }
        }
        "let-keywords" => {
            if index == 3 {
                Bindings
            } else {
                Expr
            }
        }
        "match" | "case" | "substitute*" | "substitute-keyword-arguments" => {
            if index == 1 {
                Expr
            } else {
                PatternClause
            }
        }
        "match-lambda" | "match-lambda*" | "case-lambda" | "case-lambda*" | "cond-expand" => {
            PatternClause
        }
        "cond" => Clause,
        "guard" => {
            if index == 1 {
                GuardSpec
            } else {
                Expr
            }
        }
        "modify-services" | "modify-inputs" | "modify-phases" => {
            if index == 1 {
                Expr
            } else {
                Clause
            }
        }
        "match-record" => {
            if index == 3 {
                Skip
            } else {
                Expr
            }
        }
        "match-record-lambda" => {
            if index == 2 {
                Skip
            } else {
                Expr
            }
        }
        "condition" => Bindings,
        // Any other definer is a macro whose shape is unknown: field specs,
        // formals, and bodies alike are left alone.
        _ if head.starts_with("define") => Skip,
        _ => Expr,
    }
}

struct Form<'a> {
    node: &'a Node,
    head: &'a Node,
    parent_head: Option<String>,
}

/// Every form in code position, outermost first, following quoting,
/// gexp staging, and the slot rules of the known binding and pattern forms.
fn code_forms(snapshot: &Snapshot) -> Vec<Form<'_>> {
    let source = &snapshot.text;
    let mut out = Vec::new();
    let macros = local_macros(snapshot);
    fn walk<'a>(
        node: &'a Node,
        source: &str,
        quoted: bool,
        slot: Slot,
        parent_head: Option<&str>,
        macros: &BTreeSet<&str>,
        out: &mut Vec<Form<'a>>,
    ) {
        match node.kind {
            Kind::Prefixed => {
                let prefix = node.children.first().map(|p| p.text(source)).unwrap_or("");
                let quoted = match prefix {
                    "," | ",@" | "#$" | "#$@" | "#+" | "#+@" => false,
                    "'" | "`" | "#~" | "#'" | "#`" => true,
                    _ => quoted,
                };
                for child in node.data() {
                    walk(child, source, quoted, slot, parent_head, macros, out);
                }
            }
            Kind::List => {
                let data: Vec<&Node> = node.data().collect();
                match slot {
                    Slot::Skip => {}
                    Slot::Clause => {
                        for child in &data {
                            walk(child, source, quoted, Slot::Expr, None, macros, out);
                        }
                    }
                    Slot::PatternClause => {
                        for child in data.iter().skip(1) {
                            walk(child, source, quoted, Slot::Expr, None, macros, out);
                        }
                    }
                    Slot::GuardSpec => {
                        for child in data.iter().skip(1) {
                            walk(child, source, quoted, Slot::Clause, None, macros, out);
                        }
                    }
                    Slot::Bindings => {
                        for pair in &data {
                            if pair.kind == Kind::List {
                                for init in pair.data().skip(1) {
                                    walk(init, source, quoted, Slot::Expr, None, macros, out);
                                }
                            }
                        }
                    }
                    Slot::Expr if quoted => {
                        // Quoted data: only unquoted parts are code.
                        for child in &data {
                            walk(child, source, true, Slot::Expr, None, macros, out);
                        }
                    }
                    Slot::Expr => {
                        let Some(head) = data.first() else { return };
                        out.push(Form {
                            node,
                            head,
                            parent_head: parent_head.map(str::to_string),
                        });
                        let name = (head.kind == Kind::Symbol).then(|| head.text(source));
                        let args = &data[1..];
                        if head.kind != Kind::Symbol {
                            walk(head, source, false, Slot::Expr, None, macros, out);
                        }
                        for (i, arg) in args.iter().enumerate() {
                            // A macro this document defines has an unknown shape,
                            // so nothing inside its arguments is checked.
                            let arg_slot = match name {
                                Some(n) if macros.contains(n) => Slot::Skip,
                                Some(n) => argument_slot(n, i + 1, args),
                                None => Slot::Expr,
                            };
                            walk(arg, source, false, arg_slot, name, macros, out);
                        }
                    }
                }
            }
            // Vectors and arrays are self-evaluating data; comments are not
            // code; atoms have nothing inside.
            _ => {}
        }
    }
    for child in snapshot.tree.root.data() {
        walk(child, source, false, Slot::Expr, None, &macros, &mut out);
    }
    out
}

pub fn calls(snapshot: &Snapshot) -> Vec<Call> {
    let source = &snapshot.text;
    code_forms(snapshot)
        .into_iter()
        .filter(|form| form.head.kind == Kind::Symbol)
        .map(|form| {
            let data: Vec<&Node> = form.node.data().collect();
            let args = &data[1..];
            let mut positional = 0;
            let mut keywords = Vec::new();
            for arg in args {
                if arg.kind == Kind::Keyword {
                    keywords.push((arg.text(source).to_string(), arg.range.clone()));
                } else if keywords.is_empty() {
                    positional += 1;
                }
            }
            let field_syntax = !args.is_empty()
                && args.iter().all(|a| {
                    a.kind == Kind::List && a.data().next().is_some_and(|n| n.kind == Kind::Symbol)
                });
            let fields = if field_syntax {
                args.iter()
                    .filter_map(|a| a.data().next())
                    .map(|n| (n.text(source).to_string(), n.range.clone()))
                    .collect()
            } else {
                Vec::new()
            };
            Call {
                head: form.head.text(source).to_string(),
                head_range: form.head.range.clone(),
                positional,
                keywords,
                complete: is_closed(form.node),
                field_syntax,
                fields,
                range: form.node.range.clone(),
                parent_head: form.parent_head,
            }
        })
        .collect()
}

fn local_macros(snapshot: &Snapshot) -> BTreeSet<&str> {
    let source = &snapshot.text;
    let mut out = BTreeSet::new();
    for form in snapshot
        .tree
        .root
        .descendants()
        .filter(|f| f.kind == Kind::List)
    {
        let data: Vec<&Node> = form.data().collect();
        let (Some(head), Some(name)) = (data.first(), data.get(1)) else {
            continue;
        };
        match head.text(source) {
            "define-syntax" | "define-syntax-parameter" if name.kind == Kind::Symbol => {
                out.insert(name.text(source));
            }
            "define-syntax-rule" if name.kind == Kind::List => {
                if let Some(n) = name.data().next().filter(|n| n.kind == Kind::Symbol) {
                    out.insert(n.text(source));
                }
            }
            "let-syntax" | "letrec-syntax" if name.kind == Kind::List => {
                for pair in name.data().filter(|p| p.kind == Kind::List) {
                    if let Some(n) = pair.data().next().filter(|n| n.kind == Kind::Symbol) {
                        out.insert(n.text(source));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Field names of records the document defines, by constructor name, read
/// from the shape of `define-record-type*`, `define-record-type`,
/// `define-configuration`, and similar definers: every symbol argument
/// before the first list may name a constructor, every list argument a
/// field.
pub fn local_record_fields(snapshot: &Snapshot) -> BTreeMap<String, BTreeSet<String>> {
    let source = &snapshot.text;
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for form in snapshot.tree.root.data().filter(|f| f.kind == Kind::List) {
        let data: Vec<&Node> = form.data().collect();
        let Some(head) = data.first().filter(|h| h.kind == Kind::Symbol) else {
            continue;
        };
        let head = head.text(source);
        if !head.starts_with("define-") || head == "define-module" || head == "define-syntax" {
            continue;
        }
        let mut constructors = Vec::new();
        let mut fields = BTreeSet::new();
        for arg in &data[1..] {
            match arg.kind {
                Kind::Symbol if fields.is_empty() => {
                    constructors.push(arg.text(source).to_string());
                }
                Kind::List => {
                    if let Some(name) = arg.data().next().filter(|n| n.kind == Kind::Symbol) {
                        // (define-record-type <t> (ctor a b) ...): the
                        // first list before any field names the constructor.
                        if head == "define-record-type"
                            && fields.is_empty()
                            && constructors.len() == 1
                        {
                            constructors.push(name.text(source).to_string());
                        } else {
                            fields.insert(name.text(source).to_string());
                        }
                    }
                }
                _ => {}
            }
        }
        if fields.is_empty() {
            continue;
        }
        for constructor in constructors {
            out.entry(constructor)
                .or_default()
                .extend(fields.iter().cloned());
        }
    }
    out
}

/// Codes distinguish semantic checks from reader errors, which carry none.
fn diagnostic(
    snapshot: &Snapshot,
    code: &str,
    range: std::ops::Range<usize>,
    severity: DiagnosticSeverity,
    message: String,
) -> Diagnostic {
    Diagnostic {
        range: snapshot.range(range),
        severity: Some(severity),
        code: Some(NumberOrString::String(code.into())),
        source: Some(SOURCE.into()),
        message,
        ..Default::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arity {
    pub required: usize,
    pub optional: usize,
    pub rest: bool,
    /// `None` when keywords are unknown (primitives report counts only).
    pub keywords: Option<Vec<String>>,
    pub allow_other_keys: bool,
}

/// The arity a call must satisfy: a case-lambda's clauses merged into one
/// over-approximation (fewest required, most accepted, keywords unioned), so
/// only calls no clause could accept are reported.
pub fn arity(binding: &Datum) -> Option<Arity> {
    if binding.assq("kind").and_then(Datum::as_symbol) != Some("procedure") {
        return None;
    }
    let clauses = crate::semantic::arity_clauses(binding);
    if clauses.is_empty() {
        return None;
    }
    let mut merged: Option<Arity> = None;
    for clause in clauses {
        let one = clause_arity(clause)?;
        merged = Some(match merged {
            None => one,
            Some(m) => Arity {
                required: m.required.min(one.required),
                optional: (m.required + m.optional).max(one.required + one.optional)
                    - m.required.min(one.required),
                rest: m.rest || one.rest,
                keywords: match (m.keywords, one.keywords) {
                    (Some(mut a), Some(b)) => {
                        for keyword in b {
                            if !a.contains(&keyword) {
                                a.push(keyword);
                            }
                        }
                        Some(a)
                    }
                    _ => None,
                },
                allow_other_keys: m.allow_other_keys || one.allow_other_keys,
            },
        });
    }
    merged
}

fn clause_arity(arguments: &Datum) -> Option<Arity> {
    let names = |key: &str| -> Option<Vec<String>> {
        match arguments.assq(key) {
            Some(Datum::List(items)) if items.first().and_then(Datum::as_symbol) == Some(key) => {
                Some(items.iter().skip(1).map(|d| d.to_string()).collect())
            }
            Some(Datum::List(items)) => Some(items.iter().map(|d| d.to_string()).collect()),
            Some(single) => Some(vec![single.to_string()]),
            None => None,
        }
    };
    if let Some(required) = arguments.assq("required-count").and_then(Datum::as_integer) {
        let optional = arguments
            .assq("optional-count")
            .and_then(Datum::as_integer)
            .unwrap_or(0);
        return Some(Arity {
            required: required as usize,
            optional: optional as usize,
            rest: arguments.assq("rest?") == Some(&Datum::Boolean(true)),
            keywords: None,
            allow_other_keys: true,
        });
    }
    let required = names("required").map(|n| n.len()).unwrap_or(0);
    let optional = names("optional").map(|n| n.len()).unwrap_or(0);
    let keywords = names("keyword").unwrap_or_default();
    let rest = matches!(arguments.assq("rest"), Some(Datum::Symbol(_)));
    Some(Arity {
        required,
        optional,
        rest,
        keywords: Some(keywords),
        allow_other_keys: arguments.assq("allow-other-keys?") == Some(&Datum::Boolean(true)),
    })
}

/// Only complete forms are checked: a form still being typed has too few
/// arguments by nature.
pub fn check_call(snapshot: &Snapshot, call: &Call, arity: &Arity) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if !call.complete {
        return out;
    }
    let given = call.positional;
    let max = arity.required + arity.optional;
    if given < arity.required {
        out.push(diagnostic(
            snapshot,
            "arity",
            call.head_range.clone(),
            DiagnosticSeverity::ERROR,
            format!(
                "{} expects at least {} argument{}, got {given}",
                call.head,
                arity.required,
                if arity.required == 1 { "" } else { "s" }
            ),
        ));
    } else if !arity.rest && given > max {
        out.push(diagnostic(
            snapshot,
            "arity",
            call.head_range.clone(),
            DiagnosticSeverity::ERROR,
            format!(
                "{} expects at most {max} argument{}, got {given}",
                call.head,
                if max == 1 { "" } else { "s" }
            ),
        ));
    }
    if let Some(known) = &arity.keywords {
        if !arity.allow_other_keys && !arity.rest {
            for (keyword, range) in &call.keywords {
                let name = keyword.trim_start_matches("#:");
                if !known.iter().any(|k| k == name) {
                    out.push(diagnostic(
                        snapshot,
                        "keyword",
                        range.clone(),
                        DiagnosticSeverity::ERROR,
                        if known.is_empty() {
                            format!("{} takes no keyword arguments", call.head)
                        } else {
                            format!(
                                "unknown keyword {keyword} for {}; expected {}",
                                call.head,
                                known
                                    .iter()
                                    .map(|k| format!("#:{k}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        },
                    ));
                }
            }
        }
    }
    out
}

/// Calls whose head is a literal: `(1 2)`, `("a")`, `(#t)`.
pub fn literal_heads(snapshot: &Snapshot) -> Vec<Diagnostic> {
    let source = &snapshot.text;
    code_forms(snapshot)
        .into_iter()
        .filter(|form| {
            matches!(
                form.head.kind,
                Kind::Number | Kind::String | Kind::Boolean | Kind::Char
            )
        })
        .map(|form| {
            diagnostic(
                snapshot,
                "not-a-procedure",
                form.head.range.clone(),
                DiagnosticSeverity::ERROR,
                format!("{} is not a procedure", form.head.text(source)),
            )
        })
        .collect()
}

pub fn check_record(snapshot: &Snapshot, call: &Call, fields: &Datum) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let Some(Datum::List(entries)) = fields.assq("fields") else {
        return out;
    };
    let known: BTreeMap<&str, &Datum> = entries
        .iter()
        .skip(1)
        .filter_map(|f| f.assq("name").and_then(Datum::as_symbol).map(|n| (n, f)))
        .collect();
    let record = fields
        .assq("name")
        .map(|n| n.to_string())
        .unwrap_or_default();
    let mut inherits = false;
    // Sugar names a constructor macro accepts for a field (`sha256` for
    // `origin`'s `hash`), mapped to the field they stand for.
    let mut aliases: BTreeMap<String, &str> = BTreeMap::new();
    for (name, field) in &known {
        let listed = match field.assq("aliases") {
            Some(Datum::List(items)) => items.clone(),
            Some(one) => vec![one.clone()],
            None => Vec::new(),
        };
        for alias in listed.iter().filter_map(Datum::as_symbol) {
            aliases.insert(alias.to_string(), name);
        }
    }
    // A form none of whose fields are known is probably not this record at
    // all (`local-file` is a macro over a record it does not construct).
    let plausible = call.fields.iter().any(|(name, _)| {
        name == "inherit"
            || known.contains_key(name.as_str())
            || aliases.contains_key(name.as_str())
    });
    if !plausible {
        return out;
    }
    let typing = (!call.complete).then(|| call.fields.len().saturating_sub(1));
    for (index, (name, range)) in call.fields.iter().enumerate() {
        if name == "inherit" {
            inherits = true;
            continue;
        }
        // The last field of an unclosed form is still being typed.
        if !known.contains_key(name.as_str())
            && !aliases.contains_key(name.as_str())
            && Some(index) != typing
        {
            out.push(diagnostic(
                snapshot,
                "unknown-field",
                range.clone(),
                DiagnosticSeverity::ERROR,
                format!("{record} has no field {name}"),
            ));
        }
    }
    // Missing required fields: only when the form is complete, nothing is
    // inherited, and the field set is fully known.
    if call.complete
        && !inherits
        && fields.assq("source").and_then(Datum::as_symbol) != Some("unavailable")
    {
        let written: BTreeSet<&str> = call
            .fields
            .iter()
            .map(|(n, _)| aliases.get(n.as_str()).copied().unwrap_or(n.as_str()))
            .collect();
        let missing: Vec<&str> = known
            .iter()
            .filter(|(name, field)| {
                !written.contains(*name)
                    && (field.assq("required?") == Some(&Datum::Boolean(true))
                        || matches!(field.assq("default"), Some(Datum::Pair(k, _)) if k.as_symbol() == Some("required")))
            })
            .map(|(name, _)| *name)
            .collect();
        if !missing.is_empty() {
            out.push(diagnostic(
                snapshot,
                "missing-field",
                call.head_range.clone(),
                DiagnosticSeverity::WARNING,
                format!(
                    "{record} is missing required field{} {}",
                    if missing.len() == 1 { "" } else { "s" },
                    missing.join(", ")
                ),
            ));
        }
    }
    out
}

/// Run every check that needs the worker. `None` when the worker is not up
/// or the workspace is restricted, so the caller publishes syntax
/// diagnostics alone rather than an empty semantic set.
pub async fn analyze(
    resolver: &Resolver,
    snapshot: &Snapshot,
    imports: &[Import],
) -> Option<Vec<Diagnostic>> {
    let identity = resolver.identity()?;
    let guix = guix::active(&identity);
    let mut out = literal_heads(snapshot);
    let calls = calls(snapshot);
    // Names the document binds itself are never checked against imports.
    let local: BTreeSet<String> = scope::visible_bindings(snapshot, 0)
        .into_iter()
        .map(|b| b.name)
        .collect();
    let local_records = local_record_fields(snapshot);
    let mut resolved: BTreeMap<String, Option<Datum>> = BTreeMap::new();
    // Record field sets by constructor name, for forms that resolved to a
    // Guix record macro; `None` when the name is not such a record.
    let mut records: BTreeMap<String, Option<Datum>> = BTreeMap::new();
    for call in &calls {
        if local.contains(&call.head)
            || scope::resolve_lexical(snapshot, call.head_range.start, &call.head).is_some()
        {
            continue;
        }
        // A direct argument of a record form named like one of its fields
        // is that field, whatever else the name means.
        if let Some(parent) = &call.parent_head {
            // `package/inherit` and friends take the fields of the record they
            // wrap.
            let parent = guix::underlying_record(parent).unwrap_or(parent.as_str());
            if local_records
                .get(parent)
                .is_some_and(|fields| fields.contains(&call.head))
            {
                continue;
            }
            if guix
                && !local.contains(parent)
                && scope::resolve_lexical(snapshot, call.head_range.start, parent).is_none()
            {
                let fields =
                    record_fields(resolver, imports, parent, &mut resolved, &mut records).await?;
                if fields.is_some_and(|f| guix::record_field_names(f).contains(&call.head)) {
                    continue;
                }
            }
        }
        let Some(lookup) = lookup(resolver, imports, &call.head, &mut resolved).await? else {
            continue;
        };
        let Some(binding) = lookup.assq("binding") else {
            continue;
        };
        if let Some(arity) = arity(binding) {
            out.extend(check_call(snapshot, call, &arity));
        } else if guix && call.field_syntax {
            let fields =
                record_fields(resolver, imports, &call.head, &mut resolved, &mut records).await?;
            if let Some(fields) = fields {
                out.extend(check_record(snapshot, call, fields));
            }
        }
    }
    Some(out)
}

/// The cached lookup of `name`; `None` (outer) when the worker is not
/// ready or the workspace is restricted.
async fn lookup(
    resolver: &Resolver,
    imports: &[Import],
    name: &str,
    resolved: &mut BTreeMap<String, Option<Datum>>,
) -> Option<Option<Datum>> {
    if let Some(cached) = resolved.get(name) {
        return Some(cached.clone());
    }
    let found = match resolver.lookup(imports, name).await {
        Ok(found) => found,
        Err(Resolved::NotReady) | Err(Resolved::Restricted(_)) => return None,
        Err(_) => None,
    };
    resolved.insert(name.to_string(), found.clone());
    Some(found)
}

/// The field description of `name` when it resolves to a Guix record
/// constructor macro, cached. Inner `None`: not a record.
async fn record_fields<'a>(
    resolver: &Resolver,
    imports: &[Import],
    name: &str,
    resolved: &mut BTreeMap<String, Option<Datum>>,
    records: &'a mut BTreeMap<String, Option<Datum>>,
) -> Option<Option<&'a Datum>> {
    if !records.contains_key(name) {
        let mut fields = None;
        if let Some(lookup) = lookup(resolver, imports, name, resolved).await? {
            let binding = lookup.assq("binding");
            let is_macro = binding
                .and_then(|b| b.assq("kind"))
                .and_then(Datum::as_symbol)
                == Some("macro");
            if is_macro {
                if let (Some(module), Some(exported)) = (
                    guix::lookup_module(&lookup),
                    binding
                        .and_then(|b| b.assq("name"))
                        .and_then(Datum::as_symbol),
                ) {
                    if let Resolved::Found(found) = resolver
                        .request(
                            "configuration",
                            guix::configuration_params(&module, exported),
                        )
                        .await
                    {
                        if guix::is_record(&found) {
                            fields = Some(found);
                        }
                    }
                }
            }
        }
        records.insert(name.to_string(), fields);
    }
    Some(records.get(name).unwrap().as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::Documents;
    use tower_lsp::lsp_types::{Position, TextDocumentItem, Url};

    fn snapshot(text: &str) -> Snapshot {
        Documents::default().open(TextDocumentItem {
            uri: Url::parse("file:///t.scm").unwrap(),
            language_id: "scheme".into(),
            version: 1,
            text: text.into(),
        })
    }

    #[test]
    fn calls_skip_quoted_and_staged_code() {
        let snap = snapshot("(f 1 #:k 2)\n'(g 1)\n`(h ,(i 2))\n#~(j #$(k 3))\n(l (m 1) (n))\n(o 1");
        let calls = calls(&snap);
        let heads: Vec<(&str, usize, bool)> = calls
            .iter()
            .map(|c| (c.head.as_str(), c.positional, c.complete))
            .collect();
        assert_eq!(
            heads,
            [
                ("f", 1, true),
                ("i", 1, true),
                ("k", 1, true),
                ("l", 2, true),
                ("m", 1, true),
                ("n", 0, true),
                ("o", 1, false)
            ]
        );
        assert_eq!(calls[0].keywords[0].0, "#:k");
        let l = calls.iter().find(|c| c.head == "l").unwrap();
        assert!(l.field_syntax);
        assert_eq!(
            l.fields.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["m", "n"]
        );
    }

    #[test]
    fn arity_checks_report_precisely() {
        let snap = snapshot("(connect)\n(connect \"h\" 80 90 100)\n(connect \"h\" #:timeout 1 #:nope 2)\n(fold + 0)\n(cons 1 2 3)\n(open");
        let named = arity(&Datum::parse_one("((kind . procedure) (arguments (required host) (optional port) (keyword timeout tls?) (allow-other-keys? . #f) (rest . #f)))").unwrap()).unwrap();
        assert_eq!(
            named,
            Arity {
                required: 1,
                optional: 1,
                rest: false,
                keywords: Some(vec!["timeout".into(), "tls?".into()]),
                allow_other_keys: false
            }
        );
        let counted = arity(&Datum::parse_one("((kind . procedure) (arguments (required-count . 2) (optional-count . 0) (rest? . #f)))").unwrap()).unwrap();
        // case-lambda: fold takes (kons knil list1), (kons knil list1 list2), or
        // (kons knil list1 . rest); merged, that is at least 3 with rest.
        let fold = arity(&Datum::parse_one("((kind . procedure) (arguments (required kons knil list1) (optional) (keyword) (allow-other-keys? . #f) (rest . #f)) (arities ((required kons knil list1) (optional) (keyword) (allow-other-keys? . #f) (rest . #f)) ((required kons knil list1 list2) (optional) (keyword) (allow-other-keys? . #f) (rest . #f)) ((required kons knil list1) (optional) (keyword) (allow-other-keys? . #f) (rest . rest))))").unwrap()).unwrap();
        assert_eq!(
            fold,
            Arity {
                required: 3,
                optional: 1,
                rest: true,
                keywords: Some(vec![]),
                allow_other_keys: false
            }
        );
        assert_eq!(counted.keywords, None);
        assert!(arity(&Datum::parse_one("((kind . macro))").unwrap()).is_none());

        let calls = calls(&snap);
        let messages: Vec<String> = calls
            .iter()
            .flat_map(|c| match c.head.as_str() {
                "connect" => check_call(&snap, c, &named),
                "cons" => check_call(&snap, c, &counted),
                "open" => check_call(&snap, c, &named),
                _ => Vec::new(),
            })
            .map(|d| d.message)
            .collect();
        assert_eq!(
            messages,
            [
                "connect expects at least 1 argument, got 0",
                "connect expects at most 2 arguments, got 4",
                "unknown keyword #:nope for connect; expected #:timeout, #:tls?",
                "cons expects at most 2 arguments, got 3",
            ]
        );
        // A rest parameter accepts any number of positionals; the incomplete
        // `(open` is never reported.
        let with_rest = Arity {
            rest: true,
            ..named.clone()
        };
        assert!(check_call(&snap, &calls[1], &with_rest).is_empty());
        let first = check_call(&snap, &calls[0], &named)[0].clone();
        assert_eq!(first.range.start, Position::new(0, 1));
        assert_eq!(first.severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn literal_heads_and_record_fields() {
        let snap = snapshot("(1 2)\n'(3 4)\n(\"s\")\n(openssh-configuration (port-number 22) (nonsense 1))\n(file-system (mount-point \"/\"))\n(file-system (inherit base) (type \"ext4\"))\n(file-system (device \"d\") (mount-point \"/\") (type \"ext4\"))");
        let literal: Vec<String> = literal_heads(&snap)
            .into_iter()
            .map(|d| d.message)
            .collect();
        assert_eq!(
            literal,
            ["1 is not a procedure", "\"s\" is not a procedure"]
        );

        let ssh = Datum::parse_one("((name . openssh-configuration) (source . source-file) (fields ((name . port-number) (required? . #f)) ((name . x11-forwarding?) (required? . #f))))").unwrap();
        let fs = Datum::parse_one("((name . file-system) (source . source-file) (fields ((name . device) (required? . #t)) ((name . mount-point) (required? . #t)) ((name . type) (required? . #t)) ((name . flags) (required? . #f))))").unwrap();
        let calls = calls(&snap);
        let messages: Vec<String> = calls
            .iter()
            .flat_map(|c| match c.head.as_str() {
                "openssh-configuration" => check_record(&snap, c, &ssh),
                "file-system" => check_record(&snap, c, &fs),
                _ => Vec::new(),
            })
            .map(|d| format!("{:?} {}", d.severity.unwrap(), d.message))
            .collect();
        assert_eq!(
            messages,
            [
                "Error openssh-configuration has no field nonsense",
                "Warning file-system is missing required fields device, type",
            ]
        );
    }

    #[test]
    fn calls_know_their_parent_form() {
        let snapshot = snapshot("(operating-system (bootloader (bootloader-configuration (bootloader grub))) '(quoted (x)))");
        let calls = calls(&snapshot);
        let parents: Vec<(&str, Option<&str>)> = calls
            .iter()
            .map(|c| (c.head.as_str(), c.parent_head.as_deref()))
            .collect();
        assert_eq!(
            parents,
            [
                ("operating-system", None),
                ("bootloader", Some("operating-system")),
                ("bootloader-configuration", Some("bootloader")),
                ("bootloader", Some("bootloader-configuration")),
            ]
        );
    }

    #[test]
    fn binding_pattern_and_module_positions_are_not_calls() {
        let text = "(define-module (m) #:use-module ((srfi srfi-1) #:hide (delete)) #:export (version))\n\
                    (let* ((version (compute)) (location (where))) (match x ((#f y) (f y)) ((\"profile\" p) p)) (case y ((1 2) (g)) (else (h))))\n\
                    (define* (name #:key (version 1)) (cond ((test) => use) (else 0)))\n\
                    (lambda (location) (parameterize ((%current-system s)) (body)))\n\
                    (modify-services %base (delete avahi) (guix-service-type config => (guix-configuration (inherit config))))\n\
                    (guard (e ((error? e) (recover))) (risky))\n\
                    (define-record-type* <thing> thing make-thing thing? (version thing-version))\n\
                    (match-record r <thing> (version) (use version))";
        let parsed = snapshot(text);
        let found = calls(&parsed);
        let heads: Vec<&str> = found.iter().map(|c| c.head.as_str()).collect();
        assert_eq!(
            heads,
            [
                "define-module",
                "let*",
                "compute",
                "where",
                "match",
                "f",
                "case",
                "g",
                "h",
                "define*",
                "cond",
                "test",
                "lambda",
                "parameterize",
                "body",
                "modify-services",
                "guix-configuration",
                "inherit",
                "guard",
                "error?",
                "recover",
                "risky",
                "define-record-type*",
                "match-record",
                "use",
            ]
        );
        assert!(
            literal_heads(&parsed).is_empty(),
            "patterns are not literal heads"
        );
        assert_eq!(literal_heads(&snapshot("(1 2) '(3 4) #(5 6)")).len(), 1);
    }

    #[test]
    fn local_definitions_shape_the_analysis() {
        let text = "(define-syntax-rule (cpu-models (date name) ...) (list (cons date name) ...))\n\
                    (define-record-type* <boot> boot make-boot boot? (version boot-version) (label boot-label))\n\
                    (define-configuration role-configuration (host (string \"x\") \"doc\"))\n\
                    (define-record-type <point> (point x y) point? (x point-x))\n\
                    (define-split-configuration client (bind? (bind #f) \"doc\"))\n\
                    (cpu-models (\"2006\" \"core2\") (\"2010\" \"west\"))\n\
                    (boot (version 1) (label \"a\"))";
        let parsed = snapshot(text);
        assert!(
            literal_heads(&parsed).is_empty(),
            "macro arguments are not calls"
        );
        let found = calls(&parsed);
        let heads: Vec<&str> = found.iter().map(|c| c.head.as_str()).collect();
        assert_eq!(
            heads,
            [
                "define-syntax-rule",
                "define-record-type*",
                "define-configuration",
                "define-record-type",
                "define-split-configuration",
                "cpu-models",
                "boot",
                "version",
                "label"
            ]
        );
        let records = local_record_fields(&parsed);
        assert_eq!(
            records["boot"],
            ["label", "version"].map(String::from).into_iter().collect()
        );
        assert_eq!(
            records["role-configuration"],
            ["host"].map(String::from).into_iter().collect()
        );
        assert_eq!(
            records["point"],
            ["x"].map(String::from).into_iter().collect()
        );
        assert!(records["client"].contains("bind?"));
    }
}
