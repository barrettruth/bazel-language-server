# Hands-on experiments (parent agent)

Machine: darwin aarch64, Bazel 8.7.0 (nixpkgs) unless stated. Toolchain supplied by
`nix develop ~/dev/bazel-language-server`. Corpus: `experiments/torture/`, a bzlmod
workspace written to break a language server. Scale corpus: `/tmp/bigrepo`,
2000 packages / 6000 targets.

## 1. Measurement trap: `nix develop` dominates naive timings

Timing `bazel` through `nix develop -c` measures flake evaluation, not Bazel.

| invocation                              | wall   |
| --------------------------------------- | ------ |
| `nix develop -c bazel query '//...'`    | 13.5 s |
| same query inside an already-entered shell | **0.17 s** |

A 79x error. Any latency claim about Bazel-backed IDE features must be taken
inside a warm shell against a warm server, or it is measuring the harness.

## 2. Warm Bazel query is interactive-viable; cold is not

`/tmp/bigrepo`, 2000 packages, 6000 targets, private `--output_base`:

| query                                  | cold  | warm    |
| -------------------------------------- | ----- | ------- |
| `//...` (full target enumeration)      | 9.7 s | 0.278 s |
| `rdeps(//..., //pkg/p1/p50:a)`         | —     | 0.226 s |

Sub-300 ms for whole-repo enumeration and reverse-dependency lookup. The
server-warm path is fast enough for `workspace/symbol` and find-references.
The cold path (9.7 s here; 6 m 34 s for `deps(//...)` on bazelbuild/bazel per
agent 05) is not, and is unavoidable on first open.

## 3. The fetch cliff, demonstrated end-to-end against starpls

The decisive experiment. Same starpls v0.1.22 binary, same file, same cursor
position, same `bazel` on PATH. The only variable is whether `bazel fetch` has
populated **the output base starpls itself reads** (the default one — starpls
does not use a private output base).

`external/` before: `bazel_tools`, `local_config_platform`. After
`bazel fetch --repo=@@bazel_skylib+` (0.267 s): `bazel_skylib+` appears.

| probe on `load("@bazel_skylib//rules:write_file.bzl", "write_file")` | before   | after |
| -------------------------------------------------------------------- | -------- | ----- |
| goto-def on the `.bzl` path                                           | `[]`     | `external/bazel_skylib+/rules/write_file.bzl` |
| goto-def on the symbol `write_file`                                   | `[]`     | line 30 of that file |
| hover on `write_file`                                                 | `(variable) write_file: Unknown` | full rendered docstring |

The pre-fetch failure is **silent** — an empty result, no diagnostic, no log the
user sees. It is indistinguishable from "this feature is not implemented."
`@sh` (the `repo_name`-aliased `rules_shell`) still returns `[]` after the
skylib fetch, because it was never fetched.

## 4. starpls capability probe (v0.1.22, torture corpus)

Advertised: `completionProvider`, `declarationProvider`, `definitionProvider`,
`documentSymbolProvider`, `hoverProvider`, `referencesProvider`,
`signatureHelpProvider`, `textDocumentSync`. Absent: formatting, rename,
code actions, code lens, semantic tokens, **`workspace/symbol`** (returns
`-32601 method not found`), diagnostics beyond syntax.

| probe                                            | result |
| ------------------------------------------------ | ------ |
| `load(":local.bzl")` relative path                | resolves (target range collapses to 0:0) |
| `load()` into fetched external repo               | resolves |
| cross-package label `"//lib/sub:sub_srcs"`        | resolves to the *package*, range = whole `filegroup` |
| same-package label `:srcs` inside `$(location …)` | `[]` |
| `select()` key `"@platforms//os:macos"`           | `[]` |
| `repo_name`-aliased `@sh//…`                      | `[]` |
| find-references on target name `srcs`             | `[]` (single-file identifier scan only) |
| `documentSymbol` on BUILD                         | works, names targets `:srcs`, `:aliased`, … |
| macro-generated `//lib:from_legacy_0`             | n/a — the target is invisible to the server |

`declarationProvider` is advertised but `workspace/symbol` is not, so there is
no way to jump to a target you cannot already see.

## 5. Hover quality regresses when Bazel *is* available

Counter-intuitive. Hovering `genrule`:

- **without** `bazel` on PATH — served from the bundled `builtin.pb`: ordered
  params, types, defaults (`srcs: list[Label] = []`), and the full Build
  Encyclopedia prose.
- **with** `bazel` on PATH — served from `bazel info build-language`:
  alphabetised params, no defaults, `outs: Unknown`, and the docstring reduced
  to "See the Bazel Build Encyclopedia for more details."

Independently corroborates agent 06: the release binary strips doc strings from
`build-language`. Live extraction is *worse* than the stale pinned blob for
documentation, better for accuracy about which rules exist.

## 6. Errors the corpus surfaced that an LSP should have caught statically

Each of these cost a `bazel query` round trip to discover; none is reported by
any existing server.

| construct                                                    | Bazel's response |
| ------------------------------------------------------------ | ---------------- |
| same module `bazel_dep`'d twice under two `repo_name`s        | `depends on bazel_skylib@1.7.1 at least twice` |
| `sum()` in a `.bzl` file                                      | `name 'sum' is not defined` (Starlark has no `sum`) |
| symbolic-macro attr (configurable by default) passed to `tags` | `attribute "tags" is not configurable` |
| `compatibility_level` in `module()`                           | no-op, deprecated |

## 7. `--output=location` collapses macro-generated targets

`bazel query //lib:all --output=location` reports `from_legacy_0`, `_1`, `_2`
all at `lib/BUILD.bazel:85:13` — the single macro call site. Likewise
`from_symbolic` and `from_symbolic_group` share `87:15`. Rule *kinds* are the
private implementation names (`_local_rule`, `_write_file`, `_copy_file`), which
appear nowhere in the user's source. Query gives you existence and a call site;
it cannot give you a per-target definition range, and the kind it reports is not
a name the user can search for.
