//! Bazel 8.7 rc syntax and semantics.

mod catalog;
mod commands;
mod completion;
mod diagnostics;
mod hover;
mod index;
mod native_options;
mod navigation;
mod provider;
mod structural;
pub mod syntax;
mod view;

pub use catalog::{CatalogHandle, Flag, FlagCatalog, FlagSpelling, ResolvedFlag};
pub use diagnostics::diagnostics;
pub use index::{ConfigurationHandle, ConfigurationSnapshot, ImportSite, ProblemSeverity};
pub use provider::respond;
pub use view::ConfigurationView;
