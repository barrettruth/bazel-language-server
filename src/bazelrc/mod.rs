//! Bazel 8.7 rc syntax and semantics.

mod catalog;
mod commands;
mod completion;
mod index;
mod navigation;
mod provider;
mod structural;
pub mod syntax;

pub use catalog::{CatalogHandle, Flag, FlagCatalog};
pub use index::{ConfigurationHandle, ConfigurationSnapshot, ImportSite, ProblemSeverity};
pub use provider::{diagnostics, respond};
