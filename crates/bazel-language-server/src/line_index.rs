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
}
