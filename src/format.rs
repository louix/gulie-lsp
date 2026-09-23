//! Structural formatting: reindent each line by the shape of the form it
//! starts in, the way Emacs' `scheme-mode` does, and trim trailing blanks.
//! See `docs/FORMATTING.md` for the rules and their deviations from Emacs.

use crate::documents::Snapshot;
use crate::syntax::{parse, Kind, Node};
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use tower_lsp::lsp_types::{Position, TextEdit};

/// Width of one body indentation step (`lisp-body-indent`).
const BODY: i64 = 2;
/// Column advance of a tab in existing text (`tab-width` in Guix sources).
const TAB: i64 = 8;

/// The `scheme-indent-function` properties of Emacs 31's `scheme-mode`, in
/// full, so Emacs-indented files come back unchanged.
const SCHEME: &[(&str, Rule)] = &[
    ("access-components", Rule::Args(1)),
    ("and-let*", Rule::Args(1)),
    ("assignment-components", Rule::Args(1)),
    ("begin", Rule::Args(0)),
    ("call-with-input-file", Rule::Args(1)),
    ("call-with-output-file", Rule::Args(1)),
    ("call-with-port", Rule::Args(1)),
    ("call-with-values", Rule::Args(1)),
    ("case", Rule::Args(1)),
    ("check-case", Rule::Args(1)),
    ("combination-components", Rule::Args(1)),
    ("comment-components", Rule::Args(1)),
    ("conditional-components", Rule::Args(1)),
    ("declaration-components", Rule::Args(1)),
    ("define-library", Rule::Args(1)),
    ("define-record-type", Rule::Args(1)),
    ("define-values", Rule::Args(1)),
    ("definition-components", Rule::Args(1)),
    ("delay", Rule::Args(0)),
    ("delay-components", Rule::Args(1)),
    ("disjunction-components", Rule::Args(1)),
    ("do", Rule::Args(2)),
    ("dynamic-wind", Rule::Args(3)),
    ("element", Rule::Args(1)),
    ("eval-when", Rule::Args(1)),
    ("fluid-let", Rule::Args(1)),
    ("guard", Rule::Args(1)),
    ("in-package", Rule::Args(1)),
    ("in-package-components", Rule::Args(1)),
    ("lambda", Rule::Args(1)),
    ("lambda-checked", Rule::Args(1)),
    ("lambda-components", Rule::Args(1)),
    ("lambda-components*", Rule::Args(1)),
    ("lambda-components**", Rule::Args(1)),
    ("let", Rule::Let),
    ("let*", Rule::Args(1)),
    ("let*-values", Rule::Args(1)),
    ("let-optionals", Rule::Args(2)),
    ("let-optionals*", Rule::Args(2)),
    ("let-syntax", Rule::Args(1)),
    ("let-values", Rule::Args(1)),
    ("letrec", Rule::Args(1)),
    ("letrec*", Rule::Args(1)),
    ("letrec-syntax", Rule::Args(1)),
    ("library", Rule::Args(1)),
    ("list-search-negative", Rule::Args(1)),
    ("list-search-positive", Rule::Args(1)),
    ("list-transform-negative", Rule::Args(1)),
    ("list-transform-positive", Rule::Args(1)),
    ("local-declare", Rule::Args(1)),
    ("macro", Rule::Args(1)),
    ("make", Rule::Args(1)),
    ("make-environment", Rule::Args(0)),
    ("match", Rule::Args(1)),
    ("match-lambda", Rule::Args(0)),
    ("match-lambda*", Rule::Args(0)),
    ("match-let", Rule::Let),
    ("match-let*", Rule::Args(1)),
    ("match-letrec", Rule::Args(1)),
    ("mode", Rule::Args(1)),
    ("named-lambda", Rule::Args(1)),
    ("open-block-components", Rule::Args(1)),
    ("opt*-lambda", Rule::Args(1)),
    ("opt-lambda", Rule::Args(1)),
    ("parameterize", Rule::Args(1)),
    ("pathname-components", Rule::Args(1)),
    ("procedure-components", Rule::Args(1)),
    ("receive", Rule::Args(2)),
    ("root", Rule::Args(1)),
    ("sequence", Rule::Args(0)),
    ("sequence-components", Rule::Args(1)),
    ("style", Rule::Args(1)),
    ("syntax-case", Rule::Args(2)),
    ("syntax-rules", Rule::Defun),
    ("syntax-table-define", Rule::Args(2)),
    ("test-group", Rule::Args(1)),
    ("test-group-with-cleanup", Rule::Args(1)),
    ("unless", Rule::Args(1)),
    ("using-syntax", Rule::Args(1)),
    ("when", Rule::Args(1)),
    ("with-input-from-file", Rule::Args(1)),
    ("with-input-from-port", Rule::Args(1)),
    ("with-input-from-string", Rule::Args(1)),
    ("with-mode", Rule::Args(1)),
    ("with-output-to-file", Rule::Args(1)),
    ("with-output-to-port", Rule::Args(1)),
    ("with-output-to-string", Rule::Args(0)),
    ("with-syntax", Rule::Args(1)),
    ("with-values", Rule::Args(1)),
    ("λ", Rule::Args(1)),
];

/// Guile forms Emacs does not know, indented the way Guile and Guix do.
const GUILE: &[(&str, Rule)] = &[
    ("case-lambda", Rule::Args(0)),
    ("case-lambda*", Rule::Args(0)),
    ("lambda*", Rule::Args(1)),
    ("syntax-parameterize", Rule::Args(1)),
    ("while", Rule::Args(1)),
    ("with-fluids", Rule::Args(1)),
];

/// How a head symbol indents its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// The first `n` arguments are distinguished; the rest are body forms.
    Args(u8),
    /// Like `define`: body forms from the first argument that starts a line.
    Defun,
    /// `let`: two distinguished arguments when named, else one.
    Let,
}

/// Head symbol to rule. Later layers override earlier ones: built-in
/// Scheme, then library rules, then workspace `.dir-locals.el`, then the
/// user's settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    rules: HashMap<String, Rule>,
    /// Characters that act as reader prefixes on symbols, as `$` does for
    /// gexps under Guix, so a head of `$` alone belongs to the next datum.
    prefixes: String,
    /// `#:prefix` values of the document's imports, so that
    /// `(sys.operating-system ...)` indents like `(operating-system ...)`.
    import_prefixes: Vec<String>,
}

impl Rules {
    pub fn scheme() -> Rules {
        let mut rules = Rules::default();
        for (name, rule) in SCHEME.iter().chain(GUILE) {
            rules.rules.insert(name.to_string(), *rule);
        }
        rules
    }

    pub fn extend<'a>(&mut self, rules: impl IntoIterator<Item = (&'a str, u8)>) {
        for (name, n) in rules {
            self.rules.insert(name.to_string(), Rule::Args(n));
        }
    }

    pub fn treat_as_prefixes(&mut self, chars: &str) {
        self.prefixes.push_str(chars);
    }

    /// Also look rules up through these import prefixes, for heads the
    /// rules do not name directly.
    pub fn strip_import_prefixes<'a>(&mut self, prefixes: impl IntoIterator<Item = &'a str>) {
        for prefix in prefixes {
            if !prefix.is_empty() && !self.import_prefixes.iter().any(|p| p == prefix) {
                self.import_prefixes.push(prefix.to_string());
            }
        }
    }

    fn is_prefix_only(&self, name: &str) -> bool {
        !name.is_empty() && name.chars().all(|c| self.prefixes.contains(c))
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The rule for `head`, given the form's data (head first). Without an
    /// explicit rule, a name starting with `def` indents like `define`.
    fn rule(&self, head: &str, data: &[&Node]) -> Option<Rule> {
        let explicit = self.rules.get(head).or_else(|| {
            self.import_prefixes
                .iter()
                .filter_map(|p| head.strip_prefix(p.as_str()))
                .find_map(|name| self.rules.get(name))
        });
        match explicit {
            Some(Rule::Let) => {
                let named = data
                    .get(1)
                    .is_some_and(|n| matches!(n.kind, Kind::Symbol | Kind::Number));
                Some(Rule::Args(if named { 2 } else { 1 }))
            }
            Some(rule) => Some(*rule),
            None if head.len() > 3 && head.starts_with("def") => Some(Rule::Defun),
            None => None,
        }
    }
}
/// Rules declared in an Emacs `.dir-locals.el`, as Guix and its channels do:
/// `(put 'name 'scheme-indent-function N)` anywhere in the file.
pub fn dir_locals_rules(text: &str) -> Vec<(String, u8)> {
    fn quoted<'a>(node: &Node, text: &'a str) -> Option<&'a str> {
        match node.kind {
            Kind::Prefixed if node.children[0].text(text) == "'" => {
                let datum = node.data().next()?;
                (datum.kind == Kind::Symbol).then(|| datum.text(text))
            }
            Kind::List => {
                let items: Vec<&Node> = node.data().collect();
                match items.as_slice() {
                    [q, s] if q.text(text) == "quote" && s.kind == Kind::Symbol => {
                        Some(s.text(text))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
    let tree = parse(text);
    let mut out = Vec::new();
    for node in tree.root.descendants().filter(|n| n.kind == Kind::List) {
        let items: Vec<&Node> = node.data().collect();
        let [put, name, property, count] = items.as_slice() else {
            continue;
        };
        if put.text(text) != "put" || quoted(property, text) != Some("scheme-indent-function") {
            continue;
        }
        if let (Some(name), Ok(n)) = (quoted(name, text), count.text(text).parse::<u8>()) {
            out.push((name.to_string(), n));
        }
    }
    out
}

/// Rules from a file's own Emacs `Local Variables:` trailer, such as
///
/// ```text
/// ;;; Local Variables:
/// ;;; eval: (put 'with-thing 'scheme-indent-function 1)
/// ;;; End:
/// ```
///
/// Only the last few kilobytes are examined, as Emacs does.
pub fn file_local_rules(text: &str) -> Vec<(String, u8)> {
    let tail_start = text.len().saturating_sub(3000);
    let tail = &text[text.ceil_char_boundary(tail_start)..];
    let Some(at) = tail.rfind("Local Variables:") else {
        return Vec::new();
    };
    let block = &tail[at..];
    // Everything before the marker on its line is the comment prefix.
    let prefix = tail[..at].rsplit('\n').next().unwrap_or("");
    let mut forms = String::new();
    for line in block.lines().skip(1) {
        let Some(rest) = line.strip_prefix(prefix) else {
            break;
        };
        let rest = rest.trim();
        if rest == "End:" {
            break;
        }
        if let Some(form) = rest.strip_prefix("eval:") {
            forms.push_str(form);
            forms.push('\n');
        }
    }
    dir_locals_rules(&forms)
}

/// Add the rules a document carries itself: its `Local Variables:`
/// trailer and the `#:prefix` values of its imports.
pub fn add_document_rules(rules: &mut Rules, snapshot: &Snapshot) {
    let local = file_local_rules(&snapshot.text);
    rules.extend(local.iter().map(|(name, n)| (name.as_str(), *n)));
    let imports = crate::scope::imports(snapshot);
    rules.strip_import_prefixes(imports.iter().filter_map(|i| i.prefix.as_deref()));
}

/// Rules for a document outside the server, with no worker to ask: Scheme,
/// then Guix when `guix` is set or the document imports it, then the
/// nearest `.dir-locals.el` at or above `dir`, then the document's own.
pub fn standalone_rules(snapshot: &Snapshot, dir: Option<&Path>, guix: bool) -> Rules {
    use crate::adapters::guix as adapter;
    let mut rules = Rules::scheme();
    if guix || adapter::imports_guix(snapshot) {
        rules.extend(adapter::indent_rules());
        rules.treat_as_prefixes(adapter::INDENT_PREFIXES);
    }
    let mut dir = dir;
    while let Some(d) = dir {
        if let Ok(text) = std::fs::read_to_string(d.join(".dir-locals.el")) {
            let locals = dir_locals_rules(&text);
            rules.extend(locals.iter().map(|(name, n)| (name.as_str(), *n)));
            break;
        }
        dir = d.parent();
    }
    add_document_rules(&mut rules, snapshot);
    rules
}

/// Where formatting stopped short because of a reader error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    /// First line left untouched (zero-based).
    pub line: u32,
    pub message: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub edits: Vec<TextEdit>,
    pub stopped: Option<Stop>,
}

/// Edits that reindent `lines` (zero-based, end exclusive; the whole
/// document when `None`) and trim their trailing whitespace. Lines before
/// the range are never changed but do determine the indentation of lines
/// inside it. Edits are sorted and disjoint.
pub fn format(snapshot: &Snapshot, rules: &Rules, lines: Option<Range<u32>>) -> Outcome {
    let text: &str = &snapshot.text;
    let line_count = snapshot.lines.line_count();
    let mut formatter = Formatter {
        text,
        snapshot,
        rules,
        shift: vec![0; line_count],
        first_datum: vec![None; line_count],
        literals: Vec::new(),
    };
    for node in snapshot.tree.root.descendants() {
        if node.kind.is_datum() {
            let line = snapshot.lines.line_of(node.range.start);
            formatter.first_datum[line].get_or_insert(node.range.start);
        }
        if matches!(
            node.kind,
            Kind::String | Kind::BlockComment | Kind::Char | Kind::Unknown
        ) {
            formatter.literals.push(node.range.clone());
        }
    }
    let stopped = snapshot.tree.errors.first().map(|error| Stop {
        line: snapshot.lines.line_of(error.range.start) as u32,
        message: error.message,
    });
    let limit = stopped
        .as_ref()
        .map_or(line_count, |stop| stop.line as usize);
    let wanted = |line: usize| {
        line < limit
            && lines
                .as_ref()
                .is_none_or(|range| range.contains(&(line as u32)))
    };

    let mut edits = Vec::new();
    for line in 0..line_count {
        let start = snapshot.lines.line_start(line).unwrap();
        let mut end = snapshot
            .lines
            .line_start(line + 1)
            .map_or(text.len(), |next| next - 1);
        if text[start..end].ends_with('\r') {
            end -= 1;
        }
        let content = &text[start..end];
        let first = start + content.len() - content.trim_start_matches([' ', '\t']).len();
        if !wanted(line) {
            continue;
        }
        let desired = if formatter.inside_literal(start) {
            // Inside a string or block comment: leave the line alone.
            None
        } else if first == end {
            Some(0)
        } else {
            match &text[first..] {
                // Emacs leaves `;;;` alone and moves `;` to the comment
                // column; neither is touched here.
                t if t.starts_with(";;;") || (t.starts_with(';') && !t.starts_with(";;")) => None,
                _ => Some(formatter.indentation(first)),
            }
        };
        if let Some(desired) = desired {
            let current = &text[start..first];
            formatter.shift[line] = desired - width(current);
            let replacement = " ".repeat(desired as usize);
            if current != replacement {
                edits.push(TextEdit {
                    range: snapshot.range(start..first),
                    new_text: replacement,
                });
            }
        }
        // Trailing blanks, unless the line is blank (handled above) or the
        // blanks belong to a string or block comment.
        let trimmed = content.trim_end_matches([' ', '\t']);
        let trailing = start + trimmed.len();
        if first < end && trailing < end && !formatter.inside_literal(trailing) {
            edits.push(TextEdit {
                range: snapshot.range(trailing..end),
                new_text: String::new(),
            });
        }
    }
    Outcome { edits, stopped }
}

/// Apply `edits` (sorted, disjoint, as produced by [`format`]) to `text`.
pub fn apply(text: &str, edits: &[TextEdit]) -> String {
    let lines = crate::syntax::position::LineIndex::new(text);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for edit in edits {
        let start = lines.offset(text, position(edit.range.start)).unwrap();
        let end = lines.offset(text, position(edit.range.end)).unwrap();
        out.push_str(&text[cursor..start]);
        out.push_str(&edit.new_text);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

fn position(p: Position) -> crate::syntax::position::Position {
    crate::syntax::position::Position {
        line: p.line,
        character: p.character,
    }
}

/// Display width of leading whitespace, with tabs to the next tab stop.
fn width(blank: &str) -> i64 {
    blank.chars().fold(0, |col, ch| {
        if ch == '\t' {
            (col / TAB + 1) * TAB
        } else {
            col + 1
        }
    })
}

struct Formatter<'a> {
    text: &'a str,
    snapshot: &'a Snapshot,
    rules: &'a Rules,
    /// Per line: how far its tokens moved right after reindentation.
    shift: Vec<i64>,
    /// Per line: the outermost datum that starts on it, if any.
    first_datum: Vec<Option<usize>>,
    /// Ranges of string, character, and block-comment tokens.
    literals: Vec<Range<usize>>,
}

impl Formatter<'_> {
    fn line_of(&self, offset: usize) -> usize {
        self.snapshot.lines.line_of(offset)
    }

    fn inside_literal(&self, offset: usize) -> bool {
        let next = self.literals.partition_point(|r| r.start < offset);
        next > 0 && offset < self.literals[next - 1].end
    }

    /// Column of `offset` once the lines before it have been reindented.
    fn column(&self, offset: usize) -> i64 {
        let line = self.line_of(offset);
        let start = self.snapshot.lines.line_start(line).unwrap();
        width(&self.text[start..offset]) + self.shift[line]
    }

    /// Column of the outermost datum starting on the line of `offset`.
    fn line_column(&self, offset: usize) -> i64 {
        let first = self.first_datum[self.line_of(offset)].unwrap_or(offset);
        self.column(first)
    }

    /// Innermost list, vector, or array whose interior contains `point`.
    fn container(&self, point: usize) -> &Node {
        let mut node = &self.snapshot.tree.root;
        let mut container = node;
        'descend: loop {
            for child in &node.children {
                let inside = child.range.start < point && point < child.range.end;
                let open = child
                    .children
                    .first()
                    .map_or(child.range.end, |c| c.range.end);
                if inside && !child.children.is_empty() && point >= open {
                    node = child;
                    if matches!(child.kind, Kind::List | Kind::Vector | Kind::Array) {
                        container = child;
                    }
                    continue 'descend;
                }
            }
            return container;
        }
    }

    /// The column the token at `point` (the first on its line) should start
    /// in.
    fn indentation(&self, point: usize) -> i64 {
        let form = self.container(point);
        if form.kind == Kind::Root {
            return 0;
        }
        let open = &form.children[0];
        let open_col = self.column(open.range.end - 1);
        let mut data: Vec<&Node> = form.data().collect();
        // A head made only of prefix characters, such as `$` in a match
        // pattern under Guix, is a prefix of the datum after it.
        if data.len() >= 2
            && data[1].range.start < point
            && data[0].kind == Kind::Symbol
            && self.rules.is_prefix_only(data[0].text(self.text))
        {
            data.remove(0);
        }
        let before: Vec<&Node> = data
            .iter()
            .copied()
            .take_while(|d| d.range.start < point)
            .collect();
        let (Some(head), Some(last)) = (before.first(), before.last()) else {
            // Nothing but the open paren (and comments) precedes the line.
            return open_col + 1;
        };
        let last_on_opener = self.line_of(last.range.start) == self.line_of(open.range.start);
        // A quoted or unquoted symbol counts as a symbol head, named by the
        // symbol itself, since Emacs skips reader prefixes.
        let mut symbol = *head;
        while symbol.kind == Kind::Prefixed {
            match symbol.data().next() {
                Some(datum) => symbol = datum,
                None => break,
            }
        }
        if !matches!(symbol.kind, Kind::Symbol | Kind::Number) {
            // Data: under the first element, or under the first datum of the
            // previous element's line.
            return if last_on_opener {
                self.column(head.range.start)
            } else {
                self.line_column(last.range.start)
            };
        }
        // A call: arguments align under the first argument when one shares
        // the head's line, else under the previous argument's line. A space
        // after the open paren asks for alignment under the head instead.
        let spaced = self.text[open.range.end..].starts_with([' ', '\t']);
        let normal = if last_on_opener {
            match data.get(1) {
                Some(first_argument) if before.len() > 1 && !spaced => {
                    self.column(first_argument.range.start)
                }
                _ => self.column(head.range.start),
            }
        } else {
            self.line_column(last.range.start)
        };
        let name = symbol.text(self.text);
        let stripped = name.trim_start_matches(|c| self.rules.prefixes.contains(c));
        let name = if stripped.is_empty() { name } else { stripped };
        // Which argument (1-based) starts this line.
        let k = u8::try_from(before.len()).unwrap_or(u8::MAX);
        match self.rules.rule(name, &data) {
            Some(Rule::Args(n)) => special(n, k, open_col, normal),
            Some(Rule::Defun | Rule::Let) => {
                if last_on_opener {
                    open_col + BODY
                } else {
                    normal
                }
            }
            None => normal,
        }
    }
}

/// `lisp-indent-specform`: argument `k` of a form with `n` distinguished
/// arguments.
fn special(n: u8, k: u8, open_col: i64, normal: i64) -> i64 {
    let body = open_col + BODY;
    if k <= n {
        if k <= 2 {
            open_col + 2 * BODY
        } else {
            normal
        }
    } else if k == n + 1 && (n == 0 || body <= normal) {
        body
    } else {
        normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::Documents;
    use tower_lsp::lsp_types::{TextDocumentItem, Url};

    fn snapshot(text: &str) -> Snapshot {
        Documents::default().open(TextDocumentItem {
            uri: Url::parse("file:///f.scm").unwrap(),
            language_id: "scheme".into(),
            version: 1,
            text: text.into(),
        })
    }

    fn formatted(text: &str) -> String {
        let outcome = format(&snapshot(text), &Rules::scheme(), None);
        apply(text, &outcome.edits)
    }

    fn check(source: &str, expected: &str) {
        assert_eq!(formatted(source), expected, "formatting {source:?}");
        assert_eq!(formatted(expected), expected, "idempotence on {expected:?}");
    }

    #[test]
    fn special_forms_distinguish_arguments_from_bodies() {
        check(
            "(define (f x)\n\"doc\"\n(let ((y 1))\n(+ x y)))",
            "(define (f x)\n  \"doc\"\n  (let ((y 1))\n    (+ x y)))",
        );
        check(
            "(let loop ((i 0))\n(when (< i 3)\n(loop (+ i 1))))",
            "(let loop ((i 0))\n  (when (< i 3)\n    (loop (+ i 1))))",
        );
        check("(let\n((x 1))\nx)", "(let\n    ((x 1))\n  x)");
        check("(lambda (x)\ny\nz)", "(lambda (x)\n  y\n  z)");
        check("(begin\nx\n  y)", "(begin\n  x\n  y)");
        check(
            "(do ((i 0 (+ i 1)))\n((= i 3))\n(body))",
            "(do ((i 0 (+ i 1)))\n    ((= i 3))\n  (body))",
        );
    }

    #[test]
    fn calls_align_under_the_first_argument_or_the_head() {
        check("(list 1\n2\n   3)", "(list 1\n      2\n      3)");
        check("(list\n1\n2)", "(list\n 1\n 2)");
        check("(if a\nb\nc)", "(if a\n    b\n    c)");
        check("(cond\n((a) b)\n(else c))", "(cond\n ((a) b)\n (else c))");
        check(
            "(string-append a\n   b) ; c",
            "(string-append a\n               b) ; c",
        );
    }

    #[test]
    fn data_aligns_under_the_first_element() {
        // A number head is a word to Emacs, so this is a call.
        check("'(1 2\n3)", "'(1 2\n    3)");
        check("'(\"a\"\n\"b\")", "'(\"a\"\n  \"b\")");
        check("#(1\n2)", "#(1\n  2)");
        check("((a)\nb)", "((a)\n b)");
        check(
            "`((\"alice\" ,(f))\n(\"bob\" 2))",
            "`((\"alice\" ,(f))\n  (\"bob\" 2))",
        );
        check(
            "(foo x (bar\ny) z\nw)",
            "(foo x (bar\n        y) z\n        w)",
        );
    }

    #[test]
    fn comments_strings_and_prefixes_are_preserved() {
        check(
            "(define x\n  ;; doc\n1)\n;;; top\n; keep\n(f\n#~(g\n#$x)\n#;(h)\ni)",
            "(define x\n  ;; doc\n  1)\n;;; top\n; keep\n(f\n #~(g\n    #$x)\n #;(h)\n i)",
        );
        check(
            "(display \"multi\n  line\n\")\n  (x)",
            "(display \"multi\n  line\n\")\n(x)",
        );
        check(
            "#!/usr/bin/env guile\n  !#\n  (x)",
            "#!/usr/bin/env guile\n  !#\n(x)",
        );
        check("#|\n  block\n  |#\n  (x)   \n", "#|\n  block\n  |#\n(x)\n");
        check("(f \"a  \"   \n  \tb)\t", "(f \"a  \"\n   b)");
    }

    #[test]
    fn tabs_and_blank_lines() {
        check("(f\n\ta\n\t\t\n)", "(f\n a\n\n )");
        check("(f a\r\nb)\r\n", "(f a\r\n   b)\r\n");
    }

    #[test]
    fn stops_at_the_first_reader_error() {
        let text = "(define x\n1)\n(broken\n(y\n";
        let outcome = format(&snapshot(text), &Rules::scheme(), None);
        assert_eq!(outcome.stopped.as_ref().map(|s| s.line), Some(2));
        assert_eq!(
            apply(text, &outcome.edits),
            "(define x\n  1)\n(broken\n(y\n"
        );
        let text = "(x\n  ))\n(y\nz)";
        let outcome = format(&snapshot(text), &Rules::scheme(), None);
        assert_eq!(outcome.stopped.as_ref().map(|s| s.line), Some(1));
        assert_eq!(apply(text, &outcome.edits), "(x\n  ))\n(y\nz)");
    }

    #[test]
    fn range_formatting_uses_but_keeps_earlier_lines() {
        let text = "(define (f)\n      (let ((x 1))\nx\ny))";
        let outcome = format(&snapshot(text), &Rules::scheme(), Some(2..3));
        assert_eq!(
            apply(text, &outcome.edits),
            "(define (f)\n      (let ((x 1))\n        x\ny))"
        );
    }

    #[test]
    fn rules_layer_and_come_from_dir_locals() {
        let mut rules = Rules::scheme();
        rules.extend([("my-let", 1), ("lambda", 0)]);
        let text = "(my-let (x)\nbody)\n(lambda\n(x)\ny)";
        let outcome = format(&snapshot(text), &rules, None);
        assert_eq!(
            apply(text, &outcome.edits),
            "(my-let (x)\n  body)\n(lambda\n  (x)\n  y)"
        );
        let locals = "((scheme-mode . ((eval . (put 'foo-configuration 'scheme-indent-function 0))\n (eval . (put (quote with-thing) (quote scheme-indent-function) 2)))))";
        assert_eq!(
            dir_locals_rules(locals),
            vec![
                ("foo-configuration".to_string(), 0),
                ("with-thing".to_string(), 2)
            ]
        );
    }

    #[test]
    fn import_prefixes_reach_the_rules() {
        let mut rules = Rules::scheme();
        rules.extend([("operating-system", 0)]);
        let text = "(sys.operating-system\n(host-name \"h\"))\n(sys.when x\ny)\n(other.when x\ny)";
        let plain = apply(text, &format(&snapshot(text), &rules, None).edits);
        assert!(plain.starts_with("(sys.operating-system\n (host-name"));
        rules.strip_import_prefixes(["sys."]);
        assert_eq!(
            apply(text, &format(&snapshot(text), &rules, None).edits),
            "(sys.operating-system\n  (host-name \"h\"))\n(sys.when x\n  y)\n(other.when x\n            y)"
        );
    }

    #[test]
    fn file_local_variables_and_prefix_characters() {
        let text = "(with-thing x\ny)\n\n;;; Local Variables:\n;;; eval: (put 'with-thing 'scheme-indent-function 1)\n;;; End:\n";
        assert_eq!(file_local_rules(text), vec![("with-thing".to_string(), 1)]);
        let mut rules = Rules::scheme();
        let local = file_local_rules(text);
        rules.extend(local.iter().map(|(n, c)| (n.as_str(), *c)));
        assert!(apply(text, &format(&snapshot(text), &rules, None).edits)
            .starts_with("(with-thing x\n  y)"));
        rules.treat_as_prefixes("$~+");
        let text = "(match x\n(($ <t> a b\nc) d))";
        assert_eq!(
            apply(text, &format(&snapshot(text), &rules, None).edits),
            "(match x\n  (($ <t> a b\n          c) d))"
        );
        assert_eq!(
            apply(
                "(+ 1\n2)",
                &format(&snapshot("(+ 1\n2)"), &rules, None).edits
            ),
            "(+ 1\n   2)"
        );
    }
}
