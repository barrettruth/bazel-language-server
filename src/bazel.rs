//! Workspace discovery and Bazel invocation.
//!
//! Isolated from the server so the risky part — subprocesses, version drift,
//! external repositories — can be exercised without an editor.
//!
//! Nothing here may be called from an LSP request handler. Invariant 1: Bazel
//! runs on its own thread and publishes results; handlers read a snapshot.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Files that mark a Bazel workspace root, in priority order.
///
/// `MODULE.bazel` first: a vendored legacy `WORKSPACE` deeper in the tree must
/// not outrank the real root. `WORKSPACE` remains a boundary marker in Bazel 9
/// even though the file itself no longer does anything.
pub const ROOT_MARKERS: &[&str] = &[
    "MODULE.bazel",
    "WORKSPACE.bazel",
    "WORKSPACE.bzlmod",
    "WORKSPACE",
    "REPO.bazel",
];

/// User-facing configuration for the Bazel subsystem.
///
/// The whole subsystem is optional. With `enable = false`, or with no `bazel`
/// on `PATH`, the server still serves the static tier — see invariant 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BazelConfig {
    pub enable: bool,
    /// Binary to invoke. `bazelisk` works too.
    pub path: String,
    /// Give the server its own `--output_base` so queries never queue behind
    /// the user's build.
    ///
    /// Off by default: a second Bazel server measured **+1.2 GB** at 20k
    /// packages, and the lock only bites code that waits on Bazel — which,
    /// per invariant 1, nothing does.
    pub private_output_base: bool,
    pub args: Vec<String>,
    pub timeout_seconds: u64,
}

impl Default for BazelConfig {
    fn default() -> Self {
        Self {
            enable: true,
            path: "bazel".to_string(),
            private_output_base: false,
            args: Vec::new(),
            timeout_seconds: 120,
        }
    }
}

impl BazelConfig {}

/// A located Bazel workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub root: PathBuf,
    /// Which marker identified the root.
    pub marker: &'static str,
}

/// Walk upward from `start` looking for a workspace root.
///
/// Markers are ranked rather than taken first-found, so a nested `WORKSPACE`
/// never beats an ancestor's `MODULE.bazel` at the same depth.
#[must_use]
pub fn find_workspace(start: &Path) -> Option<Workspace> {
    let mut dir = if start.is_dir() {
        Some(start)
    } else {
        start.parent()
    };
    while let Some(current) = dir {
        for marker in ROOT_MARKERS {
            if current.join(marker).is_file() {
                return Some(Workspace {
                    root: current.to_path_buf(),
                    marker,
                });
            }
        }
        dir = current.parent();
    }
    None
}

/// Outcome of one Bazel invocation.
#[derive(Debug)]
pub struct Invocation {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub status: Option<i32>,
}

impl Invocation {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }
}

/// Runs Bazel. Owned by a single dedicated thread; not `Sync` by intent.
#[derive(Debug, Clone)]
pub struct BazelClient {
    config: BazelConfig,
    workspace: PathBuf,
    output_base: Option<PathBuf>,
}

impl BazelClient {
    #[must_use]
    pub fn new(config: BazelConfig, workspace: PathBuf) -> Self {
        let output_base = config
            .private_output_base
            .then(|| private_output_base(&workspace));
        Self {
            config,
            workspace,
            output_base,
        }
    }

    /// Whether Bazel is enabled and the binary can actually be run.
    ///
    /// Reported rather than assumed: invariant 3 says a missing Bazel must be
    /// explained, not silently degraded.
    ///
    /// # Errors
    ///
    /// If the subsystem is disabled, the binary is missing, or it exits non-zero.
    pub fn probe(&self) -> Result<String> {
        if !self.config.enable {
            bail!("the Bazel subsystem is disabled (bazel.enable = false)");
        }
        let out = self
            .run(&["--version"])
            .with_context(|| format!("could not run `{}`", self.config.path))?;
        if !out.ok() {
            bail!("`{} --version` failed: {}", self.config.path, out.stderr);
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Invoke Bazel and wait.
    ///
    /// Blocking, and deliberately so: this runs on the Bazel thread, never in a
    /// request handler.
    ///
    /// When this grows cancellation, it must **not** use [`std::process::Child::kill`].
    /// That sends `SIGKILL`, which leaves the server-side command running to
    /// completion while the command lock is held by a dead pid; every later
    /// invocation then fails with `Another command (pid=…) is running`. `SIGINT`
    /// is what the client turns into a `Cancel` RPC, and releases the lock in
    /// 13-42 ms.
    ///
    /// # Errors
    ///
    /// If the process cannot be spawned or its output cannot be collected.
    pub fn run(&self, args: &[&str]) -> Result<Invocation> {
        let mut command = Command::new(&self.config.path);
        command.current_dir(&self.workspace);
        if let Some(base) = &self.output_base {
            command.arg(format!("--output_base={}", base.display()));
        }
        command.args(&self.config.args).args(args);

        tracing::debug!(?args, "bazel");
        let output = command
            .output()
            .with_context(|| format!("spawning `{}`", self.config.path))?;

        Ok(Invocation {
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status.code(),
        })
    }
}

/// A per-workspace output base under the system temp directory.
fn private_output_base(workspace: &Path) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in workspace.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    std::env::temp_dir().join(format!("bls-output-base-{hash:016x}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_are_conservative() {
        let config = BazelConfig::default();
        assert!(config.enable, "Bazel is on by default when present");
        assert!(
            !config.private_output_base,
            "a second server costs over a gigabyte; opt in"
        );
    }

    #[test]
    fn module_bazel_outranks_a_nested_workspace() {
        let dir = std::env::temp_dir().join("bls-root-test/nested/deep");
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.parent().unwrap().parent().unwrap();
        std::fs::write(root.join("MODULE.bazel"), "module(name='t')\n").unwrap();

        let found = find_workspace(&dir).expect("a workspace");
        assert_eq!(found.root, root);
        assert_eq!(found.marker, "MODULE.bazel");

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn private_output_base_is_stable_per_workspace() {
        let a = private_output_base(Path::new("/ws/one"));
        let b = private_output_base(Path::new("/ws/one"));
        let c = private_output_base(Path::new("/ws/two"));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
