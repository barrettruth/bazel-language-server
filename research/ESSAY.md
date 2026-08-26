# The present state of Bazel language tooling

Bazel has no maintained language server. It has one talented person's side
project, and an IDE plugin that isn't an LSP.

**starpls** is the de facto standard: bundled by Helix, packaged in nixpkgs and
Mason, the default in nvim-lspconfig. It is also 383 of 390 commits by a single
personal account, last released 2025-08-30, last commit 2025-12-03, with 20 open
PRs — including ones from Bazel core maintainers — unreviewed since July. Modular
and others already run forks. **hirschgarten**, JetBrains' Bazel plugin, is the
only actually-complete implementation — target rename with label rewriting,
BCR-backed `bazel_dep` completion, fourteen version-keyed builtin snapshots — and
none of it is reusable, because it's IntelliJ PSI in Kotlin. Everything else is
dead: tilt-dev/starlark-lsp (2024-07), bazel-stack-vscode (2023-08),
cameron-martin/bazel-lsp (renovate commits over a 2025-02 corpse).

The demand is not ambiguous. `vscode-bazel#1`, "Implement language server for
Starlark," has been open since **2018** with 71 reactions; the extension has
975k installs and still ships `"bazel.lsp.command": ""`. Roughly 95% of VS Code
Bazel users edit BUILD files with no semantic support at all.

## The gaps

Not polish — whole features. No LSP server implements `workspace/symbol`, so you
cannot jump to a target you can't already see. None implements rename. None
implements cross-file find-references: starpls' scans the *current file* for
identifiers. None reports an unresolved label. None formats (that PR is 12 months
old). The gap is uniform and it is the same gap: **nobody has connected the editor
to the build graph.** Starlark-as-a-language is largely solved; Bazel-as-a-graph
is untouched.

I measured what that costs. Same starpls binary, same cursor, same `bazel` on
PATH: goto-definition on `load("@bazel_skylib//rules:write_file.bzl", …)` returns
`[]` before `bazel fetch`, and resolves correctly after. Hover degrades from a
full docstring to `Unknown`. The failure is silent — no diagnostic, no log —
and indistinguishable from an unimplemented feature. Cross-repository navigation
is a hidden function of build state.

## Room, and why it's hard

Yes, and the ground just shifted. Bazel 8 added `--proto:rule_classes`, which
makes `bazel query --output=streamed_proto` return full stardoc `RuleInfo` —
attribute types, defaults, enum values, docstrings, defining `.bzl` — in **140 ms
warm over 4,095 targets**. On 6,000 targets I measured `//...` at 278 ms and
`rdeps` at 226 ms. `bazel mod dump_repo_mapping` is documented as *"intended for
use by tools such as IDEs and Starlark language servers."* The graph is now
reachable at interactive latency. Nobody has claimed it.

The hard parts are real. Bazel holds one lock per output base — a query behind a
running build blocked 19.1 s — and both existing servers share the user's output
base, so they freeze exactly when the user is busy; a private output base costs
400 MB. Legacy macros make target sets underivable without evaluation: my corpus
declares `//lib:from_legacy_0..2`, which no parser can see. `builtin.pb` never
carried rule attribute types; starpls' copy came from a PR Google closed as *not
planned* and is frozen at 2024-12-27. Starlarkification broke the static
approach outright — only 35 of 63 BUILD globals in Bazel 9's own `builtin.pb` are
correct, and 13 are stubs that `fail()`. And `--experimental_starlark_type_syntax`
now defaults **true**, so `def f(x: int)` parses in released Bazel while
buildifier 8.5.1 rejects it.

Everything is version- and workspace-dependent. That is the thesis: not a
language server with Bazel awareness bolted on, but a build-graph client that
happens to speak LSP.
