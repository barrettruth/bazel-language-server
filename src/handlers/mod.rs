//! Request handlers over document and index snapshots.
//!
//! Cursor and label resolution shared across handlers lives in `cursor`.

mod cursor;
mod definition;
mod diagnostics;
mod folding;
mod highlight;
mod hover;
mod implementation;
mod inlay;
mod lens;
mod links;
mod references;
mod rename;
mod selection;
mod semantic;
pub(super) mod symbols;

#[cfg(test)]
mod fixture;

pub use definition::definition;
pub use diagnostics::syntax_diagnostics;
pub use folding::folding_ranges;
pub use highlight::document_highlight;
pub use hover::hover;
pub use implementation::implementation;
pub use inlay::inlay_hints;
pub use lens::{RUN_COMMAND, code_lenses};
pub use links::document_links;
pub use references::references;
pub use rename::{prepare_rename, rename};
pub use selection::selection_ranges;
pub use semantic::{LEGEND, semantic_tokens};
pub use symbols::{document_symbols, workspace_symbols};
