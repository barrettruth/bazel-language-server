//! Apparent repository names, and where their contents sit on disk.
//!
//! `@bazel_skylib` in one workspace and `@bazel_skylib` in another need not be
//! the same repository, so an apparent name means nothing on its own. The
//! mapping from it to a canonical name is per-workspace and
//! `bazel mod dump_repo_mapping ""` is the only correct source: guessing that
//! `@rules_go` is `rules_go+` was already wrong once, when the format changed
//! in Bazel 8. Measured here: `platforms` maps to `platforms` and
//! `bazel_skylib` to `bazel_skylib+`, in the same workspace.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use rustc_hash::FxHashMap;

use crate::bazel::BazelClient;

/// What an apparent repository name turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// The main repository, named by its module name or by `@`.
    Main,
    /// Fetched, and its root is here.
    At(PathBuf),
    /// A real repository of this workspace that has not been fetched, so
    /// nothing under it exists to navigate to yet.
    Unfetched(String),
    /// The mapping has been read and does not contain this name.
    Unknown,
    /// No mapping has been read, so nothing can be said about any name.
    Unavailable,
}

/// The repository mapping of one workspace, and the tree its externals live in.
#[derive(Debug, Default)]
pub struct Repos {
    /// Apparent name to canonical name. The main repository appears as `""`,
    /// under both `""` and the module's own name.
    mapping: FxHashMap<String, String>,
    /// The user's own output base, never ours — with `privateOutputBase` on,
    /// our `external/` is a second copy of every repository, and a definition
    /// landing in it opens a file the user does not recognise and cannot
    /// usefully edit.
    output_base: PathBuf,
}

impl Repos {
    /// Ask Bazel for both, on the thread that is allowed to.
    ///
    /// # Errors
    ///
    /// If either command cannot run or answers with something that is not what
    /// it documents.
    pub fn read(client: &BazelClient) -> Result<Self> {
        Self::read_started(client, |_| {})
    }

    pub fn read_started(
        client: &BazelClient,
        mut started: impl FnMut(crate::bazel::Interrupt),
    ) -> Result<Self> {
        let dumped = client
            .run_started(&["mod", "dump_repo_mapping", ""], &mut started)
            .context("asking Bazel for the repository mapping")?;
        if !dumped.ok() {
            // A wrapper written `bazelisk $@` rather than `bazelisk "$@"` drops
            // the empty argument, and Bazel then reports a missing one. That is
            // a wrapper to fix, not a workspace without a mapping, and the two
            // call for entirely different things from the reader.
            if dumped.stderr.contains("No repository name(s) specified") {
                bail!(
                    "`{}` reached Bazel without the empty argument that names the main \
                     repository, which is what a wrapper script written `bazelisk $@` instead of \
                     `bazelisk \"$@\"` does to it",
                    client.binary()
                );
            }
            bail!(
                "`bazel mod dump_repo_mapping` failed: {}",
                dumped.stderr.lines().next_back().unwrap_or_default()
            );
        }
        let mapping = serde_json::from_slice(&dumped.stdout)
            .context("the repository mapping is one JSON object of apparent to canonical names")?;

        let base = client
            .run_shared_started(&["info", "output_base"], &mut started)
            .context("asking Bazel for the output base")?;
        if !base.ok() {
            bail!(
                "`bazel info output_base` failed: {}",
                base.stderr.lines().next_back().unwrap_or_default()
            );
        }
        Ok(Self {
            mapping,
            output_base: PathBuf::from(String::from_utf8_lossy(&base.stdout).trim()),
        })
    }

    /// What `apparent` names, as written between `@` and `//`.
    ///
    /// A name absent from the mapping is tried as a canonical one, because
    /// `@@canonical//` is legal in source and is already past the mapping. The
    /// directory settles it either way: it exists or the repository is not
    /// fetched.
    #[must_use]
    pub fn locate(&self, apparent: &str) -> Resolved {
        // The main repository is in every mapping, under `""` at least, so an
        // empty one has not been read rather than genuinely holding nothing.
        if self.mapping.is_empty() {
            return Resolved::Unavailable;
        }
        let known = self.mapping.get(apparent);
        if known.is_some_and(String::is_empty) {
            return Resolved::Main;
        }
        let canonical = known.map_or(apparent, String::as_str);
        let at = self.output_base.join("external").join(canonical);
        if at.is_dir() {
            return Resolved::At(at);
        }
        if known.is_some() {
            Resolved::Unfetched(canonical.to_string())
        } else {
            Resolved::Unknown
        }
    }

    /// The tree externals live in, for a caller reporting on the subsystem.
    #[must_use]
    pub fn output_base(&self) -> &Path {
        &self.output_base
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.mapping.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real answer from `tests/workspace`, which is where every shape below
    /// comes from: a canonical name that gained a `+`, one that did not, an
    /// alias, the module naming itself, and an extension-generated repository.
    fn measured() -> Repos {
        let json = r#"{"direct_repo_rule_target":"+_repo_rules+direct_repo_rule_target",
            "":"","torture":"","bazel_skylib":"bazel_skylib+","platforms":"platforms",
            "sh":"rules_shell+","bazel_tools":"bazel_tools"}"#;
        Repos {
            mapping: serde_json::from_str(json).expect("the measured mapping"),
            output_base: std::env::temp_dir().join("bls-repos-test"),
        }
    }

    #[test]
    fn the_module_naming_itself_is_the_main_repository() {
        let repos = measured();
        assert_eq!(repos.locate(""), Resolved::Main);
        assert_eq!(repos.locate("torture"), Resolved::Main);
    }

    /// Apparent and canonical differ by a suffix that is not guessable — and
    /// `platforms` shows the suffix is not always there.
    #[test]
    fn a_name_in_the_mapping_that_is_not_on_disk_is_unfetched() {
        let repos = measured();
        assert_eq!(
            repos.locate("bazel_skylib"),
            Resolved::Unfetched("bazel_skylib+".to_string())
        );
        assert_eq!(
            repos.locate("platforms"),
            Resolved::Unfetched("platforms".to_string())
        );
        // An alias resolves to what it aliases, which shares no text with it.
        assert_eq!(
            repos.locate("sh"),
            Resolved::Unfetched("rules_shell+".to_string())
        );
    }

    #[test]
    fn a_fetched_repository_resolves_to_its_root() {
        let repos = measured();
        let fetched = repos.output_base.join("external/bazel_skylib+/rules");
        std::fs::create_dir_all(&fetched).expect("a fetched repository");

        assert_eq!(
            repos.locate("bazel_skylib"),
            Resolved::At(repos.output_base.join("external/bazel_skylib+"))
        );
        // `@@canonical//` is already past the mapping and is legal in source.
        assert_eq!(
            repos.locate("bazel_skylib+"),
            Resolved::At(repos.output_base.join("external/bazel_skylib+"))
        );

        std::fs::remove_dir_all(&repos.output_base).ok();
    }

    /// A name this workspace has never heard of is not the same as one it has
    /// heard of and not fetched: the first is a typo, the second is a `bazel
    /// fetch` away.
    #[test]
    fn a_name_the_workspace_does_not_know_is_not_an_unfetched_one() {
        assert_eq!(measured().locate("not_a_dependency"), Resolved::Unknown);
    }

    /// Every mapping contains the main repository, so an empty one was never
    /// read — which is a different answer from every name being unknown.
    #[test]
    fn no_mapping_says_so_rather_than_calling_every_name_unknown() {
        assert_eq!(
            Repos::default().locate("bazel_skylib"),
            Resolved::Unavailable
        );
        assert_eq!(Repos::default().locate(""), Resolved::Unavailable);
    }
}
