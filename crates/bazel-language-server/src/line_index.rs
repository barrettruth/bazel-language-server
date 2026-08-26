//! Byte offsets to LSP positions.
//!
//! LSP counts characters in UTF-16 code units by default, so a line holding
//! astral-plane characters shifts every column after it. Emoji do appear in
//! BUILD files, in `genrule` commands and docstrings.

use lsp_types::Position;

pub struct LineIndex {
    /// Byte offset of the start of each line.
    starts: Vec<usize>,
}

impl LineIndex {
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { starts }
    }

    /// Convert a byte offset, counting columns in UTF-16 code units.
    #[must_use]
    pub fn position(&self, text: &str, offset: usize) -> Position {
        let offset = offset.min(text.len());
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let column = text
            .get(self.starts[line]..offset)
            .map_or(0, |slice| slice.chars().map(char::len_utf16).sum::<usize>());
        #[allow(clippy::cast_possible_truncation)]
        Position {
            line: line as u32,
            character: column as u32,
        }
    }

    /// Convert a position back to a byte offset, reading the column as UTF-16
    /// code units.
    ///
    /// A column past the end of its line clamps to the line's end rather than
    /// spilling into the next one, and a column landing inside a surrogate pair
    /// rounds down to the start of that character: no offset in the middle of a
    /// UTF-8 sequence is a position in any file.
    #[must_use]
    pub fn offset(&self, text: &str, position: Position) -> usize {
        let Some(&start) = self.starts.get(position.line as usize) else {
            return text.len();
        };
        let end = self
            .starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(text.len());
        let line = &text[start..end];
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);

        let target = position.character as usize;
        let mut column = 0usize;
        for (byte, ch) in line.char_indices() {
            if target < column + ch.len_utf16() {
                return start + byte;
            }
            column += ch.len_utf16();
        }
        start + line.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_utf16_units() {
        let text = "a = 1\nb = \"\u{1f600}x\"\n";
        let index = LineIndex::new(text);

        assert_eq!(
            index.position(text, 0),
            Position {
                line: 0,
                character: 0
            }
        );
        assert_eq!(
            index.position(text, 6),
            Position {
                line: 1,
                character: 0
            }
        );

        // `b`, space, `=`, space, `"` is five units and the emoji is two more,
        // so `x` sits at 7. Counting bytes instead would say 9.
        let x = text.find('x').unwrap();
        assert_eq!(x - 6, 9, "byte offset within the line");
        assert_eq!(
            index.position(text, x),
            Position {
                line: 1,
                character: 7
            }
        );
    }

    #[test]
    fn clamps_past_the_end() {
        let text = "a\n";
        let index = LineIndex::new(text);
        assert_eq!(
            index.position(text, 999),
            Position {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn offsets_are_the_inverse_of_positions() {
        // Line 2 is astral-plane throughout, so a column that counted bytes,
        // chars or UTF-16 units would all disagree.
        let text =
            "filegroup(\n    name = \"\u{1f600}srcs\",\n)\n\u{4e2d}\u{6587} = \"\u{1f4a9}\"\n";
        let index = LineIndex::new(text);

        for (offset, _) in text
            .char_indices()
            .chain(std::iter::once((text.len(), ' ')))
        {
            let position = index.position(text, offset);
            assert_eq!(
                index.offset(text, position),
                offset,
                "round trip at byte {offset} via {position:?}"
            );
        }
    }

    #[test]
    fn a_column_inside_a_surrogate_pair_rounds_down() {
        let text = "x = \"\u{1f600}\"\n";
        let index = LineIndex::new(text);
        let emoji = text.find('\u{1f600}').unwrap();

        // The emoji occupies UTF-16 columns 5 and 6. Column 6 is the second
        // half of the pair and is no character's start; the offset it maps to
        // must still be one, or slicing the text panics.
        assert_eq!(
            index.offset(
                text,
                Position {
                    line: 0,
                    character: 6
                }
            ),
            emoji
        );
        assert_eq!(
            index.offset(
                text,
                Position {
                    line: 0,
                    character: 7
                }
            ),
            emoji + '\u{1f600}'.len_utf8()
        );
    }

    #[test]
    fn a_column_past_the_line_stays_on_it() {
        let text = "ab\ncd\n";
        let index = LineIndex::new(text);
        assert_eq!(
            index.offset(
                text,
                Position {
                    line: 0,
                    character: 99
                }
            ),
            2,
            "the newline is the end of line 0, not the start of line 1"
        );
        assert_eq!(
            index.offset(
                text,
                Position {
                    line: 99,
                    character: 0
                }
            ),
            text.len()
        );
    }

    /// Windows line endings are a line terminator, not content: a column past
    /// the end must land before the `\r`, never between it and the `\n`.
    #[test]
    fn carriage_returns_are_not_columns() {
        let text = "ab\r\ncd\r\n";
        let index = LineIndex::new(text);
        assert_eq!(
            index.offset(
                text,
                Position {
                    line: 0,
                    character: 99
                }
            ),
            2
        );
    }
}
