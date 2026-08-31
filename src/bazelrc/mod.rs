//! Bazel 8.7 rc syntax and semantics.

mod provider;
pub mod syntax;

pub use provider::{respond, syntax_diagnostics};
