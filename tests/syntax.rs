use guile_lsp::syntax::{parse, Kind, Node, Tree};
use std::path::{Path, PathBuf};

fn fixtures() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "scm") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .as_path(),
        &mut out,
    );
    out.sort();
    out
}

fn leaves<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
    if node.children.is_empty() {
        out.push(node);
    }
    for child in &node.children {
        leaves(child, out);
    }
}

fn assert_lossless(source: &str, tree: &Tree) {
    let mut all = Vec::new();
    leaves(&tree.root, &mut all);
    let mut rebuilt = String::new();
    let mut cursor = 0;
    for leaf in all {
        assert_eq!(leaf.range.start, cursor, "gap or overlap before {:?}", leaf);
        rebuilt.push_str(leaf.text(source));
        cursor = leaf.range.end;
    }
    assert_eq!(rebuilt, source);
    assert_eq!(tree.root.range, 0..source.len());
}

fn kinds(tree: &Tree) -> Vec<Kind> {
    tree.root.descendants().map(|n| n.kind).collect()
}

fn head(node: &Node, source: &str) -> String {
    if node.children.is_empty() {
        return node.text(source).to_string();
    }
    node.data()
        .next()
        .map(|n| n.text(source).to_string())
        .unwrap_or_default()
}

#[test]
fn every_fixture_round_trips_losslessly() {
    let fixtures = fixtures();
    assert!(fixtures.len() >= 15);
    for path in fixtures {
        let source = std::fs::read_to_string(&path).unwrap();
        assert_lossless(&source, &parse(&source));
    }
}

#[test]
fn well_formed_fixtures_have_no_errors() {
    for path in fixtures() {
        let source = std::fs::read_to_string(&path).unwrap();
        let tree = parse(&source);
        let incomplete = path.file_name().unwrap() == "incomplete.scm";
        assert_eq!(
            !tree.errors.is_empty(),
            incomplete,
            "{}: {:?}",
            path.display(),
            tree.errors
        );
    }
}

#[test]
fn guile_reader_syntax_is_recognized() {
    let source = r##"#!/usr/bin/env guile
!#
(define x #:keyword)
(define y #~(list #$a #$@b #+c #+@d))
(define z '#nil)
(define c #\space) (define c2 #\x41) (define c3 #\λ) (define c4 #\()
(define n #xFF) (define n2 #e1.5) (define n3 1/3) (define n4 +inf.0) (define n5 1+)
(define bv #vu8(1 2 3)) (define arr #2((1 2) (3 4))) (define f32 #f32(1.0 2.0))
(define s "multi
line \x41; λ \" escape")
(define sym '|hello world|) (define sym2 'a|b c|d)
#;(datum comment) (define after 1)
#| block #| nested |# comment |#
(define dotted '(a . b))
(define vec #(1 2 3))
(define q `(a ,b ,@c #'d #`e #,f #,@g))
(define t #true) (define f #False)
#!fold-case
(define ue #{weird sym}#) ; trailing comment
(define bits #*0110)
[define brackets 1]
"##;
    let tree = parse(source);
    assert_eq!(tree.errors, vec![], "{:?}", tree.errors);
    assert_lossless(source, &tree);
    let found = kinds(&tree);
    for kind in [
        Kind::BlockComment,
        Kind::Keyword,
        Kind::Prefixed,
        Kind::Nil,
        Kind::Char,
        Kind::Number,
        Kind::Array,
        Kind::String,
        Kind::Symbol,
        Kind::DatumComment,
        Kind::Vector,
        Kind::Boolean,
        Kind::Directive,
        Kind::List,
        Kind::LineComment,
        Kind::Whitespace,
    ] {
        assert!(found.contains(&kind), "missing {kind:?}");
    }
    let unknown: Vec<&str> = tree
        .root
        .descendants()
        .filter(|n| n.kind == Kind::Unknown)
        .map(|n| n.text(source))
        .collect();
    assert!(unknown.is_empty(), "unknown tokens: {unknown:?}");

    let texts: Vec<(Kind, &str)> = tree
        .root
        .descendants()
        .filter(|n| {
            n.children.is_empty()
                && !n.kind.is_trivia()
                && n.kind != Kind::Open
                && n.kind != Kind::Close
        })
        .map(|n| (n.kind, n.text(source)))
        .collect();
    for expected in [
        (Kind::Keyword, "#:keyword"),
        (Kind::Prefix, "#~"),
        (Kind::Prefix, "#$@"),
        (Kind::Prefix, "#+@"),
        (Kind::Nil, "#nil"),
        (Kind::Char, "#\\space"),
        (Kind::Char, "#\\λ"),
        (Kind::Char, "#\\("),
        (Kind::Number, "#xFF"),
        (Kind::Number, "#e1.5"),
        (Kind::Number, "1/3"),
        (Kind::Number, "+inf.0"),
        (Kind::Symbol, "1+"),
        (Kind::Symbol, "|hello world|"),
        (Kind::Symbol, "a|b c|d"),
        (Kind::Symbol, "#{weird sym}#"),
        (Kind::Boolean, "#False"),
        (Kind::Directive, "#!fold-case"),
        (Kind::Prefix, "#,@"),
        (Kind::Symbol, "."),
        (Kind::BitVector, "#*0110"),
    ] {
        assert!(
            texts.contains(&expected),
            "missing {expected:?} in {texts:?}"
        );
    }
    let opens: Vec<&str> = tree
        .root
        .descendants()
        .filter(|n| n.kind == Kind::Open)
        .map(|n| n.text(source))
        .collect();
    for open in ["#vu8(", "#2(", "#f32(", "#(", "[", "("] {
        assert!(opens.contains(&open), "missing opener {open}");
    }
    let string = tree
        .root
        .descendants()
        .find(|n| n.kind == Kind::String)
        .unwrap();
    assert!(string.text(source).ends_with("escape\""));
}

#[test]
fn datum_comment_and_prefixes_wrap_the_next_datum() {
    let source = "#;(a b) 'x `(,y ,@z)";
    let tree = parse(source);
    assert!(tree.errors.is_empty());
    let top: Vec<(Kind, &str)> = tree
        .root
        .children
        .iter()
        .filter(|n| !matches!(n.kind, Kind::Whitespace))
        .map(|n| (n.kind, n.text(source)))
        .collect();
    assert_eq!(
        top,
        vec![
            (Kind::DatumComment, "#;(a b)"),
            (Kind::Prefixed, "'x"),
            (Kind::Prefixed, "`(,y ,@z)")
        ]
    );
    assert_eq!(tree.root.data().count(), 2, "datum comments are not data");
}

#[test]
fn incomplete_forms_are_closed_at_end_of_input_with_structure_intact() {
    let source = "(operating-system\n  (host-name \"fixture\")\n  (services\n   (cons* (service openssh-service-type\n                   (openssh-configuration\n                    (port-nu";
    let tree = parse(source);
    assert_lossless(source, &tree);
    let messages: Vec<&str> = tree.errors.iter().map(|e| e.message).collect();
    assert_eq!(messages.len(), 6, "{messages:?}");
    assert!(messages.iter().all(|m| m.starts_with("unclosed form")));
    let os = tree.root.data().next().unwrap();
    assert_eq!(head(os, source), "operating-system");
    let forms: Vec<String> = os.data().skip(1).map(|n| head(n, source)).collect();
    assert_eq!(forms, ["host-name", "services"]);
    let innermost = tree.root.node_at(source.len() - 1);
    assert_eq!(innermost.kind, Kind::Symbol);
    assert_eq!(innermost.text(source), "port-nu");
    // Errors point at the opening delimiters.
    assert_eq!(tree.errors[0].range, 0..1);
}

#[test]
fn unterminated_string_reports_once_and_keeps_earlier_forms() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/guile/incomplete.scm"
    ))
    .unwrap();
    let tree = parse(&source);
    let messages: Vec<&str> = tree.errors.iter().map(|e| e.message).collect();
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.starts_with("unterminated string"))
            .count(),
        1
    );
    let heads: Vec<String> = tree.root.data().map(|n| head(n, &source)).collect();
    assert_eq!(heads, ["define-module", "define"]);
}

#[test]
fn recovery_from_stray_and_mismatched_delimiters() {
    let source = "(a b] c) ) (d '";
    let tree = parse(source);
    assert_lossless(source, &tree);
    let messages: Vec<(&str, &str)> = tree
        .errors
        .iter()
        .map(|e| (e.message, &source[e.range.clone()]))
        .collect();
    assert_eq!(
        messages,
        vec![
            ("mismatched closing delimiter", "]"),
            ("unexpected closing delimiter", ")"),
            ("unexpected closing delimiter", ")"),
            ("unclosed form; expected a closing delimiter", "("),
            ("expected a datum after this prefix", "'"),
        ]
    );
    let heads: Vec<String> = tree.root.data().map(|n| head(n, source)).collect();
    assert_eq!(heads, ["a", "c", "d"]);
}

#[test]
fn unsupported_hash_syntax_is_one_token_without_cascading() {
    let source = "(list #toto #xg 1) (ok)";
    let tree = parse(source);
    assert_lossless(source, &tree);
    let unknown: Vec<&str> = tree
        .root
        .descendants()
        .filter(|n| n.kind == Kind::Unknown)
        .map(|n| n.text(source))
        .collect();
    assert_eq!(unknown, ["#toto", "#xg"]);
    assert_eq!(tree.errors.len(), 2);
    assert_eq!(tree.root.data().count(), 2);
    assert_eq!(tree.root.data().next().unwrap().data().count(), 4);
}

#[test]
fn unterminated_comments_and_symbols_do_not_panic() {
    for source in [
        "#| open",
        "#! shebang",
        "#{sym",
        "\"str",
        "#\\",
        "'",
        "#",
        "#;",
        "(",
        "#(",
        "#vu8(",
        "a|b",
    ] {
        let tree = parse(source);
        assert_lossless(source, &tree);
        assert!(!tree.errors.is_empty(), "{source:?}");
    }
    assert!(parse("").errors.is_empty());
    assert!(parse("#|a #|b|# c|# x").errors.is_empty());
}

/// Set GUIX_SOURCE_DIR to Guix's share/guile/site/3.0 to parse the whole Guix tree.
#[test]
fn guix_sources_parse_without_errors() {
    let Ok(dir) = std::env::var("GUIX_SOURCE_DIR") else {
        eprintln!("GUIX_SOURCE_DIR not set; skipping");
        return;
    };
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "scm") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(Path::new(&dir), &mut files);
    assert!(files.len() > 100);
    let mut failures = Vec::new();
    let mut bytes = 0;
    let start = std::time::Instant::now();
    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        bytes += source.len();
        let tree = parse(&source);
        assert_lossless(&source, &tree);
        for error in tree.errors.iter().take(3) {
            failures.push(format!(
                "{}: {} at {:?}",
                path.display(),
                error.message,
                &source[error.range.clone()]
                    .chars()
                    .take(30)
                    .collect::<String>()
            ));
        }
    }
    eprintln!(
        "parsed {} files, {} bytes in {:?}",
        files.len(),
        bytes,
        start.elapsed()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
