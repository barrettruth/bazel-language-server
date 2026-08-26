//! Request handling against in-memory state.
//!
//! Everything here is parse, walk and convert against a snapshot. No Bazel:
//! that is invariant 1, expressed as a module boundary. The only filesystem
//! call is a `stat` asking whether a label names a source file, which costs
//! one syscall and cannot block on anything.
//!
//! One module per request, each owning the helpers only it uses. What several
//! requests share — resolving the cursor to a string, and the string to a
//! target and its sites — is in `cursor`. The dispatch table is in `main.rs`.

mod cursor;
mod definition;
mod diagnostics;
mod highlight;
mod hover;
mod references;
mod rename;
mod symbols;

#[cfg(test)]
mod fixture;

pub use definition::definition;
pub use diagnostics::syntax_diagnostics;
pub use highlight::document_highlight;
pub use hover::hover;
pub use references::references;
pub use rename::{prepare_rename, rename};
pub use symbols::{document_symbols, workspace_symbols};
