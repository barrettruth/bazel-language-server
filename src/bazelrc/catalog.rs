//! Flag metadata reported by the configured Bazel 8.7 executable.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use arc_swap::ArcSwapOption;
use base64::Engine as _;
use prost::Message as _;
use rustc_hash::FxHashMap;

use crate::bazel::{BazelClient, Interrupt};

mod proto {
    #![allow(dead_code, clippy::pedantic, clippy::all)]

    include!(concat!(env!("OUT_DIR"), "/bazel_flags.rs"));
}

/// One option accepted by the configured Bazel executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    pub name: Box<str>,
    pub has_negative_flag: bool,
    pub documentation: Option<Box<str>>,
    pub commands: Vec<Box<str>>,
    pub abbreviation: Option<Box<str>>,
    pub allows_multiple: bool,
    pub effect_tags: Vec<Box<str>>,
    pub metadata_tags: Vec<Box<str>>,
    pub documentation_category: Option<Box<str>>,
    pub requires_value: bool,
    pub default_value: Option<Box<str>>,
    pub old_name: Option<Box<str>>,
    pub deprecation_warning: Option<Box<str>>,
    pub option_expansions: Vec<Box<str>>,
    pub type_converter: Option<Box<str>>,
    pub enum_values: Vec<Box<str>>,
}

impl Flag {
    /// Whether this flag is legal in an rc section for `command`.
    #[must_use]
    pub fn supports(&self, command: &str) -> bool {
        self.commands
            .iter()
            .any(|candidate| candidate.as_ref() == command)
    }
}

/// The complete `help flags-as-proto` answer from one Bazel 8.7 executable.
#[derive(Debug, Default)]
pub struct FlagCatalog {
    reported: Box<str>,
    flags: FxHashMap<Box<str>, Flag>,
}

impl FlagCatalog {
    /// Invoke Bazel and decode its self-reported option metadata.
    ///
    /// # Errors
    ///
    /// If Bazel cannot be invoked, declines the command, or returns malformed
    /// base64 or protobuf output.
    pub fn read_started(
        client: &BazelClient,
        reported: impl Into<Box<str>>,
        started: impl FnOnce(Interrupt),
    ) -> Result<Self> {
        let invocation = client.run_shared_started(
            &["--ignore_all_rc_files", "help", "flags-as-proto"],
            started,
        )?;
        if !invocation.ok() {
            bail!(
                "`{} help flags-as-proto` failed: {}",
                client.binary(),
                invocation.stderr.trim()
            );
        }
        Self::decode(reported.into(), &invocation.stdout)
    }

    fn decode(reported: Box<str>, encoded: &[u8]) -> Result<Self> {
        let encoded: Vec<_> = encoded
            .iter()
            .copied()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .context("decoding Bazel's base64 flag catalog")?;
        let collection = proto::FlagCollection::decode(bytes.as_slice())
            .context("decoding Bazel's flag catalog protobuf")?;
        let flags = collection
            .flag_infos
            .into_iter()
            .map(Flag::from)
            .map(|flag| (flag.name.clone(), flag))
            .collect();
        Ok(Self { reported, flags })
    }

    #[must_use]
    pub fn reported(&self) -> &str {
        &self.reported
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Flag> {
        self.flags.get(name)
    }

    /// Find a flag by its canonical, abbreviated, or deprecated name.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<&Flag> {
        self.get(name).or_else(|| {
            self.flags.values().find(|flag| {
                flag.abbreviation.as_deref() == Some(name) || flag.old_name.as_deref() == Some(name)
            })
        })
    }

    pub fn flags(&self) -> impl Iterator<Item = &Flag> {
        self.flags.values()
    }
}

impl From<proto::FlagInfo> for Flag {
    fn from(flag: proto::FlagInfo) -> Self {
        let has_negative_flag = flag.has_negative_flag();
        let allows_multiple = flag.allows_multiple();
        let requires_value = flag.requires_value();
        Self {
            name: flag.name.into(),
            has_negative_flag,
            documentation: flag.documentation.map(Into::into),
            commands: boxed(flag.commands),
            abbreviation: flag.abbreviation.map(Into::into),
            allows_multiple,
            effect_tags: boxed(flag.effect_tags),
            metadata_tags: boxed(flag.metadata_tags),
            documentation_category: flag.documentation_category.map(Into::into),
            requires_value,
            default_value: flag.default_value.map(Into::into),
            old_name: flag.old_name.map(Into::into),
            deprecation_warning: flag.deprecation_warning.map(Into::into),
            option_expansions: boxed(flag.option_expansions),
            type_converter: flag.type_converter.map(Into::into),
            enum_values: boxed(flag.enum_values),
        }
    }
}

fn boxed(values: Vec<String>) -> Vec<Box<str>> {
    values.into_iter().map(Into::into).collect()
}

/// Lock-free publication of the actor-owned flag catalog.
#[derive(Clone, Default)]
pub struct CatalogHandle(Arc<ArcSwapOption<FlagCatalog>>);

impl CatalogHandle {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn load(&self) -> Option<Arc<FlagCatalog>> {
        self.0.load_full()
    }

    pub fn store(&self, catalog: FlagCatalog) {
        self.0.store(Some(Arc::new(catalog)));
    }

    pub fn clear(&self) {
        self.0.store(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_exact_bazel_schema() {
        let reported = proto::FlagInfo {
            name: "jobs".to_owned(),
            documentation: Some("Number of concurrent jobs".to_owned()),
            commands: vec!["build".to_owned(), "test".to_owned()],
            requires_value: Some(true),
            default_value: Some("auto".to_owned()),
            type_converter: Some("jobs".to_owned()),
            ..Default::default()
        };
        let bytes = proto::FlagCollection {
            flag_infos: vec![reported],
        }
        .encode_to_vec();
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let catalog = FlagCatalog::decode("bazel 8.7.0".into(), encoded.as_bytes()).unwrap();
        let jobs = catalog.get("jobs").unwrap();
        assert_eq!(catalog.reported(), "bazel 8.7.0");
        assert!(jobs.supports("test"));
        assert!(jobs.requires_value);
        assert_eq!(jobs.default_value.as_deref(), Some("auto"));
    }

    #[test]
    fn publication_can_be_cleared() {
        let handle = CatalogHandle::new();
        handle.store(FlagCatalog::default());
        assert!(handle.load().is_some());
        handle.clear();
        assert!(handle.load().is_none());
    }
}
