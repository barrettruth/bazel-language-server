//! Native option/value grouping according to the exact flag catalog.

use super::syntax::{Line, Token};
use super::{FlagCatalog, ResolvedFlag};

#[derive(Clone, Copy)]
pub struct NativeOption<'syntax, 'catalog> {
    pub option: &'syntax Token,
    pub resolved: Option<ResolvedFlag<'catalog>>,
    pub value: Option<&'syntax Token>,
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
