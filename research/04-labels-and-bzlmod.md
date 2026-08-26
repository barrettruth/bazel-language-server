# 04 — Label resolution, repository mapping, and bzlmod

Research date **2026-08-25**. Upstream checkout `upstream/bazel` is at
`3a9b19c85b11` (2026-08-24), `.bazelversion` = `9.2.0`, `BazelLockFileValue.LOCK_FILE_VERSION = 28`.
All empirical results below were produced locally with **nixpkgs `bazel_8` = 8.7.0**
(`/nix/store/w24fpafgc7ddaqbwy939v1g33k3lf6fg-bazel-8.7.0`) and **bazelisk 1.29.0 →
Bazel 9.2.0**, in scratch workspaces under `/tmp/bzl-exp{1..4}` and `/tmp/bzlmod-scratch`.

---

## 0. The one-paragraph answer

A label is *not* a path. Resolving `@foo//bar:baz` to a file requires three
lookups that a filesystem walk cannot do: (a) apparent→canonical repo name via a
**repository mapping** that depends on the *file the label is written in*, (b)
the canonical repo's **physical directory**, which only exists after that repo
has been fetched into `$output_base/external/<canonical>`, and (c) the **package
boundary** — the nearest ancestor directory containing `BUILD`/`BUILD.bazel`,
subject to `.bazelignore` and `REPO.bazel`. (a) is cheaply obtainable statically
(`bazel mod dump_repo_mapping`, ~0.2 s warm, no repo fetching). (b) is *not* —
on a fresh clone `$output_base/external/` does not exist at all. (c) is
filesystem-only and cheap. There is no static path for repos produced by module
extensions beyond their *names*.

---

## 1. Complete label grammar

### 1.1 Authoritative source

| Concern | File |
|---|---|
| Tokenizer/splitter | `src/main/java/com/google/devtools/build/lib/cmdline/LabelParser.java` (233 lines) |
| Character validation | `src/main/java/com/google/devtools/build/lib/cmdline/LabelValidator.java` (350 lines) |
| Repo-name validation | `src/main/java/com/google/devtools/build/lib/cmdline/RepositoryName.java` (`validate`, l.148) |
| Label object + Starlark API | `src/main/java/com/google/devtools/build/lib/cmdline/Label.java` (843 lines) |
| Package identity | `src/main/java/com/google/devtools/build/lib/cmdline/PackageIdentifier.java` |
| Apparent→canonical map | `src/main/java/com/google/devtools/build/lib/cmdline/RepositoryMapping.java` (171 lines) |
| Path constants | `src/main/java/com/google/devtools/build/lib/cmdline/LabelConstants.java` |
| **Target patterns** (a *different* grammar) | `src/main/java/com/google/devtools/build/lib/cmdline/TargetPattern.java` (950 lines) |

`LabelValidator.java` is explicitly documented as a shared entry point: *"The
methods in this file are called both by Bazel and external programs"* — it is the
file to port.

### 1.2 The splitting table (verbatim from `LabelParser.Parts.parse`, l.85–107)

```
 raw                  | repo   | repoIs-   | pkgIs-   | pkg       | pkgEndsWith- | target
                      |        | Canonical | Absolute |           | TripleDots   |
----------------------+--------+-----------+----------+-----------+--------------+-----------
"foo/bar"             | null   | false     | false    | ""        | false        | "foo/bar"
"..."                 | null   | false     | false    | ""        | true         | ""
"...:all"             | null   | false     | false    | ""        | true         | "all"
"foo/..."             | null   | false     | false    | "foo"     | true         | ""
"//foo/bar"           | null   | false     | true     | "foo/bar" | false        | "bar"
"//foo/..."           | null   | false     | true     | "foo"     | true         | ""
"//foo/...:all"       | null   | false     | true     | "foo"     | true         | "all"
"//foo/all"           | null   | false     | true     | "foo/all" | false        | "all"
"@repo"               | "repo" | false     | true     | ""        | false        | "repo"
"@@repo"              | "repo" | true      | true     | ""        | false        | "repo"
"@repo//foo/bar"      | "repo" | false     | true     | "foo/bar" | false        | "bar"
"@@repo//foo/bar"     | "repo" | true      | true     | "foo/bar" | false        | "bar"
":quux"               | null   | false     | false    | ""        | false        | "quux"
"foo/bar:quux"        | null   | false     | false    | "foo/bar" | false        | "quux"
"//foo/bar:quux"      | null   | false     | true     | "foo/bar" | false        | "quux"
"@repo//foo/bar:quux" | "repo" | false     | true     | "foo/bar" | false        | "quux"
```

Splitting algorithm, precisely (l.109–166):

1. `repoIsCanonical = raw.startsWith("@@")`.
2. If `raw.startsWith("@")`:
   - `doubleSlashIndex = raw.indexOf("//")`. If `< 0`, this is the **`@foo`
     shorthand**: `repo = raw[1..]` (or `raw[2..]`), `pkg = ""`,
     `target = repo`, `pkgIsAbsolute = true`. So `@foo ≡ @foo//:foo`.
     Consequence: **`@` and `@@` on their own are syntax errors** — the target
     name is empty. Verified:
     `bazel query '@@'` → `ERROR: invalid target name '': empty target name`.
   - Otherwise `repo = raw[1 or 2 .. doubleSlashIndex]`, package starts at
     `doubleSlashIndex + 2`.
   - Note: `repo` may be **empty**: `@//pkg:t` and `@@//pkg:t` both parse with
     `repo = ""`. `""` maps to `RepositoryName.MAIN`. Verified: all three of
     `//pkg:srcs`, `@//pkg:srcs`, `@@//pkg:srcs` resolve to the same target.
3. Else: `pkgIsAbsolute = (doubleSlashIndex == 0)`; `repo = null` (meaning "the
   current repo").
4. `colonIndex = raw.indexOf(':', startOfPackage)` — the **first** colon after
   the repo part. Everything before is `rawPkg`, everything after is `target`.
   A second colon is therefore part of the target name and is rejected by
   `validateTargetName`.
5. `pkgEndsWithTripleDots = rawPkg.endsWith("/...") || rawPkg.equals("...")`.
6. Shorthands:
   - no colon and pkg ends in `...` → `target = ""`.
   - no colon and not absolute → **`foo/bar ≡ :foo/bar`** (pkg is empty, the
     whole string is the target).
   - no colon and absolute → **`//foo/bar ≡ //foo/bar:bar`** (last package
     segment).
7. `target.endsWith("/.")` is silently truncated by 2 chars
   (`validateAndProcessTargetName`, l.228 — *"TODO(bazel-team): This should be an
   error, but we can't make it one for legacy reasons"*).

### 1.3 Validation — repository names

`RepositoryName.validate` (l.148):

```java
private static final Pattern VALID_REPO_NAME = Pattern.compile("[\\w\\-.+]*");
// Must start with a letter. Can contain ASCII letters and digits, underscore, dash, and dot.
private static final Pattern VALID_USER_PROVIDED_NAME = Pattern.compile("[a-zA-Z0-9][-.\\w]*$");
public static final Pattern VALID_MODULE_NAME = Pattern.compile("[a-z]([a-z0-9._-]*[a-z0-9])?");
```

- `""` and `"_builtins"` are always accepted (early return).
- `"."` and `".."` get a dedicated error: *"invalid repository name '%s': repo
  names are not allowed to be '%s'"*.
- Otherwise: *"invalid repository name '%s': repo names may contain only A-Z,
  a-z, 0-9, '-', '_', '.' and '+'"*.
- **User-provided** names (`repo_name=`, `use_repo` keys, repo rule `name=`)
  are stricter: `validateUserProvidedRepoName` forbids `+` and requires a
  leading alphanumeric — *"invalid user-provided repo name '%s': valid names may
  contain only A-Z, a-z, 0-9, '-', '_', '.', and must start with a letter or a
  number"*.
- `RepositoryName.isApparent(name)` = `!name.isEmpty() && !name.contains("+")`.
  **This is the cheap heuristic an LSP can use to distinguish canonical from
  apparent names** — the `+` is deliberately reserved (see §2.2).
- **Module** names are stricter still: lowercase-first, lowercase/digit-last,
  only `[a-z0-9._-]` in between.

### 1.4 Validation — package names

`LabelValidator.validatePackageName` (l.96) + `ALLOWED_CHARACTERS_IN_PACKAGE_NAME`
(l.56):

```java
CharMatcher.inRange('0','9').or(inRange('a','z')).or(inRange('A','Z'))
    .or(CharMatcher.anyOf(" !\"#$%&'()*+,-./;<=>?@[]^_`{|}~"))
```

Error strings, exactly:

| Condition | Message |
|---|---|
| `""` | *(valid — this is `//:foo`)* |
| starts with `/` | `package names may not start with '/'` |
| bad char | `package names may contain A-Z, a-z, 0-9, or any of ' !"#$%&'()*+,-./;<=>?[]^_\`{|}~' (any ASCII character except 0-31, 127, ':', or '\')` |
| ends with `/` | `package names may not end with '/'` |
| contains `//` | `package names may not contain '//' path separators` |
| a segment is all dots (`.`, `..`, `...` as a *package* segment) | `package name component contains only '.' characters` |

Note what is **allowed** and surprises people: spaces, `"`, `#`, `$`, `*`, `+`,
`?`, `[`, `]`, `{`, `}`, `|`, `~`, `@`, `^`, backtick. Forbidden: control chars
0–31, DEL (127), `:`, `\`.

### 1.5 Validation — target names

`LabelValidator.validateTargetName` (l.146). Allowed: `javaLetterOrDigit`, plus
`" \"#$&'()*+,;<=>?[]{|}~"` (needs quoting in query), plus `"!%-@^_\`"` (no
quoting), plus **all non-ASCII ≥ U+0080**, plus `.` and `/` with restrictions.

| Condition | Message |
|---|---|
| `""` | `empty target name` |
| starts with `/` | `target names may not start with '/'` |
| `..`, `../…`, `…/../…`, `…/..` | `target names may not contain up-level references '..'` |
| `./…`, `…/./…` | `target names may not contain '.' as a path segment` |
| `//` anywhere | `target names may not contain '//' path separators` |
| ends with `\r` | `target names may not end with carriage returns (perhaps the input source is CRLF-terminated)` |
| char 0-31 or 127 | `target names may not contain non-printable characters: '\xNN'` |
| any other bad char | `target names may not contain '<c>'` |
| ends with `/` | `target names may not end with '/'` |
| exactly `.` or ends with `/.` | *(accepted; legacy)* |

Additionally, `LabelParser.perhapsYouMeantMessage` appends
`" (perhaps you meant \":<target>\"?)"` when `pkg.endsWith('/' + target)`.

Real messages, verified with `bazel query` on Bazel 8.7.0:

```
@@                -> ERROR: invalid target name '': empty target name
@                 -> ERROR: invalid target name '': empty target name
//pkg:            -> ERROR: invalid target name '': empty target name
//pkg:../x        -> ERROR: invalid target name '../x': target names may not contain up-level references '..'
//pkg:a:b         -> ERROR: invalid target name 'a:b': target names may not contain ':'
//foo//bar:x      -> ERROR: invalid package name 'foo//bar': package names may not contain '//' path separators
//./pkg:x         -> ERROR: invalid package name './pkg': package name component contains only '.' characters
@rules_go         -> ERROR: no such target '@@rules_go+//:rules_go': target 'rules_go' not declared in package '' defined by .../external/rules_go+/BUILD.bazel
@nonexistent//a:b -> ERROR: no such package '@@[unknown repo 'nonexistent' requested from @@]//a': ... No repository visible as '@nonexistent' from main repository.
```

The last one is `RepositoryMapping.get` (l.133) returning
`RepositoryName.toNonVisible(...)`, whose `getNameWithAt()` renders as
`@@[unknown repo 'X' requested from @@Y]`. **An LSP should recognise this
sentinel string in Bazel output as "unresolvable apparent repo".**

### 1.6 The three parse entry points — which context applies where

`Label.java` exposes three, and *which one is used decides whether `:foo` and
`foo/bar` are legal*:

| Method | Accepts | Repo of a bare `//pkg` | Used by |
|---|---|---|---|
| `parseCanonical(raw)` (l.156) | must be absolute; `@repo` is taken **literally as canonical** | `MAIN` | internal, deserialization, `//conditions:default` constant |
| `parseWithRepoContext(raw, repoContext)` (l.199) | must be absolute (`//…` or `@…`) | `repoContext.currentRepo()` | command line, `--override_repository`, target patterns |
| `parseWithPackageContext(raw, pkgContext)` (l.216) | also `:foo` and `foo/bar` | `packageContext.currentRepo()` | **everything in BUILD/.bzl**: attribute values, `load()`, `select()` keys, `$(location)` |

`parseWithPackageContextInternal` (l.233) only calls `checkPkgIsAbsolute()`
**when `parts.pkg()` is non-empty**. Consequences for a BUILD file:

- `":foo"` → `//<current pkg>:foo` ✅
- `"foo/bar.c"` → `//<current pkg>:foo/bar.c` ✅ (this is why `srcs` entries work)
- `"foo/bar:quux"` → **error** `invalid label 'foo/bar:quux': absolute label must begin with '@' or '//'`
- `"//foo/..."` → **error** `invalid label '//foo/...': package name cannot contain '...'`
  (`checkPkgDoesNotEndWithTripleDots`, l.198). **`...` is never valid in a label
  attribute** — only in target patterns and `package_group`/`visibility`.

`computeRepoNameWithRepoContext` (l.176) — the repo-resolution core:

```java
if (parts.repo() == null) {
  if (ABSOLUTE_PACKAGE_NAMES.contains(parts.pkg())) return RepositoryName.MAIN;
  return repoContext.currentRepo();
}
if (parts.repoIsCanonical()) return RepositoryName.createUnvalidated(parts.repo());
return repoContext.repoMapping().get(parts.repo());
```

with (l.100):

```java
private static final ImmutableSet<String> ABSOLUTE_PACKAGE_NAMES =
    ImmutableSet.of("conditions", "visibility");
```

**`//conditions:default` and `//visibility:public|private` always mean the main
repo, from any file in any repo.** They are not real targets. An LSP must
special-case them (no goto-def target exists).

### 1.7 Relative labels: `.bzl` vs `BUILD`

Two distinct rules, both important and both a common source of bugs:

**`Label("…")` inside a `.bzl`** —
`StarlarkRuleClassFunctions.label` (l.2135):

```java
// The label string is interpreted with respect to the .bzl module containing the call to
// `Label()`.  ... Hence, we opt for stack inspection.
BazelModuleContext moduleContext = BazelModuleContext.ofInnermostBzlOrFail(thread, "Label()");
return Label.parseWithPackageContext(
    (String) input, moduleContext.packageContext(), thread.getThreadLocal(Label.RepoMappingRecorder.class));
```

So `Label(":x")` in `@rules_go//go/private:foo.bzl` is
`@@rules_go+//go/private:x`, **not** relative to the BUILD file that loaded it.
Same for `LabelConverter.forBzlEvaluatingThread` (used by `exec_group`,
`attr.label(default=…)`, toolchain type strings, transitions).

**A bare string in a rule attribute in a BUILD file** — converted by
`LabelConverter` built from the *BUILD file's* `PackageIdentifier` + the *BUILD
file's repo's* `RepositoryMapping`. A legacy macro defined in
`@rules_foo//:defs.bzl` that does `native.cc_library(deps = ["@zlib//:zlib"])`
resolves `@zlib` in the **caller's** repo mapping, which is why macro authors
must use `Label()` (or `attr.label` defaults) instead of raw strings.

**`load()`** — `BzlLoadFunction.getLoadLabels` (l.1102) uses
`Label.parseWithPackageContext(unparsedLabel, PackageContext.of(base, repoMapping))`
where `base` is the package of the *loading* file. Then `checkValidLoadLabel`
(l.1000) enforces:

- name must end in `.bzl` (or `.scl`) → *"The label must reference a file with
  extension \".bzl\""*;
- package must not be `//external`;
- in `.scl` files, the load string must literally start with `//` (no `@repo`).

### 1.8 Target patterns — a *separate* grammar

`TargetPattern.Parser.parse` (l.814). Wildcards are **not** part of the label
grammar; they exist only on the command line, in `query`, and in
`--deleted_packages`/`--build_tag_filters`-style flags.

```java
private static final ImmutableList<String> ALL_RULES_IN_SUFFIXES   = ImmutableList.of("all");
private static final ImmutableList<String> ALL_TARGETS_IN_SUFFIXES = ImmutableList.of("*", "all-targets");
```

| Pattern | `Type` | Meaning |
|---|---|---|
| `foo/bar/baz` (no colon, relative, main repo) | `PATH_AS_TARGET` (`InterpretPathAsTarget`) | interpret as a *path*; expands to the target owning that file |
| `//foo:bar`, `@r//foo:bar` | `SINGLE_TARGET` | one target |
| `//foo:all` | `TARGETS_IN_PACKAGE` (rulesOnly=true) | all **rules** in `//foo` |
| `//foo:*`, `//foo:all-targets` | `TARGETS_IN_PACKAGE` (rulesOnly=false) | all rules **and** files |
| `//foo/...`, `//foo/...:all` | `TARGETS_BELOW_DIRECTORY` (rulesOnly=true) | all rules in `//foo` and below |
| `//foo/...:*`, `//foo/...:all-targets` | `TARGETS_BELOW_DIRECTORY` (rulesOnly=false) | + files |
| `//...` | `TARGETS_BELOW_DIRECTORY` from the repo root | |
| `//foo/...:quux` | **error** `'...' can only be used with wildcard targets` | |

Critical LSP distinction, confirmed by the code path: `//foo:all` written
**inside a BUILD attribute** is `SINGLE_TARGET` with target name literally
`"all"` — `Label.parseWithPackageContext` never sees `TargetPattern`. Bazel will
report `no such target '//foo:all'` unless a target is actually named `all`.
`bazel query //pkg:all` on the other hand lists every rule.

`Parser` carries `relativeDirectory` (the CWD relative to the workspace root),
`currentRepo`, and `repoMapping` — this is why `bazel query bar:baz` from
`<ws>/foo` means `//foo/bar:baz`. `Preconditions` forbids a non-empty
`relativeDirectory` in a non-main repo.

### 1.9 Where labels appear, and with which grammar

| Site | Grammar | Notes |
|---|---|---|
| `deps`, `srcs`, `data`, any `attr.label`/`label_list`/`label_keyed_string_dict` | `parseWithPackageContext` | strings converted lazily by `LabelConverter` |
| `load("…", "sym")` | `parseWithPackageContext` + `.bzl`/`.scl` extension check | |
| `select({…})` **keys** | `parseWithPackageContext` via `BuildType.Selector` ctor (`BuildType.java` l.717: `Label key = LABEL.convert(entry.getKey(), what, context)`) | keys point at `config_setting`, `constraint_value`, `alias`, or the constant `"//conditions:default"` (`DEFAULT_CONDITION_KEY`, l.679) |
| `$(location …)`, `$(locations …)`, `$(rootpath[s] …)`, `$(execpath[s] …)`, `$(rlocationpath[s] …)` in `genrule.cmd`, `*_binary.args`, `env`, … | `parseWithPackageContext` with the **rule's own label** as base (`LocationExpander.LocationFunction.apply`, l.278) | the label must be a *declared prerequisite* of the rule; otherwise `label '%s' in %s expression is not a declared prerequisite of this rule`. Not a target pattern. `$(` … `)` scanning is naive: `LocationExpander.expand` (l.174) finds `$(`, takes text up to the first space as the function name, up to the first `)` as the argument |
| `exec_compatible_with`, `target_compatible_with`, `toolchains`, `exec_group(exec_compatible_with=…)` | label lists; in a `.bzl` via `LabelConverter.forBzlEvaluatingThread` (`StarlarkRuleClassFunctions` l.2163 `parseLabels(execCompatibleWith, labelConverter, "exec_compatible_with")`) | resolves against the **`.bzl`'s** repo |
| `register_toolchains(…)`, `register_execution_platforms(…)` in MODULE.bazel | **target patterns**, not labels — `"@go_toolchains//:all"` is legal | |
| `visibility = [...]`, `package_group(packages=…, includes=…)` | **`PackageSpecification.fromString`**, a third grammar | `//foo/...`, `//foo`, `public`, `private`, `-//foo/...` (negation), `__pkg__`, `__subpackages__`. `PackageSpecification.java` l.50–57. `//...` is legacy-equivalent to `public`. Repo-qualified specs are supported for output (`@somerepo//pkg/subpkg`) but `fromString` does **not** accept a repo prefix — the repo comes from context |
| `filegroup(srcs = glob([...]))` | **not labels at all** — glob patterns over the package's own files | `glob()` never crosses into a subpackage; results become implicit file targets of *this* package |
| `alias(actual = …)` | label; goto-def should follow one hop and offer both | |
| `exports_files([...])` | plain filenames in the current package; creates file targets with explicit visibility | |
| MODULE.bazel `use_extension("@rules_go//go:extensions.bzl", "go_sdk")` | label (repo-mapped in the *module's* mapping) + a Starlark identifier | |
| `.bazelrc`: `--override_repository=name=/path`, `--inject_repository=name=/path`, `build --@rules_foo//:flag=v` | historically **canonical** repo names; `--override_repository` gained support for **apparent** names only at HEAD (`CHANGELOG.md` l.928, Release 10.0.0-pre.20251217.3, 2026-01-13 — *not* in 9.0/9.1/9.2) | an LSP that parses `.bazelrc` must not assume either form |

---

## 2. Apparent vs canonical repository names

### 2.1 Definitions

- **Apparent name** — what you write after a single `@`. It is *relative to the
  repo containing the file*. `@rules_go` in your MODULE.bazel means whatever
  your `bazel_dep(name="rules_go", …)` (or `repo_name=`) declared; `@rules_go`
  inside `@@protobuf+//…` means whatever *protobuf's* MODULE.bazel declared, or
  nothing at all.
- **Canonical name** — globally unique, written after `@@`. It is the directory
  name under `$output_base/external/`. Never write it in checked-in source; the
  format is explicitly unstable (`ModuleKey.java` l.111–124 and
  <https://bazel.build/external/module#repository_names_and_strict_deps>).

`Label`'s renderings (`Label.java` l.449–470, `RepositoryName.java` l.240–294):

| Method | Example |
|---|---|
| `getCanonicalForm()` | `//pkg:t` (main) / `@@rules_go+//pkg:t` |
| `getUnambiguousCanonicalForm()` | `@@//pkg:t` (main) / `@@rules_go+//pkg:t` |
| `getDisplayForm(mainRepoMapping)` | `//pkg:t` / `@rules_go//pkg:t` / `@@rules_go+//pkg:t` if invisible |

### 2.2 Canonical name construction

`ModuleKey.getCanonicalRepoName` (`ModuleKey.java` l.97):

```java
if (WELL_KNOWN_MODULES.containsKey(name)) return WELL_KNOWN_MODULES.get(name);
if (ROOT.equals(this)) return RepositoryName.MAIN;              // ""
String suffix = includeVersion ? version.toString() : "";
return RepositoryName.createUnvalidated(String.format("%s+%s", name, suffix));
```

`WELL_KNOWN_MODULES` (l.45, *"Keep in sync with src/tools/bzlmod/utils.bzl"*):

```java
"bazel_tools" -> RepositoryName.BAZEL_TOOLS   // canonical name is literally "bazel_tools"
"platforms"   -> RepositoryName.createUnvalidated("platforms")  // literally "platforms"
```

**These two are the only *modules* whose canonical name has no `+`.** Verified —
`bazel mod dump_repo_mapping ""` in `/tmp/bzlmod-scratch` returned
`"platforms":"platforms"` and `"bazel_tools":"bazel_tools"` while everything
else got a `+`. Two other `+`-free canonical names exist: the **root module**
(canonical name is the empty string, rendered `@@`) and, on Bazel ≤ 8,
WORKSPACE-suffix repos such as `local_config_platform`. So
`RepositoryName.isApparent` (§1.3) has false positives — it is a heuristic, not
a decision procedure.

Version elision: `BazelDepGraphFunction.computeCanonicalRepoNameLookup` (l.140)
groups the dep graph by module name and only includes the version when a module
appears at **more than one** version (i.e. under `multiple_version_override`):

```java
multipleVersionsModules.contains(key.name())
    ? key.getCanonicalRepoNameWithVersion()      // "bazel_skylib+1.7.1"
    : key.getCanonicalRepoNameWithoutVersion()   // "bazel_skylib+"
```

Verified in `/tmp/bzl-exp3` (`multiple_version_override(module_name="bazel_skylib", versions=["1.5.0","1.7.1"])`):

```
{"bazel_skylib":"bazel_skylib+1.7.1", "rules_cc":"rules_cc+", ...}
```

**Extension-generated repos** — `BazelDepGraphValue.getRepositoryMapping`
(l.140): `repoNamePrefix = extensionUniqueNames.get(extensionId) + "+"`, and
`extensionUniqueNames` comes from
`BazelDepGraphFunction.makeUniqueNameCandidate` (l.187):

```java
id.bzlFileLabel().getRepository().getName() + "+" + extensionName + extensionNameDisambiguator
```

so the canonical name of a repo imported via `use_repo(ext, "foo")` is

```
<canonical repo of the .bzl defining the extension> + "+" + <extension symbol> + "+" + <name inside the extension>
```

Verified (Bazel 8.7.0 unless noted):

| Source | Canonical |
|---|---|
| `use_extension("@rules_go//go:extensions.bzl","go_sdk")` → `use_repo(go_sdk,"go_default_sdk")` | `rules_go++go_sdk+go_default_sdk` |
| gazelle's `go_deps` | `gazelle++go_deps+com_github_gogo_protobuf` |
| `@@platforms//host:extension.bzl%host_platform` | `platforms+host_platform+host_platform` |
| rules_python `python` extension | `rules_python++python+python_3_11_host` |

Note the **double `+`** when the defining module's canonical name already ends
in `+` (`rules_go+` + `+` + `go_sdk`), and the single `+` for well-known
`platforms`. Disambiguator: a second extension with the same symbol name gets
`…+go_sdk2`. Isolated extensions (`use_extension(..., isolate=True)`) use
`%s+_%s%s+%s+%s+%s` with a leading `_` on the extension name (l.209).

**`use_repo_rule` (innate extensions)** — `ModuleFileGlobals.useRepoRule`
(l.820) synthesises an extension with `bzlFile = "//:MODULE.bazel"` and
`extensionName = bzlFile + ' ' + ruleName` (`ModuleExtensionId.isInnate()` =
`extensionName().contains(" ")`). **This naming changed between LTS releases** —
measured on the identical MODULE.bazel (`/tmp/bzl-exp4`):

```console
$ cat MODULE.bazel
module(name = "exp4", version = "0.0.1")
bazel_dep(name = "bazel_skylib", version = "1.7.1")
local_repo = use_repo_rule("@bazel_tools//tools/build_defs/repo:local.bzl", "local_repository")
local_repo(name = "mylocal", path = "../bzl-exp1")

# Bazel 8.7.0
{"mylocal":"+_repo_rules+mylocal","":"","exp4":"","bazel_skylib":"bazel_skylib+",
 "bazel_tools":"bazel_tools","local_config_platform":"local_config_platform"}

# Bazel 9.2.0
{"mylocal":"+local_repository+mylocal","":"","exp4":"","bazel_skylib":"bazel_skylib+",
 "bazel_tools":"bazel_tools"}
```

(Same run also shows Bazel 9 dropping the WORKSPACE-suffix repo
`local_config_platform` from the main mapping.)

Corroborated by `CHANGELOG.md` l.2029 (Release 9.0.0-pre.20250307.1,
*Incompatible changes*): *"The canonical names of repos created with
`use_repo_rule` have changed, which may require updating command-line flags such
as `--override_repository`."*
**An LSP must not hardcode either form.**

### 2.3 `~` → `+`: exactly when it changed

| Bazel | Format |
|---|---|
| 6.x, 7.0 | `rules_go~0.46.0` (always versioned); main-repo extensions got a `_main` prefix; numeric versions got a `v` prefix |
| **7.1.0** | version elided when unique: `rules_go~` (PR [#21035](https://github.com/bazelbuild/bazel/pull/21035) / cherry-pick [#21316](https://github.com/bazelbuild/bazel/pull/21316)). Overrides stopped using `rules_foo~override` |
| **7.3.0** | flag `--incompatible_use_plus_in_repo_names` added, **default false** (PR [#23103](https://github.com/bazelbuild/bazel/pull/23103), tracked by [#23127](https://github.com/bazelbuild/bazel/issues/23127)) |
| **8.0.0** | flag flipped **true and graveyarded** (cannot be unflipped) — commit [`60924fd`](https://github.com/bazelbuild/bazel/commit/60924fdc2972184494f6382d39e8c786aa14b9a9); `CHANGELOG.md` l.2784. Also in 8.0.0: all labels in error messages / BEP / logs now use `@@` |
| 9.x | unchanged (`+`) |

Motivation was Windows path performance ([#22865](https://github.com/bazelbuild/bazel/issues/22865)). Side effects of the flip, from PR #23103: no more `_main` prefix for main-repo extension repos (hence the leading bare `+` in `+local_repository+mylocal`), and no more `v` prefix on numeric versions.

**Practical rule for an LSP that must support 7.x and 8/9.x:** treat
`[~+]` as the separator class and use `name.contains('~') || name.contains('+')`
for "is canonical", or better, never infer — always ask
`bazel mod dump_repo_mapping`.

### 2.4 The repository mapping

`RepositoryMapping` (171 lines) is just
`ImmutableMap<String /*apparent*/, RepositoryName /*canonical*/>` plus a
`contextRepo`. `get()` (l.133) falls back to
`createUnvalidated(name).toNonVisible(contextRepo, SpellChecker.didYouMean(...))`.
There is **one mapping per repository**, so resolving a label requires knowing
which repo the *file* lives in.

Where it lives:

1. **Nowhere on disk as a standalone file for the main repo.** It is computed
   in-memory by `BazelDepGraphValue.getFullRepoMapping(ModuleKey)` from the
   resolved module graph + `use_repo` declarations.
2. **`bazel mod dump_repo_mapping <repo>...`** — the supported public API.
   Added in **Bazel 7.1.0** (PR [#20686](https://github.com/bazelbuild/bazel/pull/20686),
   cherry-picked as [#21023](https://github.com/bazelbuild/bazel/pull/21023)); the
   7.1.0 release note says verbatim: *"returns the repository mappings of the
   given repositories in NDJSON. This information can be used by IDEs and
   Starlark language servers to resolve labels with `--enable_bzlmod`."*
   - Output is **NDJSON**: one JSON object per line, one line per requested repo,
     in argument order.
   - Arguments are **canonical** repo names (`""` = main repo). Unknown name →
     `ERROR: Repositories not found: does_not_exist+.`
   - Calling it with **no** arguments is a deliberate error (reserved for a
     future "dump everything" mode).

   Measured, main repo, `/tmp/bzl-exp2` (`bazel_skylib` + `rules_go` with
   `repo_name="io_bazel_rules_go"`), **cold output base**:

   ```
   $ bazel --output_base=/tmp/ob-exp2 mod dump_repo_mapping ""
   {"":"","exp1":"","bazel_skylib":"bazel_skylib+","io_bazel_rules_go":"rules_go+",
    "bazel_tools":"bazel_tools","local_config_platform":"local_config_platform"}
   real 0m9.134s     # includes JVM server start + downloading 147 registry files
   ```

   `$output_base/external/` afterwards contained **only** `bazel_tools` (a
   symlink into the install base) and `local_config_platform`. **No module repo
   was fetched.** Warm re-invocation for another repo: **0.197 s**.

   Note the mapping contains `""` → `""` and the module's **own name**
   (`"exp1":""`), so a module can refer to itself by name.

   For a non-main repo (Bazel 8.7.0, `rules_go+`), the mapping already includes
   extension repos *by name* without the extensions having been evaluated:

   ```
   $ bazel mod dump_repo_mapping rules_go+
   {"go_toolchains":"rules_go++go_sdk+go_toolchains",
    "io_bazel_rules_nogo":"rules_go++go_sdk+io_bazel_rules_nogo",
    "com_github_gogo_protobuf":"gazelle++go_deps+com_github_gogo_protobuf",
    ..., "io_bazel_rules_go":"rules_go+", "bazel_skylib":"bazel_skylib+",
    "platforms":"platforms", "com_google_protobuf":"protobuf+",
    "bazel_tools":"bazel_tools", "local_config_platform":"local_config_platform"}
   ```

3. **`_repo_mapping` in the runfiles tree.** Every executable's runfiles
   directory gets a file literally named `_repo_mapping` at its root
   (`src/main/protobuf/spawn.proto` l.312/344: *"the `_repo_mapping` file with
   the repo mapping manifest"*). Written by
   `src/main/java/com/google/devtools/build/lib/analysis/RepoMappingManifestAction.java`,
   `writeEntry` l.322:

   ```java
   writer.format("%s,%s,%s\n", source, targetApparentName, targetRepoDirectoryName);
   ```

   — ISO-8859-1, one CSV line per entry, `source` = canonical name of the repo
   doing the lookup, third field = the target's *runfiles directory name*.
   Consumed by the runfiles libraries in
   `@bazel_tools//tools/{bash,cpp,java}/runfiles`; also reachable from Starlark
   via `py_internal.create_repo_mapping_manifest`, and compacted by
   `--incompatible_compact_repo_mapping_manifest`.
   Two reasons it is a bad source for an LSP: it is a **build output** (only
   exists after building a target with runfiles), and it is deliberately
   **partial** — l.317: *"We only write entries for repos whose canonical names
   appear in runfiles paths."*
4. **`recordedRepoMappingEntries` inside `MODULE.bazel.lock`**, per module
   extension (see §3.4). These are *partial* — only the entries an extension
   actually used.

Upstream's own statement of the resolution order for the main repo
(`site/en/external/faq.md`, release-9.1.0):

> If the context repo is the main repo (`@@`):
> 1. If `bar` is an apparent repo name introduced by the root module's
>    MODULE.bazel file (through any of `bazel_dep`, `use_repo`, `module`,
>    `use_repo_rule`), then `@bar` resolves to what that MODULE.bazel file claims.
> 2. Otherwise, if `bar` is a repo defined in WORKSPACE (which means that its
>    canonical name is `@@bar`), then `@bar` resolves to `@@bar`.
> 3. Otherwise, `@bar` resolves to something like
>    `@@[unknown repo 'bar' requested from @@]`, and this will ultimately result
>    in an error.

`Label.RepoMappingRecorder` / `SimpleRepoMappingRecorder` (l.246–281) is the
mechanism by which Bazel records which mapping lookups a `.bzl` performed, so
that repo marker files can be invalidated on mapping change (7.1.0 change
*"Make repo marker files sensitive to repo mapping changes"*).

---

## 3. On-disk layout

### 3.1 Directories

```
$output_base/                             # bazel info output_base
├── external/                             # LabelConstants.EXTERNAL_REPOSITORY_LOCATION
│   ├── @<canonical>.marker                # fetch marker, 65 bytes for a simple repo
│   ├── <canonical>/                       # e.g. rules_go+, gazelle++go_deps+org_golang_x_net
│   ├── bazel_tools -> $install_base/embedded_tools
│   └── _main -> <workspace>               # Bazel 9.2.0 only; absent in 8.7.0
├── modextwd/                             # LabelConstants.MODULE_EXTENSION_WORKING_DIRECTORY_LOCATION
├── MODULE.bazel.lock                      # the *hidden* lockfile (see §3.4)
├── execroot/_main/…                       # only after a build
├── install -> ~/.cache/bazel/_bazel_$USER/install/<hash>
└── server/, java.log*, command.log, lock, DO_NOT_BUILD_HERE
```

`RepositoryName.getExecPath` (l.303) prepends `external/` unless
`--experimental_sibling_repository_layout`, in which case the prefix is `..`
(`LabelConstants.EXPERIMENTAL_EXTERNAL_PATH_PREFIX`) and repos become siblings of
`execroot/_main`. An LSP must honour that flag if the user sets it.

Non-archive repos are **symlinks**: `local_repository` /
`local_path_override` produce e.g.
`external/+_repo_rules+mylocal -> /private/tmp/bzl-exp1`. So `readlink` chains
must be followed, and the same physical file can be reachable under two repos.

### 3.2 What exists before a build — precisely

Measured on `/tmp/bzl-exp1` (`MODULE.bazel` with `bazel_skylib` +
`rules_go`), each step against a **freshly deleted** output base:

| Command | Wall time | `$output_base/external/` after | Workspace `MODULE.bazel.lock` |
|---|---|---|---|
| *(nothing — fresh git clone)* | — | **does not exist** | absent unless committed |
| `bazel info output_base` | ~3 s (server start) | **does not exist** | not created |
| `bazel mod dump_repo_mapping ""` | 9.1 s | `bazel_tools` (symlink), `local_config_platform` | created, 20 402 bytes, `registryFileHashes` only |
| `bazel mod graph` | ~90 s | **25 repo dirs, 161 MB** | 166 913 bytes |
| `bazel fetch --repo=@@rules_go+` | 0.7 s (warm server, warm repo cache) | + `rules_go+` only | unchanged |
| `bazel query --output=location @@platforms//host:constraints.bzl` | 1.75 s | + 12 repos (transitively required to *load* that package) | unchanged |
| `bazel query --output=location @bazel_skylib//lib:paths.bzl` (2nd time) | **0.207 s** | unchanged | unchanged |

**Answer to the headline question: no. On a freshly cloned repo with no build,
an LSP cannot resolve `@rules_go//go:def.bzl` from the filesystem — the
directory `$output_base/external/rules_go+/` does not exist.** It can, however,
resolve `@rules_go` → `@@rules_go+` in ~9 s (once) via
`bazel mod dump_repo_mapping ""`, and then materialise just that one repo with
`bazel fetch --repo=@@rules_go+`.

Surprise worth flagging: **`bazel mod graph` is not cheap.** It fetches every
module repo and evaluates every module extension (the hidden lockfile after
`mod graph` on `/tmp/bzl-exp1` listed 16 evaluated extensions including
`@@rules_go+//go:extensions.bzl%go_sdk` and `@@gazelle+//:extensions.bzl%go_deps`).
Never call it on LSP startup. `mod dump_repo_mapping` is the cheap sibling.

`bazel query` **auto-fetches** whatever it needs to load the requested package.
That makes it a correct-but-expensive fallback: 1.75 s and 12 extra repos for
one cold `.bzl`; 0.2 s once warm.

### 3.3 `bazel fetch --repo`

```
bazel fetch --repo=@@rules_go+          # canonical
bazel fetch --repo=@bazel_skylib        # apparent, from the main repo's viewpoint — also works
```

Fetches exactly one repo. This is what `starpls` does
(`upstream/starpls/crates/starpls_bazel/src/client.rs` l.185):

```rust
fn fetch_repo(&self, repo: &str) -> anyhow::Result<()> {
    self.run_command(["fetch", "--repo", &format!("@@{}", repo)])?;
```

### 3.4 `MODULE.bazel.lock` — can it be read statically?

**Short answer: since Bazel 7.2 it cannot be used to map apparent→canonical.**

Lockfile format history:

| Bazel | `lockFileVersion` | Top-level keys |
|---|---|---|
| 7.0 | 1 | `moduleFileHash`, `flags`, `localOverrideHashes`, **`moduleDepGraph`**, `moduleExtensions` |
| 7.2 (PR [#22154](https://github.com/bazelbuild/bazel/pull/22154), "incremental format") | bumped | `registryFileHashes`, `selectedYankedVersions`, `moduleExtensions` — `moduleDepGraph` **removed** |
| 8.7.0 (measured) | **24** | `lockFileVersion`, `registryFileHashes`, `selectedYankedVersions`, `moduleExtensions`, `facts` |
| 9.2.0 (measured) | **28** | + `factsVersions` |
| HEAD `3a9b19c8` | **28** (`BazelLockFileValue.LOCK_FILE_VERSION`) | same |

(`BazelLockFileValue.java` l.46–49 notes that 7.x has a HACK requiring version
increments in steps of 2, hence the even numbers.)

Real workspace lockfile for `/tmp/bzlmod-scratch` (3 `bazel_dep`s + one
`use_extension`), 166 669 bytes:

```json
{
  "lockFileVersion": 24,
  "registryFileHashes": {                       // 141 entries
    "https://bcr.bazel.build/bazel_registry.json": "8a28e4aff06ee6...",
    "https://bcr.bazel.build/modules/abseil-cpp/20210324.2/MODULE.bazel": "7cd0312e06...",
    "https://bcr.bazel.build/modules/rules_go/0.50.1/MODULE.bazel": "b91a308dc5...",
    "https://bcr.bazel.build/modules/rules_go/0.50.1/source.json": "205765fd30..."
  },
  "selectedYankedVersions": {},
  "moduleExtensions": {
    "@@rules_python+//python/private/pypi:pip.bzl%pip_internal": {
      "general": {
        "bzlTransitiveDigest": "vlSyuSrNYbirX9OZLGuz9vkFVyRp1tUXOWeMbjaR8SE=",
        "usagesDigest": "OLoIStnzNObNalKEMRq99FqenhPGLFZ5utVLV4sz7OI=",
        "recordedFileInputs": { "@@rules_python+//tools/publish/requirements_linux.txt": "8175b4..." },
        "recordedDirentsInputs": {},
        "envVariables": { "RULES_PYTHON_REPO_DEBUG": null },
        "generatedRepoSpecs": {
          "rules_python_publish_deps_311_backports_tarfile_py3_none_any_77e284d7": {
            "repoRuleId": "@@rules_python+//python/private/pypi:whl_library.bzl%whl_library",
            "attributes": { "filename": "backports.tarfile-1.2.0-py3-none-any.whl",
                            "sha256": "77e284d754…", "urls": ["https://files.pythonhosted.org/…"] }
          }
        },
        "recordedRepoMappingEntries": [
          ["rules_python+", "bazel_features", "bazel_features+"],
          ["rules_python+", "pypi__pip", "rules_python++internal_deps+pypi__pip"],
          ["rules_python++python+pythons_hub", "python_3_11_host", "rules_python++python+python_3_11_host"]
        ]
      }
    }
  },
  "facts": {}
}
```

What an LSP **can** get from it statically:

- Which registry served each `<module>@<version>` and the exact URL → the
  MODULE.bazel of every resolved module is addressable
  (`https://bcr.bazel.build/modules/<name>/<version>/MODULE.bazel`), and the
  *set of `registryFileHashes` keys of the form `.../<name>/<version>/source.json`*
  is exactly the **resolved** version set: e.g. `rules_go/0.50.1/source.json`
  present, `rules_go/0.41.0/MODULE.bazel` present but no `source.json` → 0.41.0
  was considered by MVS but not selected. That is enough to reconstruct
  apparent→canonical for **Bazel-module** repos without a network call, *if* you
  also parse the MODULE.bazel files (which are in the repository cache under
  `$(bazel info repository_cache)/content_addressable/sha256/<hash>/file`).
- For each *evaluated* extension: the full list of generated repos and their
  repo-rule attributes (`generatedRepoSpecs`), plus a partial repo mapping
  (`recordedRepoMappingEntries`) — i.e. a snapshot of the extension's output.
- `bzlTransitiveDigest`/`usagesDigest` for staleness detection.

What it **cannot** give you:

- The resolved dep graph as a first-class structure (gone since 7.2).
- The main repo's repository mapping.
- Anything about extensions that were never evaluated in this workspace. In
  `/tmp/bzlmod-scratch` only **4–5** extensions appear (the "non-reproducible"
  ones); the `go_sdk` extension that the root module actually uses is **absent**
  from the workspace lockfile.

**Two lockfiles.** `BazelLockFileValue.java` l.33–107 documents them:

| | path | contents | committed? |
|---|---|---|---|
| regular | `<workspace>/MODULE.bazel.lock` (`KEY`) | minimal, mergeable, deterministic | yes |
| hidden | `$output_base/MODULE.bazel.lock` (`HIDDEN_KEY`) | *"format and contents are explicitly unspecified"*, performance-only | no |

Measured on `/tmp/bzlmod-scratch`: workspace 166 669 B with 5 extensions listed;
hidden 467 242 B with **16** extensions and `registryFileHashes: {}`. The
extensions marked `reproducible = True` (via
`module_ctx.extension_metadata(reproducible=True)`) land in the hidden file, the
rest in the committed one. **An LSP may read the hidden lockfile for a richer
picture but must treat the schema as unstable.**

`--lockfile_mode={update|refresh|error|off}` controls whether Bazel writes it.
An LSP shelling out to `bazel` should consider `--lockfile_mode=error` or `off`
to avoid dirtying the user's working tree from a background query — but note
`error` fails if anything is missing.

### 3.5 Vendor mode

`bazel vendor --vendor_dir=DIR [--repo=@@x]` (Bazel 7.1+, stable in 8) produces:

```
DIR/
├── VENDOR.bazel                       # ignore('@@x')/pin('@@x') directives; LabelConstants.VENDOR_FILE_NAME
├── _registries/                       # mirrored BCR files
├── @bazel_skylib+.marker
├── bazel_skylib+/                     # real directory, checked into VCS
└── bazel-external -> $output_base/external
```

With `--vendor_dir` set, `IgnoredSubdirectoriesFunction` adds the vendor dir to
the ignored prefixes if it lives inside the workspace
(`IgnoredSubdirectoriesFunction.computeIgnoredPrefixes`). For an LSP, a vendored
workspace is the *easy* case: `DIR/<canonical>/` is a plain directory that
exists before any build.

---

## 4. MODULE.bazel semantics

All directives are `@StarlarkMethod`s on
`src/main/java/com/google/devtools/build/lib/bazel/bzlmod/ModuleFileGlobals.java`.
Line numbers below are from that file at HEAD.

| Directive | l. | Signature (key params) | Notes |
|---|---|---|---|
| `module()` | 82 | `name`, `version`, `compatibility_level`, `repo_name`, `bazel_compatibility` | at most once, must be first. `module(repo_name=)` is *"The name of the repository representing this module, **as seen by the module itself**"* — rules_go uses `io_bazel_rules_go`. `compatibility_level` is a **no-op since Bazel 8** (`WARNING: The attribute 'compatibility_level' in module() is a no-op and will be removed in a future Bazel release.` — observed on 8.7.0) |
| `bazel_dep()` | 227 | `name`, `version`, `max_compatibility_level`, `repo_name`, `dev_dependency` | `bazel_dep(repo_name=)` is the apparent name **the declaring module** uses, defaulting to the module name; `repo_name = None` makes it a *nodep* dependency (only honoured if the module is already in the graph by other means) |
| `use_extension()` | 415 | `extension_bzl_file` (label), `extension_name`, `dev_dependency`, `isolate` | returns a proxy; tags are called on it |
| `use_repo()` | 633 | `extension_proxy`, `*args`, `**kwargs` | `kwargs` are `local_name = "name_in_extension"`. Since Bazel 10-pre (CHANGELOG l.928, 2026-01) kwarg *values* may contain `{name}`/`{version}` substitutions |
| `use_repo_rule()` | 801 | `repo_rule_bzl_file`, `repo_rule_name` | returns a callable proxy; creates an "innate" extension. Added Bazel 7.0 (CHANGELOG l.3281) |
| `override_repo()` | 682 | `extension_proxy`, `*args`, `**kwargs` | Bazel 8.0 (CHANGELOG l.2694) |
| `inject_repo()` | 741 | `extension_proxy`, `*args`, `**kwargs` | Bazel 8.0 |
| `register_toolchains()` | 363 | `*toolchain_labels`, `dev_dependency` | **target patterns**, e.g. `"@go_toolchains//:all"` |
| `register_execution_platforms()` | 322 | `*platform_labels`, `dev_dependency` | ditto |
| `include()` | 872 | `label` | added **Bazel 7.2.0** (PR [#21855](https://github.com/bazelbuild/bazel/pull/21855), cherry-pick [#22204](https://github.com/bazelbuild/bazel/pull/22204)); identifier is `CompiledModuleFile.INCLUDE_IDENTIFIER`. Root module only in 7.x/8.x; at HEAD also modules under a non-registry override. Label must start with `//`, filename must end `.MODULE.bazel` and not start with `.`. Variable bindings do **not** cross the include boundary |
| `single_version_override()` | 904 | `module_name`, `version`, `registry`, `patches`, `patch_cmds`, `patch_strip` | root module only |
| `multiple_version_override()` | 999 | `module_name`, `versions`, `registry` | the *only* thing that makes canonical names versioned |
| `archive_override()` | 1058 | `module_name`, `**kwargs` (urls, integrity, strip_prefix, patches…) | non-registry override → module version becomes empty |
| `git_override()` | 1103 | `module_name`, `**kwargs` (remote, commit, init_submodules…) | non-registry override |
| `local_path_override()` | 1148 | `module_name`, `path` | non-registry override; repo becomes a symlink |
| `flag_alias()` | 1180 | `name`, `starlark_flag` | root module only |

`NonRegistryOverride` forces `Version.EMPTY`, hence
`getCanonicalRepoNameWithoutVersion()` and a canonical name of `<name>+`.

### 4.1 What an LSP can offer per directive

| Directive | Completion | Hover | Goto-def | Diagnostics |
|---|---|---|---|---|
| `bazel_dep(name=…)` | module names — needs a BCR index; none is published, see §4.3 | `metadata.json`: homepage, maintainers, `yanked_versions[v]` reason | `MODULE.bazel` of the dep: `$output_base/external/<canonical>/MODULE.bazel` if fetched, else fetch `https://bcr.bazel.build/modules/<n>/<v>/MODULE.bazel` (cheap, cacheable, and its hash is already in `registryFileHashes`) | version not in `metadata.json.versions`; version is yanked; **selected version ≠ declared version** (Bazel itself warns: `WARNING: For repository 'rules_cc', the root module requires module version rules_cc@0.0.9, but got rules_cc@0.1.1 in the resolved dependency graph`) |
| `bazel_dep(version=…)` | `metadata.json.versions[]`, newest first, yanked ones demoted | changelog/source URL from `source.json` | — | |
| `use_extension(bzl, name)` | `.bzl` files in the dep; then exported `module_extension` symbols in that file | extension doc | the `module_extension(...)` call site | file/symbol does not exist; extension not exported |
| tag calls (`go_sdk.download(...)`) | tag class names from `tag_class(...)`; attribute names/types from the `tag_class` attrs | attr docs | `tag_class` definition | unknown tag class / attribute |
| `use_repo(ext, …)` | repo names the extension *actually* generates — only knowable from the lockfile's `generatedRepoSpecs` or by running `bazel mod tidy` | — | — | **`bazel mod tidy` is the canonical fixer** (Bazel 7.1+); surface missing/extra `use_repo` entries as a code action that runs it |
| `register_toolchains` | target patterns in the named repo | — | the `toolchain()` rule | |
| `*_override` | module names already in the graph | resolved version, override kind | for `local_path_override`, the local `MODULE.bazel` | overriding a module not in the graph; non-root module using an override |
| `include()` | `*.MODULE.bazel` files in the main repo | — | the included file | label not `//`-absolute; filename rules |

### 4.2 Concrete goto-def target for a `bazel_dep`

`https://bcr.bazel.build/modules/rules_go/0.50.1/MODULE.bazel` (fetched
2026-08-25) — note it is a *real, editable-looking* Starlark file and is the
right hover/peek content:

```python
module(
    name = "rules_go",
    version = "0.50.1",
    compatibility_level = 0,
    repo_name = "io_bazel_rules_go",
)

bazel_dep(name = "bazel_features", version = "1.9.1", repo_name = "io_bazel_rules_go_bazel_features")
bazel_dep(name = "bazel_skylib", version = "1.2.0")
bazel_dep(name = "platforms", version = "0.0.10")
bazel_dep(name = "rules_proto", version = "6.0.0")
bazel_dep(name = "protobuf", version = "3.19.2", repo_name = "com_google_protobuf")

go_sdk = use_extension("//go:extensions.bzl", "go_sdk")
go_sdk.download(name = "go_default_sdk", version = "1.21.8")
use_repo(go_sdk, "go_toolchains", "io_bazel_rules_nogo")
register_toolchains("@go_toolchains//:all")

bazel_dep(name = "gazelle", version = "0.36.0")
```

This is also the proof that **the apparent name a module has for itself and for
its deps is fully static** — `module(repo_name = "io_bazel_rules_go")` here is
why `bazel mod dump_repo_mapping rules_go+` reports the self-entry
`"io_bazel_rules_go":"rules_go+"`, and `bazel_dep(name="protobuf",
repo_name="com_google_protobuf")` here is why the same mapping contains
`"com_google_protobuf":"protobuf+"`.

### 4.3 Bazel Central Registry — the actual API surface

Reader: `src/main/java/com/google/devtools/build/lib/bazel/bzlmod/IndexRegistry.java`.
URL construction (l.135, l.255, l.395, l.626):

```
<registry>/bazel_registry.json
<registry>/modules/<name>/metadata.json
<registry>/modules/<name>/<version>/MODULE.bazel
<registry>/modules/<name>/<version>/source.json
<registry>/modules/<name>/<version>/patches/<file>       (referenced from source.json)
```

Fetched live 2026-08-25:

```console
$ curl -s https://bcr.bazel.build/bazel_registry.json
{
    "mirrors": []
}
```

`BazelRegistryJson` (l.260) has exactly two fields: `mirrors` (list) and
`moduleBasePath` (used for `local_path`-type sources). Mirrors are applied by
rewriting `scheme://authority/path` to `<mirror>/<authority>/<path>`
(l.473–491).

```console
$ curl -s https://bcr.bazel.build/modules/rules_go/0.50.1/source.json
{
    "integrity": "sha256-9KkxRRjKas+hbMSrQ7C4zh5OpkuBw42KN3KIPxUzRrg=",
    "strip_prefix": "",
    "url": "https://github.com/bazelbuild/rules_go/releases/download/v0.50.1/rules_go-v0.50.1.zip"
}
```

`source.json` schema variants (`IndexRegistry.java` l.266–298):

| `"type"` | class | fields |
|---|---|---|
| `"archive"` (default) | `ArchiveSourceJson` | `url`, `mirror_urls`, `integrity`, `strip_prefix`, `patches` (map file→integrity), `patch_strip`, `archive_type`, `overlay` |
| `"local_path"` | `LocalPathSourceJson` | `path` (resolved against `bazel_registry.json`'s `moduleBasePath`) |
| `"git_repository"` | `GitRepoSourceJson` | `remote`, `commit`, `shallow_since`, `tag`, `init_submodules`, `verbose`, `strip_prefix`, `add_prefix` (added Bazel 7.1, PR #21036) |

```console
$ curl -s https://bcr.bazel.build/modules/rules_go/metadata.json
{
  "homepage": "https://github.com/bazelbuild/rules_go",
  "maintainers": [ { "email": "fabian@meumertzhe.im", "github": "fmeum",
                     "name": "Fabian Meumertzheim", "github_user_id": 4312191 }, … ],
  "repository": [ "github:bazel-contrib/rules_go" ],
  "versions": [ "0.33.0", "0.34.0", …, "0.62.0", "0.63.0" ],   // 46 entries
  "yanked_versions": {
    "0.33.0": "Obsolete experimental version that emits debug prints. Update to 0.39.1 or higher",
    …
  }
}
```

**Is there a machine-readable index of all modules? No.**

```
https://bcr.bazel.build/                    -> 200, an HTML redirect to https://registry.bazel.build/
https://bcr.bazel.build/modules            -> 404
https://bcr.bazel.build/modules/index.json -> 404
https://bcr.bazel.build/index.json         -> 404
https://bcr.bazel.build/modules.json       -> 404
https://registry.bazel.build/api/modules   -> 404
https://registry.bazel.build/search?q=…    -> 404
```

`registry.bazel.build` is a server-rendered HTML UI with no documented JSON API.
The only enumeration is the git repo:

```console
$ curl -s .../repos/bazelbuild/bazel-central-registry/git/trees/<modules-sha>
truncated: False    module count: 1250
```

**1250 modules** as of 2026-08-25 (`https://api.github.com/repos/bazelbuild/bazel-central-registry/git/trees/<sha>`;
the `/contents/modules` REST endpoint caps at 1000 and silently truncates — use
the git-trees API). Repo root also contains `metadata.schema.json` and
`source.schema.json`, useful for validating BCR files if the LSP is used to
*author* registry entries.

**Recommendation for module-name completion:** fetch the `modules` tree once via
the git-trees API (one request, ~1250 names, cacheable with an ETag against
`refs/heads/main`), and fetch `metadata.json` lazily per module for versions.
Fall back to "no completion" offline. `--registry=` may point at a private
registry (`.bazelrc` `common --registry=…`), which the LSP must honour; the same
five URL shapes apply to any registry.

---

## 5. Module extensions: why static resolution is fundamentally incomplete

A module extension is arbitrary Starlark that runs in the loading phase and
calls repo rules. `use_repo(go_deps, "com_github_x_y")` says *"import whatever
the extension named `com_github_x_y`"*.

**What IS static** (and this is the part people get wrong): the **name mapping**
is computed without running the extension.
`BazelDepGraphValue.getRepositoryMapping` (l.140) builds
`repoNamePrefix + entry.getValue()` purely from the `use_repo` declarations in
the MODULE.bazel files and the extension's unique name. Proof — on a
**cold output base** with nothing fetched:

```console
$ bazel mod dump_repo_mapping rules_go+       # 0.197 s warm, no extension evaluation
{"com_github_gogo_protobuf":"gazelle++go_deps+com_github_gogo_protobuf", …}
```

**What is NOT static:**

1. **Existence.** `use_repo(go_deps, "com_github_typo_typo")` yields a perfectly
   well-formed canonical name `gazelle++go_deps+com_github_typo_typo` that maps
   to nothing. Bazel only errors when the repo is actually needed
   (`RepositoryDelegatorFunction`). An LSP that resolves names optimistically
   will happily produce a dangling path.
2. **The complete repo set.** An extension may generate hundreds of repos of
   which the root module imports three. `rules_python`'s `pip_internal`
   extension generated ~hundreds of `rules_python_publish_deps_311_*` repos in
   the measured lockfile. Completion of `use_repo` arguments therefore *requires*
   having run the extension.
3. **Contents.** Which targets exist inside `@com_github_x_y//:go_default_library`
   is decided by a repo rule that may run `go mod download`, generate BUILD files
   with gazelle, template files, etc.
4. **Determinism across machines.** `SingleExtensionEvalFunction` keys extension
   results on `ModuleExtensionEvalFactors` (the `"general"` / `"os:…,arch:…"`
   key seen in the lockfile), `envVariables`, `recordedFileInputs`,
   `recordedDirentsInputs`. The same MODULE.bazel yields different repos on
   macOS vs Linux for `os_dependent` extensions.
5. **`override_repo`/`inject_repo`** can redirect an extension repo to an
   entirely different canonical name after the fact
   (`BazelDepGraphValue.getRepositoryMapping` consults
   `repoOverrides.row(extensionId)` before the default name).

**What the lockfile provides instead:** for each extension that has been
evaluated in *this* workspace, `generatedRepoSpecs` gives the complete list of
repo names plus each one's `repoRuleId` and attribute dict, and
`recordedRepoMappingEntries` gives the mapping entries that extension consumed.
That is a cached, possibly-stale, possibly-partial snapshot — a good *completion
source*, an unreliable *resolution source*. Staleness is detectable by comparing
`usagesDigest`/`bzlTransitiveDigest`, but recomputing those requires Bazel.

**Design conclusion.** An LSP should:
- resolve names optimistically and statically (fast, always available);
- verify existence lazily by `stat`ing `$output_base/external/<canonical>`;
- on miss, offer a *code action / progress-reporting background task* that runs
  `bazel fetch --repo=@@<canonical>` (0.7 s warm) rather than blocking the
  hover;
- never silently run `bazel mod graph` or a build.

---

## 6. Package boundaries

### 6.1 Which package owns a label

`//a/b/c:d` belongs to the package that is the **longest prefix of `a/b/c`**
containing a build file. Build file names come from
`packages/BuildFileName.java` (`WORKSPACE`, `WORKSPACE.bazel`,
`WORKSPACE.bzlmod`, `MODULE.bazel`, `BUILD`, `BUILD.bazel`), and the search
order used by `PackageLookupFunction` is
`BazelSkyframeExecutorConstants.BUILD_FILES_BY_PRIORITY` (l.35):

```java
ImmutableList.of(BuildFileName.BUILD_DOT_BAZEL, BuildFileName.BUILD);
```

so **`BUILD.bazel` wins when both exist**. The four WORKSPACE/MODULE entries are
root-only pseudo-packages and are not part of package lookup.

Critically, `//a/b/c:d` **must** name the package `a/b/c` exactly — Bazel does
not walk up for you. If `a/b/c` has no BUILD file you get
`ERROR: no such package 'a/b/c'`. The *walk up* only matters for the inverse
question ("given a file path, which package/target owns it?"), which is what
`bazel query <path>` / `TargetPattern.InterpretPathAsTarget` does, and what an
LSP needs for "find references" and for `$(location)` completion.

### 6.2 Subpackage shadowing

If `a/b/BUILD` exists and `a/BUILD` declares `srcs = ["b/x.c"]`, that is an
error, not a silent reference:

- `ContainingPackageLookupValue.java` l.74: `Label '%s' is invalid because '%s' is a subpackage`
- `TargetLoadingUtil.java` l.85: `Label '%s' crosses boundary of subpackage '%s'`
- `PackageFunction.java` l.957 raises `Code.LABEL_CROSSES_PACKAGE_BOUNDARY`
- `BzlLoadFunction.java` l.1683 same for `load()`

`glob()` likewise stops at subpackage boundaries. **A naive filesystem-only
resolver that maps `//a:b/x.c` → `<root>/a/b/x.c` will happily open a file that
Bazel considers unreachable.** The LSP should surface this as a diagnostic.

### 6.3 `.bazelignore`

`IgnoredSubdirectoriesFunction.BAZELIGNORE_REPOSITORY_RELATIVE_PATH = ".bazelignore"`
(l.60). Semantics (`getIgnoredPrefixes`, l.77):

- one **repository-relative directory path** per line;
- **prefix** matching, not glob: a directory is ignored iff
  `directory.startsWith(prefix)` (`IgnoredSubdirectories.matchingEntry`, l.208);
- absolute paths are an error: `'%s': cannot be an absolute path`;
- **only the first `.bazelignore` found on the package path is used** (`break`
  after the first hit);
- for non-main repos, the repo must be **fetched** before its `.bazelignore` can
  be read (l.165: *"Make sure the repository is fetched"*);
- no comment syntax, no negation, no `*`. (It is *not* `.gitignore`.)

### 6.4 `REPO.bazel`

`LabelConstants.REPO_FILE_NAME = "REPO.bazel"`, evaluated by
`RepoFileFunction`, globals in `packages/RepoFileGlobals.java`:

- `repo(**kwargs)` (l.66) — *"accepts exactly the same arguments as the
  `package()` function in BUILD files"*; must be the **first** call and at most
  once. This sets repo-wide `default_visibility`, `default_applicable_licenses`,
  `features`, etc. **An LSP computing effective visibility must read
  `REPO.bazel` as well as `package()`.**
- `ignore_directories(dirs)` (l.38) — *"a directory is ignored if any of the
  given strings matches its repository-relative path according to the semantics
  of the `glob()` function"*. Unlike `.bazelignore` these are **globs**
  (`IgnoredSubdirectories.patterns`, matched with `UnixGlob.matchesPrefix`).
  Can only be called once; must come after `repo()` if `repo()` is used.

`IgnoredSubdirectories` (262 lines) holds three sets: `prefixes` (from
`.bazelignore` + vendor dir), `patterns` (from `ignore_directories`), and
`traversalExclusions`. Both sources apply to *every* repo, including external
ones.

### 6.5 Things that break naive filesystem-only resolution

| Case | Why it breaks |
|---|---|
| generated files | `//foo:bar.pb.go` has no file on disk; it is an output of a rule. Only `bazel query`/`cquery` knows |
| `alias()` | `//a:b` may be an alias whose `actual` is `@other//c:d`; goto-def should offer both |
| `filegroup()` / `exports_files()` | `//a:srcs` is a rule, not a path |
| implicit file targets | `//a:x.c` exists as a target iff `a/x.c` exists **and** `a` is a package **and** `a/x` is not a subpackage; it does not need to be listed anywhere |
| `package_group` / `__pkg__` / `__subpackages__` | `//foo:__subpackages__` in a `visibility` list is not a target |
| `//conditions:default`, `//visibility:public|private` | pseudo-labels forced into the main repo (`Label.ABSOLUTE_PACKAGE_NAMES`) |
| symlinked repos | `external/+_repo_rules+mylocal -> /tmp/bzl-exp1`; the same file is `//app:in.txt` and `@mylocal//app:in.txt` |
| `--package_path` (legacy, still supported) | a package may live under any of several roots |
| `--experimental_sibling_repository_layout` | `external/<repo>` becomes `../<repo>` |
| `.bazelignore` / `ignore_directories()` | a directory containing a real `BUILD` file may not be a package |
| `--deleted_packages` | ditto |
| repo not fetched | `$output_base/external/<canonical>` absent |
| `WORKSPACE`-era repos | in Bazel ≤ 8 with `--enable_workspace`, repo names have no `+` and come from `WORKSPACE`, not MODULE.bazel. Bazel 9.0 made `--enable_bzlmod`/`--enable_workspace` **no-ops: bzlmod always on, WORKSPACE always disabled** (`CHANGELOG.md` l.2140) |

---

## 7. ALGORITHM — resolve a label at a cursor to (file, line)

### 7.1 State the server maintains

```
WorkspaceCtx {
  workspace_root  : Path            # nearest ancestor of the open file with MODULE.bazel
                                    #   (or WORKSPACE/WORKSPACE.bazel for Bazel <= 8)
  bazel           : Path            # from .bazelversion/bazelisk, or PATH
  output_base     : Path            # `bazel info output_base`  (does NOT fetch anything)
  external_root   : Path = output_base/"external"
                                    #   or execroot sibling if --experimental_sibling_repository_layout
  bzlmod_enabled  : bool            # MODULE.bazel exists && Bazel >= 7 && not --noenable_bzlmod
  repo_mappings   : Map<canonical_repo, Map<apparent, canonical>>   # lazily filled, invalidated
                                    #   on MODULE.bazel / MODULE.bazel.lock / *.bzlmod change
  ignored         : Map<canonical_repo, IgnoredSubdirs>             # .bazelignore + REPO.bazel
  pkg_cache       : Map<(canonical_repo, dir), bool /*is package*/>
}
```

`bazel info output_base` is safe to call at startup (measured: creates no
`external/`). `bazel mod dump_repo_mapping ""` is safe but costs ~9 s cold —
run it in the background, off the critical path, and answer from the static
fallback until it lands.

### 7.2 Main routine

```python
def goto_definition(file F, cursor):
    # ---- 0. Lexical: is the cursor inside a string that can hold a label? ----
    tok = string_token_at(F, cursor)                     # from the Starlark CST
    if tok is None: return NONE
    ctx = classify(F, tok)   # LOAD_LABEL | ATTR_LABEL | SELECT_KEY | LOCATION_EXPANSION
                             # | VISIBILITY_SPEC | TARGET_PATTERN | MODULE_DIRECTIVE | NOT_A_LABEL
    if ctx == NOT_A_LABEL: return NONE
    if ctx == LOCATION_EXPANSION:
        tok = inner_arg_of("$(location|locations|rootpath|rootpaths|execpath|execpaths"
                           "|rlocationpath|rlocationpaths <ARG>)", cursor)   # LocationExpander.expand
    if ctx == VISIBILITY_SPEC:
        return goto_package_spec(tok)                    # §7.5 — different grammar
    if ctx == MODULE_DIRECTIVE:
        return goto_module_directive(tok)                # §7.6

    # ---- 1. Split (port of LabelParser.Parts.parse) ----
    parts = split_label(tok.text)                        # -> repo, repoIsCanonical,
                                                         #    pkgIsAbsolute, pkg, tripleDots, target
    if parts is ERROR: return DIAGNOSTIC(parts.msg)      # exact LabelValidator messages

    # ---- 2. Context of the *file the label is written in* ----
    ctx_repo = repo_of_path(F)                           # §7.3, canonical name or "" for main
    ctx_pkg  = enclosing_package(F)                      # §7.4
    if ctx_repo is UNKNOWN: return GIVE_UP("file outside any known repo")

    # ---- 3. Grammar checks depending on site ----
    if ctx in (LOAD_LABEL, ATTR_LABEL, SELECT_KEY, LOCATION_EXPANSION):
        if parts.tripleDots:   return DIAGNOSTIC("package name cannot contain '...'")
        if parts.pkg != "" and not parts.pkgIsAbsolute:
            return DIAGNOSTIC("absolute label must begin with '@' or '//'")
    if ctx == LOAD_LABEL and not target.endswith((".bzl", ".scl")):
        return DIAGNOSTIC('The label must reference a file with extension ".bzl"')

    # ---- 4. Repo resolution (port of Label.computeRepoNameWithRepoContext) ----
    if parts.repo is None:
        if parts.pkg in ("conditions", "visibility"):
            return PSEUDO_LABEL(parts)                   # //conditions:default, //visibility:public
        canonical = ctx_repo
    elif parts.repoIsCanonical:                          # @@foo//...
        canonical = parts.repo                           # "" means main
    else:                                                # @foo//...
        m = repo_mappings.get_or_fetch(ctx_repo)         # `bazel mod dump_repo_mapping <ctx_repo>`
        if m is UNAVAILABLE:
            canonical = static_guess(parts.repo)         # §7.7 — best-effort, may be wrong
        elif parts.repo not in m:
            return DIAGNOSTIC(f"No repository visible as '@{parts.repo}' from {ctx_repo or 'main'}",
                              suggestions=did_you_mean(parts.repo, m.keys()))
        else:
            canonical = m[parts.repo]

    # ---- 5. Repo root on disk ----
    root = repo_root(canonical)                          # §7.8
    if root is MISSING:
        return NEEDS_FETCH(canonical)                    # offer `bazel fetch --repo=@@<canonical>`

    # ---- 6. Package resolution ----
    pkg_dir = root / (parts.pkg if parts.pkgIsAbsolute else ctx_pkg)
    if is_ignored(canonical, rel(pkg_dir, root)):        # .bazelignore prefix / ignore_directories glob
        return DIAGNOSTIC("package is ignored")
    build_file = pkg_dir/"BUILD.bazel" if exists else pkg_dir/"BUILD" if exists else None
    if build_file is None:
        return DIAGNOSTIC(f"no such package '{parts.pkg}'")

    # ---- 7. Subpackage shadowing check (only when target contains '/') ----
    if "/" in parts.target:
        for d in ancestors_between(pkg_dir, pkg_dir/dirname(parts.target)):
            if has_build_file(d) and d != pkg_dir:
                return DIAGNOSTIC(f"Label '{tok.text}' crosses boundary of subpackage "
                                  f"'{rel(d, root)}'")

    # ---- 8. Target resolution, cheapest first ----
    # 8a. a source file with that exact name
    cand = pkg_dir / parts.target
    if is_regular_file(cand):
        if ctx == LOAD_LABEL: return (cand, line=1)
        # still prefer a rule of the same name if one exists (rules shadow nothing,
        # but a same-named rule is an error in Bazel, so file-wins is safe)
        return (cand, line=1)
    # 8b. a rule/macro named `target` declared in build_file
    hit = find_name_kwarg(build_file, parts.target)      # index built from the CST of build_file
    if hit: return (build_file, hit.line, hit.col)
    # 8c. the name may be produced by a macro / glob / generated output
    return slow_path(tok.text, ctx_repo, ctx_pkg)
```

### 7.3 `repo_of_path`

```python
def repo_of_path(p):
    if p.is_relative_to(external_root):
        first = p.relative_to(external_root).parts[0]
        return "" if first == "_main" else first          # Bazel 9 symlinks external/_main
    if p.is_relative_to(workspace_root): return ""
    # symlinked repo: resolve through the symlink farm
    for entry in external_root.iterdir():
        if entry.is_symlink() and p.is_relative_to(entry.resolve()):
            return entry.name                              # e.g. +_repo_rules+mylocal
    if vendor_dir and p.is_relative_to(vendor_dir): return p.relative_to(vendor_dir).parts[0]
    return UNKNOWN
```

This mirrors `starpls`'s `DefaultFileLoader::repo_for_path`
(`upstream/starpls/crates/starpls/src/document.rs` l.363) — which does *not*
handle the symlink or `_main` cases.

### 7.4 `enclosing_package`

```python
def enclosing_package(F):
    root = repo_root(repo_of_path(F))
    d = F.parent
    while d.is_relative_to(root):
        if is_ignored(repo, rel(d, root)):     return NOT_A_PACKAGE   # keep walking? no: ignored
        if (d/"BUILD.bazel").exists() or (d/"BUILD").exists(): return rel(d, root)
        d = d.parent
    return ""            # repo root package, valid only if root/BUILD[.bazel] exists
```

### 7.5 `package_group` / `visibility` specs (a different grammar)

```
"public" | "private"
| ["-"] ["@repo"] "//" pkg ["/..."]
| ["@repo"] "//" pkg ":" ("__pkg__" | "__subpackages__")
```

Goto-def: `//foo/...` and `//foo:__subpackages__` have no single definition
site — jump to `foo/BUILD`. `//foo:__pkg__` → `foo/BUILD`. A bare `//foo:grp`
inside `visibility` is a `package_group` target: resolve normally.
`PackageSpecification.fromString` does **not** accept a repo prefix; the repo is
inherited from the declaring package.

### 7.6 MODULE.bazel

```python
def goto_module_directive(tok):
    if in bazel_dep(name=…):
        canonical = repo_mappings[""].get(repo_name_of(dep)) or f"{name}+"
        if (external_root/canonical/"MODULE.bazel").exists():
            return (external_root/canonical/"MODULE.bazel", 1)
        v = resolved_version(name)                   # from lockfile registryFileHashes,
                                                     #   else metadata.json + MVS, else declared
        return VIRTUAL_DOC(f"{registry}/modules/{name}/{v}/MODULE.bazel")   # read-only peek
    if in use_extension(bzl_label, sym):
        resolve bzl_label as a normal label (recursively), then find the
        `sym = module_extension(...)` assignment in that file
    if in include("//path:x.MODULE.bazel"):
        return (workspace_root/path/"x.MODULE.bazel", 1)
    if in local_path_override(path=…):
        return (workspace_root/path/"MODULE.bazel", 1)
```

### 7.7 `static_guess` — the offline fallback

Used only when Bazel cannot be run. Correct for the *common* case, wrong in
several documented ways.

```python
def static_guess(apparent):
    if apparent in ("bazel_tools", "platforms"): return apparent   # WELL_KNOWN_MODULES
    if apparent == "":                            return ""
    # look for a directory that already exists — this handles multiple_version_override,
    # ~ vs +, and repo_name= renames without guessing:
    for c in listdir(external_root):
        if c in (apparent, apparent+"+", apparent+"~"): return c
        if c.startswith(apparent+"+") or c.startswith(apparent+"~"): return c   # versioned
    # last resort: assume module name == apparent name and single version
    return apparent + ("+" if bazel_major >= 8 else "~")
```

Wrong when: `repo_name=` renames the repo (`@io_bazel_rules_go` → `rules_go+`);
the repo comes from an extension (`@com_github_x_y` → `gazelle++go_deps+…`);
`multiple_version_override` is in effect; `override_repo`/`inject_repo` redirect.
**Never use this to emit a diagnostic** — only to offer a speculative jump.

### 7.8 `repo_root`

```python
def repo_root(canonical):
    if canonical == "": return workspace_root
    for base in (vendor_dir, external_root):        # vendor wins; it exists pre-build
        if base and (base/canonical).exists(): return realpath(base/canonical)
    return MISSING
```

### 7.9 `slow_path` — shelling out to Bazel

```python
def slow_path(label_text, ctx_repo, ctx_pkg):
    absolute = to_absolute(label_text, ctx_repo, ctx_pkg)     # "@@repo//pkg:target"
    out = run([bazel, "--output_base", output_base,
               "query", "--output=location",
               "--lockfile_mode=off",           # do not dirty MODULE.bazel.lock
               "--keep_going", "--noshow_progress",
               absolute],
              timeout=5s, cwd=workspace_root)
    # "/abs/path/BUILD.bazel:1:10: filegroup rule //pkg:srcs"
    # "/abs/path/go/def.bzl:1:1: source file @rules_go//go:def.bzl"
    if ok: return parse_location(out)
    return GIVE_UP(stderr)
```

Measured on Bazel 8.7.0: **0.207 s** warm for an already-fetched external `.bzl`;
**1.75 s** when the repo had to be fetched first (query auto-fetches); ~3 s extra
if the server has to start. Two hard constraints:

- **One Bazel server lock per output base.** A user-initiated `bazel build` in a
  terminal will block the LSP's query (and vice versa: the LSP will show
  *"Another command is running… waiting for it to complete"*). Either use a
  **separate `--output_base`** for the LSP (costs disk and a second fetch of
  every repo — the 161 MB measured above) or serialise and accept the stall.
  `starpls` uses the user's default output base.
- Queries mutate `MODULE.bazel.lock` unless `--lockfile_mode=off`/`error`.

### 7.10 Where the algorithm must give up

Enumerated exhaustively; each one should produce a *specific* message, never a
silent no-op:

1. **Repo not fetched** — `external/<canonical>` absent. → offer
   `bazel fetch --repo=@@<canonical>`.
2. **Bazel unavailable / wrong version** — `bazel fetch --repo` needs ≥ 7.0
   (PiperOrigin-RevId 570675543, "Add fetch --repo option … Only works with
   bzlmod"); `bazel mod dump_repo_mapping` needs ≥ 7.1. Fall back to
   `static_guess`. Probe once at startup by running
   `bazel mod dump_repo_mapping ""` and treating a non-zero exit as
   "no bzlmod support", which is what `starpls` does
   (`upstream/starpls/crates/starpls/src/bazel.rs` l.57–62).
3. **Apparent repo not in the mapping** — legitimately undeclared dep. Emit
   Bazel's own message plus `SpellChecker.didYouMean` candidates.
4. **Extension repo whose extension has never been evaluated** — the canonical
   name is known, the directory is not there, and `bazel fetch --repo=` will
   have to evaluate the whole extension (can be minutes for `go_deps`/`maven`).
   Ask before doing it.
5. **Generated file target** (`//foo:bar.pb.go`) — no source file, no `name=`
   kwarg. `bazel query` returns the *generating rule*'s location, which is the
   right answer; without Bazel, give up.
6. **Target created by a macro** — `name = "%s_lib" % name` or a `for` loop.
   Static CST scanning finds nothing. Requires either Starlark evaluation or
   `bazel query`. This is the single largest correctness gap for any
   non-evaluating LSP.
7. **Target created by `glob()`** — `//pkg:sub/a.txt` where `sub/a.txt` matches a
   `glob(["**/*.txt"])`. Resolvable by filesystem check (§7 step 8a) *plus* the
   subpackage-shadowing check.
8. **`select()` key that is a `constraint_value` or `platform`** — resolves
   normally, but hover should show *which* branch is active, which needs
   `cquery`.
9. **`$(location …)` argument that is not a declared prerequisite** — the label
   resolves but Bazel would reject it. Diagnose, don't jump.
10. **Cross-package-boundary label** — resolves to a real file that Bazel
    refuses. Diagnose.
11. **`--package_path` with multiple roots** — the same package may exist under
    several roots; pick the first, mention the ambiguity.
12. **`--experimental_sibling_repository_layout`** — `external_root` is wrong.
    Detect from `bazel info` / `.bazelrc`.
13. **`WORKSPACE`-only repos** (Bazel ≤ 8 with `--enable_workspace`) — no
    `MODULE.bazel`, no repo mapping, repo names have no `+`. `bazel mod` fails
    entirely. Detect and fall back to `external/<name>` verbatim.
14. **`@@[unknown repo 'x' requested from @@y]`** in Bazel output — the sentinel
    from `RepositoryName.toNonVisible`. Translate to a readable diagnostic.
15. **Symlink cycles / `..` in repo paths** — bail after N hops.
16. **Labels built by string concatenation** (`"//" + pkg + ":" + name`) — not a
    literal; no goto-def. Detect and stay silent rather than half-resolving.
17. **`.scl` files** — load labels must start with `//`; the magic
    `//:project_proto.scl` rewrites to
    `@bazel_tools//src/main/protobuf/project:project_proto.scl`
    (`BzlLoadFunction.java` l.1096).
18. **Files under `_builtins`** — `@_builtins` is a synthetic repo with no
    on-disk `external/` entry.

---

## 8. Facts worth carrying into the design docs

- `bazel mod dump_repo_mapping ""` is the *only* supported apparent→canonical
  API and it is explicitly documented as being for IDEs and language servers
  (Bazel 7.1.0 release notes; <https://bazel.build/external/module#repository_names_and_strict_deps>).
  It costs 9 s cold, 0.2 s warm, and fetches **nothing**.
- `bazel info output_base` is free of side effects and does not create
  `external/`.
- `bazel mod graph` fetches everything (161 MB / ~90 s in a 3-dep workspace).
  Do not call it.
- `bazel query --output=location <label>` is the correct fallback for
  goto-definition and returns `path:line:col: <kind> <label>`; it auto-fetches.
- Canonical names: `<module>+` normally, `<module>+<version>` under
  `multiple_version_override`, `bazel_tools`/`platforms` verbatim,
  `<repo>+<ext>+<name>` for extension repos, and a form for `use_repo_rule` that
  **changed between Bazel 8 and 9** (`+_repo_rules+x` → `+local_repository+x`).
- `~` → `+` flipped in **Bazel 8.0.0**; flag `--incompatible_use_plus_in_repo_names`
  existed only in 7.3–7.x.
- The committed `MODULE.bazel.lock` has contained **no dep graph** since Bazel
  7.2; `lockFileVersion` is 24 on 8.7.0 and 28 on 9.2.0.
- Bazel 9.0 makes `--enable_bzlmod`/`--enable_workspace` no-ops: bzlmod always
  on, WORKSPACE always off.
- Bazel 9.2.0 creates `$output_base/external/_main -> <workspace>`; 8.7.0 does
  not. Bazel 8.7.0 puts `local_config_platform` in the main repo mapping;
  9.2.0 does not.
- BCR has **1250 modules** (2026-08-25) and **no machine-readable index**; use
  the GitHub git-trees API on `bazelbuild/bazel-central-registry` for
  enumeration and `modules/<n>/metadata.json` for versions.
