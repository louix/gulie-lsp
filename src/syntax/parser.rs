//! Single-pass reader producing the lossless tree described in the parent
//! module. Works on bytes: every delimiter and dispatch character is ASCII,
//! and UTF-8 continuation bytes are never delimiters, so multibyte characters
//! flow into symbols and strings untouched.

use super::{Kind, Node, SyntaxError, Tree};

pub fn parse(source: &str) -> Tree {
    let mut parser = Parser {
        src: source.as_bytes(),
        pos: 0,
        errors: Vec::new(),
        stack: vec![Frame {
            node: Node {
                kind: Kind::Root,
                range: 0..source.len(),
                children: Vec::new(),
            },
            closer: None,
        }],
    };
    parser.run();
    parser.finish()
}

struct Frame {
    node: Node,
    /// Expected closing byte for containers; `None` for prefixes and the root.
    closer: Option<u8>,
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    errors: Vec<SyntaxError>,
    stack: Vec<Frame>,
}

const DIRECTIVES: &[&[u8]] = &[
    b"fold-case",
    b"no-fold-case",
    b"r6rs",
    b"r7rs",
    b"curly-infix",
    b"curly-infix-and-bracket-lists",
];

fn is_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b'[' | b']' | b'"' | b';')
}

impl<'a> Parser<'a> {
    fn run(&mut self) {
        while let Some(&byte) = self.src.get(self.pos) {
            match byte {
                _ if byte.is_ascii_whitespace() => {
                    let start = self.pos;
                    self.skip_while(|b| b.is_ascii_whitespace());
                    self.push(Node::leaf(Kind::Whitespace, start..self.pos));
                }
                b';' => {
                    let start = self.pos;
                    self.skip_while(|b| b != b'\n');
                    self.push(Node::leaf(Kind::LineComment, start..self.pos));
                }
                b'(' | b'[' => self.open(Kind::List, 1, if byte == b'(' { b')' } else { b']' }),
                b')' | b']' => self.close(byte),
                b'"' => self.string(),
                b'\'' | b'`' => self.prefix(1),
                b',' => self.prefix(if self.peek(1) == Some(b'@') { 2 } else { 1 }),
                b'#' => self.hash(),
                _ => self.atom(),
            }
        }
    }

    fn finish(mut self) -> Tree {
        while self.stack.len() > 1 {
            let mut frame = self.stack.pop().unwrap();
            frame.node.range.end = frame.node.children.last().unwrap().range.end;
            match frame.closer {
                Some(_) => {
                    let open = frame.node.children[0].range.clone();
                    self.error(open, "unclosed form; expected a closing delimiter");
                }
                None => self.error(
                    frame.node.range.clone(),
                    "expected a datum after this prefix",
                ),
            }
            self.attach_datum(frame.node);
        }
        let root = self.stack.pop().unwrap().node;
        self.errors.sort_by_key(|error| error.range.start);
        Tree {
            root,
            errors: self.errors,
        }
    }

    fn peek(&self, ahead: usize) -> Option<u8> {
        self.src.get(self.pos + ahead).copied()
    }

    fn skip_while(&mut self, predicate: impl Fn(u8) -> bool) {
        while let Some(&byte) = self.src.get(self.pos) {
            if !predicate(byte) {
                break;
            }
            self.pos += 1;
        }
    }

    fn starts_with(&self, at: usize, needle: &[u8]) -> bool {
        self.src[at..].starts_with(needle)
    }

    fn error(&mut self, range: std::ops::Range<usize>, message: &'static str) {
        self.errors.push(SyntaxError { range, message });
    }

    /// Append trivia or a delimiter to the innermost frame without completing
    /// a datum.
    fn push(&mut self, node: Node) {
        self.stack.last_mut().unwrap().node.children.push(node);
    }

    /// Attach a completed datum, resolving any pending prefixes around it.
    fn attach_datum(&mut self, mut node: Node) {
        loop {
            let frame = self.stack.last_mut().unwrap();
            frame.node.children.push(node);
            if frame.closer.is_some() || self.stack.len() == 1 {
                return;
            }
            let mut frame = self.stack.pop().unwrap();
            frame.node.range.end = frame.node.children.last().unwrap().range.end;
            node = frame.node;
        }
    }

    fn open(&mut self, kind: Kind, len: usize, closer: u8) {
        let start = self.pos;
        self.pos += len;
        self.stack.push(Frame {
            node: Node {
                kind,
                range: start..start,
                children: vec![Node::leaf(Kind::Open, start..self.pos)],
            },
            closer: Some(closer),
        });
    }

    fn close(&mut self, byte: u8) {
        let range = self.pos..self.pos + 1;
        self.pos += 1;
        // A pending prefix cannot take a closer as its datum.
        while self.stack.len() > 1 && self.stack.last().unwrap().closer.is_none() {
            let mut frame = self.stack.pop().unwrap();
            frame.node.range.end = frame.node.children.last().unwrap().range.end;
            self.error(
                frame.node.range.clone(),
                "expected a datum after this prefix",
            );
            self.attach_datum(frame.node);
        }
        if self.stack.len() == 1 {
            self.error(range.clone(), "unexpected closing delimiter");
            self.push(Node::leaf(Kind::Close, range));
            return;
        }
        let mut frame = self.stack.pop().unwrap();
        if frame.closer != Some(byte) {
            self.error(range.clone(), "mismatched closing delimiter");
        }
        frame
            .node
            .children
            .push(Node::leaf(Kind::Close, range.clone()));
        frame.node.range.end = range.end;
        self.attach_datum(frame.node);
    }

    fn prefix(&mut self, len: usize) {
        let start = self.pos;
        self.pos += len;
        self.stack.push(Frame {
            node: Node {
                kind: Kind::Prefixed,
                range: start..self.pos,
                children: vec![Node::leaf(Kind::Prefix, start..self.pos)],
            },
            closer: None,
        });
    }

    fn datum_comment(&mut self) {
        let start = self.pos;
        self.pos += 2;
        self.stack.push(Frame {
            node: Node {
                kind: Kind::DatumComment,
                range: start..self.pos,
                children: vec![Node::leaf(Kind::Prefix, start..self.pos)],
            },
            closer: None,
        });
    }

    fn string(&mut self) {
        let start = self.pos;
        self.pos += 1;
        loop {
            match self.src.get(self.pos) {
                None => {
                    self.error(start..self.pos, "unterminated string");
                    break;
                }
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => self.pos += if self.pos + 1 < self.src.len() { 2 } else { 1 },
                Some(_) => self.pos += 1,
            }
        }
        self.attach_datum(Node::leaf(Kind::String, start..self.pos));
    }

    /// Consume a symbol or number token starting at the current position.
    /// Handles `|...|` segments, which may contain delimiters.
    fn token_end(&mut self) -> usize {
        while let Some(&byte) = self.src.get(self.pos) {
            if byte == b'|' {
                let bar = self.pos;
                self.pos += 1;
                loop {
                    match self.src.get(self.pos) {
                        None => {
                            self.error(bar..self.pos, "unterminated |symbol|");
                            break;
                        }
                        Some(b'|') => {
                            self.pos += 1;
                            break;
                        }
                        Some(b'\\') => self.pos += 2.min(self.src.len() - self.pos),
                        Some(_) => self.pos += 1,
                    }
                }
            } else if is_delimiter(byte) {
                break;
            } else {
                self.pos += 1;
            }
        }
        self.pos.min(self.src.len())
    }

    fn atom(&mut self) {
        let start = self.pos;
        let end = self.token_end();
        let kind = if is_number(&self.src[start..end]) {
            Kind::Number
        } else {
            Kind::Symbol
        };
        self.attach_datum(Node::leaf(kind, start..end));
    }

    fn hash(&mut self) {
        let start = self.pos;
        match self.peek(1) {
            Some(b'|') => self.block_comment(),
            Some(b';') => self.datum_comment(),
            Some(b'!') => self.bang(),
            Some(b'(') => self.open(Kind::Vector, 2, b')'),
            Some(b'\'') | Some(b'`') | Some(b'~') => self.prefix(2),
            Some(b',') | Some(b'$') | Some(b'+') => {
                self.prefix(if self.peek(2) == Some(b'@') { 3 } else { 2 })
            }
            Some(b'\\') => {
                // The first character after `#\` is always part of the literal,
                // even when it is a delimiter such as `#\(` or `#\ `.
                self.pos += 2;
                if self.pos < self.src.len() {
                    self.pos += utf8_len(self.src[self.pos]);
                } else {
                    self.error(start..self.pos, "expected a character after #\\");
                }
                self.token_end();
                self.attach_datum(Node::leaf(Kind::Char, start..self.pos));
            }
            Some(b':') => {
                self.pos += 2;
                self.token_end();
                self.attach_datum(Node::leaf(Kind::Keyword, start..self.pos));
            }
            Some(b'*') => {
                self.pos += 2;
                self.token_end();
                self.attach_datum(Node::leaf(Kind::BitVector, start..self.pos));
            }
            Some(b'{') => {
                self.pos += 2;
                loop {
                    if self.pos >= self.src.len() {
                        self.error(start..self.pos, "unterminated #{ symbol");
                        break;
                    }
                    if self.starts_with(self.pos, b"}#") {
                        self.pos += 2;
                        break;
                    }
                    self.pos += 1;
                }
                self.attach_datum(Node::leaf(Kind::Symbol, start..self.pos));
            }
            Some(byte) if byte.is_ascii_alphanumeric() => self.hash_word(),
            _ => {
                self.pos += 1;
                self.token_end();
                self.error(start..self.pos, "unsupported reader syntax");
                self.attach_datum(Node::leaf(Kind::Unknown, start..self.pos));
            }
        }
    }

    /// `#` followed by a letter or digit: booleans, nil, arrays, or
    /// radix/exactness-prefixed numbers.
    fn hash_word(&mut self) {
        let start = self.pos;
        let mut end = start + 1;
        while end < self.src.len() && self.src[end].is_ascii_alphanumeric() {
            end += 1;
        }
        let word = &self.src[start + 1..end];
        let lowered = word.to_ascii_lowercase();
        let boolean = matches!(lowered.as_slice(), b"t" | b"f" | b"true" | b"false");
        let nil = lowered == b"nil";
        if self.src.get(end) == Some(&b'(') && !boolean && !nil {
            self.open(Kind::Array, end + 1 - start, b')');
            return;
        }
        if boolean || nil {
            self.pos = end;
            let token_end = self.token_end();
            if token_end == end {
                let kind = if nil { Kind::Nil } else { Kind::Boolean };
                self.attach_datum(Node::leaf(kind, start..end));
            } else {
                self.error(start..token_end, "unsupported reader syntax");
                self.attach_datum(Node::leaf(Kind::Unknown, start..token_end));
            }
            return;
        }
        self.pos = start + 1;
        let end = self.token_end();
        if is_number(&self.src[start..end]) {
            self.attach_datum(Node::leaf(Kind::Number, start..end));
        } else {
            self.error(start..end, "unsupported reader syntax");
            self.attach_datum(Node::leaf(Kind::Unknown, start..end));
        }
    }

    fn block_comment(&mut self) {
        let start = self.pos;
        self.pos += 2;
        let mut depth = 1;
        while depth > 0 {
            if self.pos >= self.src.len() {
                self.error(start..self.pos, "unterminated block comment");
                break;
            }
            if self.starts_with(self.pos, b"|#") {
                depth -= 1;
                self.pos += 2;
            } else if self.starts_with(self.pos, b"#|") {
                depth += 1;
                self.pos += 2;
            } else {
                self.pos += 1;
            }
        }
        self.push(Node::leaf(Kind::BlockComment, start..self.pos));
    }

    /// `#!`: a reader directive if the following word is one, otherwise a
    /// block comment ending at `!#` (the shebang convention).
    fn bang(&mut self) {
        let start = self.pos;
        let mut end = start + 2;
        while end < self.src.len() && !is_delimiter(self.src[end]) {
            end += 1;
        }
        if DIRECTIVES.contains(&&self.src[start + 2..end]) {
            self.pos = end;
            self.push(Node::leaf(Kind::Directive, start..end));
            return;
        }
        self.pos += 2;
        loop {
            if self.pos >= self.src.len() {
                self.error(start..self.pos, "unterminated #! comment");
                break;
            }
            if self.starts_with(self.pos, b"!#") {
                self.pos += 2;
                break;
            }
            self.pos += 1;
        }
        self.push(Node::leaf(Kind::BlockComment, start..self.pos));
    }
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// Guile number syntax: optional radix/exactness prefixes, then a real, a
/// polar complex `a@b`, or a rectangular complex `a+bi`. Anything the
/// validator rejects is a symbol.
fn is_number(token: &[u8]) -> bool {
    let mut rest = token;
    let mut radix = 10;
    while rest.len() >= 2 && rest[0] == b'#' {
        match rest[1].to_ascii_lowercase() {
            b'x' => radix = 16,
            b'b' => radix = 2,
            b'o' => radix = 8,
            b'd' => radix = 10,
            b'e' | b'i' => {}
            _ => return false,
        }
        rest = &rest[2..];
    }
    if rest.is_empty() || rest[0] == b'#' {
        return false;
    }
    if let Some(at) = rest.iter().position(|&b| b == b'@') {
        return is_real(&rest[..at], radix) && is_real(&rest[at + 1..], radix);
    }
    if !rest.last().unwrap().eq_ignore_ascii_case(&b'i') {
        return is_real(rest, radix);
    }
    let body = &rest[..rest.len() - 1];
    let is_sign = |b: u8| matches!(b, b'+' | b'-');
    if body.is_empty() || !(is_sign(body[0]) || body[0].is_ascii_digit() || body[0] == b'.') {
        return false;
    }
    // Split before the sign that starts the imaginary part: the last sign not
    // acting as a decimal exponent sign.
    let split = (1..body.len())
        .rev()
        .find(|&i| is_sign(body[i]) && !(radix == 10 && body[i - 1].eq_ignore_ascii_case(&b'e')));
    match split {
        Some(i) => {
            let (real, imaginary) = body.split_at(i);
            is_real(real, radix) && (imaginary.len() == 1 || is_ureal(&imaginary[1..], radix))
        }
        // No real part: `+i`, `-2.5i`.
        None => is_sign(body[0]) && (body.len() == 1 || is_ureal(&body[1..], radix)),
    }
}

fn is_real(token: &[u8], radix: u32) -> bool {
    let unsigned = match token.first() {
        Some(b'+') | Some(b'-') => &token[1..],
        _ => token,
    };
    if unsigned.len() != token.len()
        && (unsigned.eq_ignore_ascii_case(b"inf.0") || unsigned.eq_ignore_ascii_case(b"nan.0"))
    {
        return true;
    }
    is_ureal(unsigned, radix)
}

fn is_ureal(token: &[u8], radix: u32) -> bool {
    if let Some(slash) = token.iter().position(|&b| b == b'/') {
        return is_uinteger(&token[..slash], radix) && is_uinteger(&token[slash + 1..], radix);
    }
    if radix != 10 {
        return is_uinteger(token, radix);
    }
    is_decimal(token)
}

fn is_uinteger(token: &[u8], radix: u32) -> bool {
    !token.is_empty() && token.iter().all(|&b| (b as char).is_digit(radix))
}

fn is_decimal(token: &[u8]) -> bool {
    let (mantissa, exponent) = match token.iter().position(|&b| matches!(b, b'e' | b'E')) {
        Some(e) => (&token[..e], Some(&token[e + 1..])),
        None => (token, None),
    };
    let (int, frac) = match mantissa.iter().position(|&b| b == b'.') {
        Some(dot) => (&mantissa[..dot], Some(&mantissa[dot + 1..])),
        None => (mantissa, None),
    };
    let digits_ok = |s: &[u8]| s.iter().all(u8::is_ascii_digit);
    let has_digits = !int.is_empty() || frac.map(|f| !f.is_empty()).unwrap_or(false);
    if !has_digits || !digits_ok(int) || !frac.map(digits_ok).unwrap_or(true) {
        return false;
    }
    match exponent {
        None => true,
        Some(exp) => {
            let exp = match exp.first() {
                Some(b'+') | Some(b'-') => &exp[1..],
                _ => exp,
            };
            !exp.is_empty() && digits_ok(exp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_number;

    #[test]
    fn number_validator_matches_guile() {
        for ok in [
            "0",
            "42",
            "-7",
            "+5",
            "1/3",
            "1.5",
            ".5",
            "5.",
            "1e10",
            "1.5e-3",
            "#xFF",
            "#b101",
            "#e1.5",
            "#i#x10",
            "+inf.0",
            "-nan.0",
            "+i",
            "1+2i",
            "-1.5-2.5i",
            "1@2",
            "#x-ff",
        ] {
            assert!(is_number(ok.as_bytes()), "{ok} should be a number");
        }
        for bad in [
            "1+", "-", "+", "...", "1a", "e10", "1.2.3", "#t", "#xg", "1/", "/2", "+-1", "abc",
            "1e", ".", "-inf", "λ",
        ] {
            assert!(!is_number(bad.as_bytes()), "{bad} should not be a number");
        }
    }
}
