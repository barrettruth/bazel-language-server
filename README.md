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

It handles `BUILD`, `BUILD.bazel`, `*.bzl`, `MODULE.bazel`, `REPO.bazel`,
`VENDOR.bazel`, `WORKSPACE*`, `*.scl`, Bazel query files, and `*.bazelrc`
(including `.bazelrc`). Bazel itself is optional: the target index is parsed
from BUILD files, so the server answers without a Bazel process or build.
Workspace indexing runs after startup; open buffers remain usable while the
initial snapshot is being built, and later BUILD-file changes replace only that
file's entries.

Where Bazel is present it adds two things a parser cannot reach. Targets named
by legacy macros at evaluation time become navigable at the producing macro
call, their only source location. And `@repo//…` resolves through the repository
mapping Bazel alone can produce; an unfetched repository explains which
`bazel fetch` brings it down.

### Bazelrc

Bazelrc parsing follows upstream Bazel 8.7.0 and the advertised compatibility
line is Bazel 8.7. Vendor suffixes on a numeric 8.7 release do not imply vendor
grammar support. Other Bazel versions still receive structural answers, but
the server does not select a nearby flag catalog or silently fall back to one.

<details>
<summary>Bazelrc feature coverage</summary>

**Without Bazel**

- Command and configuration completion
- Import links and go-to-definition for imports and configurations
- Semantic tokens, folding, selection ranges, and diagnostics
- A workspace graph rooted at `.bazelrc`, with open-buffer configuration
  overlays

**With an exact Bazel 8.7 catalog**

- Command-scoped canonical, negative, and abbreviated flag completion
- Native flag hover with documentation and catalog metadata
- Conservative scope, value, old-name, deprecation, and status diagnostics

**Not supported**

- Formatting, rename, references, and configuration symbols
- Import-path and flag-value completion
- Converter-specific value and configuration expansion-cycle diagnostics
- Reconstruction of system, home, and explicit rc layers

</details>

The [normative Bazelrc 8.7 specification](docs/bazelrc-8.7.md) records the
source-backed language contract. The website keeps the corresponding
[dated support matrix](https://bazel-language-server.com/bazelrc.html#support-matrix)
as the exhaustive public inventory.

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
    "args": [],
  },
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

## Starlark and BUILD features

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
- [x] **Watched-file refresh** — replace changed BUILD files and rebuild on
      package-topology changes
- [ ] **File-rename edits** — rewrite labels when a file or package moves
- [x] **Execute command** — run a `bazel` invocation the code lens offers
- [x] **Incremental sync** — ordered UTF-16 `didChange` ranges and versioned
      diagnostics
