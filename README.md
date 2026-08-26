# bazel-language-server

Language server for Bazel build files — `BUILD`, `*.bzl`, `MODULE.bazel`,
`WORKSPACE`, `*.scl`, `.bazelrc`.

> [!WARNING]
> Early scaffold. `documentSymbol`, `workspace/symbol`, goto-definition and
> syntax diagnostics work; the Bazel-backed half does not exist yet. See
> `ROADMAP.md`.

## Why

A build-graph client that speaks LSP, rather than a Starlark language server
with Bazel bolted on. Starlark-the-language is well covered by prior art;
Bazel-the-graph is not. No existing LSP server implements `workspace/symbol`,
cross-file find-references, target rename, or unresolved-label diagnostics —
and all four need the graph.

## Status

| feature | state |
| --- | --- |
| syntax diagnostics | working |
| `textDocument/documentSymbol` | working |
| `workspace/symbol` | working, static tier only |
| `textDocument/definition` | working, main repo only |
| `textDocument/references` | working, main repo only |
| `textDocument/rename` | working, main repo only |
| completion, hover | not yet |

The server runs with no Bazel installed, and stays useful when it does: the
static tier is parsed from BUILD files directly. Targets declared by legacy
macros are invisible until the graph tier lands, and the server says so rather
than quietly undercounting.

## Try it

```sh
direnv allow
just index experiments/torture     # index a workspace, no editor needed
just doctor experiments/torture    # is Bazel usable here
```

```
$ just index experiments/torture
workspace experiments/torture (via MODULE.bazel)
indexed 6 BUILD files, 19 targets in 0.00s
  //lib:srcs                               filegroup
  //lib:generated                          write_file
  ...
static tier only: targets from legacy macros are not counted
```

## TODO: file watching

Tabled, not forgotten. Nothing watches the filesystem yet — the index is built
once at `initialize` and never refreshed, so edits to a BUILD file you do not
have open are invisible until restart. Both plausible designs have a
disqualifying problem, which is why this is deferred rather than guessed at.

### Option A — the client watches (`workspace/didChangeWatchedFiles`)

The server registers watchers at runtime via `client/registerCapability`, and
the client pushes `workspace/didChangeWatchedFiles` with a `Vec<FileEvent>` of
`{ uri, type }` where `FileChangeType` is `Created = 1`, `Changed = 2`,
`Deleted = 3`.

```rust
DidChangeWatchedFilesRegistrationOptions {
    watchers: vec![FileSystemWatcher {
        glob_pattern: GlobPattern::Pattern("**/{BUILD,BUILD.bazel,*.bzl,MODULE.bazel}".into()),
        // WatchKind is a bitmask: Create = 1, Change = 2, Delete = 4.
        // Omitted means 7.
        kind: None,
    }],
}
```

Gates to honour before sending it:

- `workspace.didChangeWatchedFiles.dynamicRegistration` — without it there is no
  way to register at all; the spec has no static server-side declaration.
- `workspace.didChangeWatchedFiles.relativePatternSupport` (**@since 3.17**) —
  required before using `GlobPattern::RelativePattern { base_uri, pattern }`
  rather than a bare `Pattern`.

**The blocker:** `DidChangeWatchedFilesRegistrationOptions` has exactly one
field, `watchers`. There is **no exclude**. "Everything except `bazel-out`" is
inexpressible, and VS Code's default `files.watcherExclude` carries no Bazel
entries — so on a large repo the client watches the entire output base.

### Option B — the server watches (`notify`)

Measured on macOS, and the reason this needs care rather than a crate choice:

| approach | result |
| --- | --- |
| one `watch(dir, Recursive)` per directory, 4,096 dirs | 72 s setup, quadratic |
| same, at **4,100 dirs** | every call returns `Ok(())`, **zero events ever delivered** |
| **one recursive watch on the root** | 0.002 s setup, 15.8 ms to the deepest package |

FSEvents consumes a file descriptor per stream path, so the wall is
`RLIMIT_NOFILE` (256 by default) wearing a disguise. notify 8.2.0 fails
silently; 9.0.0-rc.4 reports it; `main` added an `RLIMIT_NOFILE/12` budget.
rust-analyzer watches per-directory (`vfs-notify/src/lib.rs:329-331`) and so
inherits this.

### Leaning

One recursive watch that we own, with our own exclusion of `bazel-*` and
`.bazelignore` — because Option A cannot express the exclusion, and the failure
mode of Option B is avoidable by not making the call rust-analyzer makes.

Open questions: debouncing (Bazel rewrites thousands of files during a build);
whether to ignore events entirely while a build is running; Linux inotify
`max_user_watches` at 74k directories, which is unmeasured — all watching
numbers above are macOS.

## Layout

`crates/bazel-language-server` is the binary; `crates/bls-index` the target
index; `crates/bls-bazel` the Bazel client. Syntax comes from
[`starlark-cst`](https://github.com/barrettruth/starlark-cst), kept separate
because a lossless Starlark CST is reusable and none of this is.

`research/` holds the measurements the design rests on.

## License

MIT
