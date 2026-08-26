//! Byte offsets and line/column positions.
//!
//! Columns are counted in UTF-16 code units, which is what LSP means by a
//! character: a line holding an astral-plane character shifts every column
//! after it, and emoji do appear in BUILD files, in `genrule` commands and in
//! docstrings.
//!
//! The arithmetic lives here rather than beside the protocol because the index
//! resolves a position for every target it records, and two implementations of
//! it would drift into disagreeing by a column — which reads as
//! goto-definition landing next to its target rather than on it.

/// The width of a string in UTF-16 code units.
///
/// The unit a column is counted in, so this is also how long a name is in the
/// terms a range is expressed in.
#[must_use]
pub fn utf16_len(text: &str) -> u32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        text.chars().map(char::len_utf16).sum::<usize>() as u32
    }
}

/// Line starts for one document, so a position costs a binary search rather
/// than a scan from the top of the file.
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

    /// The zero-based line and UTF-16 column of a byte offset.
    #[must_use]
    pub fn position(&self, text: &str, offset: usize) -> (u32, u32) {
        let offset = offset.min(text.len());
        let line = self.starts.partition_point(|&start| start <= offset) - 1;
        let column = text.get(self.starts[line]..offset).map_or(0, utf16_len);
        #[allow(clippy::cast_possible_truncation)]
        (line as u32, column)
    }

    /// The byte offset of a zero-based line and UTF-16 column.
    ///
    /// A column past the end of its line clamps to the line's end rather than
    /// spilling into the next one, and a column landing inside a surrogate pair
    /// rounds down to the start of that character: no offset in the middle of a
    /// UTF-8 sequence is a position in any file.
    #[must_use]
    pub fn offset(&self, text: &str, line: u32, character: u32) -> usize {
        let Some(&start) = self.starts.get(line as usize) else {
            return text.len();
        };
        let end = self
            .starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(text.len());
        let line = &text[start..end];
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);

        let target = character as usize;
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

    const ASTRAL: &str = "a = 1\nb = \"\u{1f600}x\"\n";

    #[test]
    fn counts_utf16_units() {
        let index = LineIndex::new(ASTRAL);
        assert_eq!(index.position(ASTRAL, 0), (0, 0));
        assert_eq!(index.position(ASTRAL, 6), (1, 0));

        // `b`, space, `=`, space, `"` is five units and the emoji is two more,
        // so `x` sits at 7. Counting bytes instead would say 9.
        let x = ASTRAL.find('x').unwrap();
        assert_eq!(x - 6, 9, "byte offset within the line");
        assert_eq!(index.position(ASTRAL, x), (1, 7));
    }

    #[test]
    fn round_trips_every_boundary() {
        let index = LineIndex::new(ASTRAL);
        for offset in (0..=ASTRAL.len()).filter(|o| ASTRAL.is_char_boundary(*o)) {
            let (line, character) = index.position(ASTRAL, offset);
            assert_eq!(
                index.offset(ASTRAL, line, character),
                offset,
                "offset {offset} via {line}:{character}"
            );
        }
    }

    #[test]
    fn a_column_inside_a_surrogate_pair_rounds_down() {
        let index = LineIndex::new(ASTRAL);
        // The emoji starts at UTF-16 column 5 and occupies 5 and 6.
        let start = index.offset(ASTRAL, 1, 5);
        assert_eq!(index.offset(ASTRAL, 1, 6), start);
        assert!(ASTRAL.is_char_boundary(start));
    }

    #[test]
    fn clamps_past_the_end() {
        let text = "a\n";
        let index = LineIndex::new(text);
        assert_eq!(index.position(text, 999), (1, 0));
        assert_eq!(index.offset(text, 99, 0), text.len());
        // A column past the line stops before the newline.
        assert_eq!(index.offset(text, 0, 99), 1);
    }

    #[test]
    fn a_crlf_line_ends_before_its_terminator() {
        let text = "ab\r\ncd\n";
        let index = LineIndex::new(text);
        assert_eq!(index.offset(text, 0, 99), 2);
    }
}
