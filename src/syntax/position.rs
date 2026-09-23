//! Conversion between byte offsets and LSP positions (line, UTF-16 column).

/// Byte offsets of line starts, for a fixed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    line_starts: Vec<usize>,
}

/// Zero-based line and UTF-16 code unit column, as LSP expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| index + 1),
        );
        LineIndex { line_starts }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// The zero-based line containing byte `offset`.
    pub fn line_of(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&start| start <= offset) - 1
    }

    /// Byte offset where `line` starts, or `None` past the last line.
    pub fn line_start(&self, line: usize) -> Option<usize> {
        self.line_starts.get(line).copied()
    }

    /// Convert a byte offset (which must lie on a char boundary) to a position.
    pub fn position(&self, text: &str, offset: usize) -> Position {
        let offset = offset.min(text.len());
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let line_start = self.line_starts[line];
        let character = text[line_start..offset]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();
        Position {
            line: line as u32,
            character: character as u32,
        }
    }

    /// Convert an LSP position to a byte offset.
    ///
    /// Columns past the end of the line clamp to the line end, and a column
    /// inside a surrogate pair rounds down to the start of that character, so
    /// the result always lies on a char boundary. Lines past the end of the
    /// text return `None`.
    pub fn offset(&self, text: &str, position: Position) -> Option<usize> {
        let line = position.line as usize;
        let start = *self.line_starts.get(line)?;
        let end = self
            .line_starts
            .get(line + 1)
            .map(|next| next - 1)
            .unwrap_or(text.len());
        let mut units = 0usize;
        for (index, ch) in text[start..end].char_indices() {
            if (position.character as usize) < units + ch.len_utf16() {
                return Some(start + index);
            }
            units += ch.len_utf16();
        }
        Some(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_columns_round_trip_through_multibyte_text() {
        let text = "(define name \"λ🦀\")\nnext";
        let index = LineIndex::new(text);
        let crab = text.find('🦀').unwrap();
        assert_eq!(
            index.position(text, crab),
            Position {
                line: 0,
                character: 15
            }
        );
        // 🦀 occupies two UTF-16 units; the closing quote follows it.
        assert_eq!(
            index.position(text, crab + '🦀'.len_utf8()),
            Position {
                line: 0,
                character: 17
            }
        );
        assert_eq!(
            index.offset(
                text,
                Position {
                    line: 0,
                    character: 15
                }
            ),
            Some(crab)
        );
        assert_eq!(
            index.offset(
                text,
                Position {
                    line: 0,
                    character: 16
                }
            ),
            Some(crab)
        );
        assert_eq!(
            index.offset(
                text,
                Position {
                    line: 0,
                    character: 17
                }
            ),
            Some(crab + '🦀'.len_utf8())
        );
        assert_eq!(
            index.position(text, text.len()),
            Position {
                line: 1,
                character: 4
            }
        );
        assert_eq!(
            index.offset(
                text,
                Position {
                    line: 1,
                    character: 99
                }
            ),
            Some(text.len())
        );
        assert_eq!(
            index.offset(
                text,
                Position {
                    line: 0,
                    character: 99
                }
            ),
            Some(text.find('\n').unwrap())
        );
        assert_eq!(
            index.offset(
                text,
                Position {
                    line: 2,
                    character: 0
                }
            ),
            None
        );
    }
}
