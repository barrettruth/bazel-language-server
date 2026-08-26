//! Formatting, delegated to `buildifier`.
//!
//! This is the one subprocess the server runs inside a request, and it is a
//! deliberate exception rather than a crack in invariant 1. What that invariant
//! is about is Bazel: seconds to minutes of work, one global lock per output
//! base, and an answer that belongs in an index because several requests will
//! want it. buildifier is none of those. It is a parser and a printer over one
//! buffer — single-digit milliseconds, no server, no lock, no network — and its
//! answer is good for exactly one request.
//!
//! `textDocument/formatting` is also synchronous by nature: the user pressed a
//! key and is waiting for the buffer to change, so there is no snapshot that
//! could usefully answer instead. Every language server that formats runs the
//! language's formatter this way, and here it is the requirement — the point of
//! formatting a BUILD file is to agree with `buildifier` byte for byte, which a
//! reimplementation would do only until the next release of it.
//!
//! Bazel calls stay where they are: on the Bazel thread, behind `bls-bazel`,
//! unreachable from a handler.
//!
//! buildifier may be absent, exactly as Bazel may be. Missing, hung, failing or
//! answering with something that is not text all come back the same way — no
//! edits, and a warning on stderr, so a formatter that is not working is
//! distinguishable from a buffer that needs no formatting.

use std::io::{Read, Write};
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use lsp_types::{Position, Range, TextEdit};
use starlark_cst::FileKind;

use crate::line_index::LineIndex;

/// Looked up on `PATH`, like `bazel` is.
const BUILDIFIER: &str = "buildifier";

/// Ceiling on one run.
///
/// A parse and a print over one buffer is milliseconds at any size a person
/// edits, so anything approaching this is a hang — and the loop waiting on it
/// is the one serving every other document.
const TIMEOUT: Duration = Duration::from_secs(2);

/// The edits that make `text` agree with buildifier.
///
/// Empty when the buffer is already formatted, and empty when buildifier ran
/// and refused the input: the syntax diagnostics already say why, and a second
/// report of the same thing is noise.
///
/// An `Err` means buildifier itself could not be used — absent, hung, or
/// answering with something that is not text. That reaches the client as a
/// request error it shows the user, which is the only outcome they can act on;
/// a format that quietly does nothing is indistinguishable from a file that
/// needed no formatting.
pub fn format(text: &str, kind: FileKind) -> Result<Vec<TextEdit>> {
    format_with(BUILDIFIER, text, kind)
}

/// `format`, against a named binary, so the absent-formatter path is reachable
/// from a test on a machine that has one.
fn format_with(binary: &str, text: &str, kind: FileKind) -> Result<Vec<TextEdit>> {
    let formatted = match run(binary, text, file_type(kind)) {
        Ok(formatted) => formatted,
        Err(Unusable::Rejected(reason)) => {
            tracing::debug!("{binary} refused the buffer: {reason}");
            return Ok(Vec::new());
        }
        Err(Unusable::Broken(err)) => {
            tracing::warn!("{binary}: {err:#}");
            return Err(err.context(format!("`{binary}` could not format this buffer")));
        }
    };
    // An edit that changes nothing still moves the cursor and marks the buffer
    // dirty, which is a worse answer than no answer.
    if formatted == text {
        return Ok(Vec::new());
    }
    Ok(vec![TextEdit {
        range: whole_document(text),
        new_text: formatted,
    }])
}

/// Why no formatted text came back.
///
/// The two are answered differently, so they cannot share a variant: a buffer
/// buildifier parsed and disliked is the user's problem and already has
/// diagnostics, while a buildifier that is missing or wedged is ours and has to
/// be said out loud.
enum Unusable {
    /// buildifier ran and exited non-zero, having read the buffer.
    Rejected(String),
    /// buildifier could not be started, timed out, or answered with nothing
    /// usable.
    Broken(anyhow::Error),
}

/// buildifier's `-type` for a classified file.
///
/// This is what selects the conventions, and the difference is not cosmetic:
/// `build`, `module` and `workspace` put every rule argument on its own line
/// and sort the lists Bazel knows are sortable, while `bzl` and `default` leave
/// a call on one line. A `.bzl` file formatted as `build` comes back rewritten
/// in ways buildifier would never do to it on disk.
///
/// The kinds with no mode of their own take `default`, which is what
/// buildifier's own filename detection gives them.
fn file_type(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Build => "build",
        FileKind::Bzl => "bzl",
        FileKind::Module => "module",
        FileKind::Workspace => "workspace",
        FileKind::Repo
        | FileKind::Vendor
        | FileKind::Cquery
        | FileKind::Prelude
        | FileKind::Scl => "default",
    }
}

/// The range covering `text` entirely.
///
/// A whole-document replacement is what most servers send, and the reason is
/// that a minimal diff has to be right: an edit range off by one corrupts the
/// buffer, and the client has already applied it by the time anyone notices.
fn whole_document(text: &str) -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: LineIndex::new(text).position(text, text.len()),
    }
}

/// Run buildifier over `text` on stdin and return what it printed.
///
/// The buffer goes in over a pipe because the buffer is the authority: the file
/// on disk is a different document whenever the editor has unsaved changes,
/// which is most of the time anyone asks for a format.
///
/// # Errors
///
/// If buildifier cannot be spawned, exits non-zero — which it does for a syntax
/// error, already reported as a diagnostic — outputs something that is not
/// UTF-8, or fails to answer within [`TIMEOUT`].
fn run(binary: &str, text: &str, file_type: &str) -> std::result::Result<String, Unusable> {
    // Every stream is a pipe. An inherited stdout would put the child's bytes
    // in the middle of an LSP frame, and the client would read it as us.
    let mut child = Command::new(binary)
        .arg(format!("-type={file_type}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running `{binary}`"))
        .map_err(Unusable::Broken)?;

    let streams = child
        .stdin
        .take()
        .zip(child.stdout.take())
        .zip(child.stderr.take())
        .context("the child's pipes")
        .map_err(Unusable::Broken)?;
    let ((stdin, stdout), stderr) = streams;

    let input = text.to_owned();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        // After a timeout the receiver is gone and there is nobody to tell.
        drop(sender.send(pump(stdin, stdout, stderr, &input)));
    });

    let pumped = receiver.recv_timeout(TIMEOUT);
    if pumped.is_err() {
        // The request is already lost; take the process with it rather than
        // leave it holding a thread and a set of pipes.
        child.kill().ok();
    }
    let status = child
        .wait()
        .context("waiting for buildifier")
        .map_err(Unusable::Broken)?;
    let (stdout, stderr) = pumped
        .map_err(|_| Unusable::Broken(anyhow!("no answer in {} ms, killed", TIMEOUT.as_millis())))?
        .context("talking to buildifier")
        .map_err(Unusable::Broken)?;

    if !status.success() {
        let code = status.code().map_or_else(
            || "killed by a signal".to_string(),
            |code| format!("exit {code}"),
        );
        let reason = stderr.trim();
        // Exiting non-zero after reading the buffer is how buildifier reports a
        // file it cannot parse, which is the user's syntax error and not a
        // broken tool.
        return Err(Unusable::Rejected(format!(
            "{code}: {}",
            if reason.is_empty() {
                "no message"
            } else {
                reason
            }
        )));
    }
    String::from_utf8(stdout)
        .context("buildifier printed something that is not UTF-8")
        .map_err(Unusable::Broken)
}

/// Feed the child and collect what it says.
///
/// Sequential because buildifier reads its whole input before printing any
/// output, and because a stall here is bounded anyway: the timeout kills the
/// child, which closes these pipes.
fn pump(
    mut stdin: ChildStdin,
    mut stdout: ChildStdout,
    mut stderr: ChildStderr,
    input: &str,
) -> std::io::Result<(Vec<u8>, String)> {
    stdin.write_all(input.as_bytes())?;
    // Closing stdin is the EOF buildifier waits for.
    drop(stdin);

    let mut out = Vec::new();
    stdout.read_to_end(&mut out)?;
    let mut err = String::new();
    stderr.read_to_string(&mut err)?;
    Ok((out, err))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// buildifier comes from the flake and a plain checkout has none. A test
    /// that needs the real thing says so and stops, rather than failing on the
    /// environment — the absent path has its own test and does not need it.
    fn buildifier_is_installed() -> bool {
        let installed = Command::new(BUILDIFIER)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok();
        if !installed {
            eprintln!("skipped: buildifier is not on PATH");
        }
        installed
    }

    /// Exhaustive: `file_type` matches on every variant, so a new kind of file
    /// fails to compile until it has picked a mode.
    #[test]
    fn every_file_kind_has_a_mode() {
        for (kind, expected) in [
            (FileKind::Build, "build"),
            (FileKind::Bzl, "bzl"),
            (FileKind::Module, "module"),
            (FileKind::Workspace, "workspace"),
            (FileKind::Repo, "default"),
            (FileKind::Vendor, "default"),
            (FileKind::Cquery, "default"),
            (FileKind::Prelude, "default"),
            (FileKind::Scl, "default"),
        ] {
            assert_eq!(file_type(kind), expected, "{kind:?}");
        }
    }

    #[test]
    fn the_whole_document_range_ends_where_the_text_does() {
        let text = "a = 1\nb = \"\u{1f600}\"\n";
        let range = whole_document(text);
        assert_eq!(range.start, Position::new(0, 0));
        // Two lines, both terminated, so the end is the start of a third.
        assert_eq!(range.end, Position::new(2, 0));

        let unterminated = "cc_library(name = \"x\")";
        assert_eq!(
            whole_document(unterminated).end,
            Position::new(0, 22),
            "a file with no trailing newline ends on its last line"
        );
    }

    /// An absent formatter is the user's to fix, so it reaches them as an
    /// error rather than as a format that silently did nothing.
    #[test]
    fn a_missing_buildifier_is_an_error() {
        let err = format_with(
            "buildifier-that-is-not-installed",
            "cc_library(name=\"x\")\n",
            FileKind::Build,
        )
        .expect_err("a missing binary cannot format");
        assert!(
            format!("{err:#}").contains("buildifier-that-is-not-installed"),
            "the message names the binary: {err:#}"
        );
    }

    #[test]
    fn formatted_text_yields_no_edits() {
        if !buildifier_is_installed() {
            return;
        }
        let formatted = "cc_library(\n    name = \"x\",\n    srcs = [\"a.c\"],\n)\n";
        assert!(
            format(formatted, FileKind::Build)
                .expect("buildifier ran")
                .is_empty()
        );
    }

    #[test]
    fn unformatted_text_yields_one_whole_document_edit() {
        if !buildifier_is_installed() {
            return;
        }
        let text = "cc_library(name=\"x\",   srcs = [\"b.c\",\"a.c\"])\n";
        let edits = format(text, FileKind::Build).expect("buildifier ran");

        assert_eq!(edits.len(), 1, "one edit, covering everything");
        assert_eq!(edits[0].range, whole_document(text));
        assert_eq!(
            edits[0].new_text,
            "cc_library(\n    name = \"x\",\n    srcs = [\n        \"a.c\",\n        \"b.c\",\n    ],\n)\n"
        );
    }

    /// The mode reaches buildifier: the same text formats differently as a
    /// BUILD file and as a `.bzl` one, which is the whole reason `-type` is
    /// passed rather than left to guess from a name it never sees.
    #[test]
    fn the_mode_changes_the_output() {
        if !buildifier_is_installed() {
            return;
        }
        let text = "my_rule(name=\"x\",   deps = [\"//b\",\"//a\"])\n";
        let as_build = format(text, FileKind::Build).expect("buildifier ran");
        let as_bzl = format(text, FileKind::Bzl).expect("buildifier ran");

        assert_eq!(as_build.len(), 1);
        assert_eq!(as_bzl.len(), 1);
        assert!(as_build[0].new_text.contains("\n    name = \"x\","));
        assert_eq!(
            as_bzl[0].new_text,
            "my_rule(name = \"x\", deps = [\"//b\", \"//a\"])\n"
        );
    }

    /// A file buildifier cannot parse is the user's own broken buffer, already
    /// covered by diagnostics. Returning half a file would be worse than
    /// returning none of it.
    #[test]
    fn a_syntax_error_yields_no_edits() {
        if !buildifier_is_installed() {
            return;
        }
        let broken = "cc_library(name = \"x\"\n\ndef (:\n";
        assert!(
            format(broken, FileKind::Build)
                .expect("buildifier ran and refused it, which is not a broken tool")
                .is_empty()
        );
    }
}
