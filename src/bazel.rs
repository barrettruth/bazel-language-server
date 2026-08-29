//! Workspace discovery and Bazel invocation.
//!
//! Isolated from the server so the risky part — subprocesses, version drift,
//! external repositories — can be exercised without an editor.
//!
//! Nothing here may be called from an LSP request handler. Invariant 1: Bazel
//! runs on its own thread and publishes results; handlers read a snapshot.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use shared_child::SharedChild;

/// `SIGINT`, written out because this crate has no other need of `libc` and
/// POSIX fixes the number at 2 on every platform Bazel runs on.
#[cfg(unix)]
const SIGINT: std::ffi::c_int = 2;

/// The oldest Bazel this server drives.
///
/// Chosen rather than discovered. Bazel 8 is where `--proto:rule_classes`
/// landed and where bzlmod became the default, so rule schemas have a source
/// and the repository mapping has one path instead of two. Older releases get
/// the static tier and an explanation.
pub const FLOOR: Version = Version::new(8, 0, 0);

/// A Bazel release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    #[must_use]
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// The release named in what `bazel --version` printed.
    ///
    /// `bazel 8.7.0`, and `bazel 8.7.0rc2` for a candidate, which is the
    /// release it is a candidate for as far as its flags are concerned. A
    /// build from source prints `bazel no_version` and names no release.
    #[must_use]
    pub fn parse(reported: &str) -> Option<Self> {
        let field = reported
            .split_whitespace()
            .find(|field| field.starts_with(|c: char| c.is_ascii_digit()))?;
        let mut parts = field.split('.');
        Some(Self::new(
            leading_number(parts.next()?)?,
            leading_number(parts.next().unwrap_or("0"))?,
            leading_number(parts.next().unwrap_or("0"))?,
        ))
    }

    /// What this release can be asked for.
    #[must_use]
    pub fn capabilities(self) -> Capabilities {
        Capabilities {
            rule_classes: self >= Version::new(8, 0, 0),
            repo_mapping: self >= Version::new(7, 1, 0),
            query_output_file: self >= Version::new(8, 2, 0),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The digits a dotted field opens with, so `0rc2` and `1-homebrew` both read
/// as the number they start with.
fn leading_number(field: &str) -> Option<u32> {
    let digits: String = field.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The oracles the installed Bazel offers.
///
/// Derived from the release rather than discovered by trying: a flag that is
/// not there costs a subprocess, a lock and an error to parse, and every one of
/// these landed in a release that can be named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// `--proto:rule_classes`, carrying stardoc `RuleInfo` for the rules a
    /// workspace instantiates. Bazel 8.
    pub rule_classes: bool,
    /// `bazel mod dump_repo_mapping`, the only correct source of the apparent
    /// to canonical repository mapping. Bazel 7.1.
    pub repo_mapping: bool,
    /// `--output_file`, which writes a query result straight to disk instead of
    /// through the gRPC hop to stdout. Bazel 8.2.
    pub query_output_file: bool,
}

/// A Bazel that answered, and what it can be asked.
#[derive(Debug, Clone)]
pub struct Probe {
    /// The line `bazel --version` printed, for a human to read.
    pub reported: String,
    pub version: Version,
    pub capabilities: Capabilities,
}

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Default for BazelConfig {
    fn default() -> Self {
        Self {
            enable: true,
            path: "bazel".to_string(),
            private_output_base: false,
            args: Vec::new(),
        }
    }
}

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

/// What a Bazel invocation reported, once its output has been consumed.
#[derive(Debug)]
pub struct Outcome {
    pub stderr: String,
    pub status: Option<i32>,
}

impl Outcome {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }
}

/// Stops a Bazel that is still running.
///
/// Cloneable, and a no-op once the command has finished, so a superseding
/// refresh may hold one without racing the invocation it means to replace.
///
/// **`SIGINT`, never `SIGKILL`.** `SIGKILL` leaves Bazel's server-side command
/// running to completion with the command lock held under a dead pid, and every
/// later invocation then fails with `Another command (pid=…) is running`.
/// `SIGINT` is what the client turns into a `Cancel` RPC, and it releases the
/// lock in 13-42 ms. `std::process::Child::kill` sends `SIGKILL`, which is why
/// it is not used here.
#[derive(Clone)]
pub struct Interrupt(Arc<SharedChild>);

impl Interrupt {
    /// Ask the command to stop. Whether it does is its own business: Bazel's
    /// query output-serialisation phase runs to completion regardless, which is
    /// why a superseded refresh is discarded rather than relied on to die.
    pub fn send(&self) {
        #[cfg(unix)]
        {
            use shared_child::unix::SharedChildExt;
            if let Err(err) = self.0.send_signal(SIGINT) {
                tracing::debug!("interrupting bazel: {err}");
            }
        }
        #[cfg(not(unix))]
        {
            drop(self.0.kill());
        }
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

    /// The binary being invoked, for a message that has to name it.
    #[must_use]
    pub fn binary(&self) -> &str {
        &self.config.path
    }

    /// The installed Bazel, if this server can drive it.
    ///
    /// Reported rather than assumed: invariant 3 says a Bazel that cannot
    /// answer must be explained, not silently degraded. Every way this fails
    /// names which one it was, because "unfetched repo", "no bazel on PATH"
    /// and "too old" call for three different things from the user.
    ///
    /// # Errors
    ///
    /// If the subsystem is disabled, the binary is missing or exits non-zero,
    /// it names no release, or that release is older than [`FLOOR`].
    pub fn probe(&self) -> Result<Probe> {
        self.probe_started(|_| {})
    }

    pub fn probe_started(&self, started: impl FnOnce(Interrupt)) -> Result<Probe> {
        if !self.config.enable {
            bail!("the Bazel subsystem is disabled (bazel.enable = false)");
        }
        let out = self
            .run_started(&["--version"], started)
            .with_context(|| format!("could not run `{}`", self.config.path))?;
        if !out.ok() {
            bail!("`{} --version` failed: {}", self.config.path, out.stderr);
        }
        let reported = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let Some(version) = Version::parse(&reported) else {
            bail!(
                "`{} --version` said {reported:?}, which names no release",
                self.config.path
            );
        };
        if version < FLOOR {
            bail!(
                "Bazel {version} is older than {FLOOR}, which this server needs for \
                 `--proto:rule_classes` and for bzlmod to be the default"
            );
        }
        Ok(Probe {
            reported,
            version,
            capabilities: version.capabilities(),
        })
    }

    /// Invoke Bazel and collect everything it printed.
    ///
    /// Blocking, and deliberately so: this runs on the Bazel thread, never in a
    /// request handler. For anything whose output is measured in megabytes use
    /// [`BazelClient::stream`]; this holds all of it at once and is for the
    /// small answers, `--version` and `info` among them.
    ///
    /// # Errors
    ///
    /// If the process cannot be spawned or its output cannot be collected.
    pub fn run_started(
        &self,
        args: &[&str],
        started: impl FnOnce(Interrupt),
    ) -> Result<Invocation> {
        let mut stdout = Vec::new();
        let outcome = self.stream(args, started, &mut |chunk| stdout.extend_from_slice(chunk))?;
        Ok(Invocation {
            stdout,
            stderr: outcome.stderr,
            status: outcome.status,
        })
    }

    /// Invoke Bazel, handing stdout to `sink` in the chunks it arrives in.
    ///
    /// The only place this crate spawns a process, so every property below
    /// holds of every Bazel it starts.
    ///
    /// Output is streamed rather than collected. A `query` over 240k targets is
    /// 205 MiB, which costs 34 MB streamed against 442 MB slurped at the same
    /// wall time — thirteen times the memory for nothing. stderr is drained on
    /// its own thread, because a child that fills a pipe nobody is reading
    /// blocks forever, and `stdin` is null so it can never wait on input that
    /// is not coming.
    ///
    /// `started` receives the [`Interrupt`] before the first byte, which is how
    /// a superseding refresh reaches a command already in flight.
    ///
    /// # Errors
    ///
    /// If the process cannot be spawned, or its output cannot be read.
    pub fn stream(
        &self,
        args: &[&str],
        started: impl FnOnce(Interrupt),
        sink: &mut dyn FnMut(&[u8]),
    ) -> Result<Outcome> {
        self.stream_with(args, true, started, sink)
    }

    pub fn run_shared_started(
        &self,
        args: &[&str],
        started: impl FnOnce(Interrupt),
    ) -> Result<Invocation> {
        let mut stdout = Vec::new();
        let outcome = self.stream_with(args, false, started, &mut |chunk| {
            stdout.extend_from_slice(chunk);
        })?;
        Ok(Invocation {
            stdout,
            stderr: outcome.stderr,
            status: outcome.status,
        })
    }

    pub fn spawn(&self, args: &[&str]) -> Result<()> {
        let mut child = self
            .command(args, true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawning `{}`", self.config.path))?;
        std::thread::spawn(move || drop(child.wait()));
        Ok(())
    }

    fn stream_with(
        &self,
        args: &[&str],
        private_output_base: bool,
        started: impl FnOnce(Interrupt),
        sink: &mut dyn FnMut(&[u8]),
    ) -> Result<Outcome> {
        let mut command = self.command(args, private_output_base);

        tracing::debug!(?args, "bazel");
        let child = Arc::new(
            SharedChild::spawn(&mut command)
                .with_context(|| format!("spawning `{}`", self.config.path))?,
        );
        started(Interrupt(Arc::clone(&child)));

        let mut errors = child.take_stderr().context("the child's stderr")?;
        let drain = std::thread::spawn(move || {
            let mut collected = String::new();
            drop(errors.read_to_string(&mut collected));
            collected
        });

        let mut out = child.take_stdout().context("the child's stdout")?;
        let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
        loop {
            let read = out.read(&mut buffer).context("reading bazel's output")?;
            if read == 0 {
                break;
            }
            sink(&buffer[..read]);
        }

        let status = child.wait().context("waiting for bazel")?;
        Ok(Outcome {
            stderr: drain.join().unwrap_or_default(),
            status: status.code(),
        })
    }

    fn command(&self, args: &[&str], private_output_base: bool) -> Command {
        let mut command = Command::new(&self.config.path);
        command
            .current_dir(&self.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if private_output_base && let Some(base) = &self.output_base {
            command.arg(format!("--output_base={}", base.display()));
        }
        command.args(&self.config.args).args(args);
        command
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
    fn a_reported_line_names_its_release() {
        assert_eq!(Version::parse("bazel 8.7.0"), Some(Version::new(8, 7, 0)));
        assert_eq!(
            Version::parse("bazel release 8.7.0"),
            Some(Version::new(8, 7, 0))
        );
    }

    /// A candidate has the flags of the release it is a candidate for, which is
    /// what the version is consulted about.
    #[test]
    fn a_candidate_reads_as_its_release() {
        assert_eq!(
            Version::parse("bazel 8.2.0rc2"),
            Some(Version::new(8, 2, 0))
        );
        assert_eq!(
            Version::parse("bazel 7.4.1-homebrew"),
            Some(Version::new(7, 4, 1))
        );
        // What the nixpkgs build reports, which is the one this is developed
        // against: a trailing hyphen where a suffix would go, and a field after
        // the version that is not one.
        assert_eq!(
            Version::parse("bazel 8.7.0- (@non-git)"),
            Some(Version::new(8, 7, 0))
        );
    }

    #[test]
    fn a_build_from_source_names_no_release() {
        assert_eq!(Version::parse("bazel no_version"), None);
        assert_eq!(Version::parse(""), None);
    }

    #[test]
    fn an_oracle_arrives_with_the_release_that_added_it() {
        // `--output_file` landed in 8.2, so the floor itself lacks it.
        assert!(!FLOOR.capabilities().query_output_file);
        assert!(Version::new(8, 2, 0).capabilities().query_output_file);
        // The other two are older than the floor, so every Bazel we drive has
        // them.
        assert!(FLOOR.capabilities().rule_classes);
        assert!(FLOOR.capabilities().repo_mapping);
    }

    #[test]
    fn releases_order_by_component() {
        assert!(Version::new(8, 10, 0) > Version::new(8, 9, 9));
        assert!(Version::new(7, 99, 0) < FLOOR);
    }

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
