# starpls — architectural deep dive

Source of truth: local clone at `/Users/bruth/dev/bazel-language-server/upstream/starpls`
(shallow, 50 commits) at `ac25eca3dbbed6347fbca5fbf14d3a027d46bcab`, plus the GitHub API.
Repo: <https://github.com/withered-magic/starpls>. Apache-2.0. All statements below are
from reading the source, not from docs.

---

## 1. Provenance & liveness

| Fact | Value |
|---|---|
| Repo created | 2023-12-19 |
| Owner | **`withered-magic`, a personal GitHub account, not an org** |
| Last commit on `main` | **2025-12-03** (`ac25eca` "Accept 1/0 as bool (#414)", authored by Son Luong Ngoc / sluongng) |
| `pushed_at` (API) | 2025-12-03T01:54:57Z |
| Latest release | **v0.1.22, 2025-08-30**; v0.1.21 2024-12-28; v0.1.20 2024-12-06 |
| Stars / forks | 217 / 32 |
| Total commits | 394 |
| Named contributors | 12 (`withered-magic` 383 of 394; everyone else 1 commit each) |
| Open issues (non-PR) | 57 |
| Open PRs | 20 |
| Archived / disabled | no |

**Verdict: effectively unmaintained since 2025-12-03 — roughly nine months of silence as of
2026-08-25 — while the community keeps sending patches that nobody merges.** The open-PR queue
is the strongest evidence:

```
2026-07-19  #438  fmeum     fix: don't consume a type spec entry for the `*` marker
2026-07-18  #437  Ahajha    Fix escape sequences in triple quotes not resetting the closing streak
2026-07-12  #436  Ahajha    Fix: Correctly resolve labels with a slash in the target name
2026-07-10  #435  Ahajha    chore: add label_list_dict
2026-07-10  #434  Ahajha    chore: Bump rules_rust, rust version, and rust edition
2026-07-09  #433  Ahajha    Fix: Enable bzlmod by default for Bazel 7 onwards
2026-06-03  #428  keith     Add support for nodeps bazel_deps
2026-05-29  #427  keith     Handle BUILD.foo as a BUILD file
2026-04-24  #425  keith     Add flag_alias MODULE.bazel function
2026-02-14  #423  ben-krieger  Correct keyword arg name for print builtin to sep
2026-01-22  #419  rafikk    feat(bazel): support repo_name = None in bazel_dep
2025-12-16  #416  sluongng  bzlmod: go-to-def for module extension
2025-12-05  #415  sluongng  Update builtin.pb
2025-11-30  #413  PeterCardenas  feat: add buildozer query
2025-11-30  #412  PeterCardenas  feat: add progress for initializing server
2025-08-11  #401  steeve    feat: implement textDocument/formatting
```

Fabian Meumertzheim (`fmeum`, Bazel core / EngFlow) and Keith Smiley (`keith`, Bazel core /
Lyft) are both blocked on this queue. The maintainer's own last merges were 2025-12-03.

**Google does not endorse starpls.** `bazelbuild/vscode-bazel`'s README
(`upstream/vscode-bazel/README.md:71-83`) lists it as one of *two* "experimental" options
alongside `cameron-martin/bazel-lsp` and explicitly says "We can't currently make any
recommendation between these two." There is zero mention of starpls anywhere in
`bazelbuild/bazel`'s `site/` directory. The only Google-adjacent connection is that Bazel
upstream accepts patches motivated by starpls bugs — e.g. `bazelbuild/bazel#27850`
("ApiExporter: capture missing attributes … This causes language servers such as starpls to
fail", filed against starpls issue #410) — but that Bazel PR is *also still open and unmerged*.

Distribution: Homebrew tap `withered-magic/brew/starpls` (Apple Silicon only), GitHub release
binaries (linux-amd64, linux-aarch64, darwin-amd64, darwin-arm64, windows), `nvim-lspconfig`
has a `starpls` config, and the Zed extension `zaucy/zed-starlark`.

No fork has taken over. The most-advanced fork is `fmeum/starpls` (pushed 2026-07-19, 0 stars);
`AleksanderGondek/starpls-nixified` is a packaging fork.

---

## 2. Crate / module graph

Cargo workspace, `resolver = "2"`, 12 members. Line counts are `.rs` only.

| Crate | Responsibility |
|---|---|
| `crates/starpls_lexer` (~2.2k LOC) | Hand-written character-level lexer for Starlark. Emits `Token { kind, len }`; handles INDENT/DEDENT synthesis, line joining, string/bytes literal prefixes and escapes (`unescape.rs`). Zero dependencies. |
| `crates/starpls_parser` (~1.4k LOC) | Hand-written recursive-descent parser over *non-trivia token kinds only*. Produces a flat `Vec<Step>` event stream, not a tree. Grammar in `src/grammar/{statements,expressions,arguments,parameters,type_comments}.rs`. |
| `crates/starpls_syntax` (~1.7k LOC) | Rowan glue: `StarlarkLanguage`, `SyntaxNode`/`SyntaxToken`, the typed AST (`ast.rs`, 1547 LOC, hand-written), and `parse_module()` which reattaches trivia and builds the lossless green tree. |
| `crates/starpls_intern` (231 LOC) | Verbatim copy of rust-analyzer's `intern` crate — global `Arc`-based interner over a sharded `DashMap`, used to intern `TyKind`. |
| `crates/starpls_bazel` (~1.1k LOC + ~78 KB data) | Everything Bazel-shaped that is *not* semantics: prost-generated `builtin.proto` and `build.proto` bindings, the `BazelClient` trait + `BazelCLI` shell-out impl, the label parser (`label.rs`, 484 LOC), hand-curated builtin JSON stubs, common-attribute tables. |
| `crates/starpls_common` (192 LOC) | The base salsa jar: `File` input, `parse` query, `line_index` query, `Diagnostics` accumulator, `Dialect`, `FileId`, and the `Db` trait that abstracts file loading. |
| `crates/starpls_hir` (~9.6k LOC) | The bulk of the system. HIR lowering (`def/lower.rs`), scopes (`def/scope.rs`), name resolution (`def/resolver.rs`), code-flow graph (`def/codeflow.rs`), and the type checker (`typeck.rs`, `typeck/infer.rs`, `typeck/call.rs`, `typeck/builtins.rs`, `typeck/intrinsics.rs`). |
| `crates/starpls_ide` (~3.3k LOC) | Feature layer: `Analysis`/`AnalysisSnapshot` façade over the salsa `Database`, plus `completions`, `goto_definition`, `hover`, `find_references`, `document_symbols`, `signature_help`, `diagnostics`, and the debug requests `show_hir`/`show_syntax_tree`. |
| `crates/starpls_test_util` (147 LOC) | Fixture markup parser (`$0` cursor, `#^^^` selection) and `make_test_builtins()`. |
| `crates/starpls` (~2.4k LOC + 2.4 MB `builtin.pb`) | The binary. LSP transport (`lsp-server` 0.7.5 / `lsp-types` 0.94.1), event loop, document manager, path interner, `DefaultFileLoader` (label→path resolution), the `check` CLI subcommand. |
| `vendor/runfiles` (547 LOC) | Vendored copy of `rules_rust` 0.38.0's runfiles library, only used so parser snapshot tests can find `test_data/` under Bazel. |
| `xtask` | One command: `cargo xtask update-parser-test-data` regenerates `.rast` snapshots. |

Dependency graph (all edges are path deps):

```
                                   starpls (bin)
                                  /    |     |    \
                    starpls_ide ─┘     |     |     └─ starpls_syntax
                   /     |      \      |     |
       starpls_hir       |       \     |     |
      /   |    |   \     |        \    |     |
     /    |    |    \    |         \   |     |
starpls_intern |  starpls_common ───────┴─────┐
               |     /        \               |
      starpls_test_util   starpls_bazel ──────┘
               |               (prost, serde)
        starpls_syntax
         /          \
 starpls_parser   rowan, line-index
        |
 starpls_lexer
```

Notable third-party pins:

- **salsa: a personal fork.** `salsa = { git = "https://github.com/withered-magic/salsa",
  package = "salsa-2022", rev = "91fdda90b344ef74e9bf35c3a5bb0fbae22ed6fb" }`. That fork of
  `salsa-rs/salsa` was last pushed **2024-01-15**, and it tracks the `salsa-2022` jar-based
  prototype which upstream salsa has since abandoned in favour of the current `salsa` 0.x API.
  This is a hard dependency in `starpls_common`, `starpls_hir`, and `starpls_ide`.
- `lsp-types = "0.94.1"` (released 2023; the crate is at 0.97+ now).
- `rowan = "0.15.11"`, `line-index = "0.1.0"`, `id-arena`, `smol_str`, `smallvec`, `either`,
  `dashmap = "5.5.3"`, `triomphe`.

---

## 3. Syntax layer

### Pipeline

`&str` → `starpls_lexer::tokenize` → `StrWithTokens` (`starpls_parser/src/text.rs`) →
`starpls_parser::parse(&Input)` → `Output { steps: Vec<Step> }` →
`starpls_syntax::parse_module` → `rowan::GreenNodeBuilder` → `ParseTree<Module>`.

The key design (lifted from rust-analyzer) is in `crates/starpls_parser/src/lib.rs:45-53`:

> "Because the parser operates only on token types and has no knowledge of text offsets, etc.,
> it instead outputs a series of steps that can be consumed by a separate parse tree builder."

`Input` is literally `Vec<SyntaxKind>` — trivia-free. `StrWithTokens::build_with_trivia`
replays the steps and re-interleaves whitespace/comments so the resulting green tree is
**fully lossless**: every byte of the input is in the tree.

### Parser type

**Hand-written recursive descent with markers** — *not* generated, *not* tree-sitter, *not*
LALR. `Parser::start()` returns a `Marker` (guarded by `drop_bomb` so you can't forget to
complete it), `Marker::complete(p, KIND)` back-patches the `Tombstone` event.
`Marker::precede()` supports left-recursive expression construction.

**The grammar is not declared anywhere.** There is no `.ungram`, no `.y`, no BNF file. The
grammar exists only as Rust functions, with the Starlark spec productions copied into doc
comments, e.g. `crates/starpls_parser/src/grammar/statements.rs:18-20`:

```rust
/// Grammar: `File = {Statement | newline} eof .
/// Statement = DefStmt | IfStmt | ForStmt | SimpleStmt .`
pub(crate) fn statement(p: &mut Parser) {
```

`crates/starpls_syntax/src/ast.rs` (1547 LOC) is likewise hand-written, not generated from a
grammar — every `AstNode` impl is spelled out.

### Error recovery

Follow-set-driven panic-mode recovery. `SyntaxKindSet` is a bitset over `SyntaxKind`
(`syntax_kind.rs`), and the single recovery primitive is
(`crates/starpls_parser/src/lib.rs:129-144`):

```rust
pub(crate) fn error_recover_until(&mut self, message: impl Into<String>, recover: SyntaxKindSet) {
    self.error(message);
    // Start a new ERROR node and consume tokens until we are at either a token
    // specified in the recovery set, or EOF.
    if !self.at(EOF) && !recover.contains(self.current()) {
        let m = self.start();
        while !self.at(EOF) && !recover.contains(self.current()) { self.bump_any(); }
        m.complete(p, ERROR);
    }
}
```

Statement-level recovery set is `STMT_RECOVERY = { NEWLINE }`. Unexpected `INDENT` at
statement position is wrapped by `error_block()`. Each production bails early and completes
its marker so the tree shape stays valid — e.g. `def_stmt` emits `PARAMETERS` even when `(`
was never closed. Errors are collected out-of-band via a `&mut dyn FnMut(SyntaxError)` sink
and pushed into the salsa `Diagnostics` accumulator by
`starpls_common::parse` (`crates/starpls_common/src/lib.rs:151-168`). The IDE layer caps
syntax errors at 128 per file (`crates/starpls_ide/src/diagnostics.rs:19`).

### Type comments are parsed as a nested grammar

Unusual and worth stealing: PEP-484 `# type: ...` comments are parsed *inside the lexer's
COMMENT token*. `crates/starpls_syntax/src/parser.rs:68-79`:

```rust
StrStep::Token { kind, text, pos } => {
    if kind == COMMENT && text.starts_with("# type: ") {
        build_type_comment(&mut builder, text, ..., errors_sink);
        return;
    }
    builder.token(StarlarkLanguage::kind_to_raw(kind), text);
}
```

`build_type_comment` opens a `TYPE_COMMENT` node, emits a `TYPE_COMMENT_PREFIX` token for
`"# type: "`, then runs a *second* parser (`parse_type_list`, grammar in
`grammar/type_comments.rs`) over the remainder, producing real syntax nodes
(`UNION_TYPE`, `PATH_TYPE`, `GENERIC_ARGUMENTS`, `FUNCTION_TYPE`, `ELLIPSIS_TYPE`,
`IGNORE_TYPE`, …). So type comments are first-class CST nodes with correct source offsets.

### BUILD vs .bzl vs MODULE.bazel: one grammar, two axes of context

There is **exactly one grammar**. Dialect is not a parser concern at all. Two orthogonal
enums carry it:

```rust
// crates/starpls_common/src/lib.rs:31
pub enum Dialect { Standard, Bazel }

// crates/starpls_bazel/src/lib.rs:126
pub enum APIContext { Bzl, Build, Module, Repo, Workspace, Prelude, Cquery, Vendor }
```

Classification is purely filename-based, in
`crates/starpls/src/document.rs:745-780` (`dialect_and_api_context_for_workspace_path`):

| Filename | Dialect | APIContext |
|---|---|---|
| `BUILD`, `BUILD.bazel`, `*.BUILD`, `*.BUILD.bazel` | Bazel | `Build` |
| `*.bzl` | Bazel | `Bzl` |
| `MODULE.bazel`, `*.MODULE.bazel` | Bazel | `Module` |
| `REPO.bazel` | Bazel | `Repo` |
| `VENDOR.bazel` | Bazel | `Vendor` |
| `WORKSPACE`, `WORKSPACE.bazel`, `WORKSPACE.bzlmod` | Bazel | `Workspace` |
| `*.cquery`, `*.query.bzl` | Bazel | `Cquery` |
| `<workspace>/tools/build_rules/prelude_bazel` | Bazel | `Prelude` |
| `*.star`, `*.sky`, anything else | Standard | `None` |

Gaps relative to our scope: **no handling of `MODULE.bazel.lock`, `.bazelrc`, `.bazelignore`,
BCR `source.json`/`metadata.json`, or `*.bzlmod`.** `BUILD.foo` (as opposed to `foo.BUILD`)
is unhandled — that's open PR #427.

`APIContext` is consumed only by the resolver, to pick which pool of global names is in scope
(`crates/starpls_hir/src/def/resolver.rs:148-177`).

---

## 4. Semantic layer: salsa

Yes — a real salsa incremental database, on the `salsa-2022` (jar) API.

### Jars

```rust
// crates/starpls_common/src/lib.rs:21
#[salsa::jar(db = Db)]
pub struct Jar(Diagnostics, File, LineIndexResult, Parse, parse, line_index_query);

// crates/starpls_hir/src/lib.rs:72
#[salsa::jar(db = Db)]
pub struct Jar(
    lower, ModuleInfo, def::Function, def::LoadStmt, def::InternedString,
    def::codeflow::CodeFlowGraphResult, def::codeflow::code_flow_graph,
    def::scope::ModuleScopes, def::scope::module_scopes, def::scope::module_scopes_query,
    typeck::builtins::{BuiltinDefs, BuiltinFunction, BuiltinGlobals, BuiltinProvider,
                       BuiltinProviders, BuiltinType, BuiltinTypes,
                       builtin_globals_query, builtin_providers_query, builtin_types_query,
                       CommonAttributes, common_attributes_query},
    typeck::intrinsics::{Intrinsics, IntrinsicClass, IntrinsicFieldTypes, IntrinsicFunction,
                         IntrinsicFunctions, intrinsic_types, intrinsic_field_types,
                         intrinsic_functions},
);

// crates/starpls_ide/src/lib.rs:59
#[salsa::db(starpls_common::Jar, starpls_hir::Jar)]
pub(crate) struct Database { … }
```

Only **one** `#[salsa::input]` on the file side (`File { id, dialect, info, contents }`) and
one for builtins (`BuiltinDefs { builtins: Builtins, rules: Builtins }`), plus one
`#[salsa::interned]` (`InternedString`) and one `#[salsa::accumulator]` (`Diagnostics`).

### Key queries

| Query | Signature | Notes |
|---|---|---|
| `parse` | `(db, File) -> Parse` | pushes syntax errors into `Diagnostics` |
| `line_index_query` | `(db, File) -> LineIndexResult` | |
| `lower` | `(db, File) -> ModuleInfo` | HIR lowering; produces `Module` + `ModuleSourceMap` |
| `module_scopes` / `module_scopes_query` | `(db, File)` / `(db, ModuleInfo) -> ModuleScopes` | scope tree; pushes `Could not resolve symbol` diagnostics |
| `code_flow_graph` | `(db, File) -> CodeFlowGraphResult` | opt-in CFG |
| `builtin_types_query`, `builtin_globals_query`, `builtin_providers_query`, `common_attributes_query` | `(db, BuiltinDefs)` | decode `builtin.pb` into `Ty`s; memoised once |
| `intrinsic_types`, `intrinsic_functions`, `intrinsic_field_types` | `(db)` / `(db, IntrinsicClass)` | pure-Starlark builtins, hardcoded |

### Type inference is *not* a salsa query — this is a big architectural deviation

Inference lives in a `GlobalContext` holding `Arc<Mutex<InferenceContext>>` plus an
`AtomicCell<bool> cancelled` flag (`crates/starpls_hir/src/typeck.rs:167-172`, `:2280+`).
Callers go through `with_tcx(db, |tcx| …)`, which takes the mutex. Consequences:

- Inference results are cached in plain `FxHashMap`s (`type_of_expr`, `type_of_param`,
  `type_of_load_item`, `resolved_load_stmts`) inside `InferenceContext`, **not** in salsa,
  so they are *not* incrementally invalidated — they are wholesale discarded on any change via
  `GlobalContext::cancel()` → `CancelGuard`.
- Cancellation is by panic: `TypecheckCancelled::throw()` does `resume_unwind`, caught by
  `Cancelled::catch` in `AnalysisSnapshot::query` (`crates/starpls_ide/src/lib.rs:399-405`).
- The whole type checker is single-threaded behind one global mutex, despite the server
  running a 4-thread task pool. Open issue #45 "Shard InferenceCtxt" (2024-02-23).
- There was a deadlock bug here fixed as recently as 2024-12-11 (`cd547df`, "avoid deadlock by
  using 'alt' formatter for inference diagnostics").

### File loading is a `dyn` escape hatch, not a query

`starpls_common::Db` (`crates/starpls_common/src/lib.rs:66-102`) declares
`load_file`, `resolve_path`, `list_load_candidates`, `resolve_build_file` as **plain trait
methods on the database**, delegating to an `Arc<dyn FileLoader>`. They do filesystem I/O and
shell out to Bazel, so they are deliberately outside salsa's dependency tracking. This is the
main correctness hole: a cross-file `load()` edge is not a tracked dependency, so editing a
loaded `.bzl` does not automatically invalidate importers — the server compensates by
cancelling all inference on any change.

### Name resolution

`def/scope.rs` builds a `Scopes` arena keyed by `ScopeHirId ∈ {Module, Expr(ExprId),
Stmt(StmtId)}`, with a parallel notion of **execution scope**:

```rust
pub(crate) enum ExecutionScopeId { Module, Def(StmtId), Comp(ExprId), Lambda(ExprId) }
```

This models Starlark's rule that a `def` body sees module-level names but assignments are
function-local, and that comprehensions have their own scope. `Scope { defs:
FxHashMap<Name, Vec<ScopeDef>>, execution_scope, parent }` — a `Vec` per name because
starpls tracks *all* re-assignments (used for `find_references` and for union types under
code-flow analysis).

```rust
pub(crate) enum ScopeDef {
    Function(FunctionDef), IntrinsicFunction(IntrinsicFunction), BuiltinFunction(BuiltinFunction),
    Variable(VariableDef), BuiltinVariable(TypeRef), Parameter(ParameterDef), LoadItem(LoadItemDef),
}
```

`Resolver::resolve_name` (`def/resolver.rs:97-121`) walks the scope chain outward and stops at
the first *execution scope* boundary that has a hit, returning **all** defs in that execution
scope. Fallback order for a miss (`resolve_name_in_prelude_or_builtins`, `:123-146`):

1. If `APIContext::Build`: the Bazel prelude file (`tools/build_rules/prelude_bazel`), loaded
   eagerly at startup (`crates/starpls/src/server.rs:143-160`, `load_bazel_prelude`).
2. Starlark intrinsics (`intrinsic_functions`).
3. `APIContext`-selected builtin globals: `repo_globals` / `cquery_globals` / `vendor_globals`
   exclusively, else `bzl_globals` plus (`Module` → `bzlmod_globals`) or
   (`Workspace` → `workspace_globals`).

### `load()` handling

`load()` is lowered to `Stmt::Load { load_stmt: LoadStmt, items: Box<[LoadItemId]> }`, where
`LoadStmt` is a `#[salsa::tracked]` struct carrying the module string and an `AstPtr`.
`LoadItem` is `Direct { name, load_stmt }` or `Aliased { alias, name, load_stmt }`.

`TyContext::resolve_load_stmt` (`typeck/infer.rs:2225-2261`) calls `db.load_file(module,
dialect, from)` and memoises the `Option<File>`; failure emits
`Could not resolve module "<path>": <err>` as a **Warning**. `infer_load_item`
(`:2117-2224`) then calls `Resolver::resolve_export_in_file(db, loaded_file, name)`, which
refuses names starting with `_` (`def/resolver.rs:62-65`) — correct Starlark visibility.

Cycle handling is explicit: `InferenceContext.load_resolution_stack` is a stack of
`(File, LoadStmt)`; self-import gives `Cannot load the current file`, and a repeat gives a
multi-line `Detected circular import` warning attached to *every* load statement in the cycle.

---

## 5. Type inference

This is starpls's differentiator and the reason it's the most important prior art. It is
explicitly modelled on **Pyright** (README acknowledgement) rather than on Hindley-Milner:
bidirectional-ish, literal-tracking, no unification, no generalisation, no type variables
except a `BoundVar(usize)` + `Substitution` mechanism used only for `list`/`dict` methods.

Location: `crates/starpls_hir/src/typeck.rs` (2068 LOC),
`typeck/infer.rs` (2374 LOC), `typeck/call.rs` (462 LOC),
`typeck/builtins.rs` (1169 LOC), `typeck/intrinsics.rs` (1913 LOC).

### `Ty` and `TyKind`

`Ty(Interned<TyKind>)` — globally interned via `starpls_intern`, so `Ty` is pointer-sized and
comparison is cheap. Full `TyKind` (`typeck.rs:1227-1343`), 33 variants:

```
Unbound, Unknown, Any, Never, None,
Bool(Option<bool>), Int(Option<i64>), Float, String(Option<InternedString>),
StringElems, Bytes, BytesElems,
List(Ty), Tuple(Tuple), Dict(Ty, Ty, Option<Arc<DictLiteral>>), Range,
Function(FunctionDef), IntrinsicFunction(IntrinsicFunction, Substitution), BuiltinFunction(BuiltinFunction),
BuiltinType(BuiltinType, Option<TyData>), BoundVar(usize), Protocol(Protocol), Union(SmallVec<[Ty; 2]>),
Struct(Option<Struct>), Attribute(Option<Attribute>), Rule(Rule),
Provider(Provider), ProviderInstance(Provider), ProviderRawConstructor(Name, Provider),
TagClass(Arc<TagClass>), ModuleExtension(Arc<ModuleExtension>), ModuleExtensionProxy(Arc<ModuleExtension>),
Tag(Arc<TagClass>), Target, Macro(Macro),
```

Note the **literal types**: `Bool(Some(true))`, `Int(Some(3))`, `String(Some(s))`. These are
what let `rule(attrs = {...})` be analysed statically — the dict literal's keys survive into
`DictLiteral { expr, known_keys: Box<[(InternedString, Ty)]> }`, so `attrs = {"srcs":
attr.label_list()}` is inspectable at type-check time. `Tuple` is `Simple(SmallVec<[Ty;2]>)`
or `Variable(Ty)`. `Protocol` is `Iterable(Ty) | Sequence(Ty)`.

Ten of the 33 variants exist purely to model Bazel. Two carry a comment that they *override*
the definitions from `builtin.pb`:

```rust
/// A Bazel struct (https://bazel.build/rules/lib/builtins/struct).
/// Use this instead of the `struct` type defined in `builtin.pb`.
Struct(Option<Struct>),
/// A Bazel attribute (https://bazel.build/rules/lib/builtins/Attribute.html).
/// Use this instead of the `Attribute` type defined in `builtin.pb`.
Attribute(Option<Attribute>),
```

### The dispatch point: `BuiltinFunction::maybe_unique_ret_type`

Every Bazel-specific behaviour funnels through one function,
`crates/starpls_hir/src/typeck/builtins.rs:157-548`. It matches on
`(parent_type, function_name)` and, when it recognises a builtin, synthesises a bespoke return
type from the *literal* argument values:

| Match | Produces |
|---|---|
| `(None, "struct")` | `TyKind::Struct(Some(Struct::Inline { call_expr, fields }))` — fields are every keyword argument with its inferred type. Field access is then exact: `struct(a=1).a : Literal[1]`. |
| `(None, "provider")` | Reads `doc=`, `fields=` (must be a dict literal), `init=`. Walks *up the AST* from the call expr to the enclosing `AssignStmt` to recover the provider's name. With `init=`, returns `tuple[Provider[X], ProviderRawConstructor]` matching Bazel's two-value return. |
| `(None, "rule" \| "repository_rule")` | `TyKind::Rule(Rule { kind: Build\|Repository, doc, attrs })`, attrs via `attrs_from_dict_literal`. |
| `(Some("attr"), name)` | `TyKind::Attribute(Some(Attribute { kind, doc, mandatory, default_value }))`. Recognised names: `bool, int, int_list, label, label_keyed_string_dict, label_list, output, output_list, string, string_keyed_label_dict, string_dict, string_list, string_list_dict`. **`label_list_dict` (new in Bazel 9) is missing** — that is open PR #435 / issue #420. |
| `(None, "tag_class")` | `TyKind::TagClass(Arc<TagClass>)` with per-attr `AttributeData`. |
| `(None, "module_extension")` | `TyKind::ModuleExtension` carrying its `tag_classes` dict. |
| `(None, "macro")` | `TyKind::Macro(Macro { attrs, doc })` — symbolic macros, added 2024-12-28 (#359). `attrs_from_dict_literal(.., allow_none=true)` so `"foo": None` records a *disallowed* attribute. |
| `(None, "use_extension")` | Actually resolves the `("//path.bzl", "name")` pair through `db.load_file`, infers the target expression, and returns `ModuleExtensionProxy`. |
| `(None, "use_repo_rule")` | Same load-and-infer, returns the `Rule` if it's a `RuleKind::Repository`. |

### `rule()` / `attr.*` → callable signature

`Rule::attrs()` (`typeck.rs:1461-1489`) yields `name` first (from `commonAttributes.json`),
then the rule's own attrs, then the remaining common attrs. Calling a `TyKind::Rule` is
handled in `infer.rs` around line 700-960: `Slots::from_rule` builds parameter slots from
those attrs, argument types are checked against `Attribute::expected_ty()`, and missing
mandatory attrs produce `Argument missing for attribute(s) …`. Unknown keywords produce
`Unexpected keyword argument "x"`.

`Attribute` has two type projections (`typeck.rs:1393-1436`):

- `expected_ty()` — what you may *pass* at the call site: `attr.label` → `string`,
  `attr.label_list` → `list[string]`.
- `resolved_ty(rule_kind)` — what `ctx.attr.foo` *is* inside the impl: `attr.label` →
  `TyKind::Target` for `RuleKind::Build`, and **`Unknown` for `RuleKind::Repository`** with a
  `TODO` admitting it should be `Label` (`typeck.rs:1410-1414`). That's open issue #373.

### `ctx` attribute inference (opt-in)

`--experimental_infer_ctx_attributes`. `TyKind::BuiltinType(ty, Some(TyData::Attributes(kind,
attrs)))` — when the builtin type is `ctx` or `repository_ctx`, `Ty::fields` intercepts the
`attr` field and substitutes `Struct::RuleAttributes { rule_kind, attrs }`
(`typeck.rs:270-286`). The link from a `rule(implementation = _impl)` back to `_impl`'s `ctx`
parameter is what makes `ctx.attr.bar : int` work.

### Providers

`Provider` is `Builtin(BuiltinProvider)` or `Custom(Arc<CustomProvider>)`. Calling a
`TyKind::Provider` yields `TyKind::ProviderInstance(provider)`; field access on the instance
enumerates `CustomProviderFields.fields`. Builtin providers are discovered by intersecting
`builtin.pb` types against a **hardcoded 34-entry allowlist**
`KNOWN_PROVIDER_TYPES` (`crates/starpls_bazel/src/lib.rs:89-123`: `CcInfo`, `DefaultInfo`,
`JavaToolchainInfo`, `PyInfo`, …). Any provider not on that list — including every
`rules_*`-defined one that isn't user-visible as a `provider()` call — is invisible. Open
issue #244 "handle provider types missing from builtins proto".

`target[SomeProvider]` is special-cased in the `Expr::Index` arm (`infer.rs:583-588`):

```rust
(TyKind::Any | TyKind::Unknown | TyKind::Target, TyKind::Provider(provider))
    => Some(TyKind::ProviderInstance(provider.clone()).intern()),
```

### `depset` and `select`: **no special handling at all**

Grepping the whole `starpls_hir` crate for `depset` and `select` returns nothing. Both are
ordinary entries in `builtin.pb`, so:

- `depset(...)` gets whatever `return_type` string `builtin.pb` declares, resolved through
  `parse_type_ref`. There is no element-type tracking — `depset([File])` is not `depset[File]`.
- `select({...})` is just a function returning whatever the proto says. **starpls does not
  understand that `select()` is assignable to a `label_list` attr**, does not check
  `select()` keys against `config_setting` targets, does not resolve `//conditions:default`,
  and does not model `select() + [...]` concatenation. For a Bazel LSP this is a significant
  hole.

### Assignability

`assign_tys(db, source, target)` (`typeck.rs:2002-2068`) — structural, no subtyping lattice,
and deliberately permissive. `Any`/`Unknown` are assignable both ways. Covariant everywhere
(with a `TODO` saying so). Special cases worth noting:

```rust
(TyKind::String(_), TyKind::BuiltinType(ty, _)) | (TyKind::BuiltinType(ty, _), TyKind::String(_))
    if ty.name(db).as_str() == "Label" => true,          // string ⇄ Label
(TyKind::Int(Some(v)), TyKind::Bool(_)) if *v == 0 || *v == 1 => true,   // testonly = 1
(_, TyKind::Union(tys)) => tys.iter().any(...),
(TyKind::Union(tys), _) => tys.iter().any(...),          // ← unsound, see TODO below
```

That last line is knowingly wrong:

```rust
// TODO(withered-magic): The logic below also temporarily allows assignments like
// `int | None` to `int`. Fix this once we support type guards.
```

### Type comments (PEP 484)

`TypeRef` (`typeck.rs:174-181`) is `Name(Name, Option<Box<[TypeRef]>>) | Path(SmallVec<[Name;1]>,
…) | Union(Vec<TypeRef>) | Provider(BuiltinProvider) | Ellipsis | Unknown`.
`TypeRefResolver::resolve_path` (`typeck.rs:1837-1900+`) maps the spellings
`Any, Unknown/unknown, None/NoneType, bool, int, float, string, bytes, list[T], dict[K,V],
range, Iterable[T]/iterable, Sequence[T]/sequence, Union[..]/union, struct[T]/structure` plus
anything in `builtin_types`. Dotted paths (`java_common.JavaRuntimeInfo`) are resolved by
walking `.fields()` from a name in the *enclosing scope* — so user-defined providers work as
type annotations. Unknown names produce a `Warning`, not an error.

### Code flow analysis (opt-in)

`--experimental_use_code_flow_analysis`. `def/codeflow.rs` (914 LOC) builds a CFG:

```rust
pub(crate) enum FlowNode {
    Start,
    Assign { expr, name, execution_scope, source, antecedent },
    Branch { antecedents: Vec<FlowNodeId> },
    Loop { antecedents: Vec<FlowNodeId> },
    Call { expr, antecedent },
    Unreachable,
}
```

Narrowing walks antecedents backwards from a use site and unions the assignments reached.
`FlowNode::Call` exists to model `fail()` making subsequent code unreachable. **Loops are not
handled**: `infer.rs:1688` — `FlowNode::Loop { .. } => break 'outer None, // TODO: Correctly
handle loops.` (This is the cause of open issue #405, "Incorrect unused variable diagnosis in
loops", and #367.)

### Diagnostics the type checker emits

Complete list, from string literals in `typeck*.rs` and `def/scope.rs`:

*Errors:* `Could not resolve symbol "x"`, `Expression is not assignable`,
`Unexpected positional argument`, `Positional argument cannot follow keyword arguments`,
`Positional argument cannot follow keyword argument unpacking`,
`Unpacked iterable argument cannot follow keyword arguments`,
`Unpacked iterable argument cannot follow keyword argument unpacking`,
`Index N is out of range for type T`, `Cannot index tuple with type "T"`.

*Warnings:* `Argument of type "A" cannot be assigned to parameter of type "B"`,
`Unexpected keyword argument "x"`, `Argument missing for parameter(s) …`,
`Argument missing for attribute(s) …`, `Cannot access field "x"`, `Cannot assign to field "x"`,
`Cannot set attribute "x"`, `Cannot reassign to method "x"`, `Cannot index T with type "U"`,
`Cannot slice expression of type "T"`, `Cannot use value of type "T" …`, `Unknown type "X"`,
`Type "T" is not indexable`, `Could not resolve module "…"`,
`Could not resolve symbol "x" in module "…"`, `Cannot load the current file`,
`Detected circular import`, `Code is unreachable` (tagged `Unnecessary`),
`Tuple size mismatch, N on left-hand side and M on right-hand side`,
unused-definition warnings (tagged `Unnecessary`), `Missing expected argument of type "T"`.

Type errors are deliberately **Warnings, not Errors** — README "Known Issues": "Type checker
shows some false positives, especially when the definitions from the builtins proto are
incorrect. Because of these two issues, some type checking diagnostics are currently set to
display as warnings."

### Documented limitations (README + code)

- Type guards / narrowing on `if x:` are not supported.
- Dataflow analysis unchecked on the roadmap.
- Type comments on parameters: "only basic types currently supported".
- Union interactions are wrong (issue #171, open since 2024-04-07).
- `return` statements are not typechecked (issue #87, open since 2024-03-25).
- No `Callable` protocol (issue #100).
- Lambda parameters unhandled (`typeck.rs:902`, `:921`).
- Function overloads, keyword-only params, optional args in intrinsics all TODO
  (`intrinsics.rs:222-229`: "Many of these signatures are wrong").
- Not aligned with the official "Bootstrapping Starlark types" proposal (issue #298).
- No Python3-style `def f(x: int) -> str` annotations (issue #393) — only `# type:` comments.

---

## 6. Builtin knowledge — the critical mechanism

This is the single most transferable piece of starpls. Three completely separate sources feed
the builtins database.

### Source 1 — `builtin.pb`, a 2.4 MB checked-in protobuf blob

**Path:** `crates/starpls/src/builtin/builtin.pb` (2,452,170 bytes, `.gitattributes` marks
`*.pb binary linguist-vendored`).

**Schema:** `crates/starpls_bazel/data/builtin.proto` (108 lines), a verbatim copy of Bazel's
`src/main/protobuf/builtin.proto`, header "Copyright 2018 The Bazel Authors … The API exporter
is used for code completion in Cider." Full schema:

```proto
package builtin;
message Builtins { repeated Type type = 1; repeated Value global = 2; }
message Type     { string name = 1; repeated Value field = 2; string doc = 3; }
enum ApiContext  { ALL = 0; BZL = 1; BUILD = 2; }
message Value    { string name = 1; string type = 2; Callable callable = 3;
                   string doc = 4; ApiContext api_context = 5; }
message Callable { repeated Param param = 1; string return_type = 2; }
message Param    { string name = 1; string type = 2; string doc = 3;
                   string default_value = 4; bool is_mandatory = 5;
                   bool is_star_arg = 6; bool is_star_star_arg = 7; }
```

Note: `Value.type` and `Param.type` and `Callable.return_type` are **plain strings**, not
structured types. starpls has to reparse them with `parse_type_ref()`
(`typeck/builtins.rs:1056-1108`), which handles Bazel doc-prose forms like
`"string; or None"`, `"sequence of strings"`, `"List of Labels"`.

**Where it comes from:** Bazel's own build. In `bazelbuild/bazel`,
`src/main/java/com/google/devtools/build/lib/BUILD:305`:

```python
genrule(
    name = "gen_api_exporter",       # (target name in that BUILD file)
    srcs = [ "//src/main/java/com/google/devtools/build/docgen:bazel_link_map",
             "//src/main/starlark/docgen:gen_be_{proto,java,cpp,objc,python,shell}_stardoc_proto",
             ":docs_embedded_in_sources" ],
    outs = ["builtin.pb"],
    cmd = "$(location //src/main/java/com/google/devtools/build/docgen:api_exporter)"
          " --output_file=$@"
          " --link_map_path=$(location //src/main/java/com/google/devtools/build/docgen:bazel_link_map)"
          " --provider=com.google.devtools.build.lib.bazel.rules.BazelRuleClassProvider"
          " --input_root=$$PWD --input_dir=$$PWD/src/main/java/com/google/devtools/build/lib"
          " --be_stardoc_proto=…",
    tools = ["//src/main/java/com/google/devtools/build/docgen:api_exporter"],
)
```

So: `bazel build //src/main/java/com/google/devtools/build/lib:builtin.pb` inside a Bazel
source checkout, then copy the artifact in. **There is no script, Makefile, xtask, or CI job
in starpls that does this.** It is a manual `git add` of a binary blob. Grep for `builtin.pb`
in all `.bzl`/`.bazel`/`.sh`/`.yml`/`.md` returns exactly one hit: `compile_data =
[":src/builtin/builtin.pb"]` in `crates/starpls/BUILD.bazel:9`.

**Version pinning:** the blob is pinned to whatever Bazel it was generated from. History shows
only two updates ever:

- `e4b563c` (2024-11-28) — original, 984,606 bytes
- `5d900cf` (2024-12-27) "chore: update builtins proto (#365)" — 2,452,170 bytes

At `5d900cf`, `.bazelversion` was **`8.0.0`**. So **the shipped `builtin.pb` describes Bazel
8.0.0's API surface.** Confirmations: `strings builtin.pb | grep -x package_metadata` → 0
matches, `applicable_licenses` → 0 matches (hence issue #410, "Unexpected keyword argument
`applicable_licenses`" when opening Bazel's own `BUILD` file), and `attr.label_list_dict`
(Bazel 9) is absent. Open issue #420 "Add Bazel 9 builtins" (2026-01-23), open PR #415
"Update builtin.pb" (2025-12-05).

**How it's loaded** — `crates/starpls/src/server.rs:365-370`:

```rust
pub(crate) fn load_bazel_builtins() -> Builtins {
    let data = include_bytes!("builtin/builtin.pb");
    // We want to crash if the bundled protobuf file is ever invalid.
    decode_builtins(&data[..]).expect("bug: invalid builtin.pb")
}
```

`include_bytes!` — so the 2.4 MB is baked into the binary, decoded at startup with
`prost::Message::decode`. Rust bindings come from `crates/starpls_bazel/build.rs`
(`prost_build::compile_protos(&["data/builtin.proto", "data/build.proto"], &["data/"])`) under
Cargo, or `rust_prost_library` under Bazel (`#[cfg(bazel)]` switch in
`crates/starpls_bazel/src/lib.rs:18-36`).

**How it's turned into types** — `builtin_types_query` / `builtin_globals_query` /
`builtin_providers_query` (`typeck/builtins.rs:602-848`), each a `#[salsa::tracked]` query on
the `BuiltinDefs` input. Post-processing worth noting:

- `BUILTINS_TYPES_DENY_LIST` (15 entries: `bool, dict, float, int, list, string, struct,
  Attribute, Target, tuple, NoneType, …`) and `BUILTINS_VALUES_DENY_LIST` (27 entries: `abs,
  all, any, len, print, range, sorted, zip, …`) — these names are dropped from the proto and
  taken from `intrinsics.rs` instead, so the pure-Starlark core is hand-maintained.
- The `"native"` type is special-cased: all rules from `bazel info build-language` are
  attached to it as methods, *plus* the WORKSPACE-only globals `register_toolchains` etc. with
  a comment admitting "This is technically incorrect if bzlmod is enabled."
- `maybe_field_type_ref_override` and a hardcoded `"Label" => "Label"` return-type override
  patch known-wrong proto entries.
- `indexable_by` is a two-entry hardcode: only `ToolchainContext[string] -> ToolchainInfo`,
  with "TODO: Audit Bazel docs for other indexable builtin types."

### Source 2 — nine hand-written JSON stub files

`crates/starpls_bazel/data/*.json`, loaded with `include_str!` + serde in
`crates/starpls_bazel/src/env.rs`. The comment at `env.rs:79` is the giveaway:

> `/// The builtin.pb file is missing 'module_extension', 'repository_rule' and 'tag_class'.`

| File | Size | Globals defined |
|---|---|---|
| `bzl.builtins.json` | 8.5 KB | `licenses, module_extension, repository_rule, tag_class` |
| `build.builtins.json` | 3.4 KB | `package` |
| `module-bazel.builtins.json` | 29.7 KB | `archive_override, bazel_dep, git_override, include, inject_repo, local_path_override, module, multiple_version_override, override_repo, register_execution_platforms, register_toolchains, single_version_override, use_extension, use_repo, use_repo_rule` |
| `repo.builtins.json` | 2.8 KB | `ignore_directories, repo` |
| `workspace.builtins.json` | 3.8 KB | `bind, register_execution_platforms, register_toolchains, workspace` |
| `cquery.builtins.json` | 1.2 KB | `build_options, providers` |
| `vendor.builtins.json` | 1.2 KB | `ignore, pin` |
| `missingModuleFields.json` | 8.9 KB | per-type extra fields patched into `builtin.pb` types |
| `commonAttributes.json` | 12.4 KB | `{build: [Attribute], repository: [Attribute]}` — `name`, `visibility`, `tags`, `deps`, … |

**The entire MODULE.bazel surface is hand-transcribed JSON**, with docs copy-pasted from
bazel.build. This is why `git_override`'s `tag` argument needed PR #402 and its `branch`
needed #377, and why `override_repo` needed #391 — each is a manual JSON edit. Issue #421
("Fails to find fields on `git_override` (likely others)") is the direct consequence.

### Source 3 — `bazel info build-language`, at runtime

`crates/starpls_bazel/src/client.rs:70-72`:

```rust
fn build_language(&self) -> anyhow::Result<Vec<u8>> { self.run_command(["info", "build-language"]) }
```

Returns a `blaze_query.BuildLanguage` protobuf (schema vendored at
`crates/starpls_bazel/data/build.proto`, 545 lines). `decode_rules`
(`crates/starpls_bazel/src/build_language.rs`) converts each `RuleDefinition` into a
`builtin::Value` with a synthetic `Callable` whose params are the rule's attributes, filtering
out `$`- and `:`-prefixed implicit attrs. This is what gives you `cc_library`, `java_binary`,
`genrule` **with the actual attribute set of the user's own Bazel version**, and it's stored
as the second field of `BuiltinDefs` (`rules`).

Attribute discriminators are mapped to type strings by
`attribute_type_string_from_discriminator`; `StringListDict` and `LabelKeyedStringDict` both
fall through to `"Unknown"` with TODOs.

**Risk:** `bazel info build-language` is `@Deprecated` in Bazel HEAD —
`src/main/java/com/google/devtools/build/lib/runtime/commands/info/BuildLanguageInfoItem.java:53-58`:
"Info item for the build language. It is deprecated, it still works, when explicitly
requested, but are not shown by default." Anything we build should not depend on it long-term.

### Summary of the builtins architecture

```
builtin.pb (2.4 MB, Bazel 8.0.0, include_bytes!, manual update)  ─┐
9 × *.builtins.json (hand-written, include_str!)                 ─┼→ BuiltinDefs (salsa::input)
`bazel info build-language` (runtime, per-workspace, deprecated) ─┘        │
                                                                            ↓
                             builtin_types_query / builtin_globals_query / builtin_providers_query
                                                                            ↓
                                        FxHashMap<String, Ty>  +  per-APIContext global pools
```

The pure-Starlark core (`len`, `str.format`, `list.append`, `dict.items`, …) is a fourth,
entirely separate source: `typeck/intrinsics.rs`, 1913 lines of hand-written Rust with
Markdown docs inlined, mirroring the Starlark spec.

---

## 7. Bazel integration

### Discovery: it shells out to `bazel`, at startup, synchronously

`crates/starpls_bazel/src/client.rs` is the only place that spawns processes
(`std::process::Command::new(&self.executable)` at line 57). The `BazelClient` trait:

```rust
pub trait BazelClient: Send + Sync + 'static {
    fn build_language(&self) -> anyhow::Result<Vec<u8>>;
    fn info(&self) -> anyhow::Result<BazelInfo>;
    fn resolve_repo_from_mapping(&self, apparent_repo: &str, from_repo: &str) -> anyhow::Result<Option<String>>;
    fn clear_repo_mappings(&self);
    fn null_query_external_repo_targets(&self, repo: &str) -> anyhow::Result<()>;
    fn repo_mapping_keys(&self, from_repo: &str) -> anyhow::Result<Vec<String>>;
    fn query_all_workspace_targets(&self) -> anyhow::Result<Vec<String>>;
    fn fetch_repo(&self, repo: &str) -> anyhow::Result<()>;
    fn dump_repo_mapping(&self, repo: &str) -> anyhow::Result<HashMap<String, String>>;
}
```

Exact commands issued:

| Purpose | Command |
|---|---|
| workspace discovery | `bazel info execution_root output_base release starlark-semantics workspace` |
| native rules | `bazel info build-language` |
| repo mapping | `bazel mod --enable_bzlmod dump_repo_mapping <repo>` |
| fetch a repo | `bazel fetch --repo @@<repo>` |
| force-materialise a repo (non-bzlmod) | `bazel query --keep_going @@<repo>//...` |
| all targets (label completion) | `bazel query "kind('.* rule', ...)"` |

Startup (`crates/starpls/src/bazel.rs`, `BazelContext::new`) runs **`bazel info` + `bazel mod
dump_repo_mapping ""` + `bazel info build-language` synchronously before the server is
usable**. If any fails, the whole context is `Default::default()` and the client gets a
`ShowMessage` error: "Failed to fetch Bazel configuration!". Open issues #310 ("recover if
initial `bazel info` calls fail"), #98/#364 (send progress while these run), #418
(`bazel info` fails with "at most one key may be specified" on some setups), #407 (label
completions hold the Bazel lock on every BUILD file change).

`workspace_name` is derived by taking the basename of `execution_root` and discarding
`__main__` / `_main` — with an inline credit to bazel-lsp:

```rust
// Taken from https://github.com/cameron-martin/bazel-lsp/blob/92644f2.../src/workspace.rs#L24.
workspace_name = PathBuf::from(value).file_name().and_then(|file_name| { … });
```

bzlmod detection (`bazel.rs:30-53`) is a string prefix match on the `release` line against
`["development", "release 7", "release 8", "release 9"]`, overridden by
`enable_bzlmod=true|false` appearing in the `starlark-semantics` string. The code comment
says: "Just hardcoding this for now since I'm lazy to parse the actual versions. This should
last us pretty long since Bazel 9 isn't anywhere on the horizon." Bazel 9 shipped; PR #408
patched in `"development"`, PR #433 (open) fixes it properly.

There is also a **pure-filesystem** workspace finder used when no Bazel is involved,
`starpls_bazel::resolve_workspace` (`crates/starpls_bazel/src/lib.rs:147-176`): walk ancestors
until you see `WORKSPACE` / `WORKSPACE.bazel` / `MODULE.bazel` / `REPO.bazel`, remembering the
nearest ancestor containing `BUILD` / `BUILD.bazel` as the package root. Returns
`(workspace_root, package_dir)`.

### Label parsing

`crates/starpls_bazel/src/label.rs`, 484 LOC, zero-copy (`Label<'a>` stores byte offsets into
the source string). `RepoKind ∈ { Canonical (@@), Apparent (@), Current (none) }`. It
distinguishes `is_relative`, `has_leading_slashes`, `has_target_shorthand` (`//foo/bar` ⇒
target `bar`). It also has a **partial-parse mode**: `Err(PartialParse { partial, err })`
returns the best-effort `Label` alongside `ParseError ∈ { InvalidRepo, InvalidPackage,
InvalidPackageEndingSlash, InvalidTarget, EmptyPackage, EmptyTarget }` — this is what powers
completion inside a half-typed label. Repo names accept `~` after the first char (canonical
Bazel 6/7 repo names).

### `@foo//bar:baz` → filesystem path

`DefaultFileLoader::resolve_label` (`crates/starpls/src/document.rs:257-324`):

```
RepoKind::Apparent && bzlmod_enabled:
    from_repo   = repo_for_path(path_of(from))          // "" if under workspace,
                                                        // else first component under $output_base/external
    canonical   = bazel mod dump_repo_mapping <from_repo> |> lookup(apparent)
    root        = canonical.is_empty() ? workspace : $output_base/external/<canonical>
    on miss     → bail "Could not resolve repository \"@x\" from current repository mapping"

RepoKind::Canonical | Apparent (non-bzlmod):
    root = (repo == workspace_name || repo == "") ? workspace : $output_base/external/<repo>

RepoKind::Current:
    (root, package) = resolve_workspace(path_of(from))  // filesystem ancestor walk

resolved = (label.is_relative() ? package : root.join(label.package())).join(label.target())
```

Repo mappings are cached in `BazelCLI.repo_mappings: RwLock<HashMap<String, HashMap<String,
String>>>` and **cached even on failure** (`client.rs:142-148`). Saving `MODULE.bazel` /
`WORKSPACE*` clears the cache (`handlers/notifications.rs:did_save_text_document`); the README
warns that adding a new dep still requires a server restart.

Lazy repo fetching: if `fs::read_to_string` fails on a path under `$output_base/external/<r>`
and that directory doesn't exist, `maybe_intern_file` sends a
`Task::FetchExternalRepoRequest` on a channel; the event loop batches these and runs
`bazel fetch --repo @@<r>` (bzlmod) or `bazel query --keep_going @@<r>//...` (WORKSPACE) on
the task pool, reporting `$/progress`.

`load()` is restricted to `.bzl`: `if !label.target().ends_with(".bzl") { bail!("cannot load a
non-bzl file") }` (`document.rs:462-464`).

`resolve_path` (for goto-def on a *label string* in a BUILD file) first tries the literal path
as a file; if that's not a file, it looks for a sibling `BUILD`/`BUILD.bazel` in the parent
directory and returns `ResolvedPath::BuildTarget { build_file, target, contents }`. There is a
`TODO` at `document.rs:625` for targets like `//foo:bar/baz.bzl`, which is what open PR #436
fixes.

### What Bazel integration does *not* do

No BEP/BES, no `aspects`, no `--output=proto` query parsing beyond `build-language`, no
`MODULE.bazel.lock` reading (open issue #429 notes the lock file churns), no `.bazelrc`
parsing, no BCR awareness, no `bazel_dep` → registry goto-def (open PR #416 adds module
extension goto-def), no `buildifier`/`buildozer` invocation (open PRs #401 formatting, #413
buildozer query).

---

## 8. LSP surface

Transport: stdio via `lsp-server` 0.7.5. Capabilities are declared once, statically, in
`crates/starpls/src/commands/server.rs:58-77`:

```rust
ServerCapabilities {
    completion_provider: Some(CompletionOptions {
        trigger_characters: Some(['.', '"', '\'', '/', ':', '@']), ..Default::default() }),
    declaration_provider: Some(DeclarationCapability::Simple(true)),
    definition_provider: Some(OneOf::Left(true)),
    document_symbol_provider: Some(OneOf::Left(true)),
    hover_provider: Some(HoverProviderCapability::Simple(true)),
    references_provider: Some(OneOf::Left(true)),
    signature_help_provider: Some(SignatureHelpOptions {
        trigger_characters: Some(['(', ',', ')']), ..Default::default() }),
    text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::INCREMENTAL)),
    ..Default::default()
}
```

That's **eight** capabilities. Everything else is `Default::default()`, i.e. absent:
no `documentFormattingProvider`, no `renameProvider`, no `codeActionProvider`, no `codeLens`,
no `semanticTokensProvider`, no `inlayHintProvider`, no `workspaceSymbolProvider`, no
`documentHighlightProvider`, no `foldingRangeProvider`, no `selectionRangeProvider`, no
`callHierarchy`, no `workspace.workspaceFolders`, no `diagnosticProvider` (diagnostics are
push-only via `textDocument/publishDiagnostics`).

Registered handlers (`crates/starpls/src/event_loop.rs:210-230`):

| Method | Handler | Real behaviour |
|---|---|---|
| `textDocument/completion` | `requests::completion` | Six contexts, see below. |
| `textDocument/documentSymbol` | `requests::document_symbols` | Module-level `Function`/`Variable` from the scope tree; **in BUILD files additionally every top-level `CallExpr` with a `name = "..."` kwarg, emitted as `:<name>`** with `SymbolKind::Variable`. Flat list, no `children`. |
| `textDocument/definition` | `requests::goto_definition` | See below. |
| `textDocument/declaration` | `requests::goto_declaration` | Same code path with `skip_re_exports = true` — chases `load`-then-reassign chains to the original definition (`try_resolve_re_export`, added #395, 2025-06-28). |
| `textDocument/hover` | `requests::hover` | Markdown. Keyword docs for `break/continue/def/for/if/load/pass/return` (hardcoded in `hover/docs.rs`); otherwise ```` ```python ```` block with `(field)`/`(method)`/`(variable)` prefix, the rendered type, and the doc string. |
| `textDocument/references` | `requests::find_references` | **Single file only.** `memchr::memmem::Finder` scans the current buffer's bytes for the identifier, then for each hit resolves the token's scope and compares `ScopeDef` identity. No cross-file search, no workspace index. Added 2024-12-01 (#324); issue #125 "Find references" is still open. |
| `textDocument/signatureHelp` | `requests::signature_help` | Finds the enclosing `CallExpr`, resolves it to a `Callable` (function / rule / provider / tag / macro), renders `name(p1: T1, p2: T2 = d) -> R` and sets `active_parameter` via `resolve_call_expr_active_param`. |
| `starpls/showSyntaxTree` | `requests::show_syntax_tree` | Custom debug request → rendered rowan tree. |
| `starpls/showHir` | `requests::show_hir` | Custom debug request → rendered HIR. |

Notifications: `didOpen`, `didClose`, `didChange` (incremental, `apply_document_content_changes`
in `utils.rs`), `didSave`. `didSave` is load-bearing: saving `BUILD`/`BUILD.bazel` triggers
`refresh_all_workspace_targets()` (re-runs `bazel query`); saving `MODULE.bazel` /
`WORKSPACE*` clears the repo-mapping cache and the fetched-repo set.

Server→client: `textDocument/publishDiagnostics`, `window/showMessage`,
`window/workDoneProgress/create` + `$/progress` (for "Fetching external repositories" and
"Refreshing workspace targets").

Completion contexts (`crates/starpls_ide/src/completions.rs:136-370`). The trick used to get a
usable parse at the cursor is at line 452: **"Reparse the file with a dummy identifier
inserted at the current offset"**, then locate the corresponding node in the modified tree.

1. `NameRef` — in-scope names + intrinsics + `APIContext`-appropriate builtins + keywords
   (only if the expression is a lone statement) + `param = ` snippets for the enclosing call.
2. `Name::Dot` — fields and methods of the receiver's inferred type.
3. `Type` — inside a `# type:` comment: the builtin type names.
4. `String::LoadModule` — directories/`.bzl` files, via `db.list_load_candidates`; under
   bzlmod the apparent-repo list comes from `bazel mod dump_repo_mapping ""`, otherwise from
   `read_dir($output_base/external)`. Uses `InsertReplaceEdit` when the client supports it.
5. `String::LoadItem` — exported (non-`_`) symbols of the already-resolved loaded module.
6. `String::DictKey` — `known_keys` of a `TyKind::Dict` with a literal.
7. `String::Label` — **only with `--experimental_enable_label_completions`**; filters the
   cached `bazel query "kind('.* rule', ...)"` output by prefix, offering package folders then
   target names.

Completion ordering is by a `CompletionRelevance` enum baked into `sort_text`.

Goto-definition dispatch (`crates/starpls_ide/src/goto_definition.rs:53-73`):

- `NameRef` → scope resolution; `LoadItem` defs are followed into the loaded file.
- `Name` under `DotExpr` → struct field (jumps to the `struct()` kwarg), provider field (jumps
  into the `fields = {}` dict literal).
- `Name` under `KeywordArgument` → **if the callee is a rule, jumps to the key in its
  `attrs = {}` dict**; otherwise to the parameter declaration.
- `LoadModule` → the loaded file (position 0:0).
- `LoadItem` → the definition in the loaded file.
- `LiteralExpr` (a string) → `db.resolve_path`; for `ResolvedPath::BuildTarget` it *scans the
  target BUILD file's top-level `CallExpr`s for `name = "<target>"`* and returns that call's
  range, falling back to 0:0. This is the label goto-def, and it's a linear AST scan per
  request, not an index.

The `check` subcommand (`crates/starpls/src/commands/check.rs`, 330 LOC) reuses the whole
`Analysis` stack headlessly, walks paths with `walkdir`, and renders diagnostics with
`annotate-snippets` — a usable CLI type checker / lint gate.

---

## 9. Testing strategy

176 `#[test]` functions total. All in-crate (`#[cfg(test)] mod tests`), no `tests/` directory,
no integration test that speaks LSP over a pipe.

| Crate | Tests | Style |
|---|---|---|
| `starpls_hir` | 81 | `expect_test::expect![[...]]` inline snapshots (54 in `typeck/tests.rs`, 13 in `def/tests.rs`, 13 CFG pretty-printer tests in `def/codeflow.rs`) |
| `starpls_ide` | 50 | fixture markup + `expect!` / `assert_eq!` |
| `starpls_bazel` | 24 | plain unit tests, mostly the label parser |
| `starpls_lexer` | 19 | `expect!` inline snapshots |
| `starpls_parser` | 2 | two driver tests over 15 on-disk `.star`/`.rast` file pairs |

**Parser: file-based snapshot tests.** `crates/starpls_parser/test_data/{ok,err}/*.star` (8 ok,
7 err) with a sibling `*.rast` expectation containing the indented tree dump plus
`error <pos>: <message>` lines. `test_parse_ok` / `test_parse_error` iterate the directory;
`TEST_FILTER` env var narrows. `.gitattributes` forces `*.star text eol=lf` so snapshots are
stable on Windows. Regenerate with `cargo xtask update-parser-test-data`. Works under both
Cargo (`CARGO_MANIFEST_DIR`) and Bazel (`runfiles::find_runfiles_dir()` → `_main/crates/...`).

**Type inference: inline expect-tests of the whole expression map.**
`check_infer(input, expect![[...]])` builds a `TestDatabase` with fake builtins
(`TestDatabaseBuilder::add_function("rule")`, `add_type(FixtureType::new("ctx", …))`), then
prints *every* expression in the source map sorted by containment, as
`start..end "text": <type>`, followed by all diagnostics. Example
(`typeck/tests.rs:754-776`):

```
foo = struct(a = 1, b = "bar")
foo.a
```
```
1..4   "foo": struct
7..13  "struct": def struct(*args, **kwargs) -> Unknown
18..19 "1": Literal[1]
7..31  "struct(a = 1, b = \"bar\")": struct
32..37 "foo.a": Literal[1]
```

Variants: `check_infer_with_code_flow_analysis`, `check_infer_with_unused_definitions`.

**IDE features: inline cursor/selection markup.** `starpls_test_util` defines
`CURSOR_MARKER = "$0"` and a `#^^^` under-line selection marker:

```rust
check_goto_definition(r#"
foo = 1
#^^
f$0oo
"#);
```

`find_selected_ranges` maps each `#^^^` back onto the *previous* line's byte range, and
`assert_eq!(fixture.selected_ranges, actual)` compares. `Fixture::from_single_file` /
multi-file fixtures feed a `SimpleFileLoader` backed by a `DashMap`.

CI: `.github/workflows/build.yml` runs `bazel build //...` + `bazel test
--build_tests_only //...` on ubuntu-latest, and `//crates/...` on windows-latest, with a
BuildBuddy remote cache. **CI is Bazel-only — `cargo test` is not run in CI**, which is
consistent with `vendor/runfiles`' unit tests failing to compile under Cargo (see §11).

---

## 10. Gaps

47 `TODO`/`FIXME` comments; **zero `unimplemented!()` and zero `todo!()`**. Concentrations:
`typeck/intrinsics.rs` (10), `typeck/infer.rs` (8), `typeck.rs` (7), `starpls_bazel/attr.rs` (4).

The load-bearing ones, verbatim:

```rust
typeck/intrinsics.rs:222  // TODO: Many of these signatures are wrong since the implementation of
                          // Starlark's type system is still heavily WIP. … We also still need to
                          // support features like optional arguments, keyword-only parameters,
                          // union types, "traits" like Sequence[T], function overloads, and so on.
typeck.rs:2001            // TODO: This function currently assumes that all types are covariant in their arguments.
typeck.rs:2056            // TODO: The logic below also temporarily allows assignments like `int | None` to `int`.
                          // Fix this once we support type guards.
typeck.rs:1412            // TODO: This should be the `Label` type, maybe we should retrieve it from the builtins?
typeck.rs:902, :921       // TODO: Handle lambda parameters.
typeck.rs:1788            // TODO: Need to resolve based on the dialect, but unclear how to get that information
infer.rs:1688             FlowNode::Loop { .. } => break 'outer None, // TODO: Correctly handle loops.
infer.rs:1307             // TODO: Strip None from optional types once we implement narrowing.
infer.rs:1593             // TODO: We should eventually apply narrowing to the effective type here.
infer.rs:2196             // TODO: This is potentially super slow.
call.rs:378               // TODO: Emit diagnostics for invalid parameters.
builtins.rs:818           // TODO: Audit Bazel docs for other indexable builtin types.
bazel.rs:32               // TODO: Just hardcoding this for now since I'm lazy to parse the actual versions.
                          // This should last us pretty long since Bazel 9 isn't anywhere on the horizon.
document.rs:625           // TODO: Handle targets like in `//foo:bar/baz.bzl`.
build_language.rs:68,70   // TODO: Handle StringListDict. / Handle LabelKeyedStringDict.
```

Roadmap items still unchecked in the README: rule attributes in goto-definition, dataflow
analysis, nested local repositories.

Issue tracker, grouped (all still open as of 2026-08-25):

**Bazel-version drift** — #420 Bazel 9 builtins (`attr.label_list_dict`); #410
`applicable_licenses`/`package_metadata` missing from `builtin.pb`; #421 `git_override` fields;
#418 `bazel info` failure; #429 lock-file churn.

**Type system** — #298 align with the "Bootstrapping Starlark types" proposal; #171 fix Union
interactions; #87 typecheck `return`; #100 `Callable` protocol; #393 Python3-style annotations;
#417 infer struct field types returned by a function; #386 borrow type from the `if`-else
branch for empty lists; #406 union-of-functions is not callable; #257 keyword-only params in
type comments; #244 providers missing from the proto; #214 `py_library` signature; #388
symbolic-macro `rule` assignability; #373 label attrs in `repository_rule` impls; #264
provider types in type-comment completion.

**Correctness** — #405 bogus unused-variable in loops; #367 same for redeclarations in loops;
#422 false "lone slash in escape sequence"; #371 stack overflow fetching builtin rules; #21
conflicting declarations; #103 eager circular-import handling.

**Missing features** — #267 `textDocument/formatting` (PR #401 open since 2025-08-11); #125
find references (workspace-wide); #43 inlay hints; #232 LSIF/SCIP; #225 multi-workspace;
#376 Bazel Starlark debug protocol; #355 goto-def for tag classes/attrs; #100 goto Bazel rule;
#265/#266 auto-import of `.bzl`; #385 file completion; #379 custom stubs; #261 `module_ctx`;
#260 `ctx` in aspect impls; #275 fetch external repo on incomplete `load` label.

**Engineering** — #370 support stable Rust; #45 shard `InferenceCtxt`; #64/#320 automate
releases; #58 tests for IDE features; #336 documentation website; #98/#364 progress
notifications for Bazel commands; #310 recover from failed `bazel info`;
#407 label completions hold the Bazel lock on every keystroke; #129 audit `Name` usage.

Architectural gaps not tracked as issues but visible in the source:

- **No workspace index.** `find_references` is single-file; there is no workspace symbol
  provider; label goto-def re-scans the target BUILD file's AST every request.
- **No rename.** Consequently no "rename a target and rewrite every referring label" — which
  is a headline feature in our scope.
- **`select()` is unmodelled.** Not a special form, not a type; no `config_setting` key
  validation, no `//conditions:default`, no `select() + [...]`.
- **`depset` is unparameterised.** No element type.
- **Inference isn't a salsa query**, so cross-file incrementality is coarse (cancel-everything).
- **`load()` edges aren't salsa dependencies**, so cross-file invalidation is not tracked.
- **Builtins are a hand-curated snapshot**, not derived from the user's Bazel.

---

## 11. Build

**Result: builds clean.** Binary at `target/release/starpls`, 12,862,952 bytes,
`starpls version: v0.1.22`.

```
$ cargo clean && time cargo build --release
   Finished release [optimized] target(s) in 34.57s
real 0m34.627s   user 3m1.004s   sys 0m16.318s
```

34.6 s wall (3 m 01 s CPU) for a full clean release build on an Apple-Silicon Mac with a warm
Cargo registry. Dependency download on the very first run added ~20 s.

Toolchain actually required, and two environment gotchas:

1. **`rust-toolchain.toml` pins `channel = "nightly-2023-12-06"`** (rustc 1.76.0-nightly,
   `e9013ac0e`). `rustup` auto-downloaded it (6 components). README says nightly is needed
   "due to usage of `trait_upcasting` as specified by RFC 3324".

   **Finding: that pin is obsolete.** Trait upcasting stabilised in Rust 1.86. I verified the
   whole workspace type-checks on today's stable:

   ```
   $ rustc +stable --version
   rustc 1.98.0 (88d9e12ae 2026-08-18)
   $ cargo +stable check --release -p starpls
       Finished `release` profile [optimized] target(s) in 31.47s
   ```

   Only 8 warnings (dead code, one elided-lifetime lint). So open issue #370 "Support stable
   rust" is already satisfied by rustc itself; it just needs the pin deleted. Open PR #434
   does exactly this and is unmerged.

2. **`protoc` is required** and is not vendored. `crates/starpls_bazel/build.rs` calls
   `prost_build::compile_protos(&["data/builtin.proto", "data/build.proto"], &["data/"])`.
   Without it:

   ```
   thread 'main' panicked at prost-build-0.12.3/src/lib.rs:1521:
   Could not find `protoc` installation and this build crate cannot proceed without this knowledge.
   ```

   Fixed with `PROTOC=$(nix build --print-out-paths nixpkgs#protobuf)/bin/protoc`
   (protobuf 35.1). Open PR #431 "Use prebuilt protoc" addresses this for the Bazel build.

3. **On nix-darwin, the default `cc` is nix GCC 15.3.0, which cannot link against libiconv:**

   ```
   error: linking with `cc` failed: exit status: 1
     = note: ld: library not found for -liconv
             collect2: error: ld returned 1 exit status
   error: could not compile `starpls_bazel` (build script) due to 1 previous error
   ```

   Fixed with `RUSTFLAGS="-C linker=/usr/bin/cc" CC=/usr/bin/cc` (Apple clang). This is an
   environment issue, not a starpls issue.

Full working invocation:

```sh
cd upstream/starpls
PROTOC=/nix/store/…-protobuf-35.1/bin/protoc \
RUSTFLAGS="-C linker=/usr/bin/cc" CC=/usr/bin/cc CXX=/usr/bin/c++ \
cargo build --release
```

Tests: `cargo test --release --workspace` **fails**, but only in the vendored runfiles crate:

```
error: environment variable `REPOSITORY_NAME` not defined at compile time
  --> vendor/runfiles/src/lib.rs:47:34
error: could not compile `runfiles` (lib test) due to 4 previous errors
```

`vendor/runfiles`' own unit tests are Bazel-only. Excluding it, everything passes:

```
$ cargo test --release --workspace --exclude runfiles
starpls_bazel  24 passed
starpls_hir    81 passed
starpls_ide    50 passed
starpls_lexer  19 passed
starpls_parser  2 passed
──────────────────────
176 passed; 0 failed;  real 0m44.114s   EXIT=0
```

The Bazel build (`bazel build //...`, `.bazelversion` = 8.2.1, bzlmod, `rules_rust` 0.53.0,
`hermetic_cc_toolchain` for cross-compiling to linux/aarch64) was not attempted — it needs
`pnpm install` in `editors/code` and would exceed the time budget.

---

## 12. What to take, what to avoid

**Take:**

- Lexer → event-stream parser → rowan builder split. Trivia-free parsing with lossless
  reassembly is the right shape for an editor, and starpls's version is ~1.4k LOC.
- `SyntaxKindSet` bitsets + a single `error_recover_until` primitive: cheap, effective
  recovery.
- Parsing `# type:` comments into real CST nodes inside the COMMENT token.
- `Dialect` × `APIContext` as the dialect model, with filename-based classification and
  per-context global pools. Eight contexts covers Bazel's reality.
- Literal types (`Int(Some(3))`, `String(Some(s))`, `DictLiteral.known_keys`) — these are what
  make `rule(attrs = {...})` and `provider(fields = {...})` statically analysable. Without
  them there is no Bazel-aware type checking.
- `maybe_unique_ret_type` as a single, greppable dispatch table for Bazel's magic constructors.
- The three-source builtins design, especially `bazel info build-language` for native rules —
  it's the only part that tracks the user's actual Bazel version.
- Zero-copy label parser with partial-parse recovery for completion.
- expect-test snapshots of the *entire* expression→type map; `$0` / `#^^^` fixture markup.

**Avoid:**

- The dead `salsa-2022` personal fork.
- Type inference outside salsa, behind one global mutex, with hand-rolled caches and
  panic-based cancellation. Make inference a query.
- `load()` resolution as an untracked `dyn Db` method — model file loading as a salsa input so
  cross-file edges are real dependencies.
- A 2.4 MB `include_bytes!` blob updated by hand twice in two years. Generate it, pin it to a
  named Bazel version, and check the version at runtime — or better, derive it from the user's
  Bazel.
- Hand-transcribing the MODULE.bazel API into JSON.
- Blocking startup on three synchronous `bazel` invocations.
- Depending on `bazel info build-language`, which is `@Deprecated` upstream.
- Shipping without `select()`, rename, workspace symbols, or a workspace-wide reference index —
  those are the features that make a Bazel LSP feel real, and they're exactly what starpls
  lacks.
