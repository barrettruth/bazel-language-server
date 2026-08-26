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
so the server answers without a Bazel process and without a build. Targets
declared by legacy macros are named at evaluation time and are reported as
missing from the index rather than silently omitted.

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

Working today, with no Bazel process:

- [x] **Diagnostics** — syntax errors, reported per keystroke against a
      recovering parser
- [x] **Document symbols** — every target declared in a BUILD file
- [x] **Workspace symbols** — every target in the workspace
- [x] **Go-to-definition** — labels and `load()` paths
- [x] **References** — every label referring to a target, cross-file
- [x] **Document highlight** — occurrences of the target under the cursor
- [x] **Rename** — rename a target and rewrite every referring label
- [x] **Hover** — the resolved target behind the label under the cursor
- [x] **Formatting** — delegated to `buildifier` where it is installed

Planned, roughly in the order they earn their place:

- [ ] **Completion** — labels from the index, rule names and their attributes
- [ ] **Unresolved-label diagnostics** — labels that name no target
- [ ] **Lint diagnostics** — `buildifier --lint=warn`, alongside the parse errors
- [ ] **Document links** — labels and `load()` paths as links, for clients that
      prefer them to go-to-definition
- [ ] **Code actions** — apply a `buildifier` fix, add a missing dependency,
      fetch an unfetched repository
- [ ] **Code lens** — build, test or run the target a line declares
- [ ] **Signature help** — the attributes of the rule being called, while typing
- [ ] **Semantic tokens** — tell labels, rule names and providers apart, which
      no TextMate grammar can do
- [ ] **Go-to-implementation** — from a rule to the function implementing it
- [ ] **Inlay hints** — the package a relative label resolves against
- [ ] **Folding ranges** — rule calls, lists and comment runs
- [ ] **Selection ranges** — expand the selection along the syntax tree
- [ ] **Workspace diagnostics** — the pull model, so unresolved labels can be
      reported repository-wide rather than per open file
- [ ] **Watched-file refresh** — reindex when BUILD files change on disk
- [ ] **File-rename edits** — rewrite labels when a file or package moves
- [ ] **Execute command** — run a `bazel` invocation the code lens offers
- [ ] **Incremental sync** — `didChange` ranges rather than whole documents
- [ ] **`.bazelrc`** — completion, hover and unknown-flag diagnostics from
      `bazel help flags-as-proto`, version-exact for the user's own binary

Deliberately absent. Type hierarchy, document colour, linked editing ranges,
on-type formatting and monikers describe languages this is not. Call hierarchy
is the near miss: mapping incoming calls to reverse dependencies reads well
until a client labels the panel "Calls" and a macro's callers and a target's
dependents appear in the same tree — find-references already answers the
question honestly. Starlark type inference stays out for the reason
`.devin/ROADMAP.md` gives: it is a third of starpls' codebase and answers
`Unknown` where it matters.
