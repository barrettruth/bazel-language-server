# 06 — Builtins and docs: where machine-readable Bazel API knowledge comes from

Research date **2026-08-25**. Bazel **9.2.0** (released 2026-07-13) is current;
`upstream/bazel` is master `3a9b19c8` (2026-08-24), `MODULE.bazel` version
`10.0.0-prerelease`. All experiments were run on macOS arm64 with the official
`bazel-9.2.0-darwin-arm64` binary and `protoc` 35.1. Artifacts are in
`/Users/bruth/dev/bazel-language-server/experiments/builtins/`.

---

## 0. Executive summary

There are exactly **five** machine-readable sources. None is sufficient alone.

| # | Source | Covers | Format | Availability |
|---|---|---|---|---|
| 1 | `builtin.pb` (`ApiExporter`) | Starlark API: 123 types, ~139 globals, `ctx.*`, `attr.*`, providers, `cc_common`, plus *rule attribute name lists* | `builtin.Builtins` proto | **Not published anywhere.** Must build Bazel from source (~2.5–4 min) |
| 2 | `starlark_doc_extract` (native rule, Bazel ≥7.0) | Any `.bzl` file's rules / providers / macros / aspects / module extensions / repo rules, fully typed | `stardoc_output.ModuleInfo` proto | In every Bazel; needs a build + correct `bzl_library` deps |
| 3 | `bazel query --output=proto --proto:rule_classes=true` (Bazel ≥8.0) | Every rule *class* actually instantiated in the queried packages, incl. inherited native attributes | `stardoc_output.RuleInfo` embedded in `blaze_query.QueryResult` | In every Bazel ≥8; 0.14 s warm over 437 targets |
| 4 | `bazel info build-language` | 43 native rule classes in Bazel 9 — **22 of which are attribute-less husks** | `blaze_query.BuildLanguage` proto | In every Bazel; near-worthless in Bazel 9 |
| 5 | BCR `source.json` `docs_url` | Pre-built `ModuleInfo` protos for **58 of 1250** BCR modules (4.6 %) | tarball of `*.binaryproto` | HTTP, no Bazel needed |

The single most important structural fact: **`builtin.pb` never carried rule
*attribute types* and, since Bazel 9, does not describe what is actually callable
in a `BUILD` file.** Of the 63 `api_context: BUILD` globals in Bazel 9.2.0's own
`builtin.pb`, **15 are not defined at all** and **13 more are poison-pill stubs
that `fail()` when called** — 28/63 = 44 % wrong (§4.4).

---

## 1. `builtin.pb`

### 1.1 The proto schema (complete, verbatim)

`upstream/bazel/src/main/protobuf/builtin.proto` — 108 lines, **byte-identical in
9.2.0 and master**. Unchanged in substance since 2018.

```proto
// Copyright 2018 The Bazel Authors. All rights reserved.
// ...
// Proto that exposes all BUILD and Starlark builtin symbols.
//
// The API exporter is used for code completion in Cider.

syntax = "proto3";
package builtin;

// option java_api_version = 2;
option java_package = "com.google.devtools.build.docgen.builtin";
option java_outer_classname = "BuiltinProtos";

// Top-level object for all BUILD and Starlark builtin modules.
// Globals contains a list of all builtin variables, functions and packages
// (e.g. "java_common" and "native" will be included, same as "None" and
// "dict").
// Types contains a list of all builtin packages (e.g. "java_common"
// and "native"). All types should be uniquely named.
message Builtins {
  repeated Type type = 1;

  repeated Value global = 2;
}

// Representation for Starlark builtin packages. It contains all the symbols
// (variables and functions) exposed by the package.
// E.g. "list" is a Type that exposes a list of fields containing: "insert",
// "index", "remove" etc.
message Type {
  string name = 1;

  // List of fields and methods of this type. All such entities are listed as
  // fields, and methods are fields which are callable.
  repeated Value field = 2;

  // Module documentation.
  string doc = 3;
}

// ApiContext specifies the context(s) in which a symbol is available. For
// example, a symbol may be available as a builtin only in .bzl files, but
// not in BUILD files.
enum ApiContext {
  ALL = 0;
  BZL = 1;
  BUILD = 2;
}

// Generic representation for a Starlark object. If the object is callable
// (can act as a function), then callable will be set.
message Value {
  string name = 1;

  // Name of the type.
  string type = 2;

  // Set when the object is a function.
  Callable callable = 3;

  // Value documentation.
  string doc = 4;

  // The context(s) in which the symbol is recognized.
  ApiContext api_context = 5;
}

message Callable {
  repeated Param param = 1;

  // Name of the return type.
  string return_type = 2;
}

message Param {
  string name = 1;

  // Parameter type represented as a name.
  string type = 2;

  // Parameter documentation.
  string doc = 3;

  // Default value for the parameter, written as Starlark expression (e.g.
  // "False", "True", "[]", "None")
  string default_value = 4;

  // Whether the param is mandatory or optional.
  bool is_mandatory = 5;

  // Whether the param is a star argument.
  bool is_star_arg = 6;

  // Whether the param is a star-star argument.
  bool is_star_star_arg = 7;
}
```

Note the header comment: *"The API exporter is used for code completion in
Cider"* — Google's internal IDE. It is an internal-tooling artifact that OSS
Bazel happens to keep buildable; nothing in Bazel's release process produces or
ships it.

### 1.2 The producing target

`src/main/java/com/google/devtools/build/lib/BUILD:293-323` (master; identical in
9.2.0):

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
    cmd = (
        "$(location //src/main/java/com/google/devtools/build/docgen:api_exporter)" +
        " --output_file=$@" +
        " --link_map_path=$(location .../docgen:bazel_link_map) " +
        " --provider=com.google.devtools.build.lib.bazel.rules.BazelRuleClassProvider" +
        " --input_root=$$PWD" +
        " --input_dir=$$PWD/src/main/java/com/google/devtools/build/lib" +
        " --be_stardoc_proto=$(location .../docgen:gen_be_proto_stardoc_proto)" +
        ...
    ),
    tools = ["//src/main/java/com/google/devtools/build/docgen:api_exporter"],
)
```

- Tool: `//src/main/java/com/google/devtools/build/docgen:api_exporter`
  (`java_binary`, `docgen/BUILD:76`), main class
  `com.google.devtools.build.docgen.ApiExporter` (`ApiExporter.java`, 437 lines).
- `ApiExporter` reflects over `@StarlarkBuiltin` / `@StarlarkMethod` annotations
  reachable from `BazelRuleClassProvider`, plus scrapes HTML doc comments out of
  the Java sources under `--input_dir`.
- **Crucially**, since Bazel 8 it also ingests six `starlark_doc_extract`
  binaryprotos via `--be_stardoc_proto=` — the Starlarkified rulesets (§4.5).
  `builtin.pb` is therefore already a *hybrid* of Java reflection and Stardoc.

### 1.3 What `ApiExporter` puts in — and leaves out

`ApiExporter.collectRuleInfo` (`ApiExporter.java:283-297`) is the whole native-rule
export:

```java
private static Value.Builder collectRuleInfo(RuleDocumentation rule) {
    Value.Builder value = Value.newBuilder();
    value.setName(rule.getRuleName());
    value.setDoc(rule.getHtmlDocumentation());
    Callable.Builder callable = Callable.newBuilder();
    callable.addParam(newParam("name", true));
    for (RuleDocumentationAttribute attr : rule.getAttributes()) {
      callable.addParam(newParam(attr.getAttributeName(), attr.isMandatory()));
    }
    value.setCallable(callable);
    return value;
}
```

`newParam(name, mandatory)` sets **only** `name` and `is_mandatory`. So for a
rule, `builtin.pb` gives you: the rule-level HTML doc, the ordered list of
attribute names, and which are mandatory. **No attribute type. No per-attribute
doc. No default value.**

I verified this text is identical in tags 8.0.0, 8.7.0, 9.0.0, 9.2.0 and master.

For the *Starlark API* the story is completely different — that part is rich.
From my freshly built `bazel-9.2.0-builtin.pb`:

```
type "actions": 12 fields
  run(...) -> NoneType : 16 params
      "outputs"      type="<a class="anchor" href="../core/list.htm…  mand=true  doclen=41
      "inputs"       type=…                       def="[]"           doclen=50
      "executable"   type="<a … href="../builtins/File…               mand=true  doclen=49
      "tools"        type=…                       def="unbound"      doclen=708
      ...
type "DefaultInfo": 4 fields
   data_runfiles     type=runfiles
   default_runfiles  type=runfiles
   files             type=depset
   files_to_run      type=FilesToRunProvider
type "attr": 14 fields — bool, int, int_list, label, label_keyed_string_dict,
   label_list, label_list_dict, output, output_list, string, string_dict,
   string_keyed_label_dict, string_list, string_list_dict  (all -> Attribute)
```

So two of the three questions in the brief are answered by `builtin.pb`:
`ctx.actions.run` takes a mandatory `executable=`; `DefaultInfo.files` is a
`depset`. The third — `cc_library.hdrs` is a `label_list` — is **not**:

```
### cc_library: 40 params
    "name"   type=(none) mand=true  doc=no
    "hdrs"   type=(none) mand=-     doc=no
```

Caveat for consumers: `Param.type` and all `doc` fields are **HTML fragments**
with `<a class="anchor" href="../core/list.html">` links, `<code>`, `<pre>`,
`${link ...}` macros. Every consumer studied (starpls, bazel-lsp via `htmd`,
JetBrains via `cleanupStardoc()`) has its own HTML→Markdown converter.

Two more defects observed in the 9.2.0 output:

- One `global { api_context: BZL }` with **no `name` at all**. Cause:
  `ApiExporter.appendGlobals:140` guards only the *name assignment* with
  `else if (!name.equals("_builtins_dummy"))`, but `builtins.addGlobal(value)` at
  line 149 runs unconditionally. starpls' bundled pb has the same empty entry
  (it shows up as `?` when you list globals).
- `ApiContext` has only `ALL | BZL | BUILD`. There is **no context for
  `MODULE.bazel`, `REPO.bazel`, `VENDOR.bazel`, `WORKSPACE`, or `cquery`**, so
  `bazel_dep`, `use_extension`, `module_extension`, `repository_rule`,
  `tag_class`, `use_repo_rule`, `single_version_override`, … are absent from
  `builtin.pb` entirely. Every consumer hand-maintains these (§1.6).

### 1.4 Is it published? **No.**

I enumerated the assets of the **100 most recent** `bazelbuild/bazel` releases
(tags `8.8.0rc2` … `9.0.1rc1`, covering all of 8.x and 9.x). The complete set of
asset name shapes is:

```
bazel-VER-{darwin,linux,windows}-{arm64,x86_64}[.exe|.zip]  (+ .sha256, .sig)
bazel-VER-dist.zip                                          (+ .sha256, .sig)
bazel-VER-installer-{darwin,linux}-*.sh                     (+ .sha256, .sig)
bazel_VER-linux-x86_64.deb                                  (+ .sha256, .sig)
bazel_nojdk-VER-*                                           (+ .sha256, .sig)
```

There is **no `builtin.pb`, no docs archive, no proto artifact** in any release.
`bazel-VER-dist.zip` is a source distribution, not a built artifact.

The installed Bazel does ship the *schemas* but not the data:

```
$ IB=$(bazel info install_base); find "$IB" -name '*.proto' | grep -E 'builtin|stardoc'
…/install/<md5>/embedded_tools/src/main/protobuf/stardoc_output.proto
…/install/<md5>/embedded_tools/src/main/protobuf/builtin.proto
```

They are exposed to builds as `@bazel_tools//src/main/protobuf:builtin_proto` and
`…:build_proto` — which is exactly how `bazel-lsp` gets its Rust bindings.

### 1.5 Attempts to make Bazel emit it at runtime — all dead

| Ref | Title | Status |
|---|---|---|
| [PR #21936](https://github.com/bazelbuild/bazel/pull/21936) | `bazel help builtin-symbols-as-proto` (fmeum, Apr 2024) | **Closed unmerged 2024-04-23** — reviewer (tetromino) objected that `builtin.Value` is the wrong shape for a rule class and that `bazel help` can't take semantics flags |
| [PR #21135](https://github.com/bazelbuild/bazel/pull/21135) | "Include additional rule info while exporting builtins" (withered-magic, starpls author) — adds attribute type/default/doc to `collectRuleInfo` | **Closed as not planned** |
| [PR #21929](https://github.com/bazelbuild/bazel/pull/21929) | Include Bzlmod globals in `builtin.proto` | Not merged — master's `ApiContext` still has only `ALL/BZL/BUILD` as of 2026-08-24 |
| [PR #24817](https://github.com/bazelbuild/bazel/pull/24817) | `bazel info starlark-environments-proto` (hauserx, Jan 2025) | Draft/closed — not in 9.2.0's `bazel help info-keys` |
| [Issue #21979](https://github.com/bazelbuild/bazel/issues/21979) | Stardoc protos should export parameter type annotations for builtin functions | Open, filed Apr 2024 |

The reviewer's stated direction (Apr 2024) was to expose Stardoc protos through
`bazel query` or a new command, and that landed as
`--proto:rule_classes` (§5.2) — but only for *rule classes of instantiated
targets*, never for the global Starlark environment.

**Consequence: as of Bazel 9.2.0 / master 2026-08, a running Bazel cannot be
asked for its Starlark builtin environment. The only way to get `builtin.pb` is
to build Bazel from source.**

### 1.6 Who consumes it

**starpls** (`withered-magic/starpls`, last commit `ac25eca` 2025-12-02 — active).

- Bundles `crates/starpls/src/builtin/builtin.pb`, **2 452 170 bytes**,
  sha256 `8dc00bb4…`. Last updated in commit `5d900cf` *"chore: update builtins
  proto"* on **2024-12-27** — i.e. **20 months stale**, an approximately Bazel-8.0
  snapshot (it contains `cc_static_library`, `macro`, `Subrule`; 117 types, 142
  globals).
- Loaded with `include_bytes!` at `crates/starpls/src/server.rs:366`:
  ```rust
  pub(crate) fn load_bazel_builtins() -> Builtins {
      let data = include_bytes!("builtin/builtin.pb");
      decode_builtins(&data[..]).expect("bug: invalid builtin.pb")
  }
  ```
- Proto schema is vendored at `crates/starpls_bazel/data/builtin.proto` and
  compiled with `prost-build` (`crates/starpls_bazel/build.rs`) or
  `rust_prost_library` under Bazel.
- **Its `builtin.pb` is not reproducible from upstream Bazel.** Its `cc_library`
  entry has typed, documented params:
  ```
  param { name: "hdrs"
          type: "List of <a href=\"/concepts/labels\">labels</a>"
          doc: "The list of header files published by this library…" }
  ```
  Upstream `collectRuleInfo` never emitted `type`/`doc`. This pb was produced with
  the author's own PR #21135 patch, which was **closed as not planned**. So the
  file is a dead end: nobody can regenerate it for Bazel 9.
- To paper over `builtin.pb`'s blind spots, starpls hand-maintains nine JSON
  files in `crates/starpls_bazel/data/` (72 062 bytes total):
  `module-bazel.builtins.json` (29 651 B), `commonAttributes.json` (12 366 B),
  `missingModuleFields.json` (8 941 B), `bzl.builtins.json` (8 512 B),
  `workspace.builtins.json`, `build.builtins.json`, `repo.builtins.json`,
  `cquery.builtins.json`, `vendor.builtins.json`. The source comment is blunt:
  `/// The builtin.pb file is missing 'module_extension', 'repository_rule' and 'tag_class'.`
- At runtime it *additionally* shells out to `bazel info build-language` and keeps
  the result as a second, separate `Builtins` (`server.rs:141`
  `analysis.set_builtin_defs(load_bazel_builtins(), bazel_cx.rules)`), decoded by
  `crates/starpls_bazel/src/build_language.rs::decode_rules`, which maps
  `Attribute.Discriminator` → a prose type string. **In Bazel 9 this yields the
  husks** (§5.1).

**bazel-lsp** (`cameron-martin/bazel-lsp`, last commit `48fead6` 2025-07-20 —
maintenance only). Different split, and it *documents the pipeline*
(`src/builtin/BUILD`):

```python
# The pbtxt file can be obtained from within bazel repository:
#   bazel build src/main/java/com/google/devtools/build/lib:gen_api_proto
#   cat bazel-bin/.../builtin.pb | protoc --decode=builtin.Builtins \
#       --proto_path src/main/protobuf builtin.proto > builtin.pbtxt
pbtxt_to_pb(name = "builtin", message_type = "builtin.Builtins", …)

# bazel info build-language | protoc --decode=blaze_query.BuildLanguage … > default_build_language.pbtxt
pbtxt_to_pb(name = "default_build_language", message_type = "blaze_query.BuildLanguage", …)
```

It checks in **text** protos (`builtin.pbtxt` 521 414 B, `default_build_language.pbtxt`
516 153 B — diffable in review) and re-encodes them at build time. Its
`builtin.pbtxt` deliberately contains **no rules at all** (11 BUILD globals, 92
BZL) — rules come from `build-language` instead. It also hardcodes a
`MISSING_GLOBALS` list of 30 symbols (`src/builtin.rs:15`) for
MODULE/WORKSPACE/BUILD gaps and converts HTML docs with the `htmd` crate.

**JetBrains `hirschgarten`** (last commit 2026-08-24 — the most active Bazel IDE
codebase). The reference implementation, and it does the most work:

- `tools/bazel_signatures/builtins/src/.../BazelBuiltinSignaturesGenerator.kt`
  clones `bazelbuild/bazel` at a tag, `git apply`s
  `resources/patches/api-exporter-enhancements@{7.5.0,8.0.0,9.0.0}.patch`,
  builds `//src/main/java/com/google/devtools/build/lib:gen_api_proto` with
  bazelisk, and converts to JSON.
- The 348-line 9.0.0 patch does two things upstream refuses to do:
  1. adds `is_positional` / `is_named` to `Param` and `MODULE`/`VENDOR`/`REPO`
     to `ApiContext` (they ship a forked `patched_builtin.proto` +
     pre-generated `PatchedBuiltinProtos.java`);
  2. re-adds exactly what PR #21135 was rejected for:
     ```java
     param.setType(attr.getTypeDesc());
     param.setDefaultValue(defaultValue);
     param.setDoc(attr.getUnexpandedHtmlDocumentation());
     ```
- It ships **14 pinned versions** in
  `intellij.bazel.core/resources/bazelSignatures/builtins/`:
  `builtins@7.5.0.json` (1 394 497 B) … `builtins@9.1.0.json` (1 444 848 B).
- `BazelBuiltinFunctionProvider.kt` picks the newest shipped version `<=` the
  project's Bazel release (from sync, falling back to `.bazelversion`, falling
  back to latest).

**tilt-dev/starlark-lsp**: last commit 2024-07-30 (`5689e7e`). **Stale**, and has
no Bazel builtin knowledge at all.
**bazel-stack-vscode**: last commit 2023-08-06. **Abandoned.**

---

## 2. Producing a `builtin.pb` — the experiment

Downloading one is impossible (§1.4), so I built two.

### Bazel 9.2.0

```sh
git clone --depth 1 --branch 9.2.0 https://github.com/bazelbuild/bazel.git /tmp/bazel-9.2.0-src
cd /tmp/bazel-9.2.0-src
bazel-9.2.0 --output_base=/tmp/ob-bazel920src build \
    --symlink_prefix=/tmp/bzlsym920/ \
    //src/main/java/com/google/devtools/build/lib:gen_api_proto
# INFO: Elapsed time: 143.481s, Critical Path: 37.47s
# INFO: 3312 processes: 313 internal, 2029 darwin-sandbox, 1012 worker.
```

| | |
|---|---|
| shallow clone | 31 MB, ~30 s |
| build | **143 s**, 3312 actions, ~350 MB output base |
| output | `bazel-out/darwin_arm64-fastbuild/bin/src/main/java/com/google/devtools/build/lib/builtin.pb` |
| size | **613 067 bytes** |
| sha256 | `cb310d56c69f94a7c7f9050d23f835fa813c44a9fc43f37818fd407c6e0ad420` |

No patches, no special flags, no `--config=remote` needed. It just works.

### Bazel master (10.0.0-prerelease, `3a9b19c8`)

Same command in `upstream/bazel`: **216 s**, 3806 actions,
**619 936 bytes**, sha256 `6d2bce72…`.

### Structure

```sh
protoc --decode=builtin.Builtins --proto_path=protos protos/builtin.proto < bazel-9.2.0-builtin.pb
```

| | 9.2.0 | 10.0-pre | starpls (≈8.0, patched) |
|---|---|---|---|
| bytes | 613 067 | 619 936 | 2 452 170 |
| `type` messages | 123 | 123 | 117 |
| `global` messages | 139 | 138 | 142 |
| `field`s across types | 567 | 587 | 527 |
| `param`s total | 3 308 | 3 298 | 3 026 |
| non-empty `doc` strings | 1 602 | 1 630 | **3 601** |
| globals by context | ALL 30 / BZL 46 / BUILD 63 | ALL 30 / BZL 46 / BUILD 62 | ALL 29 / BZL 48 / BUILD 65 |

The 4× size difference is entirely the missing per-attribute docs and types on
rules (starpls's patched build has them; stock Bazel does not).

Diff of the symbol sets, 9.2.0 → 10.0-prerelease (very stable across a major
version boundary):

```
only in 9.2.0     : global java_single_jar (BUILD); types bazel_py, j2objc, py
only in 10.0-pre  : types JavaInfo, JavaPluginInfo, java_output
```

Top-level type names in 9.2.0 (123): `Action AnalysisTestResultInfo Args Aspect
Attribute BuildSetting CcCompilationOutputs CcInfo CcLinkingOutputs
CcToolchainConfigInfo CcToolchainInfo CompilationContext ConstraintCollection …
actions apple apple_common attr bool builtin_function_or_method cc_common config
config_common ctx depset dict exec_result file_provider float fragments function
int java_common json list macro module_ctx native path platform_common proto
range repository_ctx repository_rule rule runfiles set string struct subrule_ctx
tag_class testing toolchain_type transition tuple wasm_exec_result wasm_module …`

---

## 3. Stardoc and `starlark_doc_extract`

### 3.1 Stardoc is a thin renderer now

`upstream/stardoc` — last commit `bd575db` **2026-06-23** (CI change only), last
release **0.8.1 on 2026-01-28** ("fixes compatibility issues with Bazel 9.0"),
**6 commits in the last 12 months**. Not abandoned, but in pure maintenance mode.

The README states plainly what it now is:

> Stardoc runs a [Velocity template] on the output of the
> [`native.starlark_doc_extract`] rule.
>
> Modules published to the Bazel Central Registry do not need to use Stardoc.
> They can simply publish the `starlark_doc_extract` outputs as a release artifact.

`docs/future_plans.md` (last updated January 2021) documents the migration that
has since completed: Stardoc used to *mock* Bazel's build language in Java;
they replaced that with real Bazel evaluation inside Bazel itself. The
motivating sentence is the correct guiding principle for an LSP too:

> *"the Build Language is more complicated and has only one accurate
> implementation: Bazel. Any tooling that operates on BUILD and .bzl files must
> carefully consider whether it is feasible to ask Bazel for the authoritative
> information. The alternative, falling back on simulation, not only produces
> less accurate results, but ties our hands as we try to improve Bazel."*

**For an LSP, Stardoc itself is irrelevant.** The Velocity/Markdown rendering
layer is pure loss. Target `starlark_doc_extract` directly.

### 3.2 `stardoc_output.proto`

`upstream/bazel/src/main/protobuf/stardoc_output.proto`, 436 lines,
`option java_package = "com.google.devtools.build.lib.starlarkdocextract"`.
**Byte-identical between 9.2.0 and master.** In Bazel 7 it lived at
`src/main/java/com/google/devtools/build/skydoc/rendering/proto/stardoc_output.proto`
and moved to `src/main/protobuf/` in Bazel 8.

Message shape (abridged; full text in the repo):

```proto
message ModuleInfo {
  repeated RuleInfo rule_info = 1;
  repeated ProviderInfo provider_info = 2;
  repeated StarlarkFunctionInfo func_info = 3;      // legacy macros + plain funcs
  repeated AspectInfo aspect_info = 4;
  string module_docstring = 5;
  string file = 6;                                   // display label of the .bzl
  repeated ModuleExtensionInfo module_extension_info = 7;
  repeated RepositoryRuleInfo repository_rule_info = 8;
  repeated MacroInfo macro_info = 9;                 // symbolic macros
  repeated StarlarkOtherSymbolInfo starlark_other_symbol_info = 10;
}

enum AttributeType { UNKNOWN=0; NAME=1; INT=2; LABEL=3; STRING=4; STRING_LIST=5;
  INT_LIST=6; LABEL_LIST=7; BOOLEAN=8; LABEL_STRING_DICT=9; STRING_DICT=10;
  STRING_LIST_DICT=11; OUTPUT=12; OUTPUT_LIST=13; LABEL_DICT_UNARY=14;
  LABEL_LIST_DICT=15; }

message AttributeInfo {
  string name = 1; string doc_string = 2; AttributeType type = 3;
  bool mandatory = 4;
  repeated ProviderNameGroup provider_name_group = 5;  // required providers
  string default_value = 6;
  bool nonconfigurable = 7;
  bool natively_defined = 8;         // inherited from Bazel, not declared in Starlark
  repeated string values = 9;        // attr.string(values=[...]) enum!
}

message OriginKey { string name = 1; string file = 2; }  // "<native>" for Java builtins
```

This is a **strictly better shape than `builtin.proto`** for everything an LSP
needs about rules:

- real `AttributeType` enum, not a prose/HTML string;
- `values` gives the enum for `attr.string(values=[…])` — direct attribute-value
  completion;
- `provider_name_group` + `origin_key` gives typed `deps` (which providers a dep
  must supply) *and* a goto-definition target for the provider;
- `OriginKey{name, file}` on rules, providers, aspects, macros, functions and
  extensions is a ready-made goto-definition index;
- `MacroInfo` covers symbolic macros; `FunctionParamRole` distinguishes
  positional-only / keyword-only / `*args` / `**kwargs`.

Its one gap is [issue #21979](https://github.com/bazelbuild/bazel/issues/21979):
`FunctionParamInfo` has no type annotation, so plain Starlark functions/legacy
macros are untyped.

### 3.3 Running it on a user's own repo — it works

Experiment: a hand-written `doc.bzl` with a provider, a rule and a legacy macro,
plus `starlark_doc_extract(name="doc_extract", src="doc.bzl")` in `BUILD.bazel`:

```
$ bazel build //:doc_extract
Target //:doc_extract up-to-date:
  bazel-bin/doc_extract.binaryproto            # 431 bytes
INFO: Elapsed time: 0.786s
INFO: 2 processes: 2 internal.
```

Decoded (`experiments/builtins/user-module-doc_extract.binaryproto`):

```
rule_info { rule_name: "my_rule" doc_string: "Does a thing."
  attribute { name: "name" type: NAME mandatory: true }
  attribute { name: "srcs" doc_string: "Sources" type: LABEL_LIST default_value: "[]" }
  attribute { name: "mode" doc_string: "Mode" type: STRING default_value: "\"fast\""
              values: "\"fast\"" values: "\"slow\"" }
  attribute { name: "dep"  doc_string: "A dep" type: LABEL
              provider_name_group { provider_name: "MyInfo"
                                    origin_key { name: "MyInfo" file: "//:doc.bzl" } }
              default_value: "None" }
  origin_key { name: "my_rule" file: "//:doc.bzl" } }
provider_info { provider_name: "MyInfo" doc_string: "My provider"
  field_info { name: "x" doc_string: "an x" }
  field_info { name: "y" doc_string: "a y" }
  origin_key { name: "MyInfo" file: "//:doc.bzl" } }
func_info { function_name: "my_macro"
  parameter { name: "name" mandatory: true role: PARAM_ROLE_ORDINARY }
  parameter { name: "visibility" default_value: "None" role: PARAM_ROLE_ORDINARY }
  parameter { name: "kwargs" role: PARAM_ROLE_KWARGS } … }
module_docstring: "Example user module."
file: "//:doc.bzl"
```

That is *exactly* the LSP knowledge base for user-defined rules — types,
defaults, enums, provider requirements, docs, and goto-definition keys.

**Cost**: it is an analysis-phase-only rule. `2 processes: 2 internal`, 0.15–1.8 s
for a warm server. No compilation, no toolchains, no execution.

### 3.4 Failure modes — and they are serious

**(a) `deps` must be declared, or analysis fails.**

```
$ bazel build //:cc_library_doc     # src = "@rules_cc//cc:cc_library.bzl"
ERROR: in deps attribute of starlark_doc_extract rule //:cc_library_doc:
  missing bzl_library targets for Starlark module(s)
  @@rules_cc++compatibility_proxy+cc_compatibility_proxy//:proxy.bzl
```

You must supply `bzl_library` targets covering the whole transitive `load()`
closure. Bazel's own `src/main/starlark/docgen/BUILD` needs 11 hand-listed deps
just for `proto.bzl`. Note also `bzl_library` from `bazel_skylib` has *filegroup
semantics* — it neither produces outputs nor validates completeness — so a
ruleset can have `bzl_library` targets that are silently wrong.

**(b) Wrapper macros destroy the result.** This is the big one. With deps fixed:

```
$ bazel build //:cc_library_doc   # @rules_cc//cc:cc_library.bzl, rules_cc 0.2.19
bazel-bin/cc_library_doc.binaryproto   # 117 bytes
```

decodes to, in full:

```
func_info { function_name: "cc_library"
  parameter { name: "kwargs" role: PARAM_ROLE_KWARGS }
  origin_key { name: "cc_library" file: "@rules_cc//cc:cc_library.bzl" } }
module_docstring: "cc_library rule"
```

The public `cc_library` symbol is `def cc_library(**kwargs)`. **Zero attributes.**
The real rule is two directories down:

```
$ bazel build //:cc_library_impl_doc   # @rules_cc//cc/private/rules_impl:cc_library.bzl
bazel-bin/cc_library_impl_doc.binaryproto   # 25 642 bytes
rule_name: "cc_library"  — 26 attributes:
  name:NAME srcs:LABEL_LIST module_interfaces:LABEL_LIST data:LABEL_LIST
  includes:STRING_LIST local_includes:STRING_LIST defines:STRING_LIST
  local_defines:STRING_LIST copts:STRING_LIST conlyopts:STRING_LIST
  cxxopts:STRING_LIST hdrs_check:STRING additional_linker_inputs:LABEL_LIST
  additional_compiler_inputs:LABEL_LIST win_def_file:LABEL hdrs:LABEL_LIST
  textual_hdrs:LABEL_LIST deps:LABEL_LIST implementation_deps:LABEL_LIST
  strip_include_prefix:STRING include_prefix:STRING alwayslink:BOOLEAN
  linkstatic:BOOLEAN linkstamp:LABEL linkopts:STRING_LIST licenses:STRING_LIST
```

You need `--check_visibility=false` to reach `//cc/private/...` from outside, and
you need out-of-band knowledge of *which private file* holds the real rule.
JetBrains's `StardocSignaturesGenerator` hardcodes exactly these paths (e.g.
`--bzl-file //go/private/rules:library.bzl --dep //go:def` for rules_go) and
passes `--check_visibility=false`.

**(c) Common attributes are missing.** The `starlark_doc_extract` output for
`cc_library` above has **26** attributes; the same rule via
`--proto:rule_classes` (§5.2) has **42**, because the latter includes
inherited native attributes (`visibility`, `tags`, `testonly`, `features`,
`compatible_with`, `toolchains`, `exec_properties`, `target_compatible_with`, …)
marked `natively_defined: true`. JetBrains works around this with a hand-written
`RuleCommonParams.kt` merged in at generation time, commented:
*"Should not be needed after https://github.com/bazelbuild/stardoc/issues/292 is
resolved."*

**(d) It requires a *successful analysis*.** A repo whose `.bzl` files don't
analyse (missing dep, broken toolchain, `fail()` at load time) yields nothing.

---

## 4. The Starlarkification problem

### 4.1 The official story

From the [Bazel 9 LTS announcement](https://blog.bazel.build/2026/01/20/bazel-9.html)
(2026-01-20):

> In Bazel 8.0, we moved most built-in rules into their own modules; and in 9.0,
> we've completed the migration with the removal of built-in C++ rules into
> `rules_cc`. Bazel now ships with a lean core with rich APIs, and all previously
> built-in language-specific rulesets are now shipped separately and can be
> updated independently from a Bazel release.

### 4.2 What is *actually* still native in Bazel 9.2.0 (measured)

I probed a live 9.2.0 server three ways.

**(i) `dir(native)` from a `.bzl` (58 entries):**

```
action_listener alias cc_binary cc_import cc_libc_top_alias cc_library
cc_shared_library cc_static_library cc_test cc_toolchain cc_toolchain_alias
cc_toolchain_suite config_feature_flag config_setting constraint_setting
constraint_value environment existing_rule existing_rules exports_files
extra_action fdo_prefetch_hints fdo_profile filegroup genquery genrule glob
java_binary java_import java_library java_package_configuration java_plugin
java_plugins_flag_alias java_runtime java_test java_toolchain label_flag
label_setting legacy_globals memprof_profile module_name module_version
objc_import objc_library package package_default_visibility package_group
package_name package_relative_label platform propeller_optimize repo_name
repository_name starlark_doc_extract subpackages test_suite toolchain
toolchain_type
```

**(ii) Undefined-name probe in a `BUILD` file.** Referencing each candidate
symbol and collecting `name 'X' is not defined` errors. **Not defined in a Bazel
9.2.0 `BUILD` file:**

```
bind  local_repository  new_local_repository  distribs
cc_proto_library  java_lite_proto_library  java_proto_library
proto_lang_toolchain  proto_library  py_proto_library
py_binary  py_library  py_runtime  py_test
sh_binary  sh_library  sh_test
(plus the expected .bzl-only ones: native struct provider rule aspect attr)
```

**(iii) Calling a defined one:**

```
$ cat BUILD.bazel
cc_library(name = "foo", srcs = ["foo.cc"], hdrs = ["foo.h"])
$ bazel query //:all
ERROR: Traceback (most recent call last):
	File "/private/tmp/bzws/BUILD.bazel", line 1, column 11, in <toplevel>
		cc_library(name = "foo", …)
	File "/virtual_builtins_bzl/bazel/exports.bzl", line 40, column 9, in _removed_rule_failure
Error in fail:
         This rule has been removed from Bazel. Please add a `load()` statement for it.
         This can also be done automatically by running:
         buildifier --lint=fix <path-to-BUILD-or-bzl-file>
```

and for `java_library`:

```
ERROR: //:j: no such attribute 'srcs' in 'java_library' rule
```

So the classification for Bazel 9.2.0 is **four-way**, not two-way. Of the 58
entries in `dir(native)`, 43 are rule classes (exactly matching
`build-language`) and 15 are package functions: `existing_rule`,
`existing_rules`, `exports_files`, `glob`, `legacy_globals`, `module_name`,
`module_version`, `package`, `package_default_visibility`, `package_group`,
`package_name`, `package_relative_label`, `repo_name`, `repository_name`,
`subpackages`. The 43 rule classes break down as:

| Class | Count | Symbols | Behaviour |
|---|---|---|---|
| **Real native rules, usable** | 20 | `action_listener alias cc_libc_top_alias config_feature_flag config_setting constraint_setting constraint_value environment extra_action filegroup genquery genrule java_plugins_flag_alias label_flag label_setting platform starlark_doc_extract test_suite toolchain toolchain_type` | work; full attribute sets in `build-language` |
| **Poison-pill globals** | 15 | `_REMOVED_RULES` = `cc_binary cc_import cc_library cc_shared_library cc_static_library cc_test cc_toolchain cc_toolchain_alias fdo_prefetch_hints fdo_profile memprof_profile propeller_optimize objc_import objc_library`, plus `cc_toolchain_suite` (silently aliased to `filegroup`) | defined; `fail()` on call |
| **Husks with no poison pill** | 8 | `java_binary java_import java_library java_package_configuration java_plugin java_runtime java_test java_toolchain` | defined and callable, but only common attributes: `no such attribute 'srcs' in 'java_library' rule` |
| **Gone entirely** | 15 | `py_binary py_library py_runtime py_test sh_binary sh_library sh_test proto_library cc_proto_library java_proto_library java_lite_proto_library py_proto_library proto_lang_toolchain bind local_repository new_local_repository distribs` (not all are rule classes) | `name 'X' is not defined` |

22 of the 43 rule classes are **attribute-less husks**, identifiable by the hidden
`$bzl_load_label` attribute: the 14 `cc_*`/`objc_*`/`fdo_*`/`memprof_profile`/
`propeller_optimize` ones (minus `cc_toolchain_alias`, which is a real rule class
shadowed by a poison-pill global) plus `cc_toolchain_suite` and the 8 `java_*`.

Also defined in a `BUILD` file but *not* members of `native`: `licenses`,
`environment_group`, `select`, `print`, `fail`.

`java_*` is the subtle one: `java_library` is *defined and callable* in a `BUILD`
file, appears in `dir(native)` and in `build-language` — but it has **no `srcs`**.
It is a husk that only exists so `rules_java` can bind to the rule class name.

`src/main/starlark/builtins_bzl/bazel/exports.bzl` (identical in 9.2.0 and
master) is the whole of it:

```python
_REMOVED_RULES = [
    "cc_binary", "cc_import", "cc_library", "cc_shared_library",
    "cc_static_library", "cc_test", "cc_toolchain", "cc_toolchain_alias",
    "fdo_prefetch_hints", "fdo_profile", "memprof_profile",
    "propeller_optimize", "objc_import", "objc_library",
]

def _removed_rule_failure(**_kwargs):
    fail("""
         This rule has been removed from Bazel. Please add a `load()` statement for it.
         …""")

exported_toplevels = {
    "py_internal": py_internal, "java_common": java_common_export_for_bazel,
    "cc_common": cc_common, "apple_common": apple_common_bazel,
}
exported_rules = {
    "cc_toolchain_suite": lambda name, **kwargs: _builtins.toplevel.native.filegroup(name = name),
} | {rule_name: _removed_rule_failure for rule_name in _REMOVED_RULES}
```

The husk mechanism is `BaseRuleClasses.EmptyRule`
(`src/main/java/com/google/devtools/build/lib/analysis/BaseRuleClasses.java:433-497`):

```java
public EmptyRule(String name, @Nullable String bzlLoadLabel) { … }
…
return builder.removeAttribute("deps").removeAttribute("data")
    .addAttribute(attr("$bzl_load_label", STRING).value(this.bzlLoadFile).build())
    .build();
…
// EmptyRuleConfiguredTargetFactory.create:
ruleContext.ruleError("""
    The %s rule has been removed, add the following to your BUILD/bzl file:

    load("%s", "%s")
    """.formatted(ruleName, bzlLoadLabel, ruleName));
```

**A hidden `$bzl_load_label` attribute is the machine-readable marker for
"this rule class is a husk"** — and it carries the correct `load()` label. That
is a genuinely useful signal an LSP can use for a quick-fix (§5.1).

### 4.3 Autoloading — `AutoloadSymbols.java`

`--incompatible_autoload_externally` is the compatibility bridge: it silently
rewrites the `BUILD`/`.bzl` environment so that removed symbols resolve to their
new external definitions.

**In Bazel 9.2.0** it exists but is **off by default**:

```java
// src/main/java/.../packages/semantics/FlagConstants.java:30 (tag 9.2.0)
public static final String DEFAULT_INCOMPATIBLE_AUTOLOAD_EXTERNALLY = "";
```
and its companion `--incompatible_disable_autoloads_in_main_repo` defaults to
`true`. So in a stock Bazel 9 workspace, autoloads do nothing and everything must
be explicitly `load()`ed.

**In Bazel master (10.0-pre) `AutoloadSymbols.java` has been deleted**
(commit `a3dc34c545` *"Remove AutoloadSymbols and deprecate related flags"*,
**2026-02-03**). All three flags are now no-ops:

```java
// BazelRulesModule.java:936-966
@Deprecated @Option(name = "incompatible_autoload_externally",
    effectTags = {OptionEffectTag.NO_OP}, help = "Deprecated. No-op.")
@Deprecated @Option(name = "repositories_without_autoloads", … "Deprecated. No-op.")
@Deprecated @Option(name = "incompatible_disable_autoloads_in_main_repo", defaultValue="true", … NO_OP)
```

The full `AUTOLOAD_CONFIG` table from Bazel 9.2.0
(`src/main/java/com/google/devtools/build/lib/packages/AutoloadSymbols.java:675-859`)
is the canonical **symbol → external repo** map. `ruleRedirect` = a rule (goes
into `native.*` and the `BUILD` env); `symbolRedirect` = a top-level symbol;
`renamedSymbolRedirect` = renamed on the way out.

The table has **61** entries: **43** `ruleRedirect`, **16** `symbolRedirect`,
**2** `renamedSymbolRedirect`.

**Non-rule symbols (18):**

| Symbol | Load label | Renamed to |
|---|---|---|
| `CcSharedLibraryInfo` | `@rules_cc//cc/common:cc_shared_library_info.bzl` | |
| `CcSharedLibraryHintInfo` | `@rules_cc//cc/common:cc_shared_library_hint_info.bzl` | |
| `cc_proto_aspect` | `@com_google_protobuf//bazel/private:bazel_cc_proto_library.bzl` | |
| `ProtoInfo` | `@com_google_protobuf//bazel/common:proto_info.bzl` | |
| `proto_common_do_not_use` | `@com_google_protobuf//:dummy.bzl` | |
| `cc_common` | `@rules_cc//cc/common:cc_common.bzl` | |
| `CcInfo` | `@rules_cc//cc/common:cc_info.bzl` | |
| `DebugPackageInfo` | `@rules_cc//cc/common:debug_package_info.bzl` | |
| `CcToolchainConfigInfo` | `@rules_cc//cc/toolchains:cc_toolchain_config_info.bzl` | |
| `java_common` | `@rules_java//java/common:java_common.bzl` | |
| `JavaInfo` | `@rules_java//java/common:java_info.bzl` | |
| `JavaPluginInfo` | `@rules_java//java/common:java_plugin_info.bzl` | |
| `ProguardSpecProvider` | `@rules_java//java/common:proguard_spec_info.bzl` | **`ProguardSpecInfo`** |
| `PyInfo` | `@rules_python//python:py_info.bzl` | |
| `PyRuntimeInfo` | `@rules_python//python:py_runtime_info.bzl` | |
| `PyCcLinkParamsProvider` | `@rules_python//python:py_cc_link_params_info.bzl` | **`PyCcLinkParamsInfo`** |
| `AndroidIdeInfo` | `@rules_android//providers:providers.bzl` | |
| `apple_common` | `@build_bazel_apple_support//lib:apple_common.bzl` | |

**Rules (`ruleRedirect`), 43:**

| Rules | Load label |
|---|---|
| `aar_import android_binary android_library android_local_test android_sdk android_tools_defaults_jar` | `@rules_android//rules:rules.bzl` |
| `cc_binary` | `@rules_cc//cc:cc_binary.bzl` |
| `cc_import` | `@rules_cc//cc:cc_import.bzl` |
| `cc_library` | `@rules_cc//cc:cc_library.bzl` |
| `cc_shared_library` | `@rules_cc//cc:cc_shared_library.bzl` |
| `cc_test` | `@rules_cc//cc:cc_test.bzl` |
| `cc_toolchain` | `@rules_cc//cc/toolchains:cc_toolchain.bzl` |
| `cc_toolchain_suite` | `@rules_cc//cc/toolchains:cc_toolchain_suite.bzl` |
| `fdo_prefetch_hints` | `@rules_cc//cc/toolchains:fdo_prefetch_hints.bzl` |
| `fdo_profile` | `@rules_cc//cc/toolchains:fdo_profile.bzl` |
| `memprof_profile` | `@rules_cc//cc/toolchains:memprof_profile.bzl` |
| `propeller_optimize` | `@rules_cc//cc/toolchains:propeller_optimize.bzl` |
| `objc_import` | `@rules_cc//cc:objc_import.bzl` |
| `objc_library` | `@rules_cc//cc:objc_library.bzl` |
| `cc_proto_library` | `@com_google_protobuf//bazel:cc_proto_library.bzl` |
| `proto_library` | `@com_google_protobuf//bazel:proto_library.bzl` |
| `java_proto_library` | `@com_google_protobuf//bazel:java_proto_library.bzl` |
| `java_lite_proto_library` | `@com_google_protobuf//bazel:java_lite_proto_library.bzl` |
| `proto_lang_toolchain` | `@com_google_protobuf//bazel/toolchains:proto_lang_toolchain.bzl` |
| `java_binary java_import java_library java_plugin java_test` | `@rules_java//java:<rule>.bzl` |
| `java_package_configuration java_runtime java_toolchain` | `@rules_java//java/toolchains:<rule>.bzl` |
| `py_binary py_library py_runtime py_test` | `@rules_python//python:<rule>.bzl` |
| `sh_binary sh_library sh_test` | `@rules_shell//shell:<rule>.bzl` |
| `available_xcodes xcode_config xcode_config_alias xcode_version` | `@build_bazel_apple_support//xcode:<rule>.bzl` |

Repos in which autoloads were **never** applied (to break cycles),
`PREDECLARED_REPOS_DISALLOWING_AUTOLOADS`:

```
protobuf, com_google_protobuf, proto_bazel_features, rules_android, rules_cc,
rules_java, rules_java_builtin, compatibility_proxy, rules_python,
rules_python_internal, rules_shell, apple_support, build_bazel_apple_support,
bazel_skylib, bazel_tools, bazel_features, bazel_features_version,
bazel_features_globals
```

Buildifier keeps a **fourth independent copy** of this map
(`upstream/buildtools/tables/tables.go:215-251`: `CcLoadPathPrefix =
"@rules_cc//cc"`, `JavaLoadPathPrefix = "@rules_java//java"`,
`ShellLoadPathPrefix = "@rules_shell//shell"`, `PyLoadPath =
"@rules_python//python:defs.bzl"`, `ProtoLoadPathPrefix = "@protobuf//bazel"`,
`AndroidLoadPath = "@rules_android//android:rules.bzl"`), driving ~40 warnings in
`warn/warn.go:180-220` (`native-cc-library`, `native-sh-binary`, …) with
autofixes. `buildifier --lint=fix` inserting the right `load()` is exactly what
Bazel's own error message tells users to run.

### 4.4 The consequence, measured

A static builtin database keyed only on Bazel version is now **wrong in both
directions**. Comparing the `api_context: BUILD` global set in
Bazel 9.2.0's own freshly built `builtin.pb` (63 entries) with what a
Bazel 9.2.0 `BUILD` file actually accepts (60 entries):

**In `builtin.pb` but not defined at all (15) — false completions, missed errors:**

```
cc_proto_library  java_lite_proto_library  java_proto_library  java_single_jar
proto_lang_toolchain  proto_library  proto_toolchain  py_binary  py_library
py_proto_library  py_runtime  py_test  sh_binary  sh_library  sh_test
```

**In `builtin.pb`, defined, but a poison pill that `fail()`s (13):**

```
cc_binary cc_import cc_library cc_shared_library cc_static_library cc_test
cc_toolchain fdo_prefetch_hints fdo_profile memprof_profile objc_import
objc_library propeller_optimize
```

**Actually available but missing from `builtin.pb` (12) — false "unknown symbol":**

```
cc_libc_top_alias  cc_toolchain_alias  cc_toolchain_suite  config_feature_flag
environment  environment_group  java_plugins_flag_alias  label_flag
label_setting  licenses  package  select
```

Only 35 of 63 are both present and correct. **`builtin.pb`'s BUILD context is now
a documentation index, not an environment description**, because Bazel's own
build injects the externalized rulesets' Stardoc protos back into it (§4.5).

And the correctness of the surviving entries is **per-workspace**: whether
`cc_library` is valid at all depends on whether the user's `MODULE.bazel` has
`bazel_dep(name = "rules_cc")`; *which* attributes it has depends on **which
version** of `rules_cc` resolves. `hdrs_check`, `local_includes`,
`module_interfaces` and `implementation_deps` exist in rules_cc 0.2.19 and did
not exist in older versions.

### 4.5 Bazel's own workaround, and what it pins

Bazel 9/10 build the C++/Java/Python/shell/proto sections of both the Build
Encyclopedia and `builtin.pb` by re-importing the externalized rules and running
`starlark_doc_extract` on shim files. `src/main/starlark/docgen/`:

```python
starlark_doc_extract(name = "gen_be_cpp_stardoc_proto", src = "cpp.bzl",
    deps = ["@rules_cc//cc:core_rules", "@rules_cc//cc/toolchains:toolchain_rules"])
starlark_doc_extract(name = "gen_be_shell_stardoc_proto", src = "sh.bzl",
    deps = ["@rules_shell//shell:rules_bzl"])
…
```

`cpp.bzl` is nothing but re-exports:

```python
"""C / C++"""
# Build Encyclopedia entry point for C / C++ rules implemented in Starlark in Blaze's @_builtins
load("@cc_compatibility_proxy//:proxy.bzl", "cc_binary", "cc_import", "cc_library", …)
load("@rules_cc//cc/toolchains:cc_toolchain.bzl", "cc_toolchain")
library_rules = struct(cc_library = cc_library, cc_import = cc_import, …)
```

`sh.bzl` reaches into private files, with a lint suppression:

```python
load("@rules_shell//shell/private:sh_binary.bzl", "sh_binary")  # buildifier: disable=bzl-visibility
```

**Therefore `builtin.pb` hardcodes the ruleset versions from Bazel's own
`MODULE.bazel`:**

| | Bazel 9.2.0 | master (10.0-pre) |
|---|---|---|
| `rules_cc` | 0.2.17 | 0.2.19 |
| `rules_java` | 9.1.0 | 9.7.2 |
| `rules_python` | 1.7.0 | 1.7.0 |
| `rules_shell` | 0.6.1 | 0.6.1 |
| `protobuf` | 33.4 | 36.0 |
| `apple_support` | 1.24.5 | 2.5.2 |
| `bazel_skylib` | 1.8.2 | 1.9.0 |

A user on Bazel 9.2.0 with `rules_cc 0.4.x` gets docs for 0.2.17.

---

## 5. Alternative sources

### 5.1 `bazel info build-language` — still present, largely useless

It exists in Bazel 9.2.0 (`bazel help info-keys` lists
`build-language  A protobuffer with the build language structure`) and starpls
calls it on every startup. Output is `blaze_query.BuildLanguage`
(`src/main/protobuf/build.proto`), **33 569 bytes**, **43 rule classes**:

```
action_listener alias cc_binary cc_import cc_libc_top_alias cc_library
cc_shared_library cc_static_library cc_test cc_toolchain cc_toolchain_alias
cc_toolchain_suite config_feature_flag config_setting constraint_setting
constraint_value environment extra_action fdo_prefetch_hints fdo_profile
filegroup genquery genrule java_binary java_import java_library
java_package_configuration java_plugin java_plugins_flag_alias java_runtime
java_test java_toolchain label_flag label_setting memprof_profile objc_import
objc_library platform propeller_optimize starlark_doc_extract test_suite
toolchain toolchain_type
```

**22 of the 43 are husks** (identified by the hidden `$bzl_load_label` attribute):
`cc_binary cc_import cc_library cc_shared_library cc_static_library cc_test
cc_toolchain cc_toolchain_suite fdo_prefetch_hints fdo_profile java_binary
java_import java_library java_package_configuration java_plugin java_runtime
java_test java_toolchain memprof_profile objc_import objc_library
propeller_optimize`.

```
### cc_library (23 attrs, 20 public)
  $bzl_load_label:STRING, $config_dependencies:LABEL_LIST, :action_listener:LABEL_LIST,
  aspect_hints, compatible_with, deprecation, distribs, exec_compatible_with,
  exec_group_compatible_with, exec_properties, features, generator_function,
  generator_location, generator_name, licenses, name, package_metadata,
  restricted_to, tags, target_compatible_with, testonly, transitive_configs, visibility
```

No `srcs`, no `hdrs`, no `deps`. Only 21 rule classes carry real attributes
(`genrule` 39, `starlark_doc_extract` 28, `genquery` 29, `filegroup` 23, …).

It has never covered Starlark-defined rules, so in Bazel 9 it is **worth exactly
one thing**: the `$bzl_load_label` value tells you which `load()` a husk needs.
Everything else in it is a subset of what §5.2 gives you, typed better.

### 5.2 `bazel query --output=proto --proto:rule_classes=true` — the best live source

Added in **Bazel 8.0** (`rule_class_info` is present in `build.proto` for 8.0.0,
8.3.0, 8.7.0, 9.2.0, master; **absent in 7.6.0**). `build.proto`:

```proto
// A key uniquely identifying the rule's rule class. Stable between repeated
// blaze query invocations …
optional string rule_class_key = 16;

// Stardoc-format rule class API definition for this rule. Includes both
// Starlark-defined and native (including inherited) attributes; does not
// include hidden or explicitly undocumented attributes.
//
// Populated only for the first rule in the stream with a given rule_class_key.
optional stardoc_output.RuleInfo rule_class_info = 17;
```

Experiment, workspace with `bazel_dep(name = "rules_cc", version = "0.2.19")` and
one `cc_library`:

```
rule { name: "//:foo"  rule_class: "cc_library"  location: "…/BUILD.bazel:3:11"
       rule_class_key: "@@rules_cc+//cc/private/rules_impl:cc_library.bzl%cc_library"
       rule_class_info { rule_name: "cc_library" doc_string: "<p>Use <code>cc_library()</code>…"
         attribute { name: "name" type: NAME mandatory: true }
         attribute { name: "hdrs" type: LABEL_LIST default_value: "[]" }
         attribute { name: "deps" type: LABEL_LIST
                     provider_name_group { provider_name: "CcInfo"
                                           origin_key { name: "CcInfo" file: "<native>" } } }
         attribute { name: "visibility" type: LABEL_LIST nonconfigurable: true
                     natively_defined: true }
         … 42 attributes total … } }
```

**This answers `cc_library.hdrs : label_list` exactly, in a real workspace, with
the resolved version of rules_cc, without patching Bazel.**

Cost, measured on bazel-skylib HEAD (`//...`, 437 targets):

| | time | bytes |
|---|---|---|
| cold server + query | 14.9 s | 947 324 |
| **warm, `--proto:rule_classes=true`** | **0.14–0.15 s** | 947 324 (140 distinct rule classes) |
| warm, without `--proto:rule_classes` | 0.27 s | 708 774 |

The marginal cost of `rule_class_info` is ~240 KB and effectively zero time; it
is deduplicated per `rule_class_key`.

Limitations:

- **Only covers rule classes that are actually instantiated.** A rule the user
  hasn't used yet is invisible. (Mitigation: it's exactly the set you need for
  completing attributes on existing targets, and it grows as the user writes.)
- **`rule_name` is the *defining* name, not the call name.** The proto says so:
  *"In query output, this is the name under which the rule was defined (which
  might be a private symbol prefixed with `_`)."* bazel-skylib yields
  `_copy_file`, `_write_file`, `_empty_test`, … because those rules are wrapped
  in macros. You have to reconcile via `rule_class_key` (which carries
  `@@repo//pkg:file.bzl%symbol`) and the target's `location`, and for macros the
  mapping call-name → rule-class is many-to-one or absent.
- No provider/aspect/module-extension info, no plain function signatures.
- Requires the packages to load cleanly.

`--output=streamed_proto` works with it (verified: length-delimited
`blaze_query.Target` stream, 24 812 B for `//lib/...`) and is the right choice for
an LSP — you can parse incrementally and stop early. Other relevant flags in
9.2.0's `bazel help query`: `--proto:definition_stack`,
`--proto:output_rule_attrs` (default `all`), `--noproto:rule_inputs_and_outputs`
(cuts most of the bulk when you only want rule classes), `--proto:default_values`
(default true), `--proto:include_starlark_rule_env` (default true),
`--proto:include_attribute_source_aspects`, `--output_file` (added 7.5.0).

### 5.3 `@_builtins` / `src/main/starlark/builtins_bzl/`

In master this directory is down to **17 `.bzl` files** and contains no rule
implementations at all:

```
bazel/exports.bzl
common/{builtin_exec_platforms,exports,paths,util}.bzl
common/cc/{cc_common,cc_common_bazel,cc_helper_internal}.bzl
common/java/java_common.bzl
common/objc/{apple_common,apple_env,apple_platform,apple_toolchain,objc_info}.bzl
common/python/py_internal.bzl
common/xcode/providers.bzl
exports.bzl
```

`common/exports.bzl` reduces to:

```python
exported_toplevels = {"_builtins_dummy": "overridden value",
                      "proto_common_do_not_use": struct(...)}
exported_rules = {}          # "A list of Starlarkified native rules."  — empty
exported_to_java = {}
```

Historically (Bazel 6/7) this was where `cc_library.bzl`, `py_binary.bzl` etc.
lived and it *was* very informative. **In Bazel 9/10 it is no longer a useful
source** — the only remaining value is `bazel/exports.bzl`'s `_REMOVED_RULES`
list (§4.2), which is the authoritative "poison pill" set.

The `README.md` does note a debugging lever worth knowing:
`--experimental_builtins_bzl_path=%workspace%` makes edits to this directory take
effect without a server restart.

### 5.4 BCR `docs_url` — real, but only 4.6 % coverage

`bazel-central-registry/docs/stardoc.md` defines the convention: put
`"docs_url"` in `modules/<name>/<version>/source.json` pointing at a tarball of
`starlark_doc_extract` `.binaryproto` outputs.

Measured on a fresh sparse clone of the BCR (2026-08-25):

| | |
|---|---|
| modules | **1 250** |
| `source.json` files | 8 953 |
| `source.json` with `docs_url` | 483 |
| **distinct modules with `docs_url`** | **58 (4.6 %)** |

The 58: `apple_support aspect_rules_esbuild aspect_rules_jasmine aspect_rules_js
aspect_rules_lint aspect_rules_py aspect_rules_swc aspect_rules_terser
aspect_rules_ts awk.bzl bazel_env.bzl bazel_features bazel_lib
bazel_linux_packages bazeldnf bazelrc-preset.bzl cargo_env.bzl
com_clementguillot_rules_quarkus diff.bzl gateweavers_rules_vhdl gazelle_d
hermetic_launcher jq.bzl lit2md llvm platforms_contrib pypackaging.bzl
rules_appimage rules_apple rules_auto_dotnet rules_cc_hdrs_map rules_conda
rules_d rules_distroless rules_docs rules_dotnet rules_doxygen rules_elm
rules_formatjs rules_gitops rules_img rules_itest rules_multitool rules_nodejs
rules_rs rules_ruby rules_runfiles_group rules_shellspec rules_swift
rules_swift_previews rules_swift_resources sha256.bzl tar.bzl
toolchains_llvm_bootstrapped windows_support xml.bzl yaml.bzl yq.bzl`

Aspect Build and the Apple rules dominate. **`rules_cc`, `rules_java`,
`rules_python`, `rules_shell`, `rules_go`, `protobuf` — the ones that matter most
after Starlarkification — do not publish `docs_url`.**

Verified round trip:

```
$ curl -sL https://github.com/aspect-build/rules_js/releases/download/v3.4.1/rules_js-v3.4.1.docs.tar.gz | tar tz
./js/defs.doc_extract.binaryproto
./js/rpc.doc_extract.binaryproto
./js/proto.doc_extract.binaryproto
./contrib/nextjs/defs.doc_extract.binaryproto
./npm/extensions.doc_extract.binaryproto
./npm/defs.doc_extract.binaryproto
```
29 287 bytes gzipped; decodes cleanly as `stardoc_output.ModuleInfo` with full
`js_library` attribute types and docs. `https://registry.bazel.build/` renders
these (source: `bazel-contrib/bcr-ui`).

This is the **only Bazel-free, network-only, machine-readable ruleset API source**
that exists, and it is exactly the right format. Coverage is the problem, and it
is worth actively pushing rulesets to adopt it.

### 5.5 Generated `docs/*.md` in ruleset repos

`bazel-skylib` (HEAD 2026-07-29) ships 28 `docs/*_doc.md` generated by
`stardoc_with_diff_test` (`docs/BUILD`), which also *tests* that they're current.
`rules_cc`, `rules_python`, `rules_shell` have `docs/` directories too. These are
**Markdown tables** — they'd have to be scraped and parsed. Given that the same
repos can be fed to `starlark_doc_extract` directly, and that the BCR convention
publishes the underlying protos, **scraping generated Markdown is strictly worse
than every other option**. Do not do it.

### 5.6 Scraping `bazel.build`

`https://bazel.build/rules/lib/globals/build`, `…/rules/lib/builtins/ctx`,
`https://bazel.build/reference/be/c-cpp` are live HTML (433 KB and 765 KB
respectively). They are generated from the exact same `RuleDocumentation`/
Stardoc pipeline that produces `builtin.pb`, so scraping them is a lossy,
brittle re-derivation of data you can get as a proto. They also lag: they
describe "latest" only, with no version selector, and `bazel.build/rules/lib/globals/build`
no longer contains `cc_library` at all.

One legitimate use: the HTML docs are the *only* place some prose lives after
`ApiExporter` drops it. Not worth it.

---

## 6. Ranked recommendation

The decision is not "which one" — every serious implementation uses at least two.
The question is what to build first and what to skip.

### Tier 1 — build these

**1. Pinned, per-Bazel-version `builtin.pb`, generated by us, shipped in-tree.**

- Covers: the Starlark API (`ctx`, `actions`, `attr`, `depset`, `rule`,
  `provider`, `aspect`, `cc_common`, `java_common`, all provider types, all
  builtin types and methods, `select`, `glob`, …). This is ~90 % of what hover,
  signature help and `.bzl` type checking need, and it is genuinely
  version-stable (only 4 symbols changed between 9.2.0 and 10.0-prerelease).
- Cost, measured: `git clone --depth 1 --branch <tag>` (31 MB, ~30 s) +
  `bazel build //src/main/java/com/google/devtools/build/lib:gen_api_proto`
  (**143 s for 9.2.0**, 216 s for master), 613 KB output. Fully scriptable in CI;
  no patches required for the Starlark-API half.
- Ship one per minor release, select at runtime by the project's Bazel version
  (JetBrains's `BUILTIN_AVAILABLE_VERSIONS` + "newest ≤ current" is the right
  policy; 14 files × ~1.4 MB JSON is an acceptable binary size, and 613 KB × N as
  raw protos is cheaper still).
- **Do not** trust its `api_context: BUILD` list (44 % wrong, §4.4). Use only its
  `type` messages and its `ALL`/`BZL` globals; take the BUILD environment from
  tier-1 item 3 below.
- Decide early whether to carry a `collectRuleInfo` patch (types + defaults +
  docs on rule attributes, as JetBrains and starpls both do). **My advice: don't.**
  It makes your artifact non-reproducible from upstream and duplicates what
  `--proto:rule_classes` gives you correctly and per-workspace. Carry the
  `ApiContext` MODULE/REPO/VENDOR patch only if you find hand-maintaining those
  globals worse than maintaining a patch — starpls's 29 KB
  `module-bazel.builtins.json` suggests hand-maintenance is tolerable.

**2. `bazel query --output=streamed_proto --proto:rule_classes=true` as the
per-workspace rule database.**

- Covers: every rule class the workspace actually instantiates, with real
  `AttributeType`, `default_value`, `values` enums, `mandatory`,
  `nonconfigurable`, `natively_defined`, required-provider groups, and
  `OriginKey` for goto-definition — at the *resolved* ruleset version.
- Cost: 0.14 s warm for 437 targets / 140 rule classes / 947 KB; ~15 s cold
  (server start + package loading). Refresh on `BUILD`/`MODULE.bazel` change,
  debounced, in the background — the same lifecycle starpls already uses for
  `query_all_workspace_targets`.
- Requires Bazel ≥ 8.0. For Bazel 7 fall back to `bazel info build-language`.
- Known gap: macro-wrapped rules report the private rule name; reconcile through
  `rule_class_key` (`@@repo//pkg:file.bzl%symbol`) and the target `location`.

**3. A hardcoded Starlarkification table, refreshed per Bazel version.**

Three facts that no single artifact gives you and that you cannot compute:
(a) which symbols are *not defined* in a BUILD file (15 in 9.2.0);
(b) which are defined but poison pills (13 in 9.2.0, from
`builtins_bzl/bazel/exports.bzl::_REMOVED_RULES`);
(c) the correct `load()` label for each (from `AutoloadSymbols.AUTOLOAD_CONFIG`,
or at runtime from the `$bzl_load_label` attribute in `build-language`).

This powers the single highest-value Bazel-9 diagnostic — *"`cc_library` is not
global anymore, load it from `@rules_cc//cc:cc_library.bzl`"* — with an
auto-fix. Both `AutoloadSymbols.java` (through 9.x; deleted at master) and
`buildtools/tables/tables.go` are extractable sources; the latter is
version-independent and actively maintained, so **derive from buildifier's
tables and cross-check against `AutoloadSymbols` per Bazel tag**. Better still:
shell out to `buildifier --lint=fix` for the fix itself and only own the
diagnostic.

### Tier 2 — build after the core works

**4. `starlark_doc_extract` over the user's own `.bzl` files.**

- Covers exactly the thing nothing else covers: the user's own rules, providers,
  symbolic macros, aspects, module extensions and repository rules, before they
  are instantiated anywhere.
- Cost: generate a synthetic package of `starlark_doc_extract` targets and build
  it (0.15–1.8 s per file, analysis-only). Same trick JetBrains uses
  (`ij_stardoc_gen/BUILD.bazel` + `--check_visibility=false`).
- Failure modes that will bite (§3.4): missing `bzl_library` deps break analysis;
  wrapper macros (`def cc_library(**kwargs)`) yield a 117-byte empty result;
  common attributes are absent and must be merged from a hardcoded table
  (bazelbuild/stardoc#292); the workspace must analyse.
- **Do not** attempt this for external rulesets in general. Bazel's own docgen
  needs 11 hand-written deps for one file and reaches into `//cc/private/...`;
  JetBrains hand-curates the file list per ruleset. This does not scale to
  arbitrary user dependencies.

**5. BCR `docs_url` prefetch.**

- For the 58 modules that publish it, a plain HTTPS GET of a ~30 KB tarball gives
  you perfect `ModuleInfo` protos with no Bazel invocation and no analysis. Read
  `docs_url` out of `MODULE.bazel.lock` / the registry's `source.json` for each
  resolved `bazel_dep`.
- Cheap to implement (it's the same decoder as item 4), high value where present,
  zero value for `rules_cc`/`rules_java`/`rules_python`/`rules_go`. Worth also
  filing upstream requests to those rulesets — the mechanism already exists and
  costs them a five-line release script change.

### Tier 3 — do not build

**6. `bazel info build-language` as a rule source.** In Bazel 9, 22 of 43 rule
classes are attribute-less husks and Starlark rules are absent entirely.
`--proto:rule_classes` supersedes it for Bazel ≥ 8. Keep it only as the Bazel-7
fallback and as the runtime source of `$bzl_load_label`.

**7. Scraping `bazel.build` HTML or ruleset `docs/*.md`.** Lossy re-derivation of
protos you can obtain directly; brittle; unversioned.

**8. Running Stardoc (the `stardoc` rule).** It is a Velocity renderer on top of
`starlark_doc_extract`. Use `starlark_doc_extract` and skip the renderer, the
Java toolchain, and the `stardoc` `bazel_dep`.

**9. Shipping someone else's `builtin.pb`.** starpls's is 20 months stale and
**not reproducible from upstream Bazel** (it needs PR #21135, closed as not
planned). Generate your own.

### The architectural point

`builtin.pb` answers *"what is the Starlark language plus Bazel's API?"* — a
question with a per-Bazel-version answer, so pin it per version.
`stardoc_output.RuleInfo` answers *"what rules exist here and what do they take?"*
— a question whose answer is a function of the workspace's `MODULE.bazel`
resolution, so compute it live. Conflating the two is what makes every existing
Bazel LSP wrong about `cc_library` in 2026.

Concretely, the hover for `hdrs` in `cc_library(hdrs = [...])` should be served by
`rule_class_info` from a live query against the user's resolved `rules_cc`; the
hover for `ctx.actions.run` should be served by a pinned `builtin.pb`; and
`cc_library` used without a `load()` should be a diagnostic from a hardcoded
Starlarkification table with a buildifier-backed fix.
