# bazel-language-server

Language server for Bazel build files — `BUILD`, `*.bzl`, `MODULE.bazel`,
`WORKSPACE`, `*.scl`, `.bazelrc`.

> [!WARNING]
> Early scaffold. `documentSymbol`, `workspace/symbol` and syntax diagnostics
> work; the Bazel-backed half does not exist yet. See `ROADMAP.md`.

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
| goto-definition | not yet |
| completion, hover | not yet |
| find-references, rename | not yet |

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

## Layout

`crates/bazel-language-server` is the binary; `crates/bls-index` the target
index; `crates/bls-bazel` the Bazel client. Syntax comes from
[`starlark-cst`](https://github.com/barrettruth/starlark-cst), kept separate
because a lossless Starlark CST is reusable and none of this is.

`research/` holds the measurements the design rests on.

## License

MIT
