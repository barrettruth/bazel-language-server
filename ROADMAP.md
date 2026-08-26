# Roadmap

## What this is

A language server for **Bazel build files** — `BUILD`, `BUILD.bazel`, `*.bzl`,
`MODULE.bazel`, `WORKSPACE`, `*.scl`, `.bazelrc`.

The thesis: **a build-graph client that speaks LSP**, not a Starlark language
server with Bazel awareness bolted on. Starlark-the-language is largely solved
by prior art; Bazel-the-graph is untouched. Every feature nobody else ships —
`workspace/symbol`, cross-file find-references, rename, unresolved-label
diagnostics — needs the graph, and that is the whole opportunity.

## Out of scope

- Making *other* languages' servers work under Bazel: no BSP, no
  `compile_commands.json`, no `rules_*` IDE support. If a feature needs a
  compiler, it is out.
- Starlark type inference. It is a third of starpls' codebase and produces
  `Unknown` at most of the places anyone cares about. Revisit only if asked.
- Being a build tool. We never mutate the user's build state beyond an explicit,
  user-initiated action.

## Invariants

These are load-bearing. A change that violates one is wrong even if it passes.

1. **No Bazel call is ever made in the request path.** Bazel runs on a dedicated
   thread and publishes an index; handlers read a snapshot or degrade. This is
   the one property that cannot be retrofitted.
2. **The server starts and is useful without Bazel.** Absent, broken, or
   disabled via `bazel.enable`, the static tier still answers. See
   `research/stack/05-measurements-and-decisions.md` §2 for the feature matrix.
3. **Degrade loudly.** An unresolved label because a repo is unfetched is a
   diagnostic with a fix, never a silent empty result. starpls returns `[]` here
   and users cannot tell it from "unimplemented".
4. **Wrong is worse than absent.** A goto-def that lands on the wrong target
   destroys trust permanently; one that declines is merely disappointing.
5. **Never eagerly index `//...`.** Cold `deps(//...)` on a real repo is minutes.
   Lazy, per-package, demand-driven.
6. **Two tiers.** A light `(name, kind, file, offset)` table for the whole repo;
   full CSTs only for open files plus a small LRU. 189k targets is ~13 MB as a
   table and 1.6–3.3 GB as trees.
7. **stdout belongs to the protocol.** All human-readable output goes to stderr.

## Goalposts

| milestone | means |
| --- | --- |
| **G1 usable** | open a BUILD file, get symbols, diagnostics and formatting, with no Bazel installed |
| **G2 navigable** | goto-def on labels and `load()` within the main repo, still no Bazel process |
| **G3 connected** | Bazel subsystem online: external repos resolve, repo mapping honoured, failures explained |
| **G4 the gap** | `workspace/symbol`, find-references, unresolved-label diagnostics — the things nothing else ships |
| **G5 fluent** | completion and hover carrying real rule/attribute documentation |
| **G6 editing** | rename a target and rewrite every referring label |
| **G7 proven** | measured on a genuine 100k-target repo, with a memory ceiling and an index-eviction story |

## Request priority

Ranked by value per unit of risk, not by protocol order.

### Tier 1 — ship first, no Bazel needed

| request | notes |
| --- | --- |
| `initialize` / `initialized` / `shutdown` / `exit` | capability negotiation, root detection |
| `textDocument/didOpen` / `didChange` / `didClose` | full sync to start; incremental later |
| `textDocument/publishDiagnostics` | parse errors from `starlark-cst`, then buildifier lints |
| `textDocument/documentSymbol` | targets in a BUILD file; already working in the prototype |
| `textDocument/formatting` | delegate to buildifier; must match it byte for byte |

### Tier 2 — navigation, static resolution

| request | notes |
| --- | --- |
| `textDocument/definition` | `load()` paths, then `//pkg:target` in the main repo |
| `textDocument/documentLink` | labels as links; better UX than definition in some clients, worse in Neovim |
| `textDocument/hover` | resolved label → path, before any rule docs exist |

### Tier 3 — the actual gap, needs the graph

| request | notes |
| --- | --- |
| `workspace/symbol` | every target in the repo. **No LSP server implements this today.** |
| `textDocument/references` | `rdeps`, measured at 0.64 s warm on 60k targets |
| `textDocument/publishDiagnostics` (labels) | unresolved / invisible / wrong-kind labels |
| `textDocument/completion` | labels from the index; attributes from `--proto:rule_classes` |

### Tier 4 — editing and polish

| request | notes |
| --- | --- |
| `textDocument/rename` | target rename with cross-file label rewriting; nothing ships this |
| `textDocument/codeAction` | add dep, fix visibility, `bazel mod tidy`, fetch missing repo |
| `textDocument/codeLens` | build / test this target |
| `textDocument/semanticTokens` | distinguish labels, rule names, providers |

### Separate track — `.bazelrc`

Cheap and uncontested: `bazel help flags-as-proto` yields 1,052 flags with docs,
defaults, enum values and per-command applicability in one 132 ms call. That is
completion, hover, value completion and unknown-flag diagnostics, version-exact
for the user's own binary. No editor currently routes `.bazelrc` to any server.

## Known-hard problems, named up front

- **The fetch cliff.** External repos do not exist on disk until fetched. Cannot
  be resolved statically, must be reported, and a code action should offer to
  fetch.
- **Legacy macros.** `//lib:from_legacy_0` exists but no parser can see it —
  target names are computed at evaluation time. The index must be graph-derived,
  not syntax-derived, and the static tier will always undercount.
- **Starlarkification.** Only 35 of 63 BUILD globals in Bazel 9's own
  `builtin.pb` are correct and 13 are stubs that `fail()`. Rule knowledge is
  per-workspace and must come from `--proto:rule_classes`, never a pinned table.
- **Apparent vs canonical repo names.** `@rules_go` → `rules_go+`, and the format
  changed in Bazel 8. `bazel mod dump_repo_mapping` is the only correct source
  and is documented as being for exactly this use.
- **`bazel-*` convenience symlinks** point into the output base; following them
  re-enters the source tree through the execroot symlink forest.

## Risk

**Tier 3 is the project.** Everything before it is scaffolding and everything
after is polish. The likeliest failure is not performance — it is the Bazel
subsystem (G3) eating the schedule on external repos, repo mapping and version
drift. Timebox it; unresolved external labels may stay unresolved-but-diagnosed,
because main-repo navigation is most of the daily value.
