//! Bazel 8.7 rc syntax and semantics.

mod catalog;
mod index;
mod provider;
pub mod syntax;

pub use catalog::{CatalogHandle, Flag, FlagCatalog};
pub use index::{ConfigurationHandle, ConfigurationSnapshot};
pub use provider::{respond, syntax_diagnostics};
