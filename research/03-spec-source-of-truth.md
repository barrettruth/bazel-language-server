# 03 — Is there a single source of truth for the Bazel build language?

**No.** There is no artifact — document, grammar, or schema — that defines the language
Bazel 9.2.0 actually accepts. What exists is:

| Layer | Nearest thing to a spec | Normative for Bazel? | Last touched |
|---|---|---|---|
| (a) Starlark core | `bazelbuild/starlark/spec.md` (4838 lines) | **No.** Bazel diverges in ≥8 ways, silently | 2026-02-06 |
| (b) Bazel dialect (`rule()`, `ctx`, `depset`, …) | Java annotations `@StarlarkBuiltin`/`@StarlarkMethod` in `net/starlark/java/annot/` | **Yes — it *is* the implementation** | continuously |
| (c) BUILD dialect | `packages/DotBazelFileSyntaxChecker.java` (a 160-line AST visitor) + three `FileOptions` call sites | **Yes — it *is* the implementation** | continuously |
| Native rule inventory | nothing; changes per-workspace with bzlmod deps | n/a | n/a |

The only machine-readable descriptions of (b) are **generated from the Java source**
(`builtin.pb`, Build Encyclopedia) and are incomplete. (c) has no machine-readable
description at all. The published prose docs for all three layers contain
demonstrable factual errors (§6).

Everything below verified against `upstream/bazel` @ `3a9b19c` (2026-08-24,
`10.0.0-prerelease`), a sparse clone of tag `9.2.0` at `/tmp/bazel-9` (`8220c61`,
2026-07-13), and **a running `bazel 9.2.0`** (via `nix develop -c bazelisk`).

---

## 1. The Starlark spec: `upstream/starlark/spec.md`

### Provenance and status

- 4838 lines. Last content change **2026-02-06** (`c0af0ea`, PR #325 "Remove invalid
  example" — deleting an example that *"is rejected by Bazel and by starlark-go"*, i.e.
  the spec was wrong and the fix was to delete the evidence).
- Repo `pushed_at` 2026-02-06; **28 open PRs**, including trivial typo/example fixes
  filed 2026-07-19 → 2026-07-22 (#357–#361) that remain unmerged. `CODEOWNERS` is a
  single line: `* @brandjon`.
- `proposals/README.md` is an **empty table** — all proposals were deleted in #299
  (2025-03-04). The design process in `process.md` is dead in practice.
- `README.md:110-113`: *"In the future, this repository will contain a complete
  description of the build API used in Bazel."* Written ~2017. Never happened; layer (b)
  is explicitly out of scope per `process.md` ("Out of scope: `java_binary()`,
  `genrule()`, `js_common`, `repository_rule()`, evaluation of WORKSPACE files").

### Normativity

Not normative in any formal sense. No RFC 2119 keywords anywhere (`grep -c 'MUST\|SHOULD'`
= 0). It is descriptive prose with 10 open `TODO`s and 24 HTML-comment asides recording
unresolved disagreements. `process.md` says *"the reference implementation is
[Bazel](https://bazel.build/)"* — i.e. the spec defers to Bazel, and Bazel does not
defer to the spec. Nobody arbitrates.

Notable holes:

- **The lexer is not specified.** `spec.md:535`: `TODO: define indent, outdent,
  semicolon, newline, eof`. The single hardest part of implementing a Python-family
  parser is a TODO.
- `spec.md:871-879` (bytes): `TODO(bazelbuild/starlark#112)` — bytes methods
  (`join`, `find`, `split`, `strip`, `encode`, `decode`, `ord`, `chr`) unspecified.
- `spec.md:3131`: load resolution is *"determined by the application … and is not
  specified here"* — so the single most important construct for an LSP (goto-definition
  on `load`) is out of scope by construction.

### Grammar (EBNF)

Yes — `spec.md:4721-4838`, a Wirth-style EBNF, gradually introduced through the
document and collected at the end. It is **not machine-readable** (fenced ```text``` in
Markdown) and **not self-consistent with the prose**:

- `Statement = DefStmt | IfStmt | ForStmt | SimpleStmt` and
  `File = {Statement | newline} eof` permit top-level `if`/`for`, while the prose at
  `spec.md:3040` and `:3081` says *"An `if` statement at top level results in a static
  error"* / *"a `for` statement at top level results in a static error"*.
- Self-declared gaps (`spec.md:4834-4838`): *"Ambiguity is resolved using operator
  precedence"* (precedence table not in the grammar); *"The grammar does not enforce the
  legal order of params and args, nor that the first CompClause must be a 'for'."*
- No production for the `*` bare keyword-only marker's positional constraints, no
  string/bytes literal lexical grammar inside the collected grammar.

### Coverage of semantics

| Topic | Covered? |
|---|---|
| Type system | Only the dynamic type list (`§Data types`, `spec.md:537-1466`). **No static types.** Bazel is shipping a static type system (§2). |
| Evaluation order | Partial: argument order `spec.md:1389`; binary-operand order unspecified. |
| Mutability / freezing | Yes — `§Identity and mutation` `:1658`, `§Freezing a value` `:1713`, `§Hashing` `:1729`. This part is genuinely good. |
| `load()` | Syntax `:3110-3126`; *semantics* explicitly deferred to the host `:3131`. Binding scope **is** specified: `spec.md:1531-1538` — load creates *file-local* bindings in a "file block", distinct from the module block, and *"it is an error for a load statement to bind the name of a global"*. |
| Recursion | `:1420-1436` — dynamic error, *"an implementation may allow clients to disable this check"*. |
| Concurrency/freezing model | Yes, `:44-47`, `:1713`. |

### The conformance test suite is dead

`upstream/starlark/test_suite/BUILD` runs the testdata **only against Java**, from a
**jar vendored into the repo** (`starlark_deploy.jar`; see
`bazelbuild/bazel#25480` for de-vendoring). Go and Rust targets are commented out:

```python
# TODO(#305): Our dependency on the other interpreters is broken.
# alias(name = "go_starlark",   actual = "@net_starlark_go//cmd/starlark:starlark")
# alias(name = "rust_starlark", actual = "@starlark-rust//:starlark")
```

So the cross-implementation conformance suite — the one mechanism that could have made
the spec normative — has not run against Go or Rust since #306 (2025-03-10).

---

## 2. Divergences between implementations

### 2.1 google/starlark-go (`upstream/starlark-go`, HEAD `5395d01` 2026-07-08)

Has its **own** spec, `doc/spec.md` (4354 lines, last touched 2025-10-24 `6d2315c`),
with an explicit deviations appendix `§Dialect differences` at `doc/spec.md:4335-4354`:

```
* String interpolation supports the `[ioxXc]` conversions.
* String elements are bytes.
* Non-ASCII strings are encoded using UTF-8.
* Strings support hex byte escapes.
* Strings have the additional methods `elem_ords`, `codepoint_ords`, and `codepoints`.
* The `chr` and `ord` built-in functions are supported.
* The `set` built-in function is provided (option: `-set`).
* `set & set` and `set | set` compute set intersection and union, respectively.
* `assert` is a valid identifier.
* `if`, `for`, and `while` are permitted at top level (option: `-globalreassign`).
* top-level rebindings are permitted (option: `-globalreassign`).
```

Two of these are **now stale**: `doc/spec.md:849` and `:1991` still say *"The Java
implementation does not support sets"* — Bazel 9.2.0 ships `set()` on by default
(§2.3). Verified empirically below.

Dialect knobs live in `syntax/options.go:23-33`:

```go
type FileOptions struct {
	Set               bool // allow references to the 'set' built-in function
	While             bool // allow 'while' statements
	TopLevelControl   bool // allow if/for/while statements at top-level
	GlobalReassign    bool // allow reassignment to top-level names
	LoadBindsGlobally bool // load creates global not file-local bindings (deprecated)
	Recursion         bool // disable recursion check for functions in this file
}
```

plus deprecated globals in `resolve/resolve.go:109-120`.

There is also a **third** grammar in this repo: `syntax/grammar.txt` (129 lines), which
is stale relative to both `doc/spec.md` and `syntax/parse.go` (1096 lines) — it has no
bytes literals, no `~` unary operator, no set literals, and its `Binop` production omits
`<<`/`>>` even though `AssignStmt` lists `<<=`/`>>=`.

### 2.2 facebook/starlark-rust (`upstream/starlark-rust`, HEAD `79bc31f` 2026-08-24, v0.14.0 2026-06-01)

`docs/spec.md` is **four lines** — a pointer to `bazelbuild/starlark/spec.md`. The
deviation list is three bullets in `README.md`:

> - We have plenty of extensions, e.g. type annotations, recursion, top-level `for`.
> - We don't yet support later additions to Starlark, such as
>   [bytes](https://github.com/facebook/starlark-rust/issues/4).
> - In some cases creating circular data structures may lead to stack overflows.

The real knob list is `starlark_syntax/src/dialect.rs:36-71` (9 fields:
`enable_def`, `enable_lambda`, `enable_load`, `enable_keyword_only_arguments`,
`enable_positional_only_arguments`, `enable_types` (tri-state
`Disable`/`ParseOnly`/`Enable`), `enable_load_reexport`, `enable_top_level_stmt`,
`enable_f_strings`).

`Dialect::Standard` (`dialect.rs:86-97`) claims to *"Follow the Starlark language
standard as much as possible"* yet sets:

- `enable_keyword_only_arguments: false` — the spec **has** keyword-only args
  (`spec.md:1328`, added #248 2023-03-16). starlark-rust's "standard" rejects them.
- `enable_load_reexport: true, // But they plan to change it` — **directly contradicts
  `spec.md:1531-1538`** and contradicts Bazel (verified: Bazel errors
  `file ':b.bzl' does not contain symbol 'X'`).

So the three implementations' three "standard" configurations disagree with the spec and
with each other, and each documents its deviations in a different place, in a different
format, with different staleness.

### 2.3 Bazel's Java Starlark (`src/main/java/net/starlark/java/`)

**There is no deviations document.** `net/starlark/java/annot/README.md` is four lines
and says nothing about the language. Bazel's dialect knobs are
`net/starlark/java/syntax/FileOptions.java:39-139` (8 fields):

```java
allowLoadPrivateSymbols()  allowToplevelRebinding()  loadBindsGlobally()
requireLoadStatementsFirst()  stringLiteralsAreAsciiOnly()
allowTypeSyntax()  resolveTypeSyntax()  tolerateInvalidTypeExpressions()
```

The class comment concedes the point: *"These options affect the language accepted by
the frontend (in effect, the dialect)"*.

Divergences from `bazelbuild/starlark/spec.md`, established from source and confirmed by
running Bazel 9.2.0:

| # | Spec says | Bazel 9.2.0 does | Evidence |
|---|---|---|---|
| 1 | `b"..."` bytes literals (`spec.md:496-526`) | no bytes type at all | `Lexer.java` has no `b`-prefix string path |
| 2 | `\xNN` hex escapes (`spec.md` §String literals) | rejected: *"invalid escape sequence"* | `Lexer.java:396-404` — `'x'` falls to `default:` |
| 3 | `async`, `await` are reserved (`spec.md:283-291`) | ordinary identifiers | `Lexer.java` `keywordMap` has 31 entries, neither is present. Verified: `async=1 await=2` prints fine |
| 4 | grammar has no `type`/annotations | `def f(a: int) -> int`, `type T = list[int]`, `x: int = 1`, `cast`, `isinstance`, `...` all parse **by default** | `TokenKind.java` (`CAST`, `ISINSTANCE`, `ELLIPSIS`, `RARROW`); `Parser.java:1493-1505`, `:1072-1207` |
| 5 | grammar has no doc-comment token | `#:` doc comments are lexed as `DOC_COMMENT_BLOCK` / `DOC_COMMENT_TRAILING` and attached to the AST | `TokenKind.java:39,44`; `DocComments.java`. Verified: parses |
| 6 | int is arbitrary precision | same — but the **docs** say 32-bit (§6) | `StarlarkInt.java` `Int32`/`Int64`/`Big(BigInteger)` |
| 7 | `set` in spec since #290 (2024-12-17) | present, **on by default** | `MethodLibrary.java:668`; `--experimental_enable_starlark_set` default `"true"`. Verified: `S=set([1, 2, 3]) type=set` |
| 8 | string element model implementation-defined | raw UTF-8 bytes | `--internal_starlark_utf_8_byte_strings` default `true` (hidden) |
| 9 | binary int literals not in spec | lexed then rejected: `invalid octal literal: 0b1010 (use '0ob1010')` | `Lexer.java:944-953`; open spec issue #352 asks for them |
| 10 | recursion = dynamic error | matches: `Error: function 'rec' called recursively` | verified |

**The static type system is the big one.** In Bazel 9.2.0 `--experimental_starlark_type_syntax`
**defaults to `"true"`** (`packages/semantics/FlagConstants.java:36`) and
`--experimental_starlark_types_allowed_paths` defaults to empty (= all paths), so
`BzlCompileFunction.shouldUseTypeSyntax()` (`:248-267`) returns true for every `.bzl`
file that is not the prelude and not `.scl`. `--experimental_starlark_types` is a
documented **no-op** ("Previously used as `--experimental_starlark_type_syntax` +
`--experimental_starlark_type_checking`").

The type universe is `net/starlark/java/types/Types.java:58-76`:
`None, bool, int, float, str, list[…], dict[…], set[…], tuple[…], Collection[…],
Sequence[…], Mapping[…]`.

The implementation is **half-landed and inconsistent**, which no document mentions.
Verified against stock `bazel 9.2.0`, one file, one invocation:

```
defs.bzl:6:10: expected type arguments after the type constructor 'list'   # def f(x: list)
defs.bzl:7:10: unexpected expression 'list[int]'                            # def g(x: list[int])
defs.bzl:8:10: unexpected expression 'dict[(str, int)]'
defs.bzl:9:10: unexpected expression 'set[int]'
defs.bzl:10:10: unexpected expression 'tuple[(int, str)]'
defs.bzl:11:10: unexpected expression 'Sequence[int]'
defs.bzl:13:10: unexpected expression '"SomeString"'                        # string annotation
defs.bzl:14:10: type 'Label' is not defined
defs.bzl:15:10: type 'depset' is not defined
defs.bzl:17:12: unexpected expression 'list[int]'                           # -> list[int]
```

while `x: int`, `x: str`, `x: bool`, `x: float`, `x: None`, `x: int | str`, `-> int`,
`v: int = 1`, and **`type T2 = list[int]`** all succeed. Cause: with
`tolerateInvalidTypeExpressions` on (the default, because type *checking* is off),
`Parser.parseTypeExprWithFallback()` (`:1084-1095`) parses annotations with generic
`parseTest()`, producing an `IndexExpression`; `Resolver.resolveTypeOrArg()` (`:920-972`)
then hits its `default:` branch and errors. The `type` alias statement takes a different
path (`parseTypeExpr` → `TypeApplication`) and works. **`list[int]` is a hard error as an
annotation and legal as a type alias, in the same file, in stock Bazel 9.2.0.**

`cast` and `isinstance` are **conditional keywords** — `Lexer.java:600-603` adds them to
the keyword map only when `options.allowTypeSyntax()`. The token set of the language is
flag-dependent, at the lexer level. (`isinstance(1, int)` → *"isinstance() is not yet
supported"*.)

Tracking: `bazelbuild/bazel#27370` "Static type system for Starlark" (open, created
2025-10-20, updated 2026-07-22); predecessor #22935 closed 2025-10-20. The would-be
spec is the **`starlark-with-types` branch** of `bazelbuild/starlark`, last commit
**2025-08-13** (`4a80c7b`), **6 ahead / 9 behind master**, touching only `spec.md`. It is
not being kept in sync.

### 2.4 `--incompatible_*` flags live in Bazel 9.2.0

`packages/semantics/BuildLanguageOptions.java` has **65** options. Extracted
name→default (Bazel 9.2.0):

Language/syntax-affecting (the ones an LSP cannot ignore):

| Flag | Default | Effect |
|---|---|---|
| `--experimental_starlark_type_syntax` | **true** | enables annotations, `type`, `cast`, `isinstance`, `...` in `.bzl` |
| `--experimental_starlark_type_checking` | false | turns on real checking; also flips `tolerateInvalidTypeExpressions` |
| `--experimental_starlark_types_allowed_paths` | `""` (=all) | per-label allowlist for the above |
| `--experimental_starlark_types` | false | **documented no-op** |
| `--experimental_enable_starlark_set` | **true** | `set()` type + `|`/`&` on sets |
| `--experimental_enable_scl_dialect` | **true** | `.scl` file type: ASCII-only string literals, no type syntax, tiny env (`visibility`, `struct`) |
| `--experimental_enable_first_class_macros` | **true** | `macro()` symbolic macros |
| `--incompatible_enforce_starlark_utf8` | `warning` | lexer input validation (`off`/`warning`/`error`) |
| `--internal_starlark_utf_8_byte_strings` | true (hidden) | string element model |
| `--incompatible_disallow_empty_glob` | true | `glob()` semantics |
| `--incompatible_package_group_has_public_syntax` | true | `"public"`/`"private"` in `package_group` |
| `--incompatible_fix_package_group_reporoot_syntax` | true | meaning of `//...` in `package_group` |
| `--incompatible_no_attr_license` | true | removes `attr.license` |
| `--incompatible_unambiguous_label_stringification` | true | `str(Label(...))` → `@@//foo:bar` (verified) |
| `--incompatible_enable_deprecated_label_apis` | true | `Label.workspace_name` etc. |
| `--incompatible_stop_exporting_language_modules` | false | hides `android_common`, `apple_common`, `cc_common` outside allowed repos |
| `--incompatible_autoload_externally` | **`""`** | §4 |
| `--incompatible_disable_autoloads_in_main_repo` | true | §4 |
| `--max_computation_steps` / `--nested_set_depth_limit` | 0 / 3500 | resource limits that are observable as errors |

Rule-semantics-affecting but not core-language: `incompatible_always_check_depset_elements`,
`incompatible_no_implicit_file_export`, `incompatible_no_rule_outputs_param`,
`incompatible_run_shell_command_string`, `incompatible_disallow_ctx_resolve_tools`,
`incompatible_simplify_unconditional_selects_in_rule_attrs`,
`incompatible_resolve_select_keys_eagerly`, `incompatible_fail_on_unknown_attributes`,
`incompatible_disable_target_default_provider_fields`,
`incompatible_locations_prefers_executable`, `incompatible_allow_tags_propagation`,
`incompatible_no_implicit_watch_label`, `incompatible_stop_exporting_build_file_path`,
`incompatible_enable_proto_toolchain_resolution`, `incompatible_disable_transitions_on`,
`incompatible_disable_starlark_host_transitions`,
`incompatible_disable_objc_library_transition`,
`incompatible_disable_non_executable_java_binary`,
`incompatible_java_info_merge_runtime_module_flags`,
`incompatible_do_not_split_linking_cmdline`, `incompatible_use_cc_configure_from_rules_cc`,
`incompatible_check_external_repo_source_dir_package_boundary`.

**Good news, partially.** The *classic* language-mutating flags
(`--incompatible_disallow_dict_plus`, `--incompatible_depset_union`,
`--incompatible_restrict_string_escapes`, `--incompatible_disallow_struct_provider_syntax`,
`--incompatible_new_actions_api`, …) are gone from `BuildLanguageOptions`. They survive
as **98 `OptionEffectTag.NO_OP` stubs** in
`bazel/rules/BazelRulesModule.java` so old `.bazelrc`s keep parsing. Core Starlark
expression/statement semantics are no longer flag-dependent in Bazel 9 —
**except that the flag dependence moved to the type system, which is on by default and
half-implemented.**

---

## 3. Where the Bazel dialect is actually defined

### The annotation surface

`src/main/java/net/starlark/java/annot/` — 5 annotation types:
`StarlarkBuiltin.java`, `StarlarkMethod.java`, `Param.java`, `ParamType.java`,
`StarlarkAnnotations.java`, plus an annotation processor in `annot/processor/`.

`@StarlarkBuiltin` (`StarlarkBuiltin.java:57-94`) carries `name()` (what `type(x)`
returns), `doc()` (HTML), `documented()`, `category()`, `isStructType()`. Its javadoc is
explicit that *"the annotation … implicitly demarcates the Starlark API of the type"* —
**the annotation is the API definition**, not a description of one.

Counts in Bazel 9.2.0: **166 `@StarlarkBuiltin` types, 921 `@StarlarkMethod`s** across
`src/main/java/`. The interface layer is
`src/main/java/com/google/devtools/build/lib/starlarkbuildapi/` — **116 `.java` files**,
pure interfaces carrying the annotations, split from the implementations so docgen can
load them without pulling in analysis code.

Key file for layer (b): `starlarkbuildapi/StarlarkRuleFunctionsApi.java` (1233 lines)
defines, by line: `provider` (66), `macro` (202), `rule` (429), `aspect` (791), `Label`
(1031), `exec_group` (1057), `subrule` (1085), `materializer_rule` (1160). Also
`StarlarkAttrModuleApi` (`attr.*`), `StarlarkRuleContextApi` (`ctx`),
`StarlarkActionFactoryApi` (`ctx.actions`), `StarlarkNativeModuleApi` (`native`),
`core/ProviderApi`, `MacroFunctionApi`, `StarlarkSubruleApi`.

The environment assembly is `packages/StarlarkGlobals.java`, whose javadoc states it
outright:

> *"This is the source of truth for what symbols are available in what Starlark contexts
> (BUILD, .bzl, etc.), before considering how symbols may be added by registering them on
> the rule class provider, or how symbols may be substituted by builtins injection."*

with the implementation in `analysis/starlark/StarlarkGlobalsImpl.java` (methods
`getUtilToplevels`, `getFixedBuildFileToplevelsSharedWithNative`,
`getFixedBuildFileToplevelsNotInNative`, `getFixedBzlToplevels`, `getSclToplevels`,
`getModuleToplevels`, `getRepoToplevels`, `getVendorToplevels`) and the final
composition in `packages/BazelStarlarkEnvironment.java`. Registration of rule-provided
symbols happens in `bazel/rules/BazelRuleClassProvider.java`
(`addBzlToplevel` / `addBuildFileToplevel` / `addStarlarkBuiltinsInternal`).

So: **eight different Starlark environments** (BUILD, `.bzl`-for-BUILD, `.bzl`-for-MODULE,
`.scl`, MODULE.bazel, REPO.bazel, VENDOR.bazel, cquery), each a different `ImmutableMap`
built in Java. No document enumerates them.

### How docs are generated from it

`src/main/java/com/google/devtools/build/docgen/`:

- `StarlarkDocumentationCollector.java` — walks the classpath for `@StarlarkBuiltin`
  classes **and** ingests `starlark_doc_extract` protos for Starlark-defined APIs.
- `StarlarkDocumentationGenerator.java` → `//…/docgen:skydoc_bin` → the
  `bazel.build/rules/lib/**` API reference (Velocity templates in `docgen/templates/`).
- `BuildEncyclopediaGenerator.java` → `//…/docgen:docgen_bin` → `bazel.build/reference/be/**`,
  from `<!-- #BLAZE_RULE(...) -->` comment blocks in Java source. In Bazel 9.2.0 exactly
  **16 Java files** still contain `#BLAZE_RULE` (76 occurrences): `EnvironmentRule`,
  `BazelGenRuleRule`, `GenRuleBaseRule`, `TestSuiteRule`, `Alias`, `ConfigRuleClasses`,
  `ToolchainRule`, `ConstraintSettingRule`, `ConstraintValueRule`, `PlatformRule`,
  `ExtraActionRule`, `ActionListenerRule`, `ToolchainType`, `GenQueryRule`,
  `FilegroupRule`, `StarlarkDocExtractRule`.
- `ApiExporter.java` → `//…/docgen:api_exporter` → **`builtin.pb`**.

### `builtin.pb`

Generated by `src/main/java/com/google/devtools/build/lib/BUILD:293-323`:

```python
genrule(
    name = "gen_api_proto",
    srcs = [
        "//src/main/java/com/google/devtools/build/docgen:bazel_link_map",
        "//src/main/starlark/docgen:gen_be_proto_stardoc_proto",
        "//src/main/starlark/docgen:gen_be_java_stardoc_proto",
        "//src/main/starlark/docgen:gen_be_cpp_stardoc_proto",
        "//src/main/starlark/docgen:gen_be_objc_stardoc_proto",
        "//src/main/starlark/docgen:gen_be_python_stardoc_proto",
        "//src/main/starlark/docgen:gen_be_shell_stardoc_proto",
        ":docs_embedded_in_sources",
    ],
    outs = ["builtin.pb"],
    cmd = ("$(location //…/docgen:api_exporter) --output_file=$@ …"
           " --provider=com.google.devtools.build.lib.bazel.rules.BazelRuleClassProvider"
           " --be_stardoc_proto=…"),
    tools = ["//src/main/java/com/google/devtools/build/docgen:api_exporter"],
)
```

Note the six `starlark_doc_extract` inputs from `src/main/starlark/docgen/BUILD` — the
"builtin" API surface is now partly assembled from `@rules_cc`, `@rules_java`,
`@rules_python`, `@rules_shell`, `@com_google_protobuf`, `@build_bazel_apple_support`.

Schema — `src/main/protobuf/builtin.proto` (108 lines), verbatim:

```proto
// Proto that exposes all BUILD and Starlark builtin symbols.
//
// The API exporter is used for code completion in Cider.

syntax = "proto3";
package builtin;

option java_package = "com.google.devtools.build.docgen.builtin";
option java_outer_classname = "BuiltinProtos";

message Builtins {
  repeated Type type = 1;
  repeated Value global = 2;
}

message Type {
  string name = 1;
  // List of fields and methods of this type. All such entities are listed as
  // fields, and methods are fields which are callable.
  repeated Value field = 2;
  // Module documentation.
  string doc = 3;
}

// ApiContext specifies the context(s) in which a symbol is available.
enum ApiContext {
  ALL = 0;
  BZL = 1;
  BUILD = 2;
}

message Value {
  string name = 1;
  string type = 2;         // Name of the type.
  Callable callable = 3;   // Set when the object is a function.
  string doc = 4;
  ApiContext api_context = 5;
}

message Callable {
  repeated Param param = 1;
  string return_type = 2;  // Name of the return type.
}

message Param {
  string name = 1;
  string type = 2;          // Parameter type represented as a name.
  string doc = 3;
  string default_value = 4; // Starlark expression, e.g. "False", "[]", "None"
  bool is_mandatory = 5;
  bool is_star_arg = 6;
  bool is_star_star_arg = 7;
}
```

Limitations that matter:

- `Param.type` and `Callable.return_type` are **free-form strings**, not structured
  types. `Value.type` likewise. No nullability, no generics, no union.
- Only three `ApiContext` values — cannot express `.scl` / MODULE.bazel / REPO.bazel /
  VENDOR.bazel / cquery environments, which all exist (`StarlarkGlobals`).
- Not shipped in any Bazel release artifact; the only reference is the genrule.
  Consumers vendor a pre-built copy: `upstream/starpls/crates/starpls/src/builtin/builtin.pb`
  (`include_bytes!`) and `upstream/bazel-lsp/src/builtin/builtin.pbtxt`
  (with a comment recording the exact hand-run command). Both therefore encode a
  *snapshot* of one Bazel version.
- starpls already documents a hole: `crates/starpls_bazel/src/env.rs:79` —
  *"The builtin.pb file is missing `module_extension`, `repository_rule` and
  `tag_class`."* — and hand-maintains seven JSON overlays
  (`bzl.builtins.json`, `build.builtins.json`, `module-bazel.builtins.json`,
  `workspace.builtins.json`, `repo.builtins.json`, `cquery.builtins.json`,
  `vendor.builtins.json`, `missingModuleFields.json`).

### The second machine-readable surface: `bazel info build-language`

`runtime/commands/info/BuildLanguageInfoItem.java` (marked `@Deprecated`) emits
`blaze_query.BuildLanguage` (`src/main/protobuf/build.proto:553-601`): `RuleDefinition`
{name, `repeated AttributeDefinition`, documentation, label} with per-attribute
`type`, `mandatory`, `allowed_rule_classes`, `default`, `executable`, `configurable`,
`nodep`. It is a **runtime** dump — it reflects the current workspace's flags and
builtins — but *"Only contains documented rule definitions"*, which in Bazel 9 means
essentially just the ~16 remaining Java rules.

### The third: `starlark_doc_extract` / `stardoc_output.proto`

`src/main/protobuf/stardoc_output.proto` — `ModuleInfo` with `RuleInfo`, `ProviderInfo`,
`StarlarkFunctionInfo`, `AspectInfo`, `MacroInfo`, `ModuleExtensionInfo`,
`RepositoryRuleInfo`, `StarlarkOtherSymbolInfo`, `AttributeInfo`, `AttributeType`.
Extractors in `lib/starlarkdocextract/`. This is the **only** machine-readable schema
that can describe Starlark-defined rules, and it is therefore the only viable source of
rule knowledge for an LSP in Bazel 9. `stardoc` (`upstream/stardoc`, HEAD `bd575db`
2026-06-23) is now just a Velocity template runner on top of it — its own README says so,
and points at BCR's convention of publishing `starlark_doc_extract` output as a release
artifact (`bazel-central-registry/docs/stardoc.md`).

---

## 4. Native rules in Bazel 9: what's left

`--incompatible_autoload_externally` defaults to **`""`** in Bazel 9
(`packages/semantics/FlagConstants.java:30`), i.e. autoloads are **off**. Tracking issue
`bazelbuild/bazel#23043`, **closed 2026-02-05**. Release notes for 9.0.0: *"This marks
the conclusion of the Starlarkification effort."*

### Genuinely native (Java-implemented) rule classes remaining

From `bazel/rules/BazelRuleClassProvider.java` `RULE_SETS` and the `addRuleDefinition`
call sites:

| Source | Rules |
|---|---|
| `rules/core/CoreRules.java` | `$native_build_rule`, `$native_buildable_rule`, `$make_variable_expanding_rule` (abstract bases, not user-visible) |
| `bazel/rules/GenericRules.java` | `environment`, `alias`, `filegroup`, `test_suite`, `genquery`, `label_setting`, `label_flag`, `starlark_doc_extract` |
| `bazel/rules/ToolchainRules.java` | `toolchain_type`, `genrule` (+ `$genrule_base`) |
| `rules/config/ConfigRuleClasses.java` | `config_setting`, `config_feature_flag` |
| `rules/platform/*` | `platform`, `constraint_setting`, `constraint_value`, `toolchain` |
| `bazel/rules/JavaRules.java` | `java_plugins_flag_alias`, `extra_action`, `action_listener` |
| `bazel/rules/CcRules.java` | `cc_toolchain_alias`, `cc_libc_top_alias` |

Plus package-level functions, not rules: `package`, `package_group`, `exports_files`,
`glob`, `subpackages`, `licenses`, `environment_group`, `existing_rule(s)`,
`package_name`, `repository_name`, `repo_name`, `package_relative_label`,
`package_default_visibility`, `module_name`, `module_version`.

### The two different removal mechanisms (this is the trap)

**C++/ObjC — loading-phase `fail()`.** `src/main/starlark/builtins_bzl/bazel/exports.bzl:22-58`
injects a macro over each name:

```python
_REMOVED_RULES = ["cc_binary", "cc_import", "cc_library", "cc_shared_library",
    "cc_static_library", "cc_test", "cc_toolchain", "cc_toolchain_alias",
    "fdo_prefetch_hints", "fdo_profile", "memprof_profile", "propeller_optimize",
    "objc_import", "objc_library"]

def _removed_rule_failure(**_kwargs):
    fail("""
         This rule has been removed from Bazel. Please add a `load()` statement for it.
         This can also be done automatically by running:
         buildifier --lint=fix <path-to-BUILD-or-bzl-file>
         """)
```

Observed on stock 9.2.0:

```
ERROR: Traceback (most recent call last):
	File "/private/tmp/blstest/BUILD", line 1, column 11, in <toplevel>
		cc_library(name = "foo", srcs = [])
	File "/virtual_builtins_bzl/bazel/exports.bzl", line 40, column 9, in _removed_rule_failure
Error in fail: This rule has been removed from Bazel. …
```

**Java — analysis-phase error.** `bazel/rules/JavaRules.java:41-58` registers
`BaseRuleClasses.EmptyRule("java_binary", "@rules_java//java:java_binary.bzl")` etc.;
`analysis/BaseRuleClasses.java:481-500` emits:

```
The java_binary rule has been removed, add the following to your BUILD/bzl file:

load("@rules_java//java:java_binary.bzl", "java_binary")
```

`CcRules.java:123-136` also registers `EmptyRule`s for the cc names (with no bzl label),
so a cc name is *both* an `EmptyRule` rule class **and** shadowed by the builtins macro.

**Python/shell/proto/android — simply absent.** No stub, no rule class; unknown-symbol
error.

### Consequence: `dir(native)` lies

Measured on stock `bazel 9.2.0` (`print(sorted(dir(native)))` from a `.bzl`), 58 entries:

```
action_listener, alias, cc_binary, cc_import, cc_libc_top_alias, cc_library,
cc_shared_library, cc_static_library, cc_test, cc_toolchain, cc_toolchain_alias,
cc_toolchain_suite, config_feature_flag, config_setting, constraint_setting,
constraint_value, environment, existing_rule, existing_rules, exports_files,
extra_action, fdo_prefetch_hints, fdo_profile, filegroup, genquery, genrule, glob,
java_binary, java_import, java_library, java_package_configuration, java_plugin,
java_plugins_flag_alias, java_runtime, java_test, java_toolchain, label_flag,
label_setting, legacy_globals, memprof_profile, module_name, module_version,
objc_import, objc_library, package, package_default_visibility, package_group,
package_name, package_relative_label, propeller_optimize, repo_name, repository_name,
starlark_doc_extract, subpackages, test_suite, toolchain, toolchain_type
```

Every `cc_*`, `java_*`, `objc_*` entry there is a landmine: present for reflection,
fatal when called. `legacy_globals` is undocumented
(`AutoloadSymbols.java:343-364`). No `py_*`, `sh_*`, `proto_*`, `android_*`.

### The autoload redirect table

`packages/AutoloadSymbols.java:675-859` is the authoritative rule→`.bzl` map (used only
when the flag is non-empty, but it is still the canonical statement of where each symbol
went). 60 entries. Examples:

```java
.put("cc_library",       ruleRedirect("@rules_cc//cc:cc_library.bzl"))
.put("java_binary",      ruleRedirect("@rules_java//java:java_binary.bzl"))
.put("java_toolchain",   ruleRedirect("@rules_java//java/toolchains:java_toolchain.bzl"))
.put("proto_library",    ruleRedirect("@com_google_protobuf//bazel:proto_library.bzl"))
.put("py_binary",        ruleRedirect("@rules_python//python:py_binary.bzl"))
.put("sh_binary",        ruleRedirect("@rules_shell//shell:sh_binary.bzl"))
.put("android_binary",   ruleRedirect("@rules_android//rules:rules.bzl"))
.put("xcode_config",     ruleRedirect("@build_bazel_apple_support//xcode:xcode_config.bzl"))
.put("CcInfo",           symbolRedirect("@rules_cc//cc/common:cc_info.bzl", …))
.put("JavaInfo",         symbolRedirect("@rules_java//java/common:java_info.bzl", …))
.put("cc_common",        symbolRedirect("@rules_cc//cc/common:cc_common.bzl"))
```

**There is a fourth, divergent copy of this map** in buildifier —
`upstream/buildtools/tables/tables.go:215-252` + `warn/warn_bazel_api.go:728-830`:

| Symbol | Bazel `AutoloadSymbols` | buildifier 8.5.1 `tables.go` |
|---|---|---|
| `py_binary` | `@rules_python//python:py_binary.bzl` | `@rules_python//python:defs.bzl` |
| `proto_library` | `@com_google_protobuf//bazel:proto_library.bzl` | `@protobuf//bazel:proto_library.bzl` |
| `android_binary` | `@rules_android//rules:rules.bzl` | `@rules_android//android:rules.bzl` |

Bazel's own error message tells users to run `buildifier --lint=fix`, which will insert
load statements from a table Bazel does not own and does not agree with.

**Net effect for an LSP:** the set of rule names, their attributes, and their doc strings
are a function of the *workspace's* `MODULE.bazel` resolution, not of the Bazel version.
`cc_library`'s attribute list in a repo pinned to `rules_cc 0.0.17` differs from one
pinned to `rules_cc 2.x`. No static table can be correct. The only correct sources are
(i) `starlark_doc_extract` run over the resolved external repos, or (ii) evaluating the
`.bzl` files yourself.

---

## 5. Grammars: five of them, all different

| Artifact | Size | Form | Owner | Last change | BUILD-specific? |
|---|---|---|---|---|---|
| `upstream/starlark/spec.md:4721-4838` | ~115 lines | prose EBNF | bazelbuild/starlark | 2026-02-06 | no |
| `upstream/starlark-go/syntax/grammar.txt` | 129 lines | prose EBNF | starlark-go | stale vs own parser | no |
| `upstream/starlark-go/syntax/{parse,scan}.go` | 1096 + 1167 | hand-written recursive descent | starlark-go | 2026-07-08 | no |
| `upstream/tree-sitter-starlark/grammar.js` | 215 lines | tree-sitter DSL, **`grammar(Python, {...})`** | tree-sitter-grammars | **2024-12-05** (v1.3.0) | no |
| `upstream/buildtools/build/parse.y` + `lex.go` | 1327 + 947 | goyacc + hand lexer | bazelbuild/buildtools | 2026-08-24 | file-type tagged, **one grammar** |
| `bazel/net/starlark/java/syntax/{Lexer,Parser,Resolver}.java` | 1056 + 1700 + 1461 | hand-written; grammar only in `//` comments | Bazel | continuous | via post-pass |

### tree-sitter-starlark is a Python grammar with 12 overrides

`grammar.js:16` — `module.exports = grammar(Python, {...})`, requiring
`tree-sitter-python/grammar`. It **keeps** `while_statement`, `with_statement`,
`match_statement`, `decorated_definition`, `delete_statement`, `exec_statement`,
`print_statement`, `assert_statement`, f-strings, `ellipsis`, set literals, and the `<>`
comparison operator (`grammar.js:128`). It removes only `yield`, `class`, `try`,
`global`/`import`/`nonlocal`/`raise`, `is`, generator expressions, implicit string
concatenation. It over-accepts by a wide margin, has no `load` node type, and no notion
of BUILD-vs-`.bzl`. Default branch last commit **2024-12-05** — 20 months stale; it
predates sets-in-spec, Bazel type syntax, and `#:` doc comments.

### buildifier: one grammar, file type is metadata only

`build/lex.go:37-52` defines `FileType` (`TypeDefault`, `TypeBuild`, `TypeWorkspace`,
`TypeBzl`, `TypeModule`, `TypeRepo`, `TypeVendor`) inferred from the filename
(`lex.go:155-190`; `BUILD.foo.bazel`, `my.WORKSPACE`, `thing.bzl.oss` all recognised),
but all seven go through the same `parse.y`. BUILD restrictions are lints, not parse
errors.

`parse.y` is the only third-party grammar that models Bazel's type syntax: `_ARROW`
(`->`, token line 115), `parameter_type` productions (761-810), `TypedIdent`
(`parse.y:1106-1110`), and `test _IS test` (872). But it does **not** know
`type X = ...`:

```
$ buildifier -mode=check -type=bzl t.bzl
t.bzl:4:13: syntax error near MyAlias        # type MyAlias = list[int]
```

buildifier 8.5.1 rejects a construct stock Bazel 9.2.0 accepts. It does handle
`X: int = 1`, `def f(a: int, b: str = "x") -> list[str]`, and `#:` (as an ordinary
comment).

### Where BUILD-file syntax is actually handled in Bazel

Nowhere in `Parser.java`. Three mechanisms, all outside the grammar:

1. **`FileOptions` per file type.**
   - BUILD — `skyframe/PackageFunction.java:1353-1363`:
     `requireLoadStatementsFirst(false)`, `loadBindsGlobally(true)`,
     `allowToplevelRebinding(true)`. (The comment: *"For historical reasons, BUILD files
     are allowed to load a symbol and then reassign it later. (It is unclear why this is
     necessary)."*)
   - `.bzl` — `skyframe/BzlCompileFunction.java:198-214`:
     `loadBindsGlobally(key.isBuildPrelude())`,
     `stringLiteralsAreAsciiOnly(key.isSclDialect())`, `allowTypeSyntax(useTypeSyntax)`,
     `tolerateInvalidTypeExpressions(!doTypeChecking)`.
   - MODULE.bazel — `bzlmod/CompiledModuleFile.java:81` uses bare
     `StarlarkFile.parse(input)`, i.e. `FileOptions.DEFAULT`: loads first, file-local,
     no rebinding. **Different from BUILD.**

2. **`packages/DotBazelFileSyntaxChecker.java`** — a `NodeVisitor` run after parsing.
   This is the entire specification of the BUILD dialect's restrictions:

   ```java
   /**
    * … restricted syntax that BUILD, WORKSPACE, REPO.bazel, and MODULE.bazel files use.
    * This restricted syntax disallows:
    *   - control-flow statements (`for` and `if`, but not comprehensions and `if` expressions),
    *   - function definitions (`def` and `lambda`),
    *   - variadic arguments (`*args` and `**kwargs`) in function call sites, and
    *   - optionally, `load()` statements.
    */
   ```

   Call sites: `packages/PackageFactory.java:585` (`"BUILD files"`, `canLoadBzl=true`),
   `bzlmod/CompiledModuleFile.java:111` (`"MODULE.bazel files"`, `canLoadBzl=false`,
   `allowLiteralStarStarArgs=true`), `skyframe/RepoFileFunction.java:157`
   (`"REPO.bazel files"`, `canLoadBzl=false`), `bzlmod/VendorFileFunction.java:121`
   (`"VENDOR.bazel files"`).

   The `**kwargs`-at-call-site ban is real and widely unknown; MODULE.bazel is the sole
   exception, and only for *literal dicts* (`DotBazelFileSyntaxChecker.java:88-99`).

3. **`PackageFactory.checkBuildSyntax()`** (`:564-661`) subclasses the checker to also
   scrape literal `glob()`/`subpackages()` patterns for prefetching and record
   `generator_name` for `f(name="foo", ...)` calls — so the *AST shape* of a BUILD file
   has load-bearing semantics (which macro created which target) that no grammar
   captures.

   MODULE.bazel adds a fourth layer: `include("…")` is a pseudo-statement validated
   syntactically (`CompiledModuleFile.java:107-145`) — *"MUST be called with exactly one
   positional argument that is a string literal"* — unless `include` has been assigned to.

Top-level `if`/`for` in `.bzl` are rejected by the **Resolver**, not the parser
(verified: `if statements are not allowed at the top level. You may move it inside a
function or use an if expression (x if condition else y).`).

---

## 6. VERDICT: is "the docs are stale, read the source" true?

**Yes**, and it is worse than staleness: the published docs contain statements that are
flatly false in the current release, in the sections an LSP author would rely on most.

Verified on stock `bazel 9.2.0` against live docs and in-repo sources.

### 6.1 `bazel.build/rules/language` — "Int type is limited to 32-bit signed integers"

Source: `upstream/bazel/site/en/rules/language.md:136`. Live at
<https://bazel.build/rules/language>.

> * Int type is limited to 32-bit signed integers. Overflows will throw an error.

Bazel 9.2.0:

```
BIG=121932631124828532112482853211126352690 type=int
```

Implementation: `net/starlark/java/eval/StarlarkInt.java` has `Int32`, `Int64`, and
`Big(BigInteger)`. Arbitrary precision has been the behaviour since Bazel 4/5. The doc
sentence is ~7 years out of date.

### 6.2 Same page — the supported-types list is incomplete

`language.md:41-48` lists `None, bool, dict, tuple, function, int, list, string`.
Missing: **`float`** (`MethodLibrary.java:462`, and `1/3` → `0.3333333333333333 type=float`),
**`set`** (default-on since Bazel 8; `S=set([1, 2, 3]) type=set`), **`depset`**,
**`struct`**, **`Label`**. Also `is` is listed as unsupported (correct) while the page
never mentions that `assert`, `async`, `await` *are* usable identifiers in Bazel but
reserved by the spec it calls "authoritative".

### 6.3 Same page — type annotations described as off, flag named is a no-op

`language.md:50-62`:

> **Experimental**. … It may be enabled in Bazel at HEAD by using the
> `--experimental_starlark_types` flag.

Two errors: (i) `--experimental_starlark_types` is a **documented no-op** in Bazel 9.2.0
(`BuildLanguageOptions.java:714-720`: *"No-op. Previously used as
`--experimental_starlark_type_syntax` + `--experimental_starlark_type_checking`"*);
(ii) the syntax is **on by default** in the shipped 9.2.0 release, not "at HEAD" —
`FlagConstants.java:36 DEFAULT_EXPERIMENTAL_STARLARK_TYPE_SYNTAX = "true"`. Verified:
`def f(x: int, y: str = "a") -> int` compiles with no flags.

The linked "specification" (`starlark-with-types` branch) is 9 commits behind master and
has not been touched since 2025-08-13, and does not describe the actual accepted subset
(§2.3: `list[int]` is legal as a `type` alias and illegal as an annotation).

### 6.4 Same page — "two syntactic restrictions in BUILD files"

`language.md:119-120`:

> There are two syntactic restrictions in `BUILD` files: 1) declaring functions is
> illegal, and 2) `*args` and `**kwargs` arguments are not allowed.

`DotBazelFileSyntaxChecker.java` enforces **four**: `def`, `lambda`, `for`, `if`, plus
`*args`/`**kwargs` at call sites. Never mentions `lambda`. Never mentions that
`load()` may be interleaved with other statements in BUILD but not `.bzl`
(`requireLoadStatementsFirst`), or that BUILD allows top-level rebinding while `.bzl`
does not — both directly relevant to diagnostics.

### 6.5 `bazel.build/reference/be/overview` — the worst one

Live text, generated from
`src/main/java/com/google/devtools/build/docgen/templates/be/overview.vm:51-53`
(**unchanged on master**):

> Native rules ship with the Bazel binary and do not require a `load` statement.
> Native rules are available globally in BUILD files. In .bzl files, you can find them in
> the `native` module.
>
> ### Language-specific native rules
> | C / C++ | cc_binary | cc_import cc_library cc_shared_library cc_static_library | cc_test | … |
> | Java | java_binary | java_import java_library | java_test | … |
> | Python | py_binary | py_library | py_test | py_runtime |
> | Shell | sh_binary | sh_library | sh_test | |
> | Protocol Buffer | | cc_proto_library java_proto_library proto_library … | | |

Every rule in that table **requires a `load()` in Bazel 9** and fails without one
(§4, with the exact error text). The generator itself knows better: the same Build
Encyclopedia pages are built from `starlark_doc_extract` of `@rules_cc`, `@rules_java`,
`@rules_python`, `@rules_shell`, `@com_google_protobuf`
(`src/main/starlark/docgen/BUILD`). The hand-written overview template was never
updated.

### 6.6 `bazel.build/rules/lib/toplevel/native` contradicts itself on one page

Prose: *"All native rules appear as functions in this module, e.g. `native.cc_library`."*
Members list on the same page: `existing_rule, existing_rules, exports_files, glob,
module_name, module_version, package_default_visibility, package_group, package_name,
package_relative_label, repo_name, repository_name, subpackages` — **no rules at all**.
Meanwhile the runtime `dir(native)` returns 58 names including `cc_library` (fatal when
called), `package`, and `legacy_globals` (undocumented anywhere).

### 6.7 starlark-go's spec is stale about Bazel

`upstream/starlark-go/doc/spec.md:849` and `:1991`: *"The Java implementation does not
support sets."* False since Bazel 8 (`--experimental_enable_starlark_set` default true;
verified in 9.2.0). Its `§Dialect differences` is otherwise accurate — including
*"The Java implementation does not support hex escapes"*, which I confirmed at
`Lexer.java:396-404`.

### 6.8 The spec contradicts itself

`spec.md:4726` grammar admits top-level `IfStmt`/`ForStmt`; `spec.md:3040` and `:3081`
prose call them static errors. An implementer following the grammar produces a different
language than one following the prose. Bazel follows the prose (verified).

---

## What this means for the language server

1. **There is no document to implement against.** For layer (a) use the spec as a
   *starting point only*; for (b) and (c) the Java source is the only truth. Budget for
   reading `net/starlark/java/syntax/`, `packages/DotBazelFileSyntaxChecker.java`,
   `packages/StarlarkGlobals*.java`, `skyframe/{BzlCompile,Package}Function.java`.
2. **Pin a Bazel version and say so.** The accepted grammar changed between 8 and 9
   (type syntax on by default) and will change again in 10. `builtin.pb` snapshots are
   version-locked by construction; starpls and bazel-lsp both vendor one.
3. **No third-party parser currently accepts Bazel 9.2.0's `.bzl` grammar.**
   tree-sitter-starlark (2024-12) is a Python grammar and 20 months stale;
   buildifier 8.5.1 rejects `type X = ...`. If the LSP must accept what Bazel accepts,
   the grammar has to be written from `Parser.java`, or the Bazel parser has to be
   shelled out to.
4. **Rule knowledge cannot be static.** `cc_library` is `@rules_cc`'s, at whatever
   version the workspace resolved. `starlark_doc_extract` /
   `stardoc_output.proto` is the only schema that can express it; `builtin.pb` cannot.
5. **Do not trust `dir(native)` or `builtin.pb` for completion** — both list `cc_library`
   etc. as available; calling them fails. A completion that inserts `native.cc_library`
   produces a build error with a confusing message.
6. **The autoload map is worth vendoring** (`AutoloadSymbols.java:675-859`) for
   quick-fix "add the missing `load()`", but note buildifier's copy disagrees for
   `py_*`, `proto_*`, `android_*` — pick Bazel's, since Bazel is what fails the build.
