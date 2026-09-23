//! Plain Scheme data read from a syntax tree: the value model for the worker
//! protocol and for configuration files. Only the subset the protocol needs
//! is decoded precisely; everything else is kept as its source text.

use super::{parse, Kind, Node};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Datum {
    Symbol(String),
    String(String),
    Integer(i64),
    /// A number the protocol does not use as an integer; kept verbatim.
    Number(String),
    Boolean(bool),
    Keyword(String),
    List(Vec<Datum>),
    /// An improper tail: `(a . b)` becomes `Pair(a, b)` when the list has
    /// exactly one element before the dot; longer lists nest.
    Pair(Box<Datum>, Box<Datum>),
    /// Anything else (characters, vectors, quoted forms, unknown tokens),
    /// preserved as source text.
    Other(String),
}

impl Datum {
    pub fn symbol(name: &str) -> Datum {
        Datum::Symbol(name.to_string())
    }

    pub fn string(text: &str) -> Datum {
        Datum::String(text.to_string())
    }

    pub fn list(items: impl IntoIterator<Item = Datum>) -> Datum {
        Datum::List(items.into_iter().collect())
    }

    pub fn as_symbol(&self) -> Option<&str> {
        match self {
            Datum::Symbol(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Datum::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Datum::Integer(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Datum]> {
        match self {
            Datum::List(items) => Some(items),
            _ => None,
        }
    }

    /// Look up `key` in an association list of `(key . value)` or `(key value...)`
    /// entries, returning the value (or the rest of the entry as a list).
    pub fn assq(&self, key: &str) -> Option<&Datum> {
        self.as_list()?.iter().find_map(|entry| match entry {
            Datum::Pair(k, v) if k.as_symbol() == Some(key) => Some(v.as_ref()),
            Datum::List(items) if items.first().and_then(Datum::as_symbol) == Some(key) => {
                if items.len() == 2 {
                    Some(&items[1])
                } else {
                    // Return the entry itself so callers can see the full tail.
                    Some(entry)
                }
            }
            _ => None,
        })
    }

    /// Parse all top-level data in `source`. Returns `None` when the reader
    /// reports errors or an unsupported construct appears.
    pub fn parse_all(source: &str) -> Option<Vec<Datum>> {
        let tree = parse(source);
        if !tree.errors.is_empty() {
            return None;
        }
        tree.root
            .data()
            .map(|node| from_node(node, source))
            .collect()
    }

    pub fn parse_one(source: &str) -> Option<Datum> {
        let mut all = Self::parse_all(source)?;
        (all.len() == 1).then(|| all.remove(0))
    }
}

fn from_node(node: &Node, source: &str) -> Option<Datum> {
    let text = node.text(source);
    Some(match node.kind {
        Kind::Symbol => Datum::Symbol(text.to_string()),
        Kind::String => Datum::String(unescape(&text[1..text.len() - 1])),
        Kind::Number => text
            .parse::<i64>()
            .map(Datum::Integer)
            .unwrap_or_else(|_| Datum::Number(text.to_string())),
        Kind::Boolean => Datum::Boolean(matches!(text, "#t" | "#T" | "#true" | "#True")),
        Kind::Keyword => Datum::Keyword(text[2..].to_string()),
        Kind::List => {
            let items: Vec<&Node> = node.data().collect();
            // Improper list: `(a b . c)`.
            if items.len() >= 3 && items[items.len() - 2].text(source) == "." {
                let tail = from_node(items[items.len() - 1], source)?;
                let mut head: Vec<Datum> = items[..items.len() - 2]
                    .iter()
                    .map(|n| from_node(n, source))
                    .collect::<Option<_>>()?;
                if head.len() == 1 {
                    Datum::Pair(Box::new(head.remove(0)), Box::new(tail))
                } else {
                    // (a b . c) = (a . (b . c))
                    let mut datum = tail;
                    while let Some(item) = head.pop() {
                        datum = Datum::Pair(Box::new(item), Box::new(datum));
                    }
                    datum
                }
            } else {
                Datum::List(
                    items
                        .iter()
                        .map(|n| from_node(n, source))
                        .collect::<Option<_>>()?,
                )
            }
        }
        Kind::Unknown => return None,
        _ => Datum::Other(text.to_string()),
    })
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('x') => {
                let hex: String = chars.by_ref().take_while(|&h| h != ';').collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(ch) => out.push(ch),
                    None => {
                        out.push_str("\\x");
                        out.push_str(&hex);
                    }
                }
            }
            Some('\n') => {
                // Line continuation: skip leading whitespace on the next line.
                let rest = chars.as_str();
                let trimmed = rest.trim_start_matches([' ', '\t']);
                chars = trimmed.chars();
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Write a datum as Guile can read it back.
impl fmt::Display for Datum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Datum::Symbol(s) => write!(f, "{s}"),
            Datum::String(s) => {
                f.write_str("\"")?;
                for c in s.chars() {
                    match c {
                        '"' => f.write_str("\\\"")?,
                        '\\' => f.write_str("\\\\")?,
                        '\n' => f.write_str("\\n")?,
                        '\r' => f.write_str("\\r")?,
                        '\t' => f.write_str("\\t")?,
                        c => write!(f, "{c}")?,
                    }
                }
                f.write_str("\"")
            }
            Datum::Integer(i) => write!(f, "{i}"),
            Datum::Number(n) | Datum::Other(n) => write!(f, "{n}"),
            Datum::Boolean(true) => f.write_str("#t"),
            Datum::Boolean(false) => f.write_str("#f"),
            Datum::Keyword(k) => write!(f, "#:{k}"),
            Datum::List(items) => {
                f.write_str("(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str(")")
            }
            Datum::Pair(a, b) => write!(f, "({a} . {b})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_protocol_shaped_data() {
        let source = r#"((module srfi srfi-1) (name . "λ\"🦀\n") (count . 42) (ok . #t) (kw #:x) (nested (a . (b . c))))"#;
        let datum = Datum::parse_one(source).unwrap();
        assert_eq!(
            datum.assq("module"),
            Some(&Datum::list([
                Datum::symbol("module"),
                Datum::symbol("srfi"),
                Datum::symbol("srfi-1")
            ]))
        );
        assert_eq!(datum.assq("name").and_then(Datum::as_str), Some("λ\"🦀\n"));
        assert_eq!(datum.assq("count").and_then(Datum::as_integer), Some(42));
        assert_eq!(datum.assq("ok"), Some(&Datum::Boolean(true)));
        assert_eq!(datum.assq("kw"), Some(&Datum::Keyword("x".into())));
        assert_eq!(datum.assq("missing"), None);
        let printed = datum.to_string();
        assert_eq!(Datum::parse_one(&printed), Some(datum));
        assert!(printed.contains("\"λ\\\"🦀\\n\""));
    }

    #[test]
    fn improper_lists_and_escapes() {
        assert_eq!(
            Datum::parse_one("(a b . c)"),
            Some(Datum::Pair(
                Box::new(Datum::symbol("a")),
                Box::new(Datum::Pair(
                    Box::new(Datum::symbol("b")),
                    Box::new(Datum::symbol("c"))
                ))
            ))
        );
        assert_eq!(
            Datum::parse_one("\"\\x41;\\t\"").and_then(|d| d.as_str().map(String::from)),
            Some("A\t".into())
        );
        assert_eq!(Datum::parse_one("1.5"), Some(Datum::Number("1.5".into())));
        assert_eq!(Datum::parse_one("#\\a"), Some(Datum::Other("#\\a".into())));
        assert_eq!(Datum::parse_one("(unclosed"), None);
        assert_eq!(Datum::parse_one("#toto"), None);
        assert_eq!(Datum::parse_all("a b").map(|d| d.len()), Some(2));
        assert_eq!(Datum::parse_one("a b"), None);
    }
}
