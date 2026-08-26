# AGENTS.md

A language server for Bazel build files. Read `ROADMAP.md` first — the scope,
invariants and request priority live there and are not repeated here.

## Layout

| path | holds |
| --- | --- |
| `crates/bazel-language-server` | the binary: LSP loop, handlers, CLI |
| `crates/bls-index` | the two-tier target index, published via `ArcSwap` |
| `crates/bls-bazel` | workspace discovery and Bazel invocation |
| `research/` | the findings the design rests on; `stack/` is the crate choices |
| `experiments/` | the torture workspace, `lspdrive.py`, extracted `builtin.pb` |
| `upstream/` | shallow clones of prior art. Gitignored, ~1 GB, re-fetchable |

Syntax lives in a separate repo, `starlark-cst`, as a path dependency until it
is published. It is deliberately outside this tree: a lossless Starlark CST is
reusable by Buck2, Pants and Tilt, and nothing above it is.

## The invariants, restated because they are easy to violate

1. **No Bazel call in a request handler.** `bls-bazel` is not reachable from
   `handlers.rs`, and that is enforced by the module graph rather than by
   discipline. Slow work goes on the Bazel thread and publishes an index.
2. **The server works with no Bazel.** `just doctor` reports it honestly and
   `just index` still finds targets. Never make Bazel a startup requirement.
3. **stdout is the protocol.** `tracing` is wired to stderr. A `println!` in the
   server path corrupts the LSP framing and the failure looks like a client bug.
   The `index` and `doctor` subcommands may use stdout: they do not speak LSP.
4. **Degrade loudly.** An empty result must be distinguishable from an
   unimplemented one. Where the static tier undercounts — legacy macros — say so.

## Working here

```sh
direnv allow        # once; the flake supplies rust, bazel, buildifier, just
just ci             # format, lint, test
just index PATH     # index a workspace without an editor
just doctor PATH    # is the Bazel subsystem usable here
```

Tooling comes from the flake; use `direnv exec . <cmd>` when invoking directly.
There is no `packages.default` yet because `starlark-cst` is a path dependency
pointing outside the tree, which a nix build cannot see.

To exercise the real protocol rather than the functions:

```sh
cd experiments/torture
python3 ../lspdrive.py ../../target/release/bazel-language-server server \
  --root . --script /tmp/probe.json
```

## Conventions

Rust 2024, `unsafe_code = "forbid"`, `clippy::pedantic` denied in CI. Tests are
inline `#[cfg(test)]` modules; snapshots use `expect-test`.

`gen-lsp-types` is aliased as `lsp_types` but is **not** `lsp-types` 0.97. It is
generated from the LSP metaModel, so spec inheritance appears as flattened
nested structs: `WorkspaceSymbol.base_symbol_information.name`,
`InitializeParams.workspace_folders_initialize_params.workspace_folders`. Expect
to read `src/generated/structures.rs` in the registry rather than guess.

## Measurements to design against

Full data in `research/stack/05-measurements-and-decisions.md`. The ratio that
matters: our entire static tier costs ~1.4 s and ~13 MB on a 74k-package repo,
while one cold `bazel query` costs 16.76 s and ~1 GB on a third that size.
**Bazel is the only expensive thing here.** Optimise it, cache it, background it;
leave everything else alone.
