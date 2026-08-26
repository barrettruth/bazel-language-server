# 05 — Bazel's machine-readable interfaces, and whether an LSP can afford them

Everything below was **run**, not read. Dates, versions, timings and output samples are
from this session (2026-08-25).

## 0. Test rig

| | |
|---|---|
| Machine | Apple M5 Max, 18 cores, 128 GB RAM, macOS 26.6.1 (arm64) |
| Bazel A | 8.7.0 (`nixpkgs#bazel_8` → `/nix/store/w24fpafgc7ddaqbwy939v1g33k3lf6fg-bazel-8.7.0`) |
| Bazel B | 9.2.0 (via `nixpkgs#bazelisk` 1.29.0) |
| Probe workspace | `/tmp/bazel-api-probe` — bzlmod, 5 packages, 19 targets, custom rule + macro, `select()`, `genrule`, `filegroup`, `alias`, `config_setting`, `string_flag`, a module extension |
| Scale workspace | `upstream/bazel` @ `3a9b19c85b11` (2026-08-24), 1092 BUILD files, 4095 targets in `//...`, 26 854 in `deps(//...)` |
| Output bases | `/tmp/ob-probe` (the "user's"), `/tmp/ob-lsp` (a private LSP one), `/tmp/ob-bazelrepo` |

The probe workspace:

```python
# MODULE.bazel
module(name = "api_probe", version = "0.1.0", compatibility_level = 1)
bazel_dep(name = "platforms", version = "1.0.0")
bazel_dep(name = "bazel_skylib", version = "1.8.1")
bazel_dep(name = "rules_shell", version = "0.6.1")
bazel_dep(name = "stardoc", version = "0.8.0", dev_dependency = True)
single_version_override(module_name = "bazel_skylib", version = "1.8.1")
use_repo_rule_ext = use_extension("//tools:ext.bzl", "probe_ext")
use_repo(use_repo_rule_ext, "probe_generated_repo")
```

`tools/defs.bzl` defines `probe_library` (a `rule()` with a docstring, an enum-valued
`flavor` attr, a provider-constrained `deps` attr, an implicit `_implicit_tool` label attr)
and `probe_suite` (a macro that emits several `probe_library` targets plus a
`native.filegroup`). This exercises every introspection path that matters.

**Two definitions used throughout:**
- **cold** = `bazel shutdown` first; JVM restart, Skyframe rebuilt, but the output base and
  repo cache are populated. This is what the user's *first* keystroke after lunch costs.
- **warm** = server already running, previous identical command already executed.

---

## 1. Headline latency table

Probe workspace (19 targets) unless noted. Each number is a wall-clock `time` of the full
`bazel …` invocation, including the C++ client fork.

| Command | cold | warm |
|---|---|---|
| `info output_base` | 1.693 s | **0.116 s** |
| `info build-language` | — | 0.135 s |
| `help flags-as-proto` | 0.980 s | 0.132 s |
| `mod dump_repo_mapping ''` | 1.054 s | 0.132 s |
| `mod graph --output=json` | 1.242 s | 0.131 s |
| `mod show_repo @bazel_skylib` | — | 0.127 s |
| `mod show_extension …%probe_ext` | — | 0.135 s |
| `mod tidy` | — | 0.155 s |
| `query //... --output=location` | 1.264 s | 0.135 s |
| `query //... --output=streamed_jsonproto --proto:rule_classes` | — | 0.141 s |
| `query rdeps(//..., //lib:core)` | — | 0.141 s |
| `query rbuildfiles(tools/defs.bzl)` | — | 0.125 s |
| `cquery //...` (needs analysis) | 1.569 s | 0.115–0.279 s |
| `aquery //lib:core --output=jsonproto` | — | 0.136 s |
| `canonicalize-flags -- -c opt` | — | 0.140 s |
| `dump --skyframe=summary` | — | 0.126 s |
| brand-new `--output_base`, `query //...` | 2.148 s | 0.125 s |

Scale workspace (`upstream/bazel`, 4095 targets, Bazel 9.2.0):

| Command | cold (server restart) | warm |
|---|---|---|
| `query //... --output=label` | 1.987 s | **0.123 s** |
| `query //... --output=location` | — | 0.105 s |
| `query //... --output=streamed_proto` (14.8 MB) | — | 0.299 s |
| `query //... --output=streamed_jsonproto` (27.8 MB) | — | 0.661 s |
| `query //... --output=streamed_proto --proto:rule_classes --proto:output_rule_attrs= --noproto:default_values` (6.1 MB) | — | 0.140 s |
| `query buildfiles(//...)` (1092 files) | — | 0.128 s |
| `query loadfiles(//...)` (434 `.bzl`) | — | 0.112 s |
| `query rdeps(//..., //src/…/vfs:vfs)` (1899 hits) | — | 0.748 s |
| `query rbuildfiles(tools/build_rules/utilities.bzl)` | — | 0.597 s |
| `query deps(//...)` (26 854 targets) — **first ever, fetches every external repo** | **6 m 34 s** | 1.21 s |
| after editing a BUILD file: `query //...` | — | 0.209 s |
| after editing a BUILD file: `query rdeps(//..., X)` | — | 0.878 s |
| after editing a BUILD file: `query deps(//...)` | — | 0.719 s |

**The floor is the client, not the server.** 10 back-to-back `bazel info release` calls
took 1.622 s → **~162 ms per invocation** with essentially zero server work. 10 back-to-back
`query //... --output=location` took 1.365 s → 137 ms each. So *every* `bazel` fork costs
≈130 ms of rc parsing + lock acquisition + gRPC connect + teardown regardless of what you
ask for. See §12 for how to get rid of that.

**The cliff is external-repo fetching.** `deps(//...)` was 6 m 34 s the first time and
1.21 s the second. An LSP that lets a query wander into an unfetched repo hangs the editor
for minutes. Mitigation in §11.

---

## 2. `bazel query`

### 2.1 Output formats

Valid values, identical in 8.7.0 and 9.2.0 (from the error message when you pass a bogus one):

```
label, label_kind, build, minrank, maxrank, package, location, graph, xml, proto,
streamed_jsonproto, streamed_proto
```

Note there is **no plain `jsonproto`** for `query` (that's `cquery`/`aquery` only), and no
`textproto`.

Sizes for `deps(//...)` on the probe workspace (141 targets), same data, different encodings:

| format | bytes | ratio |
|---|---|---|
| `label` | 5 325 | 1× |
| `location` | 17 303 | 3.2× |
| `build` | 38 850 | 7.3× |
| `xml` | 65 362 | 12× |
| `streamed_proto` | 96 660 | 18× |
| `proto` | 96 801 | 18× |
| `streamed_jsonproto` | 262 050 | 49× |
| `streamed_jsonproto --proto:rule_classes` | 421 252 | 79× |

`streamed_jsonproto` is 2.8× the wire size of `streamed_proto` for identical content. On the
bazel repo that's 27.8 MB vs 14.8 MB and 661 ms vs 299 ms. **Use `streamed_proto`** —
length-delimited `blaze_query.Target` messages, one per target, decodable incrementally.

#### `--output=label` / `label_kind`
```
$ bazel query '//lib:all' --output=label_kind
probe_library rule //lib:core
alias rule //lib:core_alias
filegroup rule //lib:docs
genrule rule //lib:gen_manifest
filegroup rule //lib:suite
probe_library rule //lib:suite_plain
probe_library rule //lib:suite_spicy
```
Kinds are `"<class> rule"`, `"source file"`, `"generated file"`, `"package group"`.
`kind("source file", …)` etc. match against exactly these strings.

#### `--output=location` — the workspace-symbols format
```
$ bazel query '//lib:all' --output=location
/private/tmp/bazel-api-probe/lib/BUILD.bazel:7:14: probe_library rule //lib:core
/private/tmp/bazel-api-probe/lib/BUILD.bazel:43:6: alias rule //lib:core_alias
/private/tmp/bazel-api-probe/lib/BUILD.bazel:22:10: filegroup rule //lib:docs
/private/tmp/bazel-api-probe/lib/BUILD.bazel:32:8: genrule rule //lib:gen_manifest
/private/tmp/bazel-api-probe/lib/BUILD.bazel:16:12: filegroup rule //lib:suite
/private/tmp/bazel-api-probe/lib/BUILD.bazel:16:12: probe_library rule //lib:suite_plain
/private/tmp/bazel-api-probe/lib/BUILD.bazel:16:12: probe_library rule //lib:suite_spicy
```

Absolute path, 1-based line, 1-based column. **Macro-expanded targets report the macro call
site in the BUILD file**, which is exactly what goto-definition wants: `//lib:suite_plain`
was produced by `probe_suite()` and points at `lib/BUILD.bazel:16:12`, the `probe_suite(`
call, not at `tools/defs.bzl`. Three targets share one location — an LSP must dedupe or
disambiguate by name.

`--relative_locations` switches to workspace-relative paths (`lib/BUILD.bazel:7:14`), which
is what you want to store in an index. External repos still get absolute output-base paths.

#### `--output=build` — the "reconstructed source" format
```
# /private/tmp/bazel-api-probe/lib/BUILD.bazel:7:14
probe_library(
  name = "core",
  srcs = ["//lib:a.txt", "//lib:b.txt"],
  flavor = "mild",
)
# Rule core instantiated at (most recent call last):
#   /private/tmp/bazel-api-probe/lib/BUILD.bazel:7:14 in <toplevel>
# Rule probe_library defined at (most recent call last):
#   /private/tmp/bazel-api-probe/tools/defs.bzl:28:21 in <toplevel>

# /private/tmp/bazel-api-probe/lib/BUILD.bazel:22:10
filegroup(
  name = "docs",
  srcs = ["//lib:README.md"] + select({"//:is_prod": ["//lib:prod_notes.md"], "//conditions:default": ["//lib:dev_notes.md"]}),
)
```
Human-readable, includes the `select()` unflattened, and both the instantiation and the
rule-class definition site. Good for hover text; a parsing nuisance otherwise. Everything
here is available structurally in `--output=streamed_proto`.

#### `--output=xml`
```xml
<rule class="probe_library" location="/private/tmp/bazel-api-probe/lib/BUILD.bazel:7:14" name="//lib:core">
    <string name="name" value="core"/>
    <list name="srcs">
        <label value="//lib:a.txt"/>
        <label value="//lib:b.txt"/>
    </list>
    <string name="flavor" value="mild"/>
    <rule-input name="//lib:a.txt"/>
    <rule-input name="//lib:b.txt"/>
    <rule-input name="//tools:defs.bzl"/>
</rule>
```
Strictly worse than proto: no rule-class info, no instantiation stack, no selector
structure, 12× the size of `label`. `--xml:line_numbers` (default true) and
`--xml:default_values` (default false) are the only knobs. Ignore it.

#### `--output=streamed_jsonproto` / `streamed_proto`

The important one. Newline-delimited JSON (or length-delimited binary) `blaze_query.Target`.
With the right flags this single call answers hover, attribute completion, attribute-value
completion, goto-def-on-rule-name and goto-def-on-target simultaneously:

```
$ bazel query '//lib:core' --output=streamed_jsonproto \
    --proto:rule_classes --proto:instantiation_stack --proto:definition_stack \
    --proto:output_rule_attrs=flavor,srcs --relative_locations
```
```json
{ "type": "RULE",
  "rule": {
    "name": "//lib:core",
    "ruleClass": "probe_library",
    "location": "lib/BUILD.bazel:7:14",
    "attribute": [
      {"name":"flavor","type":"STRING","stringValue":"mild","explicitlySpecified":true},
      {"name":"srcs","type":"LABEL_LIST","stringListValue":["//lib:a.txt","//lib:b.txt"],"explicitlySpecified":true}],
    "ruleInput": ["//lib:a.txt","//lib:b.txt","//tools:defs.bzl"],
    "instantiationStack": ["lib/BUILD.bazel:7:14: <toplevel>"],
    "definitionStack":    ["tools/defs.bzl:28:21: <toplevel>"],
    "ruleClassKey": "//tools:defs.bzl%probe_library",
    "ruleClassInfo": {
      "ruleName": "probe_library",
      "docString": "Concatenates srcs into one file. Exists purely to be introspected.",
      "attribute": [
        {"name":"name","docString":"A unique name for this target.","type":"NAME","mandatory":true},
        {"name":"visibility","type":"LABEL_LIST","defaultValue":"[]","nonconfigurable":true,"nativelyDefined":true},
        …
        {"name":"srcs","docString":"Text files to concatenate.","type":"LABEL_LIST","defaultValue":"[]"},
        {"name":"deps","docString":"Other probe_library targets.","type":"LABEL_LIST",
         "providerNameGroup":[{"providerName":["ProbeInfo"],
                               "originKey":[{"name":"ProbeInfo","file":"//tools:defs.bzl"}]}],
         "defaultValue":"[]"},
        {"name":"flavor","docString":"Enum-valued attribute, to test completion of attribute values.",
         "type":"STRING","defaultValue":"\"plain\"","values":["\"plain\"","\"spicy\"","\"mild\""]},
        {"name":"out_prefix","docString":"Unused string attr.","type":"STRING","defaultValue":"\"\""}],
      "originKey": {"name":"probe_library","file":"//tools:defs.bzl"},
      "advertisedProviders": {"providerName":["ProbeInfo"],
                              "originKey":[{"name":"ProbeInfo","file":"//tools:defs.bzl"}]}}}}
```

Four things to notice, all of them decisive:

1. **`ruleClassInfo` is a full `stardoc_output.RuleInfo`** — per-attribute docstrings,
   defaults, `values` (the enum list, for attribute-value completion), provider constraints,
   `nativelyDefined` flags. It is emitted **once per distinct `ruleClassKey`** in the stream,
   so cost is O(#rule classes) not O(#targets). The bazel repo has 91 distinct rule classes
   across 4095 targets.
2. **`originKey.file` is the defining `.bzl` label** — `//tools:defs.bzl` — for the rule
   *and* for every provider. Goto-definition on `probe_library` in a BUILD file resolves
   from this, no `.bzl` parsing required.
3. **`definitionStack`** gives the exact `file:line:col` of the `rule()` call
   (`tools/defs.bzl:28:21`), which is even better than the label.
4. **`instantiationStack`** gives the full macro chain. For the macro-generated target:
   ```json
   {"name":"//lib:suite_spicy","location":"lib/BUILD.bazel:16:12",
    "instantiationStack":["lib/BUILD.bazel:16:12: <toplevel>",
                          "tools/defs.bzl:67:22: probe_suite"],
    "definitionStack":["tools/defs.bzl:28:21: <toplevel>"]}
   ```
   Frame 0 is the BUILD call site, frame 1 is the `probe_library()` call inside the macro.
   This is a call hierarchy an LSP can render directly.

`--noproto:flatten_selects` preserves `select()` structure, giving you the *condition
labels* — needed for goto-def on a `select()` key:
```json
{"name":"srcs","type":"SELECTOR_LIST","selectorList":{"type":"LABEL_LIST","elements":[
  {"entries":[{"label":"//conditions:default","stringListValue":["//lib:README.md"]}],"hasDefaultValue":true},
  {"entries":[{"label":"//:is_prod","stringListValue":["//lib:prod_notes.md"]},
              {"label":"//conditions:default","stringListValue":["//lib:dev_notes.md"]}],
   "hasDefaultValue":true,"noMatchError":""}]}}
```
Note the `+` concatenation is preserved as two `elements`, the first being a synthetic
single-entry selector for the non-`select` part.

Source files and generated files also carry locations:
```
SOURCE_FILE    //lib:BUILD.bazel  loc=lib/BUILD.bazel:1:1   vis=['//visibility:public']
SOURCE_FILE    //lib:a.txt        loc=lib/a.txt:1:1
GENERATED_FILE //lib:manifest.txt loc=lib/BUILD.bazel:32:8  by=//lib:gen_manifest
GENERATED_FILE //app:version.txt  loc=app/BUILD.bazel:42:8  by=//app:version
```
Source-file "locations" are always `<path>:1:1` — a pointer to the file, not a position.
Generated files point at the generating rule's call site, which is the right goto-def target.

Relevant `--proto:*` flags (`bazel help query`):

| flag | default | LSP use |
|---|---|---|
| `--proto:rule_classes` | false | **yes** — stardoc `RuleInfo` per rule class |
| `--proto:instantiation_stack` | false | **yes** — macro call chain |
| `--proto:definition_stack` | false | **yes** — `rule()` call site |
| `--proto:locations` | true | keep |
| `--proto:flatten_selects` | true | **set false** to keep `select()` keys |
| `--proto:output_rule_attrs` | `all` | set to a subset, or `` (empty) to drop all attrs |
| `--proto:default_values` | true | set false to drop unset attrs (big win) |
| `--proto:rule_inputs_and_outputs` | true | `ruleInput`/`ruleOutput` edges |
| `--proto:include_attribute_source_aspects` | false | which aspect contributed an attr |
| `--proto:include_synthetic_attribute_hash` | false | change detection per target |

Other query-wide flags an LSP wants:

- `--relative_locations` — workspace-relative paths.
- `--consistent_labels` — canonical `@@repo//pkg:tgt` form for every label, including
  `@@//lib:core` for the main repo and `@@+probe_ext+probe_generated_repo//:data.txt` for
  extension repos. **Use this for the index key**; apparent names are context-dependent.
- `--noimplicit_deps`, `--notool_deps`, `--nonodep_deps`, `--noinclude_aspects` — prune the
  graph. `nodep_deps` controls whether `visibility`-style non-dependency labels are edges.
- `--keep_going` — see §2.4.
- `--query_file=<path>` — read the expression from a file; avoids shell quoting for
  generated queries. Works.
- `--order_output=no` — required for `rbuildfiles`/`allrdeps`. Counter-intuitively it was
  *slower* on `deps(//...)` (1.414 s vs 0.422 s) because it forces the SkyQuery engine.
- `--universe_scope=//...` — enables SkyQuery mode (`rbuildfiles`, `allrdeps`).
  `--infer_universe_scope` derives it from the expression.

### 2.2 The query language, ranked by LSP value

Tested on the probe workspace. Functions available (from
`src/main/java/com/google/devtools/build/lib/query2/engine/*Function.java`):
`allpaths deps rdeps allrdeps somepath some attr filter kind labels loadfiles buildfiles
rbuildfiles siblings same_pkg_direct_rdeps tests visible inputs outputs executables mnemonic`.

**`rdeps(universe, target [, depth])` — find references.**
```
$ bazel query 'rdeps(//..., //lib:core, 1)' --output=location
…/app/BUILD.bazel:7:10:  sh_binary rule //app:app
…/app/BUILD.bazel:26:14: probe_library rule //app:app_data
…/lib/BUILD.bazel:7:14:  probe_library rule //lib:core        ← the target itself
…/lib/BUILD.bazel:43:6:  alias rule //lib:core_alias
…/lib/BUILD.bazel:32:8:  genrule rule //lib:gen_manifest
…/lib/BUILD.bazel:16:12: probe_library rule //lib:suite_plain
…/lib/BUILD.bazel:16:12: probe_library rule //lib:suite_spicy
```
0.748 s on the bazel repo for 1899 hits. **This is find-references at rule granularity, not
token granularity.** Bazel tells you *which target* refers to `//lib:core`, and where that
target was declared — not the character offset of the `"//lib:core"` string literal. To
produce LSP `Location` ranges you must re-scan the returned BUILD file for the label text.
That is cheap and correct-enough, but note macro-generated targets (`//lib:suite_plain`)
point at a call site where the label does not textually appear — the reference is inside
`tools/defs.bzl`. `instantiationStack` is how you follow that.

**`rbuildfiles(<workspace-relative path>…)` — find references for a `.bzl` file.**
```
$ bazel query --universe_scope=//... --order_output=no 'rbuildfiles(tools/defs.bzl)'
//app:BUILD.bazel
//gen:BUILD.bazel
//lib:BUILD.bazel
```
0.125 s (probe), 0.597 s (bazel repo). Takes *paths*, not labels. This is the answer to
"who `load()`s this file" and doubles as the invalidation set when a `.bzl` is edited.
Requires SkyQuery mode (`--universe_scope` + `--order_output=no`).

**`same_pkg_direct_rdeps(x)`** — 4 hits on `//lib:core`, no universe needed, sub-100 ms of
server work. The right primitive for "rename this target, fix the intra-package refs first".

**`siblings(x)`** — every target in `x`'s package including the BUILD file itself and
generated files. This is `textDocument/documentSymbol` for a BUILD file, though
`//lib:*` (i.e. `siblings` of any target in the package) is the same thing and clearer.

**`buildfiles(x)` / `loadfiles(x)`** — the transitive `.bzl` closure.
```
$ bazel query 'buildfiles(//...)'
//:BUILD.bazel  //app:BUILD.bazel  //gen:BUILD.bazel  //lib:BUILD.bazel
//tools:BUILD.bazel  //tools:defs.bzl
@bazel_skylib//lib:BUILD  @bazel_skylib//lib:paths.bzl
@bazel_skylib//rules:BUILD  @bazel_skylib//rules:common_settings.bzl
@rules_shell//shell:BUILD  @rules_shell//shell:sh_binary.bzl  @rules_shell//shell:sh_test.bzl
@rules_shell//shell/private:BUILD  @rules_shell//shell/private:sh_binary.bzl
@rules_shell//shell/private:sh_executable.bzl  @rules_shell//shell/private:sh_test.bzl
```
`loadfiles` is the same minus the BUILD files. 1092 / 434 respectively on the bazel repo,
both ~120 ms. **This is the file-watch set for an LSP.** Caveat: the `SourceFile.subinclude`
field in build.proto, which is documented to list the loaded `.bzl` labels per BUILD file,
came back **unpopulated** in every `--output=streamed_jsonproto` run I made, including with
`buildfiles()`. Do not rely on it; use `loadfiles()` on a per-package basis instead.

**`kind(regex, x)`** — `kind("probe_library rule", //...)`, `kind("source file", //lib:*)`.
Regex over the label_kind string. Basis for "workspace symbols filtered by rule type" and
for starpls's `kind('.* rule', ...)` idiom.

**`attr(name, valueRegex, x)`** — `attr(flavor, spicy, //...)` → `//lib:suite_spicy`;
`attr(tags, "\bmanual\b", //...)`; `attr(deps, "//lib:core", //...)`. Regex over the
*stringified* attribute value. Gives you "find every target with `tags = ["manual"]`" and a
crude label-reference search that includes attributes `rdeps` won't traverse (`nodep` ones).

**`somepath(x, y)` / `allpaths(x, y)`** — dependency-path explanation, e.g. a code lens
"why does `//:top` depend on `//lib:a.txt`":
```
$ bazel query 'somepath(//:top, //lib:a.txt)'
//:top  //app:app  //lib:core  //lib:a.txt
```

**`labels(attr, x)`** — `labels(srcs, //lib:core)` → `//lib:a.txt //lib:b.txt`. Resolves a
single attribute's label list, post-`select`-flattening.

**`visible(predicate, x)`** — which of `x` is visible to `predicate`. Directly implements a
visibility diagnostic / "add to visibility" code action.

**`tests(x)`** — expands `test_suite`s. Feeds a "run tests in this package" code lens.

**`filter(regex, x)`** — string filter, **unanchored**:
`filter("//lib:.*", deps(//app:app))` also matched `@bazel_skylib//lib:paths`. Anchor it.

**`deps(x [, depth])`** — the dangerous one. See §11.

### 2.3 Which of these give find-references and workspace-symbols

- **Workspace symbols**: `bazel query '//...' --output=location` (or
  `--output=streamed_proto --proto:output_rule_attrs= --noproto:default_values`).
  4095 targets in **105 ms** warm / 2.0 s cold on the bazel repo. Fully viable, and 6.1 MB
  of proto gets you kinds + locations + rule-class docs in one shot.
- **Find references (target)**: `rdeps(//..., X)` — 748 ms for 1899 hits. Viable, but only
  resolves to the *referring target*, so a text pass over the returned BUILD files is
  required to get character ranges. `same_pkg_direct_rdeps` for the fast path.
- **Find references (`.bzl` file)**: `rbuildfiles(path)` — 597 ms. Viable.
- **Find references (a `load()`ed symbol, e.g. who calls `probe_library`)**:
  `kind("probe_library rule", //...)` — 141 ms. Viable, and better than text search because
  it survives aliasing in `load(..., my_lib = "probe_library")`.
- **Find references crossing into external repos**: `rdeps(//... + @foo//..., X)` works but
  forces those repos to load. Do not do it interactively.

### 2.4 Diagnostics from query

Errors go to **stderr as free text**, prefixed `ERROR: <abs-path>:<line>:<col>: <message>`.
There is no structured diagnostic channel (see §10 for why BEP doesn't help).

```
$ bazel query '//broken:all'
ERROR: /private/tmp/bazel-api-probe/broken/BUILD.bazel:9:14: //broken:bad_attr: invalid value in 'flavor' attribute: has to be one of 'plain', 'spicy', or 'mild' instead of 'nuclear'
ERROR: Traceback (most recent call last):
	File "/private/tmp/bazel-api-probe/broken/BUILD.bazel", line 20, column 10, in <toplevel>
		filegroup(
Error in filegroup: filegroup rule 'dup' conflicts with existing filegroup rule, defined at /private/tmp/bazel-api-probe/broken/BUILD.bazel:15:10
ERROR: package contains errors: broken: …
ERROR: Evaluation of query "//broken:all" failed
                                                     # exit 7, no stdout
```
With `--keep_going` (exit 3) you get the same stderr **plus** the partial result on stdout.
An LSP must always pass `--keep_going`, otherwise one broken package blanks the whole index —
I hit exactly this: a single bad package made `deps(//...)` return zero targets.

Three distinct diagnostic classes, three different shapes:

1. **Attribute-value errors** — `path:line:col: <label>: invalid value in 'flavor'
   attribute: has to be one of 'plain', 'spicy', or 'mild' instead of 'nuclear'`. Location
   is the rule call site, column is the rule's column. Parseable.
2. **Starlark evaluation errors** — a multi-line `Traceback` with `File "...", line N,
   column M, in <fn>` frames. Different format from #1; needs its own parser.
3. **Unresolved labels** — only reported when the query actually *dereferences* the label:
   ```
   $ bazel query --keep_going 'deps(//broken:bad_dep)'
   ERROR: …/broken/BUILD.bazel:3:14: no such target '//lib:does_not_exist': target
     'does_not_exist' not declared in package 'lib' defined by …/lib/BUILD.bazel
     and referenced by '//broken:bad_dep'
   ```
   `bazel query '//broken:all'` alone does **not** flag it. Unresolved-label diagnostics
   therefore cost a `deps()` traversal, and the reported location is the rule call site
   (`3:14`), not the offending string literal.

There is also a genuinely useful *warning* channel with locations, e.g.
`WARNING: /…/MODULE.bazel:1:7: The attribute 'compatibility_level' in module() is a no-op`
and `WARNING: /…/MODULE.bazel:21:34: The module extension probe_ext … reported incorrect
imports of repositories via use_repo(): … Fix the use_repo calls by running 'bazel mod tidy'.`
That last one is a ready-made code action (§7.6).

---

## 3. `build.proto`: does it carry source locations?

`upstream/bazel/src/main/protobuf/build.proto`, 607 lines, `package blaze_query`,
`java_package com.google.devtools.build.lib.query2.proto.proto2api`. It imports
`stardoc_output.proto`. **Yes, it carries locations — but as opaque strings, not structured
line/column fields.**

```protobuf
message QueryResult { repeated Target target = 1; }

message Target {
  enum Discriminator { RULE=1; SOURCE_FILE=2; GENERATED_FILE=3; PACKAGE_GROUP=4; ENVIRONMENT_GROUP=5; }
  required Discriminator type = 1;
  optional Rule rule = 2;
  optional SourceFile source_file = 3;
  optional GeneratedFile generated_file = 4;
  optional PackageGroup package_group = 5;
  optional EnvironmentGroup environment_group = 6;
}
```

`Rule` (the goto-def payload):

| field | # | type | note |
|---|---|---|---|
| `name` | 1 | `required string` | absolute label `//foo/bar:baz` |
| `rule_class` | 2 | `required string` | e.g. `probe_library`; **not unique** — two `.bzl` files may define the same name |
| **`location`** | 3 | `optional string` | **`"<absolute_path>:<line>:<column>"`.** Documented as "the line number will be that of a rule invocation *or macro call*". |
| `attribute` | 4 | `repeated Attribute` | |
| `rule_input` | 5 | `repeated string` | predecessor labels |
| `rule_output` | 6 | `repeated string` | successor labels |
| `default_setting` | 7 | `repeated string` | package `features` |
| `skylark_environment_hash_code` | 12 | `optional string` | changes when rule behaviour changes |
| **`instantiation_stack`** | 13 | `repeated string` | `"file:line:col: function"`, outermost first, rule frame omitted. Needs `--proto:instantiation_stack`. **Paths may be package-relative.** |
| **`definition_stack`** | 14 | `repeated string` | same format, for the rule class. Needs `--proto:definition_stack`. |
| `configured_rule_input` | 15 | `repeated ConfiguredRuleInput` | cquery only |
| `rule_class_key` | 16 | `optional string` | e.g. `//tools:defs.bzl%probe_library`; stable across invocations. Needs `--proto:rule_classes`. |
| **`rule_class_info`** | 17 | `optional stardoc_output.RuleInfo` | **the doc payload.** Emitted only for the *first* rule with a given `rule_class_key`. |
| `test` / `executable` | 18/19 | `optional bool` | code-lens gating |

`SourceFile`: `name`(1), `location`(2, always `path:1:1`), `subinclude`(3, "labels of `.bzl`
files transitively loaded in this BUILD file" — **observed empty in practice**),
`package_group`(4), `visibility_label`(5), `feature`(6), `license`(8),
`package_contains_errors`(9).

`GeneratedFile`: `name`(1), `generating_rule`(2), `location`(3 — points at the generating
rule's call site).

`Attribute`: `name`(1), `type`(2, the 21-value `Discriminator` enum), `explicitly_specified`(13),
`nodep`(20), `source_aspect_name`(23), one of ~12 typed value fields, and `selector_list`(21)
when `--noproto:flatten_selects`. `Attribute` has **no location** — you cannot get the
character range of an individual attribute or of an individual label inside a list. This is
the single biggest gap for precise find-references and rename.

`Repository`(canonical_name, repo_rule_name, repo_rule_bzl_label, apparent_name, module_key,
original_name, attribute) — emitted by `bazel mod show_repo --output=streamed_proto`, not by
`query`.

`BuildLanguage`/`RuleDefinition`/`AttributeDefinition`/`AttributeValue` at the bottom of the
file are the legacy `bazel info build-language` schema — see §8.2.

**Summary for goto-def:** you get `file:line:col` for every *target* and every *rule class
definition*, and the full macro chain. You get **nothing** below rule granularity. Column
positions of labels, attribute names, `load()` symbols and `select()` keys must come from
your own parse of the file.

---

## 4. `bazel cquery`

Valid `--output`: `label_kind, label, transitions, proto, streamed_proto, textproto,
jsonproto, build, graph, starlark, files`.

cquery runs the **analysis phase**, so it costs what a `--nobuild` build costs, and it fails
on anything that fails to analyse. It caught a class of error `query` structurally cannot:

```
$ bazel cquery '//...'
ERROR: /private/tmp/bazel-api-probe/app/BUILD.bazel:7:10: in deps attribute of sh_binary rule
  //app:app: bzl_library rule '@@bazel_skylib+//rules/directory:glob' is misplaced here
  (expected sh_library)
ERROR: /private/tmp/bazel-api-probe/app/BUILD.bazel:7:10: Analysis of target '//app:app'
  (config: 7f8856b) failed
```
That is a real "wrong rule type in this attribute" diagnostic with a location, and it is
only available post-analysis. Same for provider mismatches, toolchain resolution failures,
and `target_compatible_with` incompatibility.

What cquery adds over query:

**`select()` is resolved.** `//lib:docs` `srcs` is `["//lib:README.md","//lib:dev_notes.md"]`
in cquery vs. a `SELECTOR_LIST` in query. Useful for hover ("in your current configuration
this resolves to…"), but it means cquery *loses* the information goto-def-on-a-select-key
needs. You want both, not one.

**Configuration identity.** Every result is `//lib:core (7f8856b)`, and the same target can
appear under several configurations (`//app:app_test (b6cf661)` vs `//app:app (7f8856b)`).
For an editor this is noise: the same BUILD-file line maps to N results.

**`--output=files`** — the concrete output paths:
```
$ bazel cquery '//lib:core' --output=files
bazel-out/darwin_arm64-fastbuild/bin/lib/core.cat
```
Only interesting for a "reveal generated file" command.

**`--output=starlark`** with `--starlark:expr` / `--starlark:file` — arbitrary Starlark over
the configured target. This is the richest hover source in the whole CLI:
```
$ bazel cquery '//lib:core' --output=starlark --starlark:expr='providers(target)'
{"LicenseInfo": <instance of provider LicenseInfo>,
 "//tools:defs.bzl%ProbeInfo": struct(tags_seen = "mild", text = <generated file lib/core.cat>),
 "FileProvider": …, "FilesToRunProvider": …,
 "OutputGroupInfo": struct(_hidden_top_level_INTERNAL_ = depset([])),
 "DefaultInfo": struct(files = depset([<generated file lib/core.cat>]), …)}
```
`providers()`, `target.files`, `target.label`, `build_options(target)` are available. Note
the output is Starlark `repr`, not JSON — you must parse it yourself, and opaque Java objects
render as `<unknown object com.google.…>`.

**`--transitions=lite|full`** — every dep edge annotated with the attribute name and the
configuration transition:
```
NoTransition -> //app:app_test (b6cf661)
  data#//app:app#(TestTrimmingTransition + ConfigFeatureFlagTaggedTrimmingTransition) -> 7f8856b
  [toolchain dependency]#@@rules_shell++sh_configure+local_config_shell//:osx_sh#… -> 7f8856b
```
Format is `attr#label#transition -> configHash`. Genuinely explanatory for a "why is this
built twice" hover, but the format is clearly not a stable API.

**`--show_config_fragments=direct|transitive`**:
```
//lib:docs (a9e2d38) [//:mode, BazelConfiguration, CoreOptions, PlatformConfiguration, …]
```
Tells you which build settings a target is sensitive to. Would make a nice inlay hint;
requires analysis.

**Cost.** 0.273 s of *server-reported* analysis time on 19 targets, 1.569 s cold, 0.115–0.279 s
warm. That's fine here. It is not fine on a real repo: cquery `//...` on a large monorepo is
a full `--nobuild` build, minutes and gigabytes. And it invalidates on any flag change
(`-c opt` vs `-c dbg` are different analysis caches). **cquery is not an interactive
interface at scale.** Reserve it for explicit user-invoked commands ("explain this target"),
never for on-type or on-save work.

---

## 5. `bazel aquery`

Valid `--output`: `proto, streamed_proto, textproto, jsonproto, text, summary`.

```
$ bazel aquery '//lib:core'
action 'Concatenating @@//lib:core'
  Mnemonic: ProbeCat
  Target: //lib:core
  Configuration: darwin_arm64-fastbuild
  ActionKey: 8f7b5bcd1afcc31e98f96f6240a93ca62937b97815fed2d13a54aa5adef636d1
  Inputs: [lib/a.txt, lib/b.txt]
  Outputs: [bazel-out/darwin_arm64-fastbuild/bin/lib/core.cat]
  Command Line: (exec …/bash -c 'cat $@ > bazel-out/…/lib/core.cat' '' lib/a.txt lib/b.txt)
```
`--output=jsonproto` yields `{actions, artifacts, configuration, depSetOfFiles,
pathFragments, ruleClasses, targets}` (schema: `analysis_v2.proto`, 297 lines).

**Not relevant to a Bazel-file LSP.** There is no source location anywhere in the action
graph; the finest granularity is `targetId` → a label. It requires the analysis phase, so it
inherits every cost objection to cquery, and it answers questions ("what command line will
run") that are about the *build*, not about the *BUILD file*. The one marginal use is a code
lens showing action count/mnemonics per target, which is not worth an analysis pass.

`aquery` is squarely in the scope-OUT territory of the brief: it exists to feed
`compile_commands.json` generators and BSP servers.

---

## 6. `bazel mod` — the bzlmod introspection surface

This is the most LSP-relevant subcommand, and Bazel says so itself. From `bazel help mod`,
verbatim:

> `dump_repo_mapping <canonical_repo_name>...`: Prints the mappings from apparent repo names
> to canonical repo names for the given repos in NDJSON format. […] **This command is
> intended for use by tools such as IDEs and Starlark language servers.**

The subcommand set and options are **byte-identical between 8.7.0 and 9.2.0**.

Global `mod` options:

| flag | values | default |
|---|---|---|
| `--output` | `text, json, graph, streamed_proto, streamed_jsonproto` | `text` |
| `--base_module` | `<root>` / `mod@ver` / `@apparent` / `@@canonical` | `<root>` |
| `--from` | comma-separated modules | `<root>` |
| `--depth` | int | `-1` (unbounded) |
| `--verbose` | bool | false (true for `explain`) |
| `--include_unused` | bool | false |
| `--extension_info` | `hidden, usages, repos, all` | `hidden` |
| `--extension_filter` | list of extensions | — |
| `--extension_usages` | list of modules | — |
| `--charset` | `utf8, ascii` | `utf8` |

`--output=streamed_proto`/`streamed_jsonproto` are accepted **only by `show_repo`**; the
graph-shaped subcommands reject them:
```
ERROR: Invalid --output 'streamed_jsonproto' for the 'graph' subcommand. Only 'text', 'json', and 'graph' are supported.
```

### 6.1 `mod dump_repo_mapping` — the single most useful call

```
$ bazel mod dump_repo_mapping ''            # '' = the root repo
{"probe_generated_repo":"+probe_ext+probe_generated_repo","":"","api_probe":"",
 "platforms":"platforms","bazel_skylib":"bazel_skylib+","rules_shell":"rules_shell+",
 "stardoc":"stardoc+","bazel_tools":"bazel_tools","local_config_platform":"local_config_platform"}

$ bazel mod dump_repo_mapping 'rules_shell+'
{"local_config_shell":"rules_shell++sh_configure+local_config_shell","rules_shell":"rules_shell+",
 "bazel_features":"bazel_features+","bazel_skylib":"bazel_skylib+","platforms":"platforms",
 "bazel_tools":"bazel_tools","local_config_platform":"local_config_platform"}
```
Multiple args → one JSON object per line (NDJSON). **132 ms warm, 1.05 s cold.**

Combined with `bazel info output_base` this resolves any label to a path:
`@rules_shell//shell:sh_binary.bzl` → `rules_shell+` →
`<output_base>/external/rules_shell+/shell/sh_binary.bzl`. Verified present on disk.
It also gives you the **candidate list for `@`-completion** inside a given `.bzl`/BUILD file,
correctly scoped to that file's own repo (which is why the `from_repo` argument matters —
`local_config_shell` is visible from `rules_shell+` but not from the root).

**Bazel 9 changes the path shape.** In 8.7.0 `<output_base>/external/<canonical>` is a real
directory. In 9.2.0 it is a **symlink into a content-addressed shared cache**:
```
bazel_skylib+ -> ~/.cache/bazel/_bazel_bruth/cache/repos/v1/contents/38dcd070…/19b4a473-…
```
An LSP must `realpath` these (or accept that the target moves whenever a repo is refetched),
and must not assume the external dir is stable across `bazel clean`.

### 6.2 `mod graph` / `deps` / `path` / `all_paths`

```
$ bazel mod graph
<root> (api_probe@0.1.0)
├───bazel_skylib@1.8.1
│   ├───platforms@1.0.0 (*)
│   └───rules_license@1.0.0
├───platforms@1.0.0
│   └───rules_license@1.0.0 (*)
├───rules_shell@0.6.1
│   ├───bazel_skylib@1.8.1 (*)
│   ├───platforms@1.0.0 (*)
│   └───bazel_features@1.30.0
└───stardoc@0.8.0
    ├───protobuf@29.0
    │   └───abseil-cpp@20240116.1
    │       └───googletest@1.14.0.bcr.1
    │           ├───abseil-cpp@20240116.1 (cycle)
```
`(*)` = already expanded elsewhere, `(cycle)` = cycle. `--output=json` gives:
```json
{"key":"<root>","name":"api_probe","version":"0.1.0","apparentName":"api_probe",
 "dependencies":[{"key":"bazel_skylib@1.8.1","name":"bazel_skylib","version":"1.8.1",
                  "apparentName":"bazel_skylib",
                  "dependencies":[{"key":"platforms@1.0.0",…,"unexpanded":true}],
                  "indirectDependencies":[],"cycles":[]}]}
```
`--output=graph` emits Graphviz. 131 ms warm.

`mod deps <root>` is `graph --depth=1`-ish. `mod path X` / `all_paths X` give the dependency
chain from `--from` to `X` — the answer to "why is this module in my graph".

### 6.3 `mod explain` — why this version

```
$ bazel mod explain bazel_skylib
<root> (api_probe@0.1.0)
├───bazel_skylib@1.8.1 #
├───rules_shell@0.6.1
│   ├───bazel_features@1.30.0
│   │   └───bazel_skylib@1.8.1 #
│   └───bazel_skylib@1.8.1 #
└───stardoc@0.8.0
    ├───protobuf@29.0
    │   ├───rules_kotlin@1.9.6
    │   │   └───rules_proto@7.0.2
    │   │       └───bazel_skylib@1.8.1 #
```
With `--verbose --include_unused` you get MVS resolution reasons:
```
├───bazel_skylib@1.8.1 (*) (was 1.6.1, cause single_version_override)
├───platforms@1.0.0 (*) (was 0.0.10, cause <root>)
├───platforms@0.0.10 (to 1.0.0, cause <root>)
├───rules_license@1.0.0 (*) (was 0.0.7, cause bazel_skylib@1.8.1, stardoc@0.8.0, bazel_tools@_, …)
```

**Upstream bug worth knowing.** `--output=json` carries `unused`, `originalVersion`,
`resolvedVersion`, `resolvedRequestedBy`, `unexpanded`, `cycles`, `root` — but
`resolutionReason` is **wrong**. `JsonOutputFormatter.java:119` writes
`explanation.changedVersion().toString()` instead of the reason:
```java
json.addProperty("resolvedVersion", explanation.changedVersion().toString());   // 115
json.addProperty("originalVersion", explanation.changedVersion().toString());   // 117
json.addProperty("resolutionReason", explanation.changedVersion().toString());  // 119  ← bug
```
So JSON emits `"resolutionReason":"1.6.1"` where the text output says
`cause single_version_override`. The enum
(`BazelModuleInspectorValue.java:216`, `ORIGINAL / MINIMAL_VERSION_SELECTION /
SINGLE_VERSION_OVERRIDE / MULTIPLE_VERSION_OVERRIDE / NON_REGISTRY_OVERRIDE`) never reaches
JSON. Still present at bazel HEAD `3a9b19c85b11` (2026-08-24). An LSP hover saying "1.8.1
selected because of `single_version_override`" must parse the *text* output today.

### 6.4 `mod show_repo` — goto-definition for a repo name

```
$ bazel mod show_repo @bazel_skylib @probe_generated_repo
## @bazel_skylib:
# <builtin>
http_archive(
  name = "bazel_skylib+",
  urls = ["https://github.com/bazelbuild/bazel-skylib/releases/download/1.8.1/bazel-skylib-1.8.1.tar.gz"],
  integrity = "sha256-UbUQWnYLNTdz+QTSu8XmZNCYf7ryImUWTeZdQ+kQ2Kw=",
  remote_module_file_urls = ["https://bcr.bazel.build/modules/bazel_skylib/1.8.1/MODULE.bazel"],
  …
)
# Rule http_archive defined at (most recent call last):
#   /private/tmp/ob-probe/external/bazel_tools/tools/build_defs/repo/http.bzl:444:31 in <toplevel>

## @probe_generated_repo:
probe_repo(
  name = "+probe_ext+probe_generated_repo",
  _original_name = "probe_generated_repo",
  greeting = "probe_ext",
)
# Rule probe_repo defined at (most recent call last):
#   /private/tmp/bazel-api-probe/tools/ext.bzl:7:29 in <toplevel>
```
Works on extension-generated repos, not just modules. The trailing comment is a **source
location for the repo rule** — hover/goto-def on `@probe_generated_repo` can land on
`tools/ext.bzl:7:29`. And the `remote_module_file_urls` field is the BCR `MODULE.bazel` URL,
which is what a "open this module in the BCR" code action needs.

`--output=streamed_jsonproto` emits `blaze_query.Repository`:
```json
{"canonicalName":"bazel_skylib+","repoRuleName":"http_archive",
 "repoRuleBzlLabel":"@@bazel_tools//tools/build_defs/repo:http.bzl",
 "apparentName":"@bazel_skylib","attribute":[…]}
{"canonicalName":"+probe_ext+probe_generated_repo","repoRuleName":"probe_repo",
 "repoRuleBzlLabel":"//tools:ext.bzl","apparentName":"@probe_generated_repo",
 "originalName":"probe_generated_repo","attribute":[…]}
```
`repoRuleBzlLabel` is the structured version of the trailing comment. 127 ms warm.

### 6.5 `mod show_extension` — goto-usage for `use_repo`

Argument syntax is `<module><label_to_bzl>%<name>`; for the root module use
`@@//tools:ext.bzl%probe_ext` or `<root>//tools:ext.bzl%probe_ext`. A bare
`//tools:ext.bzl%probe_ext` is rejected (`Module  does not exist in the dependency graph`).

```
$ bazel mod show_extension '@@//tools:ext.bzl%probe_ext'
## @@//tools:ext.bzl%probe_ext:

Fetched repositories:
  - probe_generated_repo (imported by <root>)

## Usage in <root> from /private/tmp/bazel-api-probe/MODULE.bazel:21
use_repo(
  probe_ext,
  "probe_generated_repo",
)
```
**`/private/tmp/bazel-api-probe/MODULE.bazel:21`** — a real file:line for the `use_repo`
call. This is goto-definition and find-references for extension-generated repo names.

`mod graph --extension_info=all` enumerates every extension and every repo it *could*
generate, whether or not `use_repo`'d:
```
<root> (api_probe@0.1.0)
├───$@@//tools:ext.bzl%probe_ext
│   └───probe_generated_repo
├───platforms@1.0.0
│   └───$@@platforms//host:extension.bzl%host_platform
│       └───host_platform
├───rules_shell@0.6.1
│   └───$@@rules_shell+//shell/private/extensions:sh_configure.bzl%sh_configure
│       └───local_config_shell
└───stardoc@0.8.0
    └───$@@rules_jvm_external+//:extensions.bzl%maven
        ├───stardoc_maven
        ├╌╌╌aopalliance_aopalliance_1_0            ← dashed = generated but not imported
        ├╌╌╌com_google_guava_guava_33_2_1_jre
        …
```
Solid `├───` = imported via `use_repo`; dashed `├╌╌╌` = available but not imported.
**This is the completion list for `use_repo(ext, "…")`.** Caveat: producing it requires
*evaluating* the extension, which for something like `rules_jvm_external`'s `maven` means
running a repository rule (network, minutes) the first time.

### 6.6 `mod tidy` — a code action, ready-made

```
$ bazel build --nobuild //app:app
WARNING: /…/MODULE.bazel:21:34: The module extension probe_ext defined in @api_probe//tools:ext.bzl
  reported incorrect imports of repositories via use_repo():

Imported, but not created by the extension (will cause the build to fail):
    ghost_repo

Fix the use_repo calls by running 'bazel mod tidy'.
ERROR: … module extension @@//tools:ext.bzl%probe_ext does not generate repository "ghost_repo",
  yet it is imported as "ghost_repo" in the usage at /…/MODULE.bazel:21:34

$ bazel mod tidy
INFO: Updated use_repo calls for @api_probe//tools:ext.bzl%probe_ext      # 155 ms
```
It rewrote `use_repo(use_repo_rule_ext, "ghost_repo", "probe_generated_repo")` back to
`use_repo(use_repo_rule_ext, "probe_generated_repo")`, restoring the file byte-for-byte.

Two caveats found by experiment:
- `mod tidy` only fixes extensions that return `mctx.extension_metadata(...)`. With my
  extension returning `None`, `mod tidy` was a **2.5 s no-op** and left `ghost_repo` in
  place. After adding `extension_metadata(root_module_direct_deps=[...])` it worked.
- `mod tidy` **fetches and shells out to buildozer** — `@buildozer+.marker` appeared in the
  output base after the first run. First invocation therefore needs network.

### 6.7 What `MODULE.bazel.lock` does *not* give you

`lockFileVersion` is **24** (Bazel 8.7.0) / **28** (Bazel 9.2.0). Top-level keys:

- 8.7.0: `lockFileVersion, registryFileHashes, selectedYankedVersions, moduleExtensions, facts`
- 9.2.0: same plus `factsVersions`

**There is no module graph in the lockfile.** `registryFileHashes` is 128 URL→hash entries
(every `MODULE.bazel` the resolver looked at, including versions it rejected);
`moduleExtensions` holds extension evaluation results. An LSP **cannot** read the resolved
dependency graph or the repo mapping out of `MODULE.bazel.lock` — it must call `bazel mod`.
That kills the "parse the lockfile, never invoke Bazel" strategy for bzlmod.

---

## 7. `bazel info`

24 keys by default; `bazel help info-keys` lists 32 (including undocumented/expensive ones).
Full output on the probe workspace:

```
bazel-bin: /private/tmp/ob-probe/execroot/_main/bazel-out/darwin_arm64-fastbuild/bin
bazel-genfiles: …/bin
bazel-testlogs: …/testlogs
character-encoding: file.encoding = ISO-8859-1, defaultCharset = ISO-8859-1, sun.jnu.encoding = UTF-8
command_log: /private/tmp/ob-probe/command.log
committed-heap-size: 184MB
execution_root: /private/tmp/ob-probe/execroot/_main
gc-count: 40
gc-time: 332ms
install_base: /Users/bruth/.cache/bazel/_bazel_bruth/install/b8505634b4001c12e1a1c6250d772351
java-home: /nix/store/…/zulu-21.jdk/Contents/Home
java-runtime: OpenJDK Runtime Environment (build 21.0.11+10-LTS) by Azul Systems, Inc.
java-vm: OpenJDK 64-Bit Server VM (build 21.0.11+10-LTS, mixed mode, sharing) by Azul Systems, Inc.
local_resources: RAM=131072MB, CPU=18.0
max-heap-size: 32178MB
output_base: /private/tmp/ob-probe
output_path: /private/tmp/ob-probe/execroot/_main/bazel-out
package_path: %workspace%
release: release 8.7.0- (@non-git)
repository_cache: /Users/bruth/.cache/bazel/_bazel_bruth/cache/repos/v1
server_log: /private/tmp/ob-probe/java.log.imc.bruth.log.java.20260825-165348.14637
server_pid: 14637
used-heap-size: 93MB
workspace: /private/tmp/bazel-api-probe
```

Format is `key: value\n`. Asking for specific keys prints them **without** the `key: ` prefix
when a single key is requested, and *with* it when several are:
```
$ bazel info output_base
/private/tmp/ob-probe
$ bazel info output_base workspace release
output_base: /private/tmp/ob-probe
workspace: /private/tmp/bazel-api-probe
release: release 8.7.0- (@non-git)
```
An unknown key is `ERROR: unknown key(s): 'bogus_key'`, exit non-zero. 116 ms warm, 1.7 s cold.

**Keys an LSP actually needs:** `output_base` (→ `<ob>/external/<canonical>` for external
files), `workspace` (the repo root, from the *server's* point of view — note it reports
`/private/tmp/...` on macOS, the resolved path, not `/tmp/...`; you must canonicalise both
sides or goto-def URIs will mismatch), `execution_root` (its basename is the workspace name,
`_main` under bzlmod — starpls uses exactly this to detect legacy WORKSPACE names),
`release` (feature gating: `mod dump_repo_mapping` needs ≥ 6.4), `repository_cache`.

**`starlark-semantics`** is nearly useless in 8.7.0:
```
$ bazel info starlark-semantics
StarlarkSemantics{internal_bazel_only_utf_8_byte_strings=true}
```
It prints only **non-default** values. It will not tell you whether
`--incompatible_disallow_struct_provider_syntax` is on unless someone set it explicitly.
starpls stores it (`BazelInfo.starlark_semantics`) purely as a cache key. That is the only
sensible use.

**`--show_make_env`** adds the Make-variable environment to the front of the normal output:
```
BINDIR: bazel-out/darwin_arm64-fastbuild/bin
COMPILATION_MODE: fastbuild
GENDIR: bazel-out/darwin_arm64-fastbuild/bin
TARGET_CPU: darwin_arm64
```
Four variables. This is the set an LSP can offer for `$(BINDIR)`-style completion inside
`genrule.cmd`, though the *rule-provided* Make variables (`$(location …)`, toolchain vars)
are not here.

### 7.2 `bazel info build-language` — legacy, and bzlmod broke it

Binary `blaze_query.BuildLanguage` on stdout, 44 949 bytes, 135 ms.

Decoded: **47 `RuleDefinition`s, 1253 `AttributeDefinition`s, and 0 non-empty documentation
strings** — neither `RuleDefinition.documentation` nor `AttributeDefinition.documentation` is
populated in the released binary. It also contains **only native rules**:

```
action_listener alias available_xcodes cc_binary cc_import cc_libc_top_alias cc_library
cc_shared_library cc_static_library cc_test cc_toolchain cc_toolchain_alias
cc_toolchain_suite config_feature_flag config_setting constraint_setting constraint_value
environment extra_action fdo_prefetch_hints fdo_profile filegroup genquery genrule
java_binary java_import java_library java_package_configuration java_plugin
java_plugins_flag_alias java_runtime java_test java_toolchain label_flag label_setting
memprof_profile objc_import objc_library platform propeller_optimize …
```
`sh_binary`, `sh_test`, `py_binary`, `proto_library` are **absent** — they are Starlark rules
in `rules_shell` / `rules_python` / `protobuf` now. So `build-language` covers a shrinking
minority of what users actually write, with no docs.

**`bazel query --output=streamed_proto --proto:rule_classes` is the modern replacement** and
strictly dominates it: it covers Starlark rules, carries docstrings, enum `values`, provider
constraints and `originKey.file`. starpls still uses `build-language`
(`crates/starpls/src/bazel.rs:65`) and consequently has no docs for Starlark rules.

---

## 8. `bazel dump`

The help text tells you not to use it:

> Dumps the internal state of the bazel server process. **This command is provided as an aid
> to debugging, not as a stable interface, so users should not try to parse the output;
> instead, use 'query' or 'info' for this purpose.**

Every invocation prints `WARNING: This information is intended for consumption by developers
only, and may change at any time. Script against it at your own risk!`

| flag | output | verdict |
|---|---|---|
| `--rules` | `RULE / COUNT / ACTIONS` table (`probe_library 2 2`) | metrics only, no |
| `--rule_classes` | one line per rule class: `filegroup(name, visibility, …, srcs, output_group, data, output_licenses)` | **52 lines = native rules only.** No Starlark rules, no types, no docs. Strictly worse than `--proto:rule_classes`. |
| `--packages` | every loaded package with every attribute of every target, fully expanded | complete but unstructured and enormous (71 packages here, includes all external repos) |
| `--skyframe=summary` | `Node count: 4219 / Edge count: 12536` | metrics |
| `--skyframe=count` | per-`SkyFunction` counts (`FILE 1259, BZL_LOAD 435, PACKAGE 71, …`) | metrics; mildly interesting as a "how big is this workspace" probe |
| `--skyframe=deps/rdeps/function_graph/value/working_set` | raw Skyframe graph | debugging only; `--skykey_filter` regex |
| `--action_cache` | `String indexer content: … Action cache (0 records):` | no |
| `--skylark_memory=<path>` | `Cannot dump Starlark heap without running in memory tracking mode.` — needs `--host_jvm_args=-javaagent:…allocation_instrumenter` | no |
| `--memory=<mode>` | heap analysis | no |

**Nothing in `dump` is worth an LSP dependency.** The one honourable mention is
`--skyframe=count`, as a cheap way to size a workspace before deciding how aggressive to be.

---

## 9. Everything else in `bazel help`

Documented commands (8.7.0): `analyze-profile aquery build canonicalize-flags clean coverage
cquery dump fetch help info license mobile-install mod print_action query run shutdown sync
test vendor version`.

`bazel help completion` reveals one more:
```
BAZEL_COMMAND_LIST="analyze-profile aquery build canonicalize-flags clean config coverage
cquery dump fetch help info license mobile-install mod print_action query run shutdown sync
test vendor version"
```
**`config`** is hidden from `bazel help` but real:
```
$ bazel config
Available configurations:
541d87575d95ed9089b4c23e003a90dcc9ac8dd1b41b4ccb3482889dd32586a9 fastbuild-noconfig
7f8856bb0405540c89cb61a09c1d0562ac1a95a54b0f8a7e6cabe690edf58098 darwin_arm64-fastbuild
b6cf661d32fe6b41dbf802eac68902d259ecb9d061c15959eac3ff87ccc6f6f8 darwin_arm64-fastbuild
```
`bazel config <hash>` dumps the fragment values; `bazel config <a> <b>` diffs them. Post-
analysis only; a debugging aid for "why two configurations", not an LSP interface.

### 9.1 `bazel help flags-as-proto` — the `.bazelrc` LSP, handed to you

Base64 of a `bazel_flags.FlagCollection` (`src/main/protobuf/bazel_flags.proto`).
579 044 base64 chars → 434 283 bytes binary → **1052 `FlagInfo` messages**, 1042 with
documentation, 56 with enumerated values. 132 ms warm, 402 ms including the `base64 -d`.

```protobuf
message FlagInfo {
  required string name = 1;              // without leading dashes
  optional bool has_negative_flag = 2;   // --noX exists
  optional string documentation = 3;
  repeated string commands = 4;          // ['build','test',…]
  optional string abbreviation = 5;      // 'c' for --compilation_mode
  optional bool allows_multiple = 6;
  repeated string effect_tags = 7;
  repeated string metadata_tags = 8;
  optional string documentation_category = 9;
  optional bool requires_value = 10;
  optional string old_name = 11;
  optional string deprecation_warning = 12;
  optional string default_value = 13;
  repeated string option_expansions = 14;
  optional string type_converter = 15;
  repeated string enum_values = 16;
}
```
Decoded sample:
```json
{"name":"compilation_mode","abbrev":"c","has_negative":false,"requires_value":true,
 "default":"fastbuild","enum_values":["fastbuild","dbg","opt"],
 "doc":"Specify the mode the binary will be built in. Values: 'fastbuild', 'dbg', 'opt'.",
 "doc_category":"OUTPUT_PARAMETERS","effect_tags":["AFFECTS_OUTPUTS","ACTION_COMMAND_LINES"],
 "commands":["aquery","build","canonicalize-flags","clean","config","coverage","cquery",
             "fetch","info","mobile-install","mod","print_action","query","run","sync","test","vendor"]}
```
That is completion (name + `--no` variant + abbreviation), hover (doc + default), value
completion (`enum_values`), diagnostics ("this flag doesn't apply to `query`" from
`commands`, "deprecated" from `deprecation_warning`/`metadata_tags`) and quick-fix
("renamed from `old_name`") for `.bazelrc` — **version-accurate**, derived from the exact
Bazel binary the user runs. No hand-maintained flag table can compete.

`bazel help completion` (17 585 lines) is the shell-completion script data: `BAZEL_COMMAND_LIST`,
`BAZEL_INFO_KEYS`, and `BAZEL_COMMAND_<CMD>_FLAGS` with `=path`/`=label` type hints
(`--vendor_dir=path`, `--repository_cache=path`). Same information, coarser; prefer
`flags-as-proto`.

### 9.2 `bazel canonicalize-flags` — a `.bazelrc` linter

```
$ bazel canonicalize-flags -- --compilation_mode=opt -c dbg --define=x=1 --//:mode=prod
--compilation_mode=dbg
--define=x=1
--//:mode=prod
```
140 ms. Resolves abbreviations, drops flags overridden later, expands `--config`, and
**validates Starlark flags against the workspace**:
```
$ bazel canonicalize-flags -- --compilation_mode=turbo
ERROR: While parsing option --compilation_mode=turbo: Not a valid compilation mode:
  'turbo' (should be fastbuild, dbg or opt)

$ bazel canonicalize-flags -- --//:nope=1
ERROR: Error loading option //:nope: no such target '//:nope': target 'nope' not declared
  in package '' defined by /private/tmp/bazel-api-probe/BUILD.bazel

$ bazel canonicalize-flags -- --no_such_flag=1
ERROR: Unrecognized option: --no_such_flag=1
```
`--for_command=<cmd>` (default `build`) makes it validate against a different command's flag
set: `canonicalize-flags --for_command=query -- --output=proto --keep_going` →
`--output=proto\n--keep_going=1`.

Two caveats: the errors carry **no line numbers** (you pass flags, not a file, so you must
map back yourself), and the exit code was **0** even for `Unrecognized option`. Parse stderr,
not `$?`.

### 9.3 `bazel print_action` — actively harmful

```
$ bazel print_action //lib:core
… Analyzing … INFO: 2 processes: 1 internal, 1 darwin-sandbox …
ERROR: ConfiguredTarget(//lib:core, 7f8856b…) is not a supported target kind
```
It is an `extra_action`-based C++/Java-only debugging command, it **executes a build** as a
side effect (note "1 darwin-sandbox" — it really ran the genrule), and it errors on anything
else. Never call this from an LSP.

### 9.4 `bazel license`, `bazel version`

`license` prints the bundled Apache-2.0 text. `version` / `version --gnu_format` gives
`Build label`, `Build target`, `Build time`. `bazel info release` is the cheaper way to get
the version.

### 9.5 `fetch`, `vendor`, `sync`, `analyze-profile`

- `bazel fetch --repo @@<canonical>` — fetch one repo. **6.4 s** for `@@bazel_tools`.
  This is the safe way to warm a repo before querying it (starpls does exactly this,
  `client.rs:185`). Not something to do on the interactive path.
- `bazel vendor --vendor_dir=…` — materialise external repos into the workspace. Relevant to
  an LSP only as a *source layout* it may encounter (`vendor` manifests are in scope per the
  brief), not as an API.
- `bazel sync` — WORKSPACE-era; deprecated.
- `bazel analyze-profile` — reads `command.profile.gz`. Build-perf tooling, not LSP.

---

## 10. Build Event Protocol

`--build_event_json_file=<path>` (also `--build_event_binary_file`, `--build_event_text_file`).
Works for `build`/`test`; for `query` it produces **nothing useful** — the event stream is
`started, buildMetadata, unstructuredCommandLine, optionsParsed, structuredCommandLine ×3,
buildFinished, progress` and no target information at all.

For a real build, the event kinds are:
`started, progress ×7, buildMetadata, unstructuredCommandLine, optionsParsed,
structuredCommandLine ×3, pattern, workspace, workspaceStatus, configuration ×2,
targetConfigured ×2, namedSet ×2, targetCompleted ×2, actionCompleted,
convenienceSymlinksIdentified, buildFinished, buildToolLogs, buildMetrics`.

What an editor could want, and whether BEP has it:

| want | in BEP? |
|---|---|
| target list | **yes** — `pattern.children[].targetConfigured.label`, one per expanded target |
| target kind | **yes** — `configured.targetKind: "probe_library rule"` |
| output files | **yes** — `completed.outputGroup[].fileSets` → `namedSet` events |
| test results | yes — `testResult`/`testSummary` events (not exercised here) |
| **source locations** | **no** |
| **structured diagnostics** | **no** |

Diagnostics are **raw stderr text embedded in `progress` events**:
```json
{"id":{"progress":{"opaqueCount":3}},
 "progress":{"stderr":"ERROR: no such package 'nowhere': BUILD file not found …\nERROR: /private/tmp/bazel-api-probe/badbuild/BUILD.bazel:6:6: no such package 'nowhere': … and referenced by '//badbuild:missing'\n"}}
```
The closest thing to structure is `failureDetail`, which has a message and a category code
(`src/main/protobuf/failure_details.proto`) but **no file, line or column**:
```json
{"id":{"targetCompleted":{"label":"//badbuild:boom",…}},
 "completed":{"failureDetail":{"message":"bash failed: error executing Genrule command …",
                               "spawn":{"code":"NON_ZERO_EXIT","spawnExitCode":3}}}}
```
and
```json
{"id":{"actionCompleted":{"primaryOutput":"bazel-out/…/badbuild/boom.txt","label":"//badbuild:boom",…}},
 "action":{"exitCode":1,"stderr":{"name":"stderr","uri":"file:///…/bazel-out/_tmp/actions/stderr-2"},
           "type":"Genrule","commandLine":[…],"failureDetail":{…}}}
```

**Verdict.** BEP buys an LSP three things over plain stderr: (a) a clean per-target
success/failure map with kinds, keyed by label; (b) `actionCompleted.stderr.uri` pointing at
the raw action stderr on disk, which is what you'd surface for a compiler error; (c) a stable
JSON envelope so you're not screen-scraping the progress UI. It buys you **zero** location
information — you still need the same `path:line:col:` regex you'd apply to stderr, just
applied to `progress.stderr` strings instead. Use BEP if you're already invoking
`build`/`test` for a code lens; do not adopt it as the diagnostics transport for
loading-phase errors, since `query` doesn't populate it.

---

## 11. Latency and the lock — the experiments

### 11.1 Cold start

| | probe (19 targets) | bazel repo (4095 targets) |
|---|---|---|
| server restart, warm output base | 1.26 s | 1.99 s |
| brand-new `--output_base`, warm repo cache | 2.15 s | — |
| brand-new `--output_base`, empty repo cache (network) | 15.8 s | (n/a) |

Cold start is dominated by JVM boot, not by re-reading BUILD files: re-loading 1092 BUILD
files added only ~700 ms over the tiny workspace. A second server costs **~2 s** the first
time and **~125 ms** thereafter.

Idle-GC (`--idle_server_tasks`, default on, runs `System.gc()` when idle) had **no measurable
effect**: query, sleep 35 s, query → 124 ms then 79 ms. Not a concern for loading-phase work.

### 11.2 A query while a build holds the lock — it blocks

I ran `bazel build //gen:slow` (a genrule that `sleep 25`) and, 3 seconds in, a
`bazel query //lib:core` against the **same** output base:

```
Another command (pid=60731) is running. Waiting for it to complete on the server (server_pid=60585)...
//lib:core

real	0m19.107s
```

**19.1 seconds for a query that takes 140 ms.** The client prints the "Waiting…" line to
stderr and then hangs until the build finishes. This is the single most important operational
fact in this document: an LSP that shares the user's output base will freeze — for the
duration of the user's build — every time it wants a completion list.

### 11.3 `--noblock_for_lock` — fails fast, exit 9

```
$ bazel --noblock_for_lock --output_base=/tmp/ob-probe query //lib:core
                                                     # stdout: empty
Another command (pid=61632) is running. Exiting immediately.   # stderr
rc=9
real	0m0.134s
```
Exit code 9 = `blaze_exit_code::LOCK_HELD_NOBLOCK_FOR_LOCK`
(`src/main/cpp/util/exit_code.h:37`, thrown at `blaze_util_posix.cc:748`). 134 ms, clean.

This is a *correct* but *useless-on-its-own* answer: the LSP now has no data. It's the right
flag for an opportunistic refresh ("try the user's server, fall back to cache"), not for a
request that must be answered.

### 11.4 `--preemptible` — do not

`--preemptible` is a startup flag on the *long-running* command, marking it killable. I ran
`bazel --preemptible build //gen:slow` and then a query 5 s later:

```
# build shell:
ERROR: build interrupted
INFO: Elapsed time: 5.061s
ERROR: Build did NOT complete successfully

# query shell:
//lib:core
real	0m0.187s
```

The query got its answer in 187 ms **by killing the user's build**. Any LSP that sets this
(or encourages users to set it) is user-hostile. Mentioned only so nobody reaches for it.

### 11.5 The separate `--output_base` — the answer, with a bill

Running the same query against `--output_base=/tmp/ob-lsp` while the build held
`/tmp/ob-probe`:
```
query //... --output=location        0.199 s
query rdeps(//..., //lib:core)       0.290 s
```
Full concurrency, no interference, no waiting. This is the only viable design.

The costs, measured:

| cost | probe workspace | bazel repo |
|---|---|---|
| second server RSS (loading phase only) | **370–423 MB** | 338 MB |
| second server RSS after `deps(//...)` (26 854 targets) | — | **724 MB** |
| output base on disk (Bazel 8) | 104–108 MB | — |
| output base on disk after fetching everything (Bazel 9) | — | 1.2 GB |
| first query in a fresh output base | 2.15 s | — |

Bazel 8 **duplicates** external repos per output base (`/tmp/ob-lsp/external` = 108 MB of
real directories alongside `/tmp/ob-probe/external` = 149 MB). Bazel 9 **symlinks** them into
`~/.cache/bazel/_bazel_<user>/cache/repos/v1/contents/<sha256>/<uuid>`, so a second output
base is nearly free on disk. Either way the *repository download cache*
(`bazel info repository_cache`) is shared, so a second output base does **not** re-download —
2.15 s, no network.

So the bill for a private output base is roughly **+400 MB RSS and +100 MB disk** on Bazel 8,
**+400 MB RSS** on Bazel 9. On a large monorepo where the user's own server is 8 GB, doubling
it is not acceptable by default. This has to be a setting, and the default should probably be
"share the user's output base, use `--noblock_for_lock`, serve stale data when locked", with
the private base as an opt-in for people with RAM.

Both `starpls` (HEAD `ac25eca`, 2025-12-03) and `bazel-lsp` (HEAD `48fead6`, 2025-07-20)
invoke `bazel` with **no `--output_base` override** in their server paths
(`starpls/crates/starpls/src/server.rs:93` → `BazelCLI::new(&bazel_path)`, and
`BazelCLI::run_command` at `client.rs:52` passes only the subcommand args). starpls's `check`
subcommand accepts `--output_base` (`commands/check.rs:41`) but the LSP server does not. They
both have the §11.2 freeze.

### 11.6 `--nofetch` — the safety valve

The 6 m 34 s `deps(//...)` was almost entirely repository fetching. `--nofetch` converts that
hang into a fast, legible failure:
```
$ bazel --output_base=/tmp/ob-nofetch query --nofetch '//...'
ERROR: Error computing the main repository mapping: error during computation of main repo
  mapping: to fix, run
	bazel fetch //...
External repository @@bazel_tools not found and fetching repositories is disabled.
real	0m0.991s
```
and after `bazel fetch --repo @@bazel_tools`:
```
ERROR: error loading package under directory '': no such package '@@protobuf+//bazel/common':
  to fix, run
	bazel fetch //...
External repository @@protobuf+ not found and fetching repositories is disabled.
real	0m0.241s
```
**Every interactive query should carry `--nofetch --keep_going`.** Fetching should be an
explicit, progress-reported, cancellable background job the LSP schedules — never a side
effect of hovering.

### 11.7 Incremental re-query after an edit

The actual LSP workload. On the bazel repo:

| action | time |
|---|---|
| `touch` a BUILD file, `query //...` | 0.209 s |
| append a `filegroup` to a BUILD file, `query //...` | 0.215 s |
| same, then `rdeps(//..., X)` | 0.878 s |
| append again, then `deps(//...)` (26 854 targets) | 0.719 s |

Bazel's Skyframe invalidation is doing exactly what you'd want: an edit to one BUILD file
re-loads one package. **Re-querying after every save is affordable.** The stat-the-workspace
diff-awareness pass costs ~100 ms on a 271 MB / ~30 k-file repo; on a genuinely huge monorepo
this is the number to watch, and `--watchfs` (default off) exists to replace it.

### 11.8 Unsaved buffers — the `--package_path` overlay trick

Bazel reads from disk; the LSP holds dirty buffers. There *is* a way to bridge this that I
verified works on **both 8.7.0 and 9.2.0**:

```
$ mkdir -p /tmp/overlay/lib && cat > /tmp/overlay/lib/BUILD.bazel <<'EOF'
filegroup(name = "core", srcs = [])
filegroup(name = "UNSAVED_EDIT", srcs = [])
EOF
$ bazel --output_base=/tmp/ob-overlay query --package_path=/tmp/overlay:%workspace% \
    '//lib:all' --output=location
/tmp/overlay/lib/BUILD.bazel:2:10: filegroup rule //lib:UNSAVED_EDIT
/tmp/overlay/lib/BUILD.bazel:1:10: filegroup rule //lib:core
```

The overlay entry wins for any package whose BUILD file it contains, and locations come back
pointing into the overlay, which the LSP maps back to the real URI. `--package_path` is not
deprecated (`PackageOptions.java:70`, no deprecation marker) and still defaults to
`%workspace%` in 9.2.0.

Caveats: resolution is **per package**, and once the overlay provides a package's BUILD file,
*all* of that package's source files resolve relative to the overlay — so globs and
`srcs = ["a.txt"]` break unless you mirror (or symlink) every file in the package. Combined
with a private output base this is workable for "run diagnostics on the buffer I'm typing in",
but it doubles the invalidation churn. A safer default is to query on save.

### 11.9 Other flags that matter for isolation

- `--ignore_all_rc_files` — an LSP should consider this so the user's `.bazelrc`
  (`--config=…`, `-c opt`) doesn't silently change what the index means. But it also drops
  `common` settings the workspace genuinely needs. Probably: don't ignore, but *do* record
  `--announce_rc` output so you can explain what you inherited:
  ```
  INFO: Reading 'startup' options from /nix/store/…/system.bazelrc: --server_javabase=…
  INFO: Reading rc options for 'query' from /private/tmp/bazel-api-probe/.bazelrc:
    Inherited 'common' options: --enable_bzlmod
  ```
  No line numbers, but the file list and the per-command inheritance are there.
- `--max_idle_secs` (default 10800 = 3 h). A private LSP server should set this low
  (e.g. 900) so it doesn't sit on 400 MB after the editor closes, or `0` to never exit if
  you'd rather never pay cold start.
- `--tool_tag=my-lsp` — tags the invocation in BEP/profiles so users can tell your calls
  apart from theirs.

---

## 12. `bazel --batch`, `--client_debug`, and speaking gRPC directly

### 12.1 `--batch` — never

```
$ bazel --batch --output_base=/tmp/ob-probe query //lib:core
  - Only in old server: --noshutdown_on_low_sys_mem
  - Only in new server: --batch                          ← it killed the running server
//lib:core
real	0m2.763s

$ bazel --batch --output_base=/tmp/ob-batch query //lib:core
real	0m6.191s
```
2.8–6.2 s per invocation, no Skyframe reuse between calls, and — the killer — **passing
`--batch` to an output base with a running non-batch server restarts that server**, because
startup options differ. An LSP using `--batch` would repeatedly nuke the user's warm build
server. Disqualified.

### 12.2 `--client_debug`

Startup-flag diagnostics only, on stderr:
```
[INFO 17:10:26.154 src/main/cpp/option_processor.cc:416] Looking for the following rc files:
  /nix/store/…/system.bazelrc,/private/tmp/bazel-api-probe/.bazelrc,/Users/bruth/.bazelrc
[INFO 17:10:26.156 src/main/cpp/rc_file.cc:94] Parsing the RcFile /private/tmp/bazel-api-probe/.bazelrc
[INFO 17:10:26.159 src/main/cpp/blaze.cc:1484] Acquired the client lock, waited 0 milliseconds
[INFO 17:10:26.166 src/main/cpp/blaze.cc:1688] Trying to connect to server (timeout: 30 secs)...
[INFO 17:10:26.174 src/main/cpp/blaze.cc:1205] Connected (server pid=61388).
[INFO 17:10:26.174 src/main/cpp/blaze.cc:1966] Released the client-side locks on the install and output bases
```
Useful once, when debugging why your LSP picked up the wrong rc file (**note it reads
`~/.bazelrc` too**). Not an API. It does confirm the lock model: the client takes a *file*
lock on the install and output bases, connects, then releases; the *command* lock lives in
the server.

### 12.3 The gRPC command server — yes, an LSP can speak it directly

`src/main/protobuf/command_server.proto` (280 lines, proto3, `package command_server`):

```protobuf
service CommandServer {
  rpc Run(RunRequest) returns (stream RunResponse) {}
  rpc Cancel(CancelRequest) returns (CancelResponse) {}
  rpc UpdateTerminalSize(TerminalSizeRequest) returns (TerminalSizeResponse) {}
  rpc Ping(PingRequest) returns (PingResponse) {}
}

message RunRequest {
  string cookie = 1;              // from <output_base>/server/request_cookie
  repeated bytes arg = 2;         // command + args, NOT startup args
  bool block_for_lock = 3;        // false ⇒ immediate error if another command runs
  string client_description = 4;  // required
  string invocation_policy = 5;
  repeated StartupOption startup_options = 6;   // logging only; already applied
  bool preemptible = 7;
  repeated google.protobuf.Any command_extensions = 8;
  bool quiet = 9;
}

message RunResponse {
  string cookie = 1;              // from <output_base>/server/response_cookie
  bytes standard_output = 2;      // chunked
  bytes standard_error = 3;       // chunked
  bool finished = 4;
  int32 exit_code = 5;            // valid when finished
  string command_id = 6;          // pass to Cancel
  bool termination_expected = 7;
  ExecRequest exec_request = 8;
  failure_details.FailureDetail failure_detail = 9;   // marked experimental
  repeated google.protobuf.Any command_extensions = 10;
}

message ServerInfo { int32 pid = 1; string address = 2; string request_cookie = 3; string response_cookie = 4; }
```

Connection details live in `<output_base>/server/`:
```
$ ls /tmp/ob-lsp/server/
cmdline  command_port  jvm.out  request_cookie  response_cookie  server_info.rawproto  server.pid.txt
$ cat command_port      → [::1]:49576
$ cat request_cookie    → bbf9d1ed2d496d382f62bb38da56e511
$ cat response_cookie   → 5cc3497d622dee1e9529e88d85f9551
```
`server_info.rawproto` is the same three things as a `ServerInfo` message.

**I did it.** No reflection (`Failed to list services: server does not support the reflection
API`), so you must ship the `.proto`:

```
$ grpcurl -plaintext -import-path . -proto src/main/protobuf/command_server.proto \
    -d '{"cookie":"bbf9d1ed…","arg":["cXVlcnk=","Ly9saWI6Y29yZQ=="],
         "block_for_lock":true,"client_description":"lsp-probe"}' \
    '[::1]:49576' command_server.CommandServer/Run
{"cookie":"5cc3497d…","commandId":"67c76ffd-889b-4986-b217-c394df8707c5"}
{"cookie":"5cc3497d…","standardError":"TG9hZGluZzogMCBwYWNrYWdlcyBsb2FkZWQK","commandId":"67c76ffd-…"}
{"cookie":"5cc3497d…","standardOutput":"Ly9saWI6Y29yZQo=","commandId":"67c76ffd-…"}
{"cookie":"5cc3497d…","finished":true,"commandId":"67c76ffd-…"}
```
`Ly9saWI6Y29yZQo=` decodes to `//lib:core\n`. A full `query //... --output=location` this way
took **234 ms** including grpcurl's own process start *and* its runtime proto compilation,
versus 296 ms for the CLI.

Why it's worth doing:

1. **Kills the ~130 ms/call client tax.** A persistent channel eliminates process fork, rc
   file re-parsing, install-base lock, and connection setup on every request. With 1052 flags
   parsed per invocation and rc files re-read every time, that's most of the floor.
2. **Real cancellation.** `Cancel(command_id)` lets you abort an in-flight query when the
   user keeps typing. Killing the CLI process is a much blunter instrument (and can leave the
   server holding the command lock).
3. **`block_for_lock: false` per request**, decided at call time, without a process-level
   startup flag.
4. **Streaming stdout.** `standard_output` arrives in chunks, so a
   `--output=streamed_proto` scan of a 15 MB result can start decoding immediately.
5. **`failure_detail`** on the response, structured (though the comment says
   "WARNING: This functionality is experimental").

Why it's not free:

- **You must still run the client once** to start the server, apply startup options
  (`--output_base`, `--max_idle_secs`, `--host_jvm_args`) and create `server/`. Startup
  options cannot be changed over gRPC — `RunRequest.startup_options` is explicitly
  "for logging only". Practical shape: `bazel --output_base=… info server_pid` to boot, then
  gRPC for everything after.
- **Cookies and port rotate** on every server restart. You must re-read `server/` and detect
  server death (`Ping`, or watch `server.pid.txt`).
- **The proto is not a stable public API** in the way `build.proto` is. It has changed
  (`UpdateTerminalSize` is recent). Pin a copy and be defensive.
- It's the same lock semantics — gRPC doesn't get you past a running build; you still need
  `block_for_lock:false` or a separate output base.

**Recommendation: fork the CLI for v1, design the Bazel-invocation layer behind an interface,
and move to gRPC once the 130 ms floor actually shows up in a profile.** The ~130 ms tax only
matters if you are making several calls per keystroke, which you shouldn't be.

---

## 13. What Bazel does *not* give you, at any price

1. **Character-level positions for anything below a target.** No attribute location, no label
   location within a list, no `load()` symbol location, no `select()` key location. Every
   precise range in the LSP — semantic tokens, rename, find-references highlighting,
   completion trigger context — must come from your own parser.
2. **Anything about a `.bzl` file's contents.** There is no `bazel query` for "what functions
   does this `.bzl` define", "what does this `load()` resolve to", "what are the fields of
   this provider". `--proto:rule_classes` gets you rule/provider *names and origin files*
   only because they were instantiated. A `.bzl` file with no users is invisible to Bazel's
   query surface. (`stardoc`'s `starlark_doc_extract` rule is the escape hatch, and it needs
   a build.)
3. **Unsaved buffers**, except via the `--package_path` overlay hack (§11.8).
4. **Structured diagnostics.** Everything is `path:line:col: message` on stderr, in at least
   two different formats (single-line and Starlark traceback), plus BEP's
   `progress.stderr` which is the same text in a JSON wrapper.
5. **Docs for native rules.** `bazel info build-language` ships with all documentation
   stripped (0 of 47 rule docstrings, 0 of 1253 attribute docstrings). Native rule/attribute
   documentation must be vendored from bazel.build or from the Bazel source tree.
6. **The module graph offline.** `MODULE.bazel.lock` v24/v28 has no graph (§6.7).
7. **Cheap `deps()` across external repos.** 6 m 34 s cold.

---

## 14. Recommendation

Assumed architecture: a **static index** (own parser over BUILD/.bzl/MODULE files — the only
source of character-precise positions) **enriched** by a small, fixed set of Bazel calls that
supply the things a parser cannot know (macro expansion, resolved rule classes, repo mapping,
version resolution).

Legend: **Interactive** = safe on the request path, sub-second; **Background** = run on
save/idle, cache the result; **Static** = must be your own parser/index.

| LSP feature | Bazel interface | Verdict |
|---|---|---|
| **Goto-def: `//pkg:target`** | `query <label> --output=location --relative_locations`, or an index built from `query //... --output=streamed_proto` | **Interactive** (135 ms warm). Gives the *declaration site* including macro call sites, which a static parser gets wrong for macro-generated targets. Fall back to static (`pkg/BUILD` + name search) when Bazel is unavailable or locked. |
| **Goto-def: `//pkg:target` → the exact `name = "…"` token** | — | **Static.** Bazel gives you `line:col` of the rule call; the LSP must scan for the `name` attribute if you want to land on the string. |
| **Goto-def: `@repo//pkg:file.bzl`** | `mod dump_repo_mapping <from_repo>` + `info output_base` → `<ob>/external/<canonical>/pkg/file.bzl` | **Interactive** (132 ms, cache per repo, invalidate on MODULE.bazel change). On Bazel 9 `realpath` the symlink into the repo contents cache. This is the one call you cannot do statically. |
| **Goto-def: `load()` symbol → its `def`/`rule()` in the `.bzl`** | `query --proto:definition_stack` gives `tools/defs.bzl:28:21` for *rule* symbols only | **Static** for the general case (functions, constants, providers). Use `definition_stack`/`originKey.file` as a cross-check and for rules defined via `rule()` indirection your parser can't follow. |
| **Goto-def: `bazel_dep(name=…)` → the module** | `mod show_repo @name` (gives `http_archive` + `remote_module_file_urls` BCR URL) and `info output_base` | **Interactive.** Also enables "open in BCR". |
| **Goto-def: `use_repo("x")` → where x is created** | `mod show_extension <ext>` (prints `Usage in <root> from …/MODULE.bazel:21`), `mod show_repo @x` (prints `Rule probe_repo defined at …/tools/ext.bzl:7:29`) | **Interactive**, but `show_extension` on a heavy extension (e.g. `rules_jvm_external%maven`) may evaluate it. Restrict to extensions already evaluated in the lockfile. |
| **Goto-def: `select()` key → `config_setting`** | `query --noproto:flatten_selects` yields `selectorList.elements[].entries[].label`; the label then resolves like any target | **Interactive** for resolution; **Static** for the column of the key inside the dict. |
| **Find refs: target** | `rdeps(//..., X)` (748 ms / 1899 hits at 4 k targets); `same_pkg_direct_rdeps(X)` for the fast path; `attr(<name>, "X", //...)` to catch `nodep` labels | **Interactive** (single call, ~1 s). But it returns *referring targets*, not ranges → post-process with a text scan of the returned BUILD files. Whole-workspace text search is a viable static alternative and gives exact ranges; use Bazel to *verify* and to catch references you'd miss (aliases, macro-internal, `select()` keys). |
| **Find refs: `.bzl` file** | `query --universe_scope=//... --order_output=no 'rbuildfiles(path/to/file.bzl)'` | **Interactive** (597 ms at 1092 BUILD files). Better than text search: it's transitive. |
| **Find refs: a rule/macro symbol** | `kind("<rule_class> rule", //...)` for rules | **Interactive** for rules. **Static** for macros — a macro leaves no trace in the query graph beyond `instantiation_stack`, which you can mine, but only for macros that actually produced targets. |
| **Rename a target + rewrite referrers** | `rdeps` to find the set, then **buildozer** to rewrite | **Background** for discovery, out-of-process for the edit. Bazel cannot rewrite; `buildozer` (bazel-buildtools 8.5.1) is the tool. |
| **Workspace symbols** | `query //... --output=streamed_proto --proto:output_rule_attrs= --noproto:default_values --relative_locations --consistent_labels --keep_going --nofetch` | **Interactive** (105–140 ms, 6.1 MB at 4 k targets). Re-run on BUILD-file save (215 ms incremental). This is the flagship use of the CLI. |
| **Document symbols** | `query '//pkg:*' --output=location` | **Static** is better — you already parsed the file and have exact ranges. Use Bazel only to surface macro-generated targets the parser can't see. |
| **Completion: labels (`//pkg:` targets)** | the `//...` index from workspace symbols | **Interactive** off the cached index; never a live call per keystroke. |
| **Completion: packages (`//pk<TAB>`)** | `query //... --output=package`, or just walk the filesystem for BUILD files | **Static** (filesystem walk) — faster and works while Bazel is locked. |
| **Completion: labels in external repos** | `mod dump_repo_mapping` for the `@repo` part; `query '@repo//...'` for targets | **Background.** `@repo//...` forces the repo to load; do it lazily, per repo, once, with `--nofetch` and an explicit fetch offer on failure. |
| **Completion: rule names** | `query //... --output=streamed_proto --proto:rule_classes` → one `RuleInfo` per rule class | **Background**, cached per `ruleClassKey`. Only covers *instantiated* rule classes; a rule defined but unused is invisible → merge with static `.bzl` analysis. |
| **Completion: attribute names** | `RuleInfo.attribute[].name` from `--proto:rule_classes`; `info build-language` for natives | **Background**, cached. `--proto:rule_classes` is strictly better than `build-language` (Starlark rules + docs + enums). |
| **Completion: attribute values (enums)** | `RuleInfo.attribute[].values` — e.g. `["\"plain\"","\"spicy\"","\"mild\""]` | **Background**, cached. Note values arrive **quoted**; strip. |
| **Completion: `visibility`, `tags`, `size`, `testonly`** | not in any Bazel interface | **Static** — hardcode. `tags` values are convention, not schema. |
| **Completion: `use_repo(ext, "…")`** | `mod graph --extension_info=all` (dashed entries = generable but not imported) | **Background**, expensive for big extensions. Prefer offering the `bazel mod tidy` code action instead. |
| **Completion: `bazel_dep` names/versions** | none — this is BCR data | **Static** (fetch `bcr.bazel.build` metadata out of band). `mod graph` only tells you what's already resolved. |
| **Completion: `.bazelrc` flags/values** | `help flags-as-proto` → 1052 `FlagInfo` with docs, defaults, `enum_values`, `commands`, `abbreviation`, `deprecation_warning` | **Background** once per Bazel version, cached by `info release`. Best-in-class; nothing static can match it. |
| **Hover: rule docs** | `RuleInfo.docString` from `--proto:rule_classes` | **Background**, cached. For native rules there are no docs anywhere in the CLI (§8.2) → vendor them. |
| **Hover: attribute docs** | `RuleInfo.attribute[].docString` + `defaultValue` + `providerNameGroup` | **Background**, cached. |
| **Hover: resolved label → path** | `mod dump_repo_mapping` + `info output_base`; `query <label> --output=location` | **Interactive.** |
| **Hover: what does this target actually build** | `cquery <label> --output=files` / `--output=starlark --starlark:expr='providers(target)'` | **On-demand only** (user-invoked command / code lens). Requires analysis: 1.6 s cold on 19 targets, minutes on a monorepo. Never on hover-by-default. |
| **Hover: `select()` resolved value** | `cquery --output=jsonproto` (selects are flattened) | **On-demand only**, same reason. |
| **Hover: why is `bazel_dep` at this version** | `mod explain <mod> --verbose --include_unused` (**text output** — the JSON `resolutionReason` is broken, §6.3) | **Interactive** (131 ms), but you must parse the tree-drawing text. File the JSON bug upstream. |
| **Diagnostics: buildifier lints** | not Bazel | **Static** — `buildifier --mode=check --lint=warn --format=json`. |
| **Diagnostics: syntax / Starlark type errors** | Bazel reports them as tracebacks on stderr, only when the package loads | **Static** — your own evaluator gives them per-keystroke without a subprocess. Use Bazel as a second opinion on save. |
| **Diagnostics: unknown attribute / bad enum value** | `query '//pkg:all'` stderr: `path:line:col: <label>: invalid value in 'flavor' attribute: has to be one of 'plain', 'spicy', or 'mild'` | **Interactive on save** (`--keep_going --nofetch`). Bazel is authoritative here; a static checker would need the full rule schema anyway, which comes from `--proto:rule_classes`. |
| **Diagnostics: unresolved label** | `query --keep_going 'deps(//pkg:all)'` → `no such target '//lib:does_not_exist' … and referenced by '//broken:bad_dep'` | **Background on save**, scoped to the edited package's `deps()`, **never** `deps(//...)` (§11.1: 6 m 34 s). Location is the rule, not the label → map back with your index. A static check against the workspace-symbol index catches most of these with exact ranges and zero latency; use Bazel for the external-repo cases. |
| **Diagnostics: duplicate target name** | query stderr Starlark traceback: `filegroup rule 'dup' conflicts with existing filegroup rule, defined at …:15:10` — **with both locations** | **Static** is trivially better (exact ranges, instant). |
| **Diagnostics: visibility violation** | `query 'visible(//app:app, //lib:*)'`, or `cquery`/`build --nobuild` for the real error | **Background.** `visible()` is loading-phase and cheap; the full check needs analysis. |
| **Diagnostics: wrong rule type in attribute** | analysis only: `cquery` → `bzl_library rule '@@…' is misplaced here (expected sh_library)` | **On-demand only.** Or approximate statically from `RuleInfo.attribute[].providerNameGroup`. |
| **Diagnostics: stale `use_repo`** | `build --nobuild` warning at `MODULE.bazel:21:34` | **Background on MODULE.bazel save.** |
| **Diagnostics: `.bazelrc`** | `canonicalize-flags --for_command=<cmd> -- <flags>` (validates names, values, Starlark flag labels); `help flags-as-proto` for the schema | **Interactive** (140 ms), but errors carry no line numbers and the exit code lies (0 on `Unrecognized option`) — feed it one flag at a time, or match messages back to lines yourself. |
| **Diagnostics: cycles** | `query` reports cycles during loading; `mod graph` marks `(cycle)` | **Background.** |
| **Code lens: build/test this target** | `query 'tests(//pkg:all)'`, `Rule.test`/`Rule.executable` in the proto | **Interactive** off the index. Run via `build`/`test` with `--build_event_json_file` for structured results. |
| **Code action: `bazel mod tidy`** | `mod tidy` (155 ms, edits MODULE.bazel in place, needs `@buildozer` fetched once) | **On-demand.** Ideal code action — Bazel does the edit for you. |
| **Code action: add dep / fix visibility** | none | **Static** + `buildozer`. |
| **Formatting** | none | **Static** — `buildifier`. |
| **Semantic tokens, inlay hints** | none | **Static.** |
| **`bazel aquery`** | — | **Not applicable.** No source locations; requires analysis; answers build questions, not BUILD-file questions. |
| **`bazel dump`** | — | **Not applicable.** Explicitly unstable ("Script against it at your own risk"), `--rule_classes` covers native rules only. |
| **`bazel print_action`** | — | **Never call.** Runs a build as a side effect; C++/Java only. |
| **BEP (`--build_event_json_file`)** | target list + kinds + per-target success + action stderr URIs; **no locations, no structured diagnostics** | **Use only** if already running `build`/`test` for a code lens. Not a diagnostics transport for loading-phase errors (`query` emits an empty BEP). |

### Operating rules that fall out of the measurements

1. **Always** `--keep_going --nofetch --noshow_progress` on interactive queries, and
   `--tool_tag=<your-lsp>`. One broken package otherwise blanks the index; one unfetched repo
   otherwise hangs for minutes.
2. **Never** run `deps(//...)`, `cquery //...`, `aquery`, or anything that crosses into
   unfetched external repos on the request path.
3. Decide the output-base policy explicitly. Sharing the user's base means a 19-second freeze
   during their builds (§11.2). A private base costs ~400 MB RSS. The defensible default is
   *share + `--noblock_for_lock` + serve the last-known-good index*, with private-base as an
   opt-in.
4. Batch Bazel calls. Each fork costs ~130 ms of pure overhead; one
   `query //... --output=streamed_proto --proto:rule_classes` answers workspace symbols,
   rule-name completion, attribute completion, attribute-value completion and hover docs in a
   single 140 ms call.
5. Key every Bazel-derived cache on `bazel info release` — the flag list, the native rule
   set, the lockfile version and the external-repo layout all changed between 8 and 9.
