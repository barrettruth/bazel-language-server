# bazel-language-server

Language server for Bazel build files.

## Installation

### From source

[`starlark-cst`](https://github.com/barrettruth/starlark-cst) is a path
dependency until it is published, so both repositories are checked out side by
side:

```sh
git clone https://github.com/barrettruth/starlark-cst.git
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
- [ ] **Completion** — labels, rule names, attributes
- [ ] **Unresolved-label diagnostics** — labels that name no target
