//! Tolerant, lossless reader for Guile source text.
//!
//! The tree keeps every byte of the input: atoms, delimiters, comments, and
//! whitespace all become nodes, so concatenating the leaves reproduces the
//! source exactly. Unbalanced or unsupported input never aborts the parse;
//! the reader records a [`SyntaxError`] and keeps going, closing open forms
//! at end of input so that partial edits still yield nested structure.
//!
//! Positions are byte offsets into the UTF-8 source. Use
//! [`position::LineIndex`] to convert to and from LSP line/UTF-16 columns.

pub mod datum;
mod parser;
pub mod position;

use std::ops::Range;

pub use parser::parse;

/// Node classification. Container kinds have children; the rest are leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Root,
    /// `( ... )` or `[ ... ]`.
    List,
    /// `#( ... )`.
    Vector,
    /// Bytevectors and typed or multi-dimensional arrays: `#vu8( ... )`,
    /// `#f32( ... )`, `#2( ... )`.
    Array,
    /// A reader prefix applied to the following datum: quote, quasiquote,
    /// unquote, syntax, and gexp forms. The first child is the [`Kind::Prefix`]
    /// leaf; the last child is the datum, unless an error was recorded.
    Prefixed,
    /// `#;` followed by the datum it comments out.
    DatumComment,

    /// Opening delimiter, including array and vector prefixes like `#vu8(`.
    Open,
    Close,
    /// Prefix token such as `'`, `,@`, `#'`, `#~`, `#$@`, or `#;`.
    Prefix,
    Symbol,
    String,
    Number,
    /// `#\...` character literal.
    Char,
    /// `#:name` keyword.
    Keyword,
    /// `#t`, `#f`, `#true`, `#false`.
    Boolean,
    /// `#nil`.
    Nil,
    /// `#*0110` bitvector literal.
    BitVector,
    /// `#!fold-case` and similar reader directives.
    Directive,
    /// `; ...` up to but excluding the newline.
    LineComment,
    /// `#| ... |#` (nesting) or `#! ... !#`.
    BlockComment,
    Whitespace,
    /// A token the reader could not classify, such as an unsupported `#`
    /// syntax. Counted as a datum so surrounding argument positions survive.
    /// Always accompanied by a [`SyntaxError`]. A stray closing delimiter
    /// stays a [`Kind::Close`] leaf attached to the enclosing form.
    Unknown,
}

impl Kind {
    /// Whitespace and comments: present in the tree but not data.
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            Kind::Whitespace | Kind::LineComment | Kind::BlockComment | Kind::DatumComment
        )
    }

    /// Nodes that contribute a value to their parent form.
    pub fn is_datum(self) -> bool {
        matches!(
            self,
            Kind::List
                | Kind::Vector
                | Kind::Array
                | Kind::Prefixed
                | Kind::Symbol
                | Kind::String
                | Kind::Number
                | Kind::Char
                | Kind::Keyword
                | Kind::Boolean
                | Kind::Nil
                | Kind::BitVector
                | Kind::Unknown
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: Kind,
    pub range: Range<usize>,
    pub children: Vec<Node>,
}

impl Node {
    fn leaf(kind: Kind, range: Range<usize>) -> Self {
        Node {
            kind,
            range,
            children: Vec::new(),
        }
    }

    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.range.clone()]
    }

    /// Children that are data, skipping delimiters, prefixes, and trivia.
    pub fn data(&self) -> impl Iterator<Item = &Node> {
        self.children.iter().filter(|child| child.kind.is_datum())
    }

    /// Depth-first traversal of every node, including `self`.
    pub fn descendants(&self) -> Descendants<'_> {
        Descendants { stack: vec![self] }
    }

    /// The innermost node whose range contains `offset`.
    pub fn node_at(&self, offset: usize) -> &Node {
        let mut node = self;
        loop {
            match node
                .children
                .iter()
                .find(|child| child.range.start <= offset && offset < child.range.end)
            {
                Some(child) => node = child,
                None => return node,
            }
        }
    }
}

pub struct Descendants<'a> {
    stack: Vec<&'a Node>,
}

impl<'a> Iterator for Descendants<'a> {
    type Item = &'a Node;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        self.stack.extend(node.children.iter().rev());
        Some(node)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub range: Range<usize>,
    pub message: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub root: Node,
    pub errors: Vec<SyntaxError>,
}
