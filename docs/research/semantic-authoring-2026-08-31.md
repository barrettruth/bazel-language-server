# Semantic authoring research, 2026-08-31

This note records the primary-source comparison and the resulting issue
topology. Popularity is deliberately excluded. Public user-facing promises
remain on the website; `ROADMAP.md` owns dependency order, and `AGENTS.md` owns
implementation invariants.

## Relevant implementations

| Tool | Strongest surface | Material boundary |
| --- | --- | --- |
| [bazel-language-server](../../README.md) | Static and evaluated targets, cross-file label operations, external repository mapping, and exact Bazel 8.7 Bazelrc semantics | No BUILD/Starlark completion, signature help, unresolved-label diagnostics, or cross-file source-symbol model |
| [starpls](https://github.com/withered-magic/starpls/blob/ac25eca3dbbed6347fbca5fbf14d3a027d46bcab/README.md) | Starlark type inference, semantic diagnostics, signatures, completion, hover, definitions, and Bazel-aware loads | Label completion is experimental; references search only the current file; no rename, formatting, semantic Bazelrc, or published Bazel compatibility contract |
| [bazel-lsp](https://github.com/cameron-martin/bazel-lsp/blob/48fead628b45af5880a2df8aa8184b0c6ab0f0b9/README.md) | Identifier and label completion/definition, hover, diagnostics, and open-file auto-import | No references, rename, symbols, formatting, semantic tokens, signature help, or Bazelrc |
| [Salesforce bazelrc-lsp](https://github.com/salesforce-misc/bazelrc-lsp/blob/2a971b8532ab3cec0a8850fff277476062df9d5c/README.md) | Bazelrc formatting and broad flag-version catalogs | No transitive configuration graph, configuration references/rename, or exact packaged Bazel 8.7 flag catalog; runtime Bazel mode is required for exact 8.7 metadata |
| [JetBrains Bazel](https://ij.bazel.build/docs/build-file-support.html) | Integrated completion, navigation, usages, refactoring, quick fixes, run, and debug | IDE- and sync-bound; no documented semantic Bazelrc support |

The [VS Code Bazel extension](https://github.com/bazel-contrib/vscode-bazel#using-a-language-server-experimental)
is complementary workflow UI rather than an independent semantic server. It
provides target browsing, tasks, coverage, buildifier, and Starlark debugging,
then delegates language-server features.

Source inspection confirms that starpls registers completion, definition,
declaration, symbols, hover, references, and signature help, but not rename or
formatting. Its reference implementation searches the current file:

- [server capabilities](https://github.com/withered-magic/starpls/blob/ac25eca3dbbed6347fbca5fbf14d3a027d46bcab/crates/starpls/src/commands/server.rs#L58-L78)
- [reference search](https://github.com/withered-magic/starpls/blob/ac25eca3dbbed6347fbca5fbf14d3a027d46bcab/crates/starpls_ide/src/find_references.rs#L98-L150)

Starpls also documents false-positive type warnings caused by incomplete
builtins and unsupported type guards. This is evidence against treating a
general Starlark type checker as the route to competitive authoring support,
not evidence against exact symbol resolution or schema-backed assistance.

## Semantic certainty contract

The useful subset does not require whole-program type inference:

1. Bazel `rule_class_info` supplies authoritative rule and attribute schemas.
2. Syntax supplies declarations, parameters, lexical scopes, and `load()`
   bindings.
3. The target index supplies completion and navigation candidates.
4. Grammar-defined positions and exact attribute schemas decide whether a
   string is semantically a label.
5. Unknown provenance or role yields no diagnostic and stops dependent
   conclusions.

Completion may be permissive because accepting a suggestion is an explicit
user choice. Diagnostics and multi-file edits require proof because a false
claim or rewrite damages trust or the workspace.

## Tracker topology

The technical roadmap remains [#8](https://github.com/barrettruth/bazel-language-server/issues/8).
The semantic-authoring epic remains
[#26](https://github.com/barrettruth/bazel-language-server/issues/26), narrowed
to compose exact schemas and source symbols without merging their provenance.

| Tracker action | Parent | Blocked by | Result |
| --- | --- | --- | --- |
| Add **Distinguish label candidates from semantic references** | #8 | Nothing | Context-bearing candidates; rename, references, and diagnostics consume only proven roles |
| Narrow [#27](https://github.com/barrettruth/bazel-language-server/issues/27) to **Publish immutable rule schemas** | #26 | Nothing beyond the Bazel actor | Exact rule/attribute metadata only; no loaded symbols or inferred types |
| Narrow [#4](https://github.com/barrettruth/bazel-language-server/issues/4) to **Complete explicit Bazel labels** | #26 | Candidate/context model | Prefix and package-aware labels from the existing index |
| Add **Complete rule names and attributes from exact schemas** | #26 | #27 and source load bindings | Rule/attribute completion without bundled or inferred metadata |
| Keep [#5](https://github.com/barrettruth/bazel-language-server/issues/5) | #26 | #27 for rules; exact source symbols for source callables | Provenance-preserving documentation hover |
| Keep [#28](https://github.com/barrettruth/bazel-language-server/issues/28) | #26 | #27 for rules; exact source symbols for functions | Schema- and syntax-backed signature help |
| Add **Publish exact Starlark source symbols** | #26 | Nothing | Declarations, scopes, parameters, and `load()` bindings; no expression types |
| Add **Navigate and refactor loaded Starlark symbols** | #26 | Exact source symbols | Cross-file definition, references, and rename with snapshot-safe edits |
| Extend [#29](https://github.com/barrettruth/bazel-language-server/issues/29) | #3 | Candidate/context model and graph-completeness evidence | Resolution states only after label-role proof |
| Keep [#30](https://github.com/barrettruth/bazel-language-server/issues/30) | #3 | #29 and proven label roles | Diagnostics only for proven invalid labels |
| Keep [#31](https://github.com/barrettruth/bazel-language-server/issues/31) and [#2](https://github.com/barrettruth/bazel-language-server/issues/2) | #3 | Proven diagnostics | Lazy, structured repair actions |

No type-inference tracker is created. A later issue must name a concrete user
outcome that exact schemas and symbols cannot provide, define the smallest
additional fact required, and preserve `Unknown` rather than inventing a
confidence score or emitting speculative diagnostics.

Tracker housekeeping follows the same boundary:

- [#8](https://github.com/barrettruth/bazel-language-server/issues/8) is the
  sole technical umbrella and should use the dependency order in `ROADMAP.md`.
- [#10](https://github.com/barrettruth/bazel-language-server/issues/10) should
  describe IMC rollout and feedback only; its v0.1 and feature-parity steps no
  longer describe technical work.
- [#9](https://github.com/barrettruth/bazel-language-server/issues/9) and
  [#15](https://github.com/barrettruth/bazel-language-server/issues/15) remain
  distribution work after the semantic surface stabilises.
- [#14](https://github.com/barrettruth/bazel-language-server/issues/14),
  [#32](https://github.com/barrettruth/bazel-language-server/issues/32), and
  [#33](https://github.com/barrettruth/bazel-language-server/issues/33) remain
  deferred; none blocks semantic authoring.
- Bazelrc formatting stays the intentional non-feature concluded by
  [#24](https://github.com/barrettruth/bazel-language-server/issues/24) and
  [#25](https://github.com/barrettruth/bazel-language-server/issues/25).

## Draft issue bodies

### Distinguish label candidates from semantic references

Parent: #8

The BUILD syntax walk currently normalises every label-shaped string under a
top-level call. Preserve each candidate's enclosing call, argument, nesting,
spelling, and range instead. A matching target or label-like spelling does not
prove that a string-valued attribute is a label.

A semantic target reference requires either a grammar-defined label position
or an exact label-valued rule attribute from the request's rule schemas.
Navigation and completion may answer a narrower best-effort question from an
explicit candidate. References, highlights, rename, and diagnostics use only
proven semantic references from the captured snapshot.

Do not add a confidence score, parallel reference table, or handler-time Bazel
lookup. The semantic role is derived from candidate context and the exact
schema already captured by the request.

### Publish exact Starlark source symbols

Parent: #26

Publish immutable source-backed declarations, lexical scopes, function
parameters, and `load()` bindings for BUILD and `.bzl` files. Open buffers
replace saved contributions from the same path, and every request uses the
source-symbol snapshot captured with its documents and target index.

This issue does not infer expression types, execute Starlark, emulate Bazel, or
merge symbols into the Bazel-derived rule schemas. Missing modules, dynamic
exports, and unsupported constructs remain unknown.

### Complete rule names and attributes from exact schemas

Parent: #26

Blocked by #27 and exact source load bindings.

Complete callable rule names visible in the current source context and the
attributes reported for the active rule class. Include requiredness, types,
defaults, and documentation only where the captured rule schemas report them.
Resolve expensive documentation after selection.

Missing or stale evaluated metadata produces a narrower completion list. Do
not use bundled rule tables, infer arbitrary expression types, or offer finite
attribute values that Bazel did not report.

### Navigate and refactor loaded Starlark symbols

Parent: #26

Blocked by exact Starlark source symbols.

Resolve identifiers through lexical scope and `load()` aliases to exact source
declarations. Add cross-file definition and references first; rename follows
only when every edited occurrence is derived from the same source-symbol and
document snapshots.

Unknown or dynamic bindings produce no result. This issue does not require
provider-field inference, control-flow analysis, or a Starlark type checker.
