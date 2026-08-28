# bazel-language-server

Language server for Bazel build files.

## Installation

### Cargo

```sh
cargo install bazel-language-server
```

### Nix

```sh
nix run github:barrettruth/bazel-language-server
```

### From source

```sh
git clone https://github.com/barrettruth/bazel-language-server.git
cd bazel-language-server
cargo install --path .
```

## Usage

Configure `bazel-language-server` in your editor of choice, for example with
[Neovim](https://neovim.io) via
[nvim-lspconfig](https://github.com/neovim/nvim-lspconfig):

```lua
vim.lsp.enable('bazel_ls')
```

It handles `BUILD`, `BUILD.bazel`, `*.bzl`, `MODULE.bazel`, `WORKSPACE` and
`*.scl`. Bazel itself is optional: the target index is parsed from BUILD files,
so the server answers without a Bazel process and without a build.

Where Bazel is present it adds two things a parser cannot reach. Targets a
legacy macro names at evaluation time become navigable, reported at the macro
call that produces them, which is the only place in the source they come from.
And `@repo//…` resolves, through the repository mapping Bazel alone can
produce — a repository that has not been fetched says so, and says which
`bazel fetch` brings it down.

## Configuration

Everything is optional and the defaults are what most workspaces want. Send it
as `initializationOptions`, or as `settings` — the second arrives as
`workspace/didChangeConfiguration` and is applied whenever it changes, so
switching `bazel.path` restarts the Bazel subsystem without restarting the
server.

```jsonc
{
  "bazel": {
    // Turn the whole Bazel subsystem off. The static tier still answers.
    "enable": true,
    // Binary to invoke. `bazelisk` works too.
    "path": "bazel",
    // Give the server its own --output_base so its queries never queue behind
    // your build. Costs a second Bazel server: measured +1.2 GB at 20k
    // packages, which is why it is off.
    "privateOutputBase": false,
    // Extra flags, passed to every invocation.
    "args": []
  }
}
```

### CLI

The server also provides standalone subcommands that work without an editor.

**Index** a workspace and print the targets it finds:

```sh
bazel-language-server index path/to/workspace
```

**Doctor** reports whether the Bazel subsystem is usable:

```sh
bazel-language-server doctor path/to/workspace
```

## Features

- [x] **Diagnostics** — syntax errors, reported per keystroke against a
      recovering parser
- [x] **Document symbols** — every target declared in a BUILD file
- [x] **Workspace symbols** — every target in the workspace
- [x] **Go-to-definition** — labels and `load()` paths, in this repository and
      in the external ones it depends on
- [x] **References** — every label referring to a target, cross-file
- [x] **Document highlight** — occurrences of the target under the cursor
- [x] **Rename** — rename a target and rewrite every referring label
- [x] **Hover** — the resolved target behind the label under the cursor, and
      why one that resolves to nothing does not
- [x] **Formatting** — delegated to `buildifier` where it is installed
- [ ] **Completion** — labels from the index, rule names and their attributes
- [ ] **Unresolved-label diagnostics** — labels that name no target
- [x] **Lint diagnostics** — `buildifier --lint=warn`, alongside the parse errors
- [x] **Document links** — labels and `load()` paths as links, for clients that
      prefer them to go-to-definition
- [ ] **Code actions** — apply a `buildifier` fix, add a missing dependency,
      fetch an unfetched repository
- [x] **Code lens** — build, test or run the target a line declares
- [ ] **Signature help** — the attributes of the rule being called, while typing
- [x] **Semantic tokens** — tell labels, rule names and providers apart, which
      no TextMate grammar can do
- [x] **Go-to-implementation** — from a rule to the function implementing it
- [x] **Inlay hints** — the package a relative label resolves against
- [x] **Folding ranges** — rule calls, lists and comment runs
- [x] **Selection ranges** — expand the selection along the syntax tree
- [ ] **Workspace diagnostics** — the pull model, so unresolved labels can be
      reported repository-wide rather than per open file
- [x] **Watched-file refresh** — reindex when BUILD files change on disk
- [ ] **File-rename edits** — rewrite labels when a file or package moves
- [x] **Execute command** — run a `bazel` invocation the code lens offers
- [ ] **Incremental sync** — `didChange` ranges rather than whole documents
- [ ] **`.bazelrc`** — full LSP semantics, including completion, hover and
      unknown-flag diagnostics from `bazel help flags-as-proto`, version-exact
      for the user's own binary
