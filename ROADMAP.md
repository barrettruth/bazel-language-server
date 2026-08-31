# Roadmap

Work is ordered by the shared capability it unlocks. User-facing promises live
on the site; this document records dependency order and architectural bounds.

## 1. Semantic authoring

Completion and signature help share one immutable catalog of rules,
attributes, documentation, and loaded symbols. The Bazel actor publishes the
Bazel-derived portion from `rule_class_info`; source parsing supplies symbols
that exist without evaluation.

Requests capture this catalog with `Documents` and `Index`. Completion returns
cheap labels and edits immediately and resolves expensive documentation only
when the client selects an item. Signature help reads the same catalog rather
than maintaining another model.

This is a new index tier, not a new protocol loop or mutable global symbol
graph. Add a prefix index only if catalog scans measure poorly.

## 2. Correctness and repair

Unresolved-label diagnostics require more than absence from `Index::target`.
Resolution must distinguish a missing target from an unavailable graph, an
unfetched repository, an unfinished index, and a target pattern that names a
set. Diagnostics follow only from states that prove the label is invalid.

Document and workspace pull reports derive their result IDs from the document
and index snapshots that produced them. A newer snapshot invalidates the
result; no parallel dirty flag or diagnostic cache tracks the same fact.

Code actions follow diagnostics. Start with buildifier fixes and repository
fetches, carrying enough diagnostic data to resolve edits or commands lazily.
Adding dependencies waits until the server can identify the owning rule and a
safe attribute edit without guessing.

## 3. Refactoring

File and package moves use a refactoring planner over one document/index
snapshot. It computes label edits for `workspace/willRenameFiles`, checks open
document versions and indexed disk ranges, and returns one annotated workspace
edit. The server does not move files itself.

The planner is a boundary around existing reference data, not a second index.

## 4. Bazel configuration

`.bazelrc` is a separate language surface. Its parser and handlers cover
startup options, commands, configurations, imports, and continuations without
adding conditional paths to the Starlark handlers.

The parser implements upstream Bazel 8.7.0. The advertised compatibility line
is Bazel 8.7; a vendor suffix on that numeric release does not add vendor
semantics. Other releases may still receive structural answers, but absence
from an 8.7 catalog never proves one of their flags invalid.

The watch thread publishes the workspace `.bazelrc` import graph. Requests use
that immutable snapshot and the current buffer without reading the filesystem.
The Bazel actor supplies a flag catalog from the configured binary's
`help flags-as-proto` output only when the reported numeric release is 8.7.
Catalog identity derives from the binary, reported Bazel version, and startup
arguments so configuration changes cannot reuse stale flag semantics. There is
no nearest-version or bundled fallback catalog.

Syntax, import/config navigation, structural language features and exact-catalog
flag intelligence are shipped. Converter-specific values remain outside the
catalog contract. Configuration-cycle diagnostics require the configuration
snapshot to retain owner-to-reference expansion edges; they should not be
approximated from the current declaration/reference lists.

## Dependency order

1. Catalog, then label completion, rule and attribute completion, and signature
   help.
2. Resolution states, then document diagnostics, workspace diagnostics, and
   diagnostic code actions.
3. Refactoring planner, then file and package move edits.
4. Bazelrc expansion edges, then configuration-cycle and repeated-expansion
   diagnostics.
