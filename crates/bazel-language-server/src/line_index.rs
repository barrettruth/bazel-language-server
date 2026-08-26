//! LSP positions over the index's line arithmetic.
//!
//! The UTF-16 column maths lives in `bls_index::line_index`, which the index
//! needs for every target it records. This is the protocol's view of it: the
//! data crate holds no LSP types, so the conversion to and from [`Position`]
//! happens here.

use lsp_types::Position;

pub struct LineIndex(bls_index::line_index::LineIndex);

impl LineIndex {
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self(bls_index::line_index::LineIndex::new(text))
    }

    /// Convert a byte offset, counting columns in UTF-16 code units.
    #[must_use]
    pub fn position(&self, text: &str, offset: usize) -> Position {
        let (line, character) = self.0.position(text, offset);
        Position { line, character }
    }

    /// Convert a position back to a byte offset.
    ///
    /// Clamping and surrogate-pair rounding are described on
    /// [`bls_index::line_index::LineIndex::offset`].
    #[must_use]
    pub fn offset(&self, text: &str, position: Position) -> usize {
        self.0.offset(text, position.line, position.character)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arithmetic is tested where it lives; this pins the mapping onto
    /// `Position`, which is the only thing this wrapper adds.
    #[test]
    fn maps_onto_lsp_positions() {
        let text = "a = 1\nb = \"\u{1f600}x\"\n";
        let index = LineIndex::new(text);
        let x = text.find('x').unwrap();

        let position = index.position(text, x);
        assert_eq!(
            position,
            Position {
                line: 1,
                character: 7
            }
        );
        assert_eq!(index.offset(text, position), x);
    }
}
