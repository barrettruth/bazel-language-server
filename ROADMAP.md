# Roadmap

Work is ordered by the shared capability it unlocks. User-facing promises live
on the site; this document records dependency order and architectural bounds.

## 1. Semantic references

The syntax walk discovers label candidates. It preserves the enclosing call,
argument, nesting, spelling, and source range instead of recording every
label-shaped string as a reference. A grammar-defined label position or an
exact rule-attribute schema may establish a semantic reference; a matching
target in the index may not.

Navigation and completion may use an explicit candidate when the user has
already asked a label-shaped question. References, rename, diagnostics, and
other workspace edits use only semantic references. Static target declarations
likewise remain useful navigation candidates without becoming proof that Bazel
evaluated the call successfully.

## 2. Authoritative semantic inputs

### Rule schemas

The Bazel actor publishes immutable `RuleSchemas` from `rule_class_info`.
Schemas retain the Bazel executable, arguments, workspace, graph generation,
and rule-class key that produced them. They contain only metadata Bazel reports:
rule and attribute names, attribute types, requiredness, defaults, providers,
and documentation. A finite value set is offered only where the source reports
one exactly.

### Source symbols

Source parsing supplies exact declarations, lexical scopes, function
parameters, and `load()` bindings. Disk state and open-buffer overlays follow
the same precedence as the existing source index. Source symbols are not
inserted into `RuleSchemas`, and neither model guesses the type of an arbitrary
expression.

Requests capture both inputs with `Documents` and `Index` while preserving
their provenance. Missing modules, unresolved bindings, dynamic values, and
unsupported constructs remain unknown. There is no general flow-sensitive
Starlark type-checker milestone.

## 3. Semantic authoring

Explicit label completion uses the target index without waiting for rule
schemas. Rule and attribute completion, documentation hover, and rule signature
help share `RuleSchemas`; source-defined callable completion, definition,
references, and rename use exact source symbols. Expensive documentation is
resolved only after the client selects an item.

Completion may offer a plausible candidate. Hover states only known facts, and
diagnostics never inherit certainty from a completion result.

## 4. Correctness and repair

Unresolved-label diagnostics require both a proven label role and a resolution
result. Resolution must distinguish a missing target from an unavailable or
partial graph, an unfetched repository, an unknown repository mapping, an
unfinished index, a source candidate, a source file, and a target pattern that
names a set. Diagnostics follow only from states that prove the label is
invalid.

Document and workspace pull reports derive their result IDs from the document
and index snapshots that produced them. A newer snapshot invalidates the
result; no parallel dirty flag or diagnostic cache tracks the same fact.

Code actions follow diagnostics. Start with buildifier fixes and repository
fetches, carrying enough diagnostic data to resolve edits or commands lazily.
Adding dependencies waits until the server can identify the owning rule and a
safe attribute edit without guessing.

## 5. Refactoring

File and package moves use a refactoring planner over one document/index
snapshot. It computes label edits for `workspace/willRenameFiles`, checks open
document versions and indexed disk ranges, and returns one annotated workspace
edit. The server does not move files itself.

The planner is a boundary around proven semantic references, not a second
index. Candidate strings never enter a multi-file edit merely because their
normalised spelling matches a target.

## 6. Bazel configuration

`.bazelrc` is a separate language surface. Its parser and handlers cover
startup options, commands, configurations, imports, and continuations without
adding conditional paths to the Starlark handlers.

The tracked [Bazelrc 8.7 specification](docs/bazelrc-8.7.md) is the normative
implementation boundary. The site renders the dated public support matrix;
research notes retain source evidence and corpus measurements.

The parser implements upstream Bazel 8.7.0. The advertised compatibility line
is Bazel 8.7; a vendor suffix on that numeric release does not add vendor
semantics. Other releases are outside the support contract, and absence from
an 8.7 catalog never proves a flag invalid.

The watch thread publishes the workspace `.bazelrc` import graph. Requests use
that immutable snapshot and the current buffer without reading the filesystem.
The Bazel actor supplies a flag catalog from the configured binary's
`help flags-as-proto` output only when the reported numeric release is 8.7.
Catalog identity derives from the binary, reported Bazel version, and startup
arguments so configuration changes cannot reuse stale flag semantics. There is
no nearest-version or bundled fallback catalog.

Syntax, import/config navigation, structural language features and exact-catalog
flag intelligence are shipped. Configuration references, symbols, conservative
rename, import and enum-value completion, structural hover, and expansion
diagnostics use the same request-local configuration view. Converter-specific
values, final effective option rendering, automatic platform configuration,
external rc discovery, and live watching of imported files outside the
workspace remain outside the shipped contract. Formatting is an intentional
non-feature because Bazel defines no canonical semantics-safe layout.

## Dependency order

1. Preserve label candidates with their context, then restrict references and
   workspace edits to proven semantic roles.
2. Publish `RuleSchemas` and exact source symbols as independent inputs.
3. Complete explicit labels; then add schema-backed rules, attributes, hover,
   and signatures, plus source-backed definition, references, and rename.
4. Add resolution states after semantic-role proof, then document diagnostics,
   workspace diagnostics, and diagnostic code actions.
5. Add the refactoring planner, then file and package move edits.
6. Model external rc discovery only if concrete editor use cases justify
   exposing host-specific configuration state.
