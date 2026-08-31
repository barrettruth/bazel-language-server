//! Native option/value grouping according to the exact flag catalog.

use super::syntax::{Line, Span, Token};
use super::{FlagCatalog, ResolvedFlag};

#[derive(Clone, Copy)]
pub struct NativeOption<'syntax, 'catalog> {
    pub option: &'syntax Token,
    pub resolved: Option<ResolvedFlag<'catalog>>,
    pub value: Option<&'syntax Token>,
}

#[derive(Clone, Copy)]
pub struct NativeValue<'syntax> {
    pub text: &'syntax str,
    pub range: Span,
}

impl<'syntax> NativeOption<'syntax, '_> {
    #[must_use]
    pub fn value_site(self) -> Option<NativeValue<'syntax>> {
        if let Some((_, value)) = self.option.text.split_once('=') {
            let start = self.option.text.find('=')? + 1;
            let range = self
                .option
                .decoded_span(start..self.option.text.len())
                .unwrap_or_else(|| {
                    let equals = self
                        .option
                        .decoded_span(start - 1..start)
                        .unwrap_or(self.option.range);
                    Span::new(equals.end, equals.end)
                });
            Some(NativeValue { text: value, range })
        } else {
            let value = self.value?;
            Some(NativeValue {
                text: &value.text,
                range: value
                    .decoded_span(0..value.text.len())
                    .unwrap_or(value.range),
            })
        }
    }
}

#[must_use]
pub fn uses<'syntax, 'catalog>(
    line: &'syntax Line,
    catalog: &'catalog FlagCatalog,
) -> Vec<NativeOption<'syntax, 'catalog>> {
    let options = line.options();
    let mut uses = Vec::new();
    let mut index = 0;
    while let Some(option) = options.get(index) {
        let resolved = catalog.resolve_option(&option.text);
        let takes_next = resolved
            .is_some_and(|resolved| resolved.flag.requires_value && !option.text.contains('='));
        let value = takes_next.then(|| options.get(index + 1)).flatten();
        uses.push(NativeOption {
            option,
            resolved,
            value,
        });
        index += 1 + usize::from(value.is_some());
    }
    uses
}
