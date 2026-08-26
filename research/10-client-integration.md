# Client integration surface + LSP protocol design for Bazel files

Research date **2026-08-25**. Every version/date below was verified against source
fetched today.

---

## 1. Filetype / `languageId` conventions

### 1.0 The root problem

**There is no standard `languageId` for Starlark or Bazel.** The LSP 3.17
specification's language-identifier table (`TextDocumentItem.languageId`) contains
zero occurrences of `starlark` or `bazel` — verified by fetching
`https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/`
and regex-searching the full 917 KB document. The nearest neighbour in the table is
`python`. So every editor invented its own identifier, and they disagree three ways:
**`starlark`** (VS Code, Helix, Zed), **`bzl`** (Neovim), **`bazel`**
(bazel-stack-vscode), plus a fourth axis where `.bazelrc` is variously `bazelrc`,
`bash`, or undetected.

### 1.1 Matrix

Sources:
- VS Code: `upstream/vscode-bazel/package.json` → `contributes.languages`, v0.14.0, HEAD `84484e6` (2026-08-19)
- VS Code alt: `upstream/bazel-stack-vscode/package.json`, v1.9.8, HEAD `a7812c1` (**2023-08-06, dead for 3 years**)
- Neovim: `runtime/lua/vim/filetype.lua` @ master (fetched today)
- Helix: `languages.toml` @ master (fetched today)
- Zed: `zaucy/zed-starlark` v0.4.1 `languages/*/config.toml` (registered in `zed-industries/extensions` `extensions.toml` as `[starlark] submodule = "extensions/starlark" version = "0.4.1"`; there is **no** first-party Zed Starlark language)
- Emacs: `bazelbuild/emacs-bazel-mode` `bazel.el`, HEAD `0a5dec6` (2026-08-10, alive)

| File | vscode-bazel | bazel-stack-vscode | Neovim `filetype` | Helix | Zed (zed-starlark) | Emacs major mode |
|---|---|---|---|---|---|---|
| `BUILD` | `starlark` | `bazel` | `bzl` | `starlark` | Starlark → `starlark` | `bazel-build-mode` |
| `BUILD.bazel` | `starlark` (`.bazel` ext) | `bazel` | `bzl` (`.bazel` ext) | `starlark` (`BUILD.*` glob) | `starlark` | `bazel-build-mode` |
| `*.bzl` | `starlark` | `bazel` | `bzl` | `starlark` | `starlark` | `bazel-starlark-mode` |
| `MODULE.bazel` | `starlark` | `bazel` (`.bazel` ext) | `bzl` | `starlark` (`bazel` ext) | `starlark` | `bazel-module-mode` |
| `*.MODULE.bazel` (`include()`) | `starlark` | `bazel` | `bzl` | `starlark` | `starlark` | `bazel-module-mode` |
| `MODULE.bazel.lock` | ✗ unmatched | ✗ | ✗ | ✗ | ✗ | `js-json-mode` |
| `WORKSPACE` | `starlark` | `bazel` | `bzl` | `starlark` | `starlark` | `bazel-workspace-mode` |
| `WORKSPACE.bazel` | `starlark` | `bazel` | `bzl` | ✗ **unmatched** | ✗ **unmatched** | `bazel-workspace-mode` |
| `WORKSPACE.bzlmod` | `starlark` (`.bzlmod` ext) | ✗ | `bzl` (explicit filename) | `starlark` (glob) | `starlark` | `bazel-workspace-mode` |
| `REPO.bazel` | `starlark` | `bazel` | `bzl` | `starlark` | `starlark` | `bazel-repo-mode` |
| `VENDOR.bazel` | `starlark` | `bazel` | `bzl` | `starlark` | ✗ | `bazel-vendor-mode` |
| `*.scl` | ✗ **unmatched** | ✗ | ✗ | ✗ | ✗ | ✗ |
| `.bazelrc` | `bazelrc` | `bazelrc` | ✗ **unmatched** | **`bash`** | `bazelrc` | `bazelrc-mode` |
| `.bazelignore` | ✗ | ✗ | ✗ | ✗ | ✗ | `bazelignore-mode` |
| `.bazeliskrc` | ✗ | ✗ | ✗ | `env` (grammar bash) | ✗ | `bazeliskrc-mode` |
| `*.star`, `*.sky` | `starlark` | `starlark` (separate id!) | **`starlark`** (not `bzl`) | `starlark` | `starlark` | ✗ |
| `*.bxl` (Buck) | ✗ | ✗ | `bzl` | `starlark` | `starlark` | ✗ |
| BCR `source.json`/`metadata.json` | `json` | `json` | `json` | `json` | `json` | `json` |

### 1.2 The specific traps

**Neovim splits `bzl` from `starlark`.** `runtime/lua/vim/filetype.lua`:

```lua
  bzl = 'bzl',            -- line 321
  bxl = 'bzl',
  bazel = 'bzl',
  BUILD = 'bzl',
  ...
  ipd = 'starlark',       -- line 1292
  sky = 'starlark',
  star = 'starlark',
  starlark = 'starlark',
  ...
  WORKSPACE = 'bzl',      -- line 1609 (filename table)
  ['WORKSPACE.bzlmod'] = 'bzl',
  BUCK = 'bzl',
  BUILD = 'bzl',
```

So a single Neovim user editing `foo.star` and `BUILD` gets two different filetypes
for the same language, and `nvim-lspconfig/lsp/starpls.lua` declares
`filetypes = { 'bzl' }` only — **starpls never attaches to `.star`/`.sky` files in
Neovim**. There is a `runtime/ftplugin/bzl.vim` (which `source`s
`$VIMRUNTIME/ftplugin/python.vim`) but **no `ftplugin/starlark.vim`**, so the
`starlark` filetype gets no indent/comment settings either.

**Helix routes `.bazelrc` to `bash`.** In `languages.toml` the `bash` language's
`file-types` list contains the bare string `"bazelrc"` (line ~1313), which matches
the dotfile `.bazelrc`. `.bazeliskrc` goes to the `env` language. Consequently
`bazelrc-lsp` cannot be wired up in Helix without a user override. Helix also sets
`grammar = "python"` for Starlark — it has no Starlark tree-sitter grammar at all.

**Helix's Starlark entry, verbatim** (`languages.toml`):

```toml
[[language]]
name = "starlark"
scope = "source.starlark"
injection-regex = "(starlark|bzl|bazel|buck)"
file-types = [
  "bzl", "bazel", "star",
  "bxl", # Buck Extension Language
  { glob = "BUILD" }, { glob = "BUCK" }, { glob = "BUILD.*" },
  { glob = "WORKSPACE" }, { glob = "WORKSPACE.bzlmod" }, { glob = "PACKAGE" },
]
comment-token = "#"
indent = { tab-width = 4, unit = "    " }
language-servers = [ "starpls", "buck2" ]
grammar = "python"
```

Note: **no `roots` key**, so Helix falls back to its default workspace root (git
root / cwd) rather than the Bazel workspace — see §3. Also no `WORKSPACE.bazel`.

**`.scl` is unsupported by literally everyone.** Bazel 9.2.0 has
`--experimental_enable_scl_dialect` defaulting to **`true`**
(`src/main/java/com/google/devtools/build/lib/packages/semantics/BuildLanguageOptions.java:192-199`),
and `PROJECT.scl` is load-bearing for `--scl_config` /
`BuildConfigurationKeyProducer`. Yet:
- buildifier's `getFileType` (`upstream/buildtools/build/lex.go:157-185`) has no
  `.scl` case → falls through to `TypeDefault`
- starpls's `dialect_and_api_context_for_workspace_path`
  (`crates/starpls/src/document.rs:748-780`) has no `.scl` case → `Dialect::Standard`
- zero editor filetype tables mention it

Supporting `.scl` (and `PROJECT.scl` specifically, which has a restricted global
set — `StarlarkGlobals.java:65` "Returns the top-levels for .scl files") is
uncontested ground.

**Emacs derives its `languageId` from the mode name.** eglot
(`eglot.el:1532-1541`) computes the id as
`(replace-regexp-in-string "\\(?:-ts\\)?-mode$" "" (symbol-name sym))` unless an
explicit `:language-id` is given in `eglot-server-programs`. Matching is by
`provided-mode-derived-p`, so registering the parent `bazel-mode` yields the single
id `bazel` for all seven of emacs-bazel-mode's buffers; registering the derived modes
individually yields `bazel-build`, `bazel-workspace`, `bazel-module`, `bazel-repo`,
`bazel-vendor`, `bazel-starlark`, `bazelrc`. Either way the ids are Emacs-specific
and appear nowhere else, so an Emacs recipe must pin `:language-id "starlark"`
explicitly. And `lsp-mode` has **no Bazel or Starlark client at all** — grepping
today's `lsp-mode.el` for `bazel|starlark|bzl` returns exactly one hit, and it is the
ignore-glob at line 395–396: `;; Bazel` / `"[/\\\\]bazel-[^/\\\\]+\\'"`.

**Zed's `languageId` = language name lowercased**
(`crates/language_core/src/language_name.rs:48-53`: `"Plain Text" => "plaintext"`,
otherwise `language_name.to_lowercase()`), so `Starlark` → `starlark`, `bazelrc` →
`bazelrc`. Helix sends the language `name` unless a `language-id` key overrides
(`helix-core/src/syntax/config.rs:29-33`), so also `starlark`.

### 1.3 Recommendation

Accept **all** of `starlark`, `bzl`, `bazel`, `bazelrc`, and the Emacs
`bazel-*` family in the server, and **never branch on `languageId`** — branch on
the **basename of the URI**, exactly as buildifier and starpls both do. buildifier's
`getFileType` (`build/lex.go:157`) and starpls's
`dialect_and_api_context_for_workspace_path` (`document.rs:748`) are the two
canonical implementations; they agree on `MODULE.bazel`/`*.MODULE.bazel`/`REPO.bazel`/
`VENDOR.bazel`/`WORKSPACE*`/`BUILD*`/`.bzl` and both ignore `languageId` entirely.
buildifier additionally strips a `.oss` suffix and lowercases the basename before
matching (`lex.go:160-164`) — worth copying for Google-internal-style repos.

Ship editor glue as part of the project (a `vim.filetype.add` snippet, a Helix
`languages.toml` fragment, a Zed extension, a VS Code `contributes.languages`
block) rather than assuming the ecosystem will converge.

---

## 2. nvim-lspconfig / `vim.lsp.config`

nvim-lspconfig HEAD `af9adce` (2026-08-24). **413 configs now live in `lsp/`**, not
`lua/lspconfig/configs/`. From the README:

> `require('lspconfig')` (the legacy "framework" of nvim-lspconfig) **is deprecated**
> in favor of `vim.lsp.config` (Nvim 0.11+). … The old configs in `lua/lspconfig/`
> are **deprecated** and will be removed.

`lua/lspconfig/configs/starpls.lua` is still present but carries a DEPRECATED banner
and `root_dir = util.root_pattern(...)`; ignore it. nvim-lspconfig requires
Nvim 0.11.3+.

### 2.1 The five Bazel-adjacent configs, verbatim

`lsp/starpls.lua`:
```lua
---@brief
---
--- https://github.com/withered-magic/starpls
---
--- `starpls` is an LSP implementation for Starlark. Installation instructions can be found in the project's README.

---@type vim.lsp.Config
return {
  cmd = { 'starpls' },
  filetypes = { 'bzl' },
  root_markers = { 'WORKSPACE', 'WORKSPACE.bazel', 'MODULE.bazel' },
}
```

`lsp/bzl.lua` (the closed-source `bzl` server behind bazel-stack-vscode):
```lua
---@brief
---
--- https://bzl.io/
--- https://docs.stack.build/docs/cli/installation
--- https://docs.stack.build/docs/vscode/starlark-language-server

---@type vim.lsp.Config
return {
  cmd = { 'bzl', 'lsp', 'serve' },
  filetypes = { 'bzl' },
  -- https://docs.bazel.build/versions/5.4.1/build-ref.html#workspace
  root_markers = { 'WORKSPACE', 'WORKSPACE.bazel' },
}
```

`lsp/bazelrc_lsp.lua`:
```lua
--- `bazelrc-lsp` is a LSP for `.bazelrc` configuration files.
---
--- The `.bazelrc` file type is not detected automatically, you can register it manually (see below) or override the filetypes:
---
--- ```lua
--- vim.filetype.add { pattern = { ['.*.bazelrc'] = 'bazelrc' } }
--- ```

---@type vim.lsp.Config
return {
  cmd = { 'bazelrc-lsp', 'lsp' },
  filetypes = { 'bazelrc' },
  root_markers = { 'WORKSPACE', 'WORKSPACE.bazel', 'MODULE.bazel' },
}
```

`lsp/buck2.lua`:
```lua
return {
  cmd = { 'buck2', 'lsp' },
  filetypes = { 'bzl' },
  root_markers = { '.buckconfig' },
}
```

`lsp/starlark_rust.lua`:
```lua
--- The LSP part of `starlark-rust` is not currently documented,
--- but the implementation works well for linting.
return {
  cmd = { 'starlark', '--lsp' },
  filetypes = { 'star', 'bzl', 'BUILD.bazel' },
  root_markers = { '.git' },
}
```
`'BUILD.bazel'` in `filetypes` is a **bug** — `filetypes` takes Vim filetypes, and
no filetype named `BUILD.bazel` exists. It is dead config.

### 2.2 Root-marker bugs to avoid

`root_markers` is an **ordered priority list**, not a set
(`runtime/doc/lsp.txt:950-971`):

> The list order decides priority. To indicate "equal priority", specify names in a
> nested list `{ { 'a.txt', 'b.lua' }, ... }`. For each item, Nvim will search
> upwards (from the buffer file) for that marker … search stops at the first
> directory containing that marker.

`{ 'WORKSPACE', 'WORKSPACE.bazel', 'MODULE.bazel' }` therefore means: find the
nearest ancestor with a `WORKSPACE`; **only if none exists anywhere up the tree**
look for `WORKSPACE.bazel`; and so on. In a bzlmod-only repo whose root has
`MODULE.bazel` but where some vendored subtree contains a legacy `WORKSPACE`, the
vendored subtree wins. In a WORKSPACE-less repo where a parent directory (e.g.
`$HOME/src/WORKSPACE` from an old experiment) has a stray `WORKSPACE`, the root
escapes the repo entirely.

The correct shape for a new server:

```lua
---@type vim.lsp.Config
return {
  cmd = { 'bazel-language-server' },
  filetypes = { 'bzl', 'starlark', 'bazelrc' },
  root_markers = {
    { 'MODULE.bazel', 'REPO.bazel', 'WORKSPACE.bazel', 'WORKSPACE', 'WORKSPACE.bzlmod' },
    { '.bazelrc', '.bazelversion' },
    '.git',
  },
  workspace_required = true,
}
```

`workspace_required` defaults to **`false`** (`lsp.txt:2027`). Leaving it false
means the server is started for any matching buffer even outside a Bazel repo —
exactly the failure mode vscode-bazel just fixed on its side (commit `84484e6`,
2026-08-19, "fix: silence workspace error in non-Bazel repos (#638)"). Set it true.

Other `vim.lsp.config` conventions the eventual server must satisfy
(`CONTRIBUTING.md:35-47`, `lsp.txt:229-243`):
- config name = binary name with `-` → `_`; `x-language-server` → `x_ls`
- ship `lsp/<name>.lua` in the repo so users can `vim.pack.add` it directly; configs
  merge across all `lsp/*.lua` in `rtp`, then `after/lsp/*.lua`, then explicit
  `vim.lsp.config()` calls
- the `---@brief` docblock at the top is extracted into `doc/configs.md`
- users enable with `vim.lsp.enable('<name>')`

Neovim defaults that matter (`lsp.txt:74-98`): `grn` → rename, `grr` → references,
`grx` → **codelens run**, `gO` → document symbols, `gx` → **documentLink** (see
§4.2). `workspace/didChangeWatchedFiles` is supported except on Linux. CodeLens and
inlay hints are **off by default** and must be enabled by the user
(`lsp.txt:56-61`).

---

## 3. Root detection, nested workspaces, and the exclusion set

### 3.1 What marks a repo root — five implementations, five answers

| Implementation | Markers |
|---|---|
| buildozer/buildifier `wspace/workspace.go:54-61` | `WORKSPACE`, `WORKSPACE.bazel`, `MODULE.bazel`, `REPO.bazel`, `.buckconfig`, `pants` (must be **executable**) |
| starpls `crates/starpls_bazel/src/lib.rs:161` | `WORKSPACE`, `WORKSPACE.bazel`, `MODULE.bazel`, `REPO.bazel` |
| bazel-lsp `src/bazel.rs:139` | `WORKSPACE`, `WORKSPACE.bazel` only |
| IntelliJ `Constants.kt:29` | `MODULE.bazel`, `WORKSPACE`, `WORKSPACE.bazel`, `WORKSPACE.bzlmod` |
| vscode-bazel `activationEvents` | `**/BUILD`, `**/WORKSPACE`, `**/WORKSPACE.bazel`, `**/MODULE.bazel`, `**/REPO.bazel` |

Nobody uses `.bazelrc` or `.bazelversion` as a root marker, and nobody agrees on
`WORKSPACE.bzlmod` (which is *not* a root marker in Bazel's own semantics — it is
only consulted when bzlmod is enabled, and it never appears without a sibling
`MODULE.bazel` in practice).

`wspace.Find` (`workspace.go:95-116`) sorts the marker names before probing purely
for determinism, then recurses to `filepath.Dir`, terminating on `""`, `/`, `.`, or
a Windows drive root `X:\`. Copy that termination set.

starpls additionally tracks the **package** root while walking up
(`starpls_bazel/src/lib.rs:147-172`): the first ancestor containing `BUILD`/
`BUILD.bazel` is remembered as the package dir, and `resolve_workspace` returns
`(workspace_root, package_root)`. A Bazel LSP needs both — the workspace root for
`//` labels and the package root for `:` labels.

### 3.2 Nested workspaces

Bazel does **not** automatically ignore a subdirectory that contains its own
`WORKSPACE`/`MODULE.bazel`; the user must list it in `.bazelignore`
(`site/en/run/bazelrc.md:299-311`). Therefore the naive "nearest ancestor with a
marker" walk is *correct* for resolving which repo a file belongs to, but it means:

- Two files in the same VS Code / Neovim workspace can legitimately belong to two
  different Bazel repos → the server must key its analysis DB on workspace root, and
  the client must be able to run **one server instance per root** (Neovim does this
  automatically: clients are reused only when `name` **and** `root_dir` match, per
  `vim.lsp.Config.reuse_client` default).
- A vendored third-party repo with its own `MODULE.bazel` will silently become its
  own root, so `//foo` inside it resolves against the *inner* root. That is the
  correct Bazel semantics.
- The outer repo will still glob into the inner repo unless it is `.bazelignore`d,
  so the LSP will index the inner tree twice (once as part of the outer repo's
  package space, once as its own root) unless it applies `.bazelignore` too.

### 3.3 The mandatory exclusion set

**Convenience symlinks.** Bazel creates them from
`OutputDirectoryLinksUtils.STANDARD_LINK_DEFINITIONS`
(`src/main/java/com/google/devtools/build/lib/buildtool/OutputDirectoryLinksUtils.java:311-356`):

| Link | Target |
|---|---|
| `<prefix>bin` | `BuildConfigurationValue::getBinDirectory` |
| `<prefix>testlogs` | `getTestLogsDirectory` |
| `<prefix>genfiles` | `getGenfilesDirectory` (suppressed when `--incompatible_skip_genfiles_symlink`) |
| `<prefix>out` | `outputPath` |
| `<prefix><workspaceBaseName>` | **`execRoot`** |

`<prefix>` defaults to the product name + `-`, i.e. `bazel-`
(`BuildRequestOptions.java:257-267`: "If omitted, the default value is the name of
the build tool followed by a hyphen"), but it is user-configurable via
`--symlink_prefix`, and `--symlink_prefix=/` or
`--experimental_convenience_symlinks=ignore` disables creation entirely.

The killer is the **last** one: `bazel-<workspaceBaseName>` → `execroot/_main`,
which is the **symlink forest** (`SymlinkForest.java`, "Creates a symlink forest
based on a package path map"). Per `site/en/remote/output-directories.md:80-141`,
`execroot/_main/` contains both `bazel-out/` *and* `<packages>/` — "Packages
referenced in the build appear as if under a regular workspace". So walking
`bazel-<ws>/` re-enters the entire source tree through symlinks, and
`bazel-<ws>/bazel-out/...` re-enters the output tree. A recursive walker that
follows symlinks will index every BUILD file 2–N times and, because `execroot`
also contains a link back to the workspace, can loop.

**Minimum exclusion set for any tree walk / file watcher:**

1. Every entry matching `bazel-*` at the **workspace root only** that is a symlink
   (also `blaze-*` for Google-internal, and `<--symlink_prefix>*` if configured).
   Do not exclude `bazel-*` at arbitrary depths — a source directory legitimately
   named `bazel-utils` exists in the wild. lsp-mode's default is the blunt
   `"[/\\\\]bazel-[^/\\\\]+\\'"` at any depth (`lsp-mode.el:395-396`); it is
   over-broad but it is the de-facto convention.
2. `.bazelignore` at the repo root: newline-separated, **workspace-root-relative,
   no glob semantics**, `#` comments must occupy a whole line. Bazel's own path
   constant is `IgnoredSubdirectoriesFunction.BAZELIGNORE_REPOSITORY_RELATIVE_PATH`
   (`IgnoredSubdirectoriesFunction.java:58-60`). IntelliJ's parser
   (`BazelIgnoreMatcherFactory.fromBazelIgnoreFile`,
   `intellij.bazel.core/src/org/jetbrains/bazel/ignore/BazelIgnoreMatcher.kt:44-60`)
   is 15 lines: drop blank lines, drop `#`-prefixed lines, prefix-match the rest.
   emacs-bazel-mode's `bazelignore--syntax-propertize` documents the whole-line
   comment rule with a direct link to Bazel 9.2.0's
   `IgnoredSubdirectoriesFunction.java#L208`.
3. `ignore_directories([...])` in `REPO.bazel` — Bazel 8+, **glob semantics**, unlike
   `.bazelignore`. Note the Starlark symbol is registered as **`ignore_directories`**
   (`RepoFileGlobals.java:38`, `name = "ignore_directories"`) while the error message
   and the docs say `ignored_directories()` (`RepoFileGlobals.java:59`,
   `bazelrc.md:309`). Handle the real name.
4. `bazel-out/`, `bazel-bin/` etc. reached *through* an already-excluded symlink are
   handled by (1); but a `--symlink_prefix` build or a
   `--experimental_convenience_symlinks=ignore` build leaves no symlinks at all, so
   also treat any path under `bazel info output_base` as external-read-only.

**Do not** simply refuse to follow symlinks: Bazel repos legitimately use symlinked
source directories, and `$(bazel info output_base)/external/<repo>` — where every
external dependency's BUILD files live, and where goto-definition on
`@rules_go//go:def.bzl` must land — is reached through them.
starpls tracks this as `Workspace::external_output_base = output_base/external`
(bazel-lsp `src/workspace.rs:19-43` has the same field, plus
`DEFAULT_WORKSPACE_NAMES = ["__main__", "_main"]` for the main-repo canonical name).

---

## 4. LSP protocol design for build files

### 4.1 `textDocument/definition` on a label inside a string literal

The problem: `deps = ["//foo/bar:baz"]`. The token is a string; the interesting
sub-ranges are the repo part, the package part, and the target part, all inside the
quotes.

**What starpls does** (`crates/starpls_ide/src/goto_definition.rs:237-310`,
`handle_literal_expr`): it takes the **whole string token's** range as
`origin_selection_range` (`Some(self.token.text_range())`) — quotes included — and
resolves the entire literal value as a path or label. Target ranges:
- `ResolvedPath::Source { path }` → an *external* location link to the file, no range
- `ResolvedPath::BuildTarget { build_file, target }` → it re-parses the target BUILD
  file, scans top-level `CallExpr`s for one whose `name = "<target>"` keyword
  argument matches, and returns **the whole call expression's range** as both
  `target_range` and `target_selection_range`; if not found it degrades to
  `TextRange::new(0, 0)` — i.e. line 1 of the BUILD file.

So starpls's answer to "what's the selection range" is *the entire literal token*,
and its answer to "what's the target range" is *the entire rule call*. The naive
degradation to offset 0 when the name lookup fails is a visible wart (jumps to top of
file rather than reporting no definition).

**What IntelliJ does** (`BazelLabelReference.kt:42-52`) — the better model:
```kotlin
internal class BazelLabelReference(element: StarlarkStringLiteralExpression, soft: Boolean) :
  PsiReferenceBase<StarlarkStringLiteralExpression>(element, TextRange(0, element.textLength), soft) {
```
The reference spans the whole literal element, but writes go through
`StarlarkStringLiteralManipulator`
(`languages/starlark/rename/StarlarkStringLiteralManipulator.kt`) whose
`getRangeInElement` returns `element.getStringContentsOffset()` — i.e. **the
quote-stripped interior**. Read range ≠ write range. That separation is exactly what
LSP needs: `originSelectionRange` should be the interior (so the highlight looks
right) while resolution uses the parsed label.

**What vscode-bazel does** (`src/definition/bazel_goto_definition_provider.ts`) —
the crude client-side version, worth knowing because it defines the UX users
currently have:
```ts
export const LABEL_REGEX = /"((?:@\w+)?\/\/|(?:.+\/)?[^:"]*(?::[^:"]+)?)"/;
...
const range = document.getWordRangeAtPosition(position, LABEL_REGEX);
const targetText = document.getText(range);
...
return [{ originSelectionRange: range, targetUri: Uri.file(location.path).with({ fragment: `${location.line}:${location.column}` }), targetRange: location.range }];
```
It resolves via `bazel query 'kind(rule, "<label>") + kind(file, "<label>")'` and
skips `//visibility*` labels explicitly. `originSelectionRange` **includes the
quotes** because the regex captures them.

**Design recommendation.** Return `LocationLink[]` (not `Location[]`) and set
`originSelectionRange` to the *sub-range of the string interior that was actually
resolved*, not the whole literal:

- cursor in `//foo/bar` of `"//foo/bar:baz"` → origin = `//foo/bar`, target = the
  package's `BUILD` file, line 1
- cursor in `baz` → origin = `baz`, target = the `name = "baz"` **string literal**
  (not the whole call), so the editor highlights the name
- cursor in `@rules_go` of `"@rules_go//go:def.bzl"` → origin = `@rules_go`, target =
  the `bazel_dep(name = "rules_go", ...)` call in `MODULE.bazel`

This is the one place where a new server can obviously beat starpls, and it needs
`LocationLink` support to be declared by the client — check
`clientCapabilities.textDocument.definition.linkSupport` and fall back to plain
`Location` (dropping the origin range) when absent. Every client in §1 supports
link mode except old ones.

Practical parsing rules: labels can appear in list literals, `select()` dict **keys**
(config settings are labels), `dict` values, `glob(exclude=)` (not labels — paths),
and inside `%`-format strings and `.format()` templates (do **not** resolve those).
`load()` first argument is a label to a `.bzl` file, and `load()` subsequent
arguments are symbol names, not labels — starpls handles those via separate
`handle_load_module`/`handle_load_item` paths
(`goto_definition.rs:212-235`) and sets `origin_selection_range` on the token there
too.

### 4.2 `textDocument/documentLink` as an alternative — **no, but ship it as well**

Client behaviour differs so radically that documentLink cannot be the primary
navigation mechanism:

| Client | documentLink binding | Effect on a `file://` target |
|---|---|---|
| VS Code | ctrl/cmd+click, underline on hover | opens in editor ✅ |
| Helix | `gf` (`goto_file`), *prefers* LSP links over its own path heuristics (`helix-term/src/commands.rs:1382-1400`, "Prefers LSP document links when the cursor/selection overlaps a link range"); resolves via `documentLink/resolve` if `resolveProvider` (`commands.rs:1362-1380`) | opens in editor ✅ |
| Neovim | **`gx`** only — `runtime/lua/vim/_core/defaults.lua:149`, "Map `gx` to call `vim.ui.open` on the `textDocument/documentLink` … at cursor". `vim.ui._get_urls` (`runtime/lua/vim/ui.lua:233-267`) converts `file://` targets with `vim.uri_to_fname` and hands them to `vim.ui.open`, which shells out to **`open`/`xdg-open`/`explorer.exe`** | opens the BUILD file in Finder / the OS default app ❌ |
| Zed | supported (`crates/project/src/lsp_command.rs`, 16 references) | in-editor |
| Emacs | eglot: not implemented | — |

So documentLink on Bazel labels would, in Neovim, launch a *file manager* when the
user presses `gx` on a dep. There is no `vim.lsp.buf.document_link()` and no link
decoration.

**Who actually uses it:** `bazelrc-lsp` v0.2.6 declares both
`document_link_provider` and `definition_provider`
(`src/language_server.rs:165-169`) and implements them over the **same** ranges —
`import` / `try-import` lines, target = the resolved `.bazelrc` file
(`src/language_server.rs:419-462`, `src/definition.rs:24`). That is the right
pattern: documentLink is a *superset advertisement* of a subset of definitions,
free to compute, and it lights up Helix's `gf` and VS Code's ctrl+click without
costing anything. `tilt-dev/starlark-lsp` stubs `DocumentLink` in
`pkg/server/fallback.go:137-143` and never implements it. starpls does not declare it.

**Recommendation:** implement `textDocument/definition` as the primary, and
additionally implement `documentLink` **only for links whose target is a whole
file** (`load()` paths, `exports_files`, `srcs` entries pointing at real files,
`bazelrc` imports) — never for targets whose resolution requires a range inside
another file, because documentLink's `target` is a bare URI with no position (LSP
has no `DocumentLink.targetRange`; you can only smuggle a position through a URI
fragment, which vscode-bazel does — `.with({ fragment: "${line}:${column}" })` — and
which Neovim's `uri_to_fname` will strip).

### 4.3 `workspace/symbol` returning targets

Nothing in the LSP ecosystem does this today:
- starpls: `ServerCapabilities` in `crates/starpls/src/commands/server.rs:57-77`
  declares `completion_provider`, `declaration_provider`, `definition_provider`,
  `document_symbol_provider`, `hover_provider`, `references_provider`,
  `signature_help_provider`, `text_document_sync` — **no `workspace_symbol_provider`**
- tilt `starlark-lsp` `pkg/server/initialize.go:18-36`: DocumentSymbol only;
  `WorkspaceSymbol` is a `fallback.go` stub
- vscode-bazel registers `registerDocumentSymbolProvider` only
  (`src/language_support/language_support_feature.ts`), backed by
  `BazelTargetSymbolProvider` which shells out to `bazel query` per BUILD file
- starlark-rust: `server_capabilities` (`starlark_lsp/src/server.rs:415-430`) has
  only sync/definition/completion/hover

The non-LSP prior art is IntelliJ's
`LabelSearchEverywhereContributor` /
`SeLabelProvider` (`intellij.bazel.core/src/org/jetbrains/bazel/ui/widgets/`,
`.../searchEverywhere/`, id `"LabelSearchEverywhereContributor"` in
`BazelPluginConstants.kt`), and vscode-bazel's `bazel.pickTarget` quick-pick
command + `enableWorkspaceTree` tree view.

Design points:
- Return `WorkspaceSymbol` (3.17) with the **`location: {uri}` variant** (URI-only,
  no range) for targets you have not parsed yet, and resolve the range lazily in
  `workspaceSymbol/resolve`. This is the whole reason that variant exists and it is
  perfect for a `bazel query`-backed index: you know `//foo:bar` lives in
  `foo/BUILD` without having parsed `foo/BUILD`.
- `name` should be the label without the leading `//` (so fuzzy matchers work on
  `foo/bar:baz`), `containerName` the package, `kind` mapped from rule class
  (`cc_binary` → `SymbolKind.Function`? there is no good mapping; most servers abuse
  `Class`/`Function`/`Object`). Consider `SymbolTag` for `testonly`/deprecated.
- The index source matters: `bazel query 'kind(".* rule", ...)'` is what starpls uses
  for label completions (`crates/starpls_bazel/src/client.rs:176-177`) — see §4.9 on
  why that is expensive.

### 4.4 `textDocument/rename` on a target name

**No LSP server implements this today.** starpls, bazel-lsp, starlark-rust, and
tilt's starlark-lsp all omit `rename_provider`. Renaming a target requires editing
string literals in arbitrarily many other BUILD files, in other packages, possibly
with different label spellings for the same target:

```
//foo:bar        # from another package
:bar             # from the same package
bar              # in some attributes (e.g. deps of a rule in the same package — actually invalid, but `//foo` short forms exist)
//foo            # when target name == package basename
@@main~//foo:bar # canonical repo-qualified, in generated files
```
`buildtools/labels/labels.go` is the canonical normalizer: `Parse`, `ParseRelative`,
`Shorten(input, pkg)`, and crucially `Equal(label1, label2, pkg)` (`labels.go:119`)
which is what buildozer uses to decide whether two spellings denote the same target.

**How buildozer does it today:** there is **no** target-rename command.
`AllCommands` (`upstream/buildtools/edit/buildozer.go:986-1017`) has
`"rename": {cmdRename, ...} "<old_attr> <new_attr>"` — that renames an **attribute**,
not a target. The actual recipe is two commands, and the second is manual:

```sh
buildozer 'set name newname' //foo:oldname
buildozer 'replace deps //foo:oldname //foo:newname' //...:*
```
`cmdReplace` (`edit/buildozer.go:559-573`) is label-aware — it calls
`labels.Equal(e.Value, oldV, env.Pkg)` so it matches `:oldname` and `//foo:oldname`
alike — but the user must enumerate the attributes (`deps`, `srcs`, `data`, `tools`,
`exports`, `visibility`, …) and the target set themselves. `cmdSubstitute`
(`buildozer.go:575+`) is the regex fallback and is *not* label-aware.

**Design recommendation:**
- Implement `textDocument/prepareRename` returning the **string-interior sub-range**
  of the `name = "..."` literal (or of the `:target` component under the cursor), so
  the editor's inline-rename box contains just the target name. This is precisely
  IntelliJ's read-range/write-range split (§4.1).
- Return a `WorkspaceEdit` with `documentChanges` of `TextDocumentEdit` (versioned),
  never a bare `changes` map — cross-file BUILD renames touch dozens of files and
  version checks matter.
- Each edit must **re-render the label in the spelling appropriate to the referring
  package** — `labels.Shorten(newLabel, referringPkg)` — otherwise you rewrite
  `:bar` into `//foo:bar` and produce a buildifier lint (`same-origin-load` /
  `unused-variable` family; the relevant one is the label-shortening warning in
  `WARNINGS.md`).
- Refuse (`prepareRename` error) when the target is referenced from outside the
  files you can see: generated BUILD files, `.bzl` macros that construct the label
  by string concatenation, `*.bzl` `native.existing_rule("bar")`, and
  `//foo:bar` appearing in `.bazelrc`, `*.bazelproject`, CI YAML, and `genrule` `cmd`
  strings. IntelliJ handles this by narrowing the search scope to Starlark files
  only and explicitly documenting the compromise
  (`StarlarkRenamePsiElementProcessor.kt:21-32`: "Without this we receive a search
  scope that is enlarged with `UseScopeEnlarger`, which leads to unexpected renames
  in e.g., Markdown files"). Its conflict detection
  (`findRuleTargetConflicts`) checks for an existing rule of the same name in the
  target BUILD file — do the same, and surface it as a `prepareRename` failure.

### 4.5 `codeLens` for build/test

**vscode-bazel does it entirely client-side** — `src/codelens/code_lens_provider.ts`,
`code_lens_builder.ts`, gated on `bazel.enableCodeLens` (default `true`, described in
`package.json` as "Deactivate to avoid running queries in the background"). Mechanics:

- refuses to run on a dirty document: "Don't show code lenses for dirty BUILD files;
  we can't reliably determine what the build targets in it are until it is saved and
  we can invoke `bazel query` with the updated file" (`code_lens_provider.ts:63-73`)
- calls `getTargetsForBuildFile(bazelExecutable, workspacePath, document.uri.fsPath)`
  → `bazel query` → `blaze_query.QueryResult` proto
- positions come from **`target.rule.location`** in the query proto, wrapped in
  `QueryLocation`; targets are bucketed by line because macros expand many rules to
  one source line (`code_lens_builder.ts`, `targetsByLine`)
- lenses produced per line: **Copy** (`bazel.copyLabelToClipboard`), **Build**
  (`bazel.buildTarget`), **Test** (`bazel.testTarget`), **Run**

**bazel-stack-vscode does it server-side via a custom request** (see §4.7):
`buildFile/labelKinds` returns `LabelKindRange[] = { kind, label, range }`
(`src/bezel/lsp.ts:200-219, 283-287`) and the client renders the lenses, with a
**10-second timeout** and a user-facing warning if the server is slower.
Which lenses appear is controlled by `initializationOptions`
(`src/bezel/lsp.ts:323-341`): `enableCodelenses`, `enableCodelensCopyLabel`,
`enableCodelensCodesearch`, `enableCodelensBrowse`, `enableCodelensStarlarkDebug`,
`enableCodelensBuild`, `enableCodelensTest`, `enableCodelensRun`.

IntelliJ's equivalent is gutter icons:
`StarlarkRunLineMarkerContributor` (`ui/gutters/`), which triggers on an
`IDENTIFIER` token whose grandparent is a top-level `StarlarkCallExpression`, and
computes the label via `calculateLabel` from the file path + `name` attribute —
**no `bazel query` at all**, purely syntactic.

**Design recommendation.** Do it server-side (`codeLensProvider` with
`resolveProvider: true`), derive the label **syntactically** from
`<rule_call>(name = "...")` + package path like IntelliJ, and only use `bazel query`
to fill in `kind` lazily in `codeLens/resolve` if you want kind-accurate lens sets
(test vs run vs build). Purely syntactic lenses work on dirty buffers, which
vscode-bazel's cannot. Remember codelens is **opt-in** in Neovim
(`vim.lsp.codelens`, `grx`) and unsupported in Helix — do not make it the only way to
reach build/test actions; also expose `workspace/executeCommand`.

### 4.6 Push vs pull diagnostics

**Every existing Bazel/Starlark server pushes.** None declares
`diagnosticProvider` (LSP 3.17 pull diagnostics):

| Server | Mechanism |
|---|---|
| starpls | `PublishDiagnostics` notification, `crates/starpls/src/event_loop.rs:175-181` |
| bazelrc-lsp 0.2.6 | `client.publish_diagnostics(...)`, `src/language_server.rs:91` |
| starlark-rust | `lsp_types::notification::PublishDiagnostics`, `starlark_lsp/src/server.rs:83` |
| tilt starlark-lsp | `s.publishDiagnostics`, `pkg/server/text_document_sync.go:24,40-44` |
| vscode-bazel (buildifier) | client-side `DiagnosticCollection`, `src/buildifier/buildifier_diagnostics_manager.ts` |

starpls's push implementation has two properties worth copying:
1. **Only open editors get diagnostics** — `event_loop.rs:157-165` skips any file
   whose `DocumentSource` is not `Editor(version)`. Files pulled in transitively for
   analysis never emit diagnostics.
2. **Debounced** — `--analysis_debounce_interval`, default **250 ms**
   ("After receiving an edit event, the amount of time in milliseconds the server
   will wait for additional events before running analysis",
   `crates/starpls/src/commands/server.rs:40-42`).

**Design recommendation: implement pull (`textDocument/diagnostic` +
`workspace/diagnostic`) and keep push as the fallback.** The Bazel case is the
textbook argument for pull: cross-file diagnostics (unresolved label, missing
`load`, dependency cycle) are expensive and are *invalidated by edits to files the
user is not looking at*. `workspace/diagnostic` with result IDs lets the client
poll the whole-repo set on its own schedule instead of the server pushing N
thousand notifications after every save. VS Code and Zed support pull; Neovim
supports it via `vim.lsp.diagnostic` when the server advertises it; Helix supports
push only. So: advertise `diagnosticProvider` with
`interFileDependencies: true, workspaceDiagnostics: true`, and additionally push for
clients that did not send `textDocument.diagnostic` client capabilities.

Buildifier diagnostics specifically should carry `code` = the warning name (e.g.
`load-on-top`, `native-cc`) and `codeDescription.href` pointing at
`https://github.com/bazelbuild/buildtools/blob/master/WARNINGS.md#<name>` — buildifier
already emits those links in its own text output.

### 4.7 Custom LSP extensions in existing Bazel servers

Grepped `upstream/starpls` and `upstream/bazel-stack-vscode`. **Nobody uses the
`$/` prefix.** (`$/` is reserved in LSP for protocol-implementation-defined messages
that a client may ignore; server-specific *features* conventionally use a
`<servername>/` prefix instead, which is what everyone does.)

**starpls** (`crates/starpls/src/extensions.rs`) — two debug-only requests:
```rust
impl Request for ShowSyntaxTree {
    type Params = ShowSyntaxTreeParams;   // { textDocument }
    type Result = String;
    const METHOD: &'static str = "starpls/showSyntaxTree";
}
impl Request for ShowHir {
    type Params = ShowHirParams;          // { textDocument }
    type Result = String;
    const METHOD: &'static str = "starpls/showHir";
}
```
Surfaced by its own VS Code extension as commands `starpls.showHir`,
`starpls.showSyntaxTree`, `starpls.showVersion`
(`editors/code/package.json`), rendered through a
`TextDocumentContentProvider` on the `starpls-hir://` scheme
(`editors/code/src/commands.ts`). Copy this pattern verbatim — a `showSyntaxTree`
/ `showHir` pair is the single highest-leverage debugging affordance for a
resolver-heavy server.

**`bzl` (bazel-stack-vscode's closed-source server)** — five custom requests
(`upstream/bazel-stack-vscode/src/bezel/lsp.ts`):

| Method | Params | Result | Purpose |
|---|---|---|---|
| `buildFile/labelLocation` | `{ textDocument, label }` | `Location` | jump to an arbitrary label without a cursor position |
| `buildFile/rulelabel` | `TextDocumentPositionParams` | `{ uri }` | "what label is under my cursor" |
| `buildFile/labelKinds` | `{ textDocument }` | `LabelKindRange[] = {kind, label, range}[]` | drives client-side codelens |
| `bazel/kill` | `{ pid }` | `BazelKillResponse` | kill a Bazel server |
| `bazel/recentInvocations` | `{ workspaceDirectory }` | `Invocation[]` | build history UI |

`buildFile/rulelabel` ("what label is at this position") is the one genuinely
missing LSP primitive — every client-side feature (copy-label, build-this,
run-this, open-in-codesearch) needs it, and there is no standard request for it.
A new server should provide it under its own namespace and, separately, expose it as
a `workspace/executeCommand` so clients that cannot send custom requests can still
use it.

**starlark-rust** (`starlark_lsp/src/server.rs:117-137`) has one:
`starlark/fileContents` with `{ uri } -> { contents: Option<String> }`, backing a
virtual `starlark:` URI scheme for native/builtin symbols that have no file on disk
(`enum LspUri { File(PathBuf), Starlark(PathBuf), Other(Uri) }`). A Bazel server
needs the same trick for builtin rule/provider documentation and for files inside
the output base that the client should not be able to edit. Prefer a
**read-only custom scheme** over pointing the client at real paths under
`output_base` — otherwise users edit generated external-repo BUILD files and lose
the changes on `bazel clean --expunge`.

### 4.8 `textDocument/formatting` delegating to buildifier

**starpls does not implement formatting at all** (no
`document_formatting_provider` in its `ServerCapabilities`). vscode-bazel does it
client-side, which is why users of starpls-in-VS-Code still get formatting: it
registers `registerDocumentFormattingEditProvider({ language: "starlark" }, ...)`
(`src/buildifier/buildifier_feature.ts:59-61`).

The exact recipe vscode-bazel uses (`src/buildifier/buildifier.ts:55-57, 100-125`),
which is the one to copy:

```ts
// format:
const args = [`--mode=fix`, `--path=${filePath}`];
if (fixOnFormat) args.push(`--lint=fix`);
// lint:
const args = [`--format=json`, `--mode=check`, `--path=${filePath}`, `--lint=${lintMode}`];
```
Content is piped on **stdin**; `filePath` is the **workspace-relative** path.

Why `--path` matters (`buildifier/buildifier.go:68-72`):

> Buildifier's reformatting depends in part on the path to the file relative to the
> workspace directory. Normally buildifier deduces that path from the file names
> given, but the path can be given explicitly with the `-path` argument. This is
> especially useful when reformatting standard input.

Formatting an unsaved buffer therefore **must** pass `--path`, or buildifier
formats a BUILD file with `default` (plain-Starlark) rules — no attribute sorting,
no `load` sorting — silently producing wrong output.

Additional buildifier facts a server needs:
- `-type` overrides inference: `build | bzl | workspace | module | repo | vendor |
  default | auto` (`buildifier/config/config.go:189`). `auto` is the default.
  Inference (`build/lex.go:157-185`) lowercases the basename and strips a `.oss`
  suffix; `.scl` and `.sky` both land on `TypeDefault`.
- Config file discovery: `-config` flag, else `$BUILDIFIER_CONFIG`, else
  `.buildifier.json` at the workspace root; `-config=off` disables; `-config=example`
  prints a sample. vscode-bazel exposes this as `bazel.buildifierConfigJsonPath`.
- Exit codes: `0` ok, `1` syntax errors, `2` usage error, `3` runtime error,
  `4` **check mode: reformat needed**. Treat 1 and 4 as non-fatal (vscode-bazel's
  `executeBuildifier(..., acceptNonSevereErrors=true)`).
- Formatting scopes (`build/rewrite.go:114-115`):
  `scopeDefault = TypeDefault | TypeBzl`, `scopeBuild = TypeBuild | TypeWorkspace |
  TypeModule | TypeRepo | TypeVendor`. `useRepoPositionalsSort` is `TypeModule`-only.

Return the result as a **single whole-document `TextEdit`** unless you diff — that
is what every buildifier integration does, and it is fine because clients preserve
the cursor. `textDocument/rangeFormatting` is not implementable through buildifier
(it has no range mode); do not advertise it.

Also implement `textDocument/onTypeFormatting`? No — buildifier is too slow per
keystroke, and `formatOnSave` covers the need.

### 4.9 Can the server afford to call Bazel?

Relevant to almost every feature above. Evidence:

- starpls gates label completion behind
  `--experimental_enable_label_completions` (default **false**,
  `crates/starpls/src/commands/server.rs:26-30`) and the query it runs is
  `bazel query "kind('.* rule', ...)"` over the whole repo
  (`crates/starpls_bazel/src/client.rs:176-177`). Refresh is triggered on **save of
  any `BUILD`/`BUILD.bazel`** (`crates/starpls/src/handlers/notifications.rs:44-66`),
  and re-entrancy is guarded by `is_refreshing_all_workspace_targets`
  (`crates/starpls/src/server.rs:331-352`).
- Saving `MODULE.bazel`/`WORKSPACE*` instead clears
  `bazel_client.clear_repo_mappings()` and `fetched_repos` (same file, lines 62-66).
- starpls's own README warns: "if your VSCode setup also has any tasks that run Bazel
  commands on open, those might temporarily block the server from starting up because
  of the Bazel lock".
- vscode-bazel has a whole setting for this: `bazel.queriesShareServer` (default
  true) plus `bazel.queryOutputBase`, documented as "you may experience degraded
  performance … you can disable this setting so that queries and builds can be
  executed in parallel".

Conclusion: any interactive Bazel invocation must use a **separate `--output_base`**
so it cannot contend on the workspace lock with the user's builds, must be
debounced, and must never be on the critical path of a completion request.

---

## 5. Distribution reality

### 5.1 What exists today

| Server | Last activity | Cargo/Go | GitHub releases | Mason | nixpkgs | Homebrew |
|---|---|---|---|---|---|---|
| **starpls** | HEAD `ac25eca` **2025-12-03**; last release **v0.1.22, 2025-08-30** | ✗ (built with Bazel, not on crates.io) | ✅ 10 assets: `{darwin,linux}-{amd64,arm64/aarch64}`, `windows-amd64.exe`, raw + `.tar.gz`/`.zip` | ✅ `starpls` @ v0.1.22 | ✅ **0.1.22** (`pkgs/by-name/st/starpls/package.nix`) | ⚠️ third-party tap only: `withered-magic/brew/starpls`, **stuck at 0.1.21**, raises on non-arm64-macOS |
| **bazel-lsp** (cameron-martin) | HEAD `48fead6` **2025-07-20** (renovate bump); last release **v0.6.4, 2025-02-12** | ✗ | ✅ 5 assets (linux amd64/arm64, osx amd64/arm64, windows) | ✗ | ✗ | ✗ |
| **bazelrc-lsp** (salesforce-misc) | HEAD `2a971b8` **2026-06-02**; last release **v0.2.6, 2026-02-06** | ✗ | ✅ raw bins + per-OS `.vsix` | ✅ `bazelrc-lsp` @ v0.2.6 | ✗ | ✗ |
| **tilt starlark-lsp** | HEAD `5689e7e` **2024-07-30** — **abandoned, 2 years stale** | `go install` | — | ✗ | ✗ | ✗ |
| **starlark-rust** LSP | HEAD `79bc31f` **2026-08-24** (alive, but LSP is a side-feature) | ✅ `cargo install starlark` | — | ✗ | ✗ | ✗ |
| **bzl** (stack.build) | client `a7812c1` **2023-08-06** — **dead** | ✗ | proprietary installer | ✗ | ✗ | ✗ |
| buildifier / buildozer | `674b293` **2026-08-24**, v8.5.1 | `go install github.com/bazelbuild/buildtools/buildozer@latest` | ✅ | ✅ `buildifier` @ 8.5.1 | ✅ `buildifier`/`buildozer` 8.5.1 | ✅ **core formula** `buildifier` 8.5.1 |
| bazelisk | — | — | ✅ | — | ✅ 1.29.0 | ✅ |

Notes:
- Mason's `starpls` entry lists **only** `darwin_arm64`, `linux_x64`, `win_x64` —
  it omits `darwin_x64` and `linux_arm64` even though the release publishes both
  (`starpls-darwin-amd64`, `starpls-linux-aarch64`). Intel-Mac and ARM-Linux Neovim
  users cannot `:MasonInstall starpls`.
- Mason packages carry a `neovim: { lspconfig: <name> }` key that must match the
  nvim-lspconfig config name (`starpls`, `bazelrc_lsp`).
- nixpkgs `starpls.meta.platforms` is the full Rust platform list; the derivation is
  at `pkgs/by-name/st/starpls/package.nix:42`.

### 5.2 What a new server must do to be installable

1. **Publish static binaries on GitHub Releases** for at least
   `{darwin,linux}×{x86_64,aarch64}` + `windows-x86_64`, with a **stable,
   templatable asset name** (`<name>-<os>-<arch>[.exe]`). Every downstream packager
   templates it: Mason (`source.asset[].file`), the Zed extension
   (`zaucy/zed-starlark/src/starpls.rs` builds
   `format!("starpls-{os}-{arch}{exe_suffix}")` and calls
   `zed::latest_github_release`), Homebrew formulae, and nixpkgs `fetchurl`.
   Ship both a raw binary and an archive — Mason and Zed want different ones.
2. **Do not require Bazel to build the server.** starpls builds with Bazel and is
   consequently absent from crates.io; `cargo install` is the single lowest-friction
   install for a Rust LSP and nixpkgs' `rustPlatform.buildRustPackage` needs a
   working `Cargo.lock`. If the server is Rust, keep `cargo build` working.
3. **Register in Mason** (`mason-org/mason-registry`, `packages/<name>/package.yaml`)
   with all five targets and the `neovim.lspconfig` key.
4. **Contribute `lsp/<name>.lua` to nvim-lspconfig** (see §2.2) — or ship it in-repo
   so `vim.pack.add` picks it up, since Nvim 0.12's `vim.pack` reads `lsp/` from any
   plugin on the `rtp`.
5. **Get into nixpkgs** (`pkgs/by-name/<xx>/<name>/package.nix`) — trivial for a
   `cargo`/`go` build, painful for a Bazel build.
6. **Homebrew**: `buildifier` is a core formula, so a Bazel LSP has precedent for
   homebrew-core rather than a personal tap. starpls's personal tap is 1 release
   behind and macOS-arm64-only, which is exactly the failure mode to avoid.
7. **A VS Code extension is not required** — `bazelbuild/vscode-bazel` v0.14.0 already
   has a generic hook:
   ```jsonc
   { "bazel.lsp.command": "<server>", "bazel.lsp.args": [...], "bazel.lsp.env": {} }
   ```
   Setting `bazel.lsp.command` non-empty switches the extension from its built-in
   `bazel query`-based providers to an LSP client
   (`src/language_support/language_support_feature.ts`,
   `LanguageSupportFeature.enableExternalLSP`), with
   `documentSelector: [{ scheme: "file", language: "starlark" }]`
   (`src/language_support/language-server-client.ts:80`) and a
   `bazel.lsp.restart` command. Note it registers **only** `language: "starlark"` —
   so `.bazelrc` (languageId `bazelrc`) is **not** routed to the LSP by
   vscode-bazel, and buildifier formatting stays client-side regardless.
8. **Zed** needs a small WASM extension (`extension.toml` with
   `[language_servers.<id>]` + `languages = ["Starlark"]`, and a `src/*.rs` that
   downloads the release binary). `zaucy/zed-starlark` v0.4.1 already declares three
   Starlark servers (`starpls`, `buck2-lsp`, `tilt`); adding a fourth is a PR to that
   repo, and the extension already ships a `bazelrc` language with
   `zaucy/tree-sitter-bazelrc`.
9. **Helix** needs a PR to `languages.toml` adding the binary to
   `[language-server]` and to the `starlark` language's `language-servers` list
   (currently `[ "starpls", "buck2" ]`), plus — ideally — a `roots` key, which the
   Starlark entry does not currently have.

### 5.3 Position encoding

Worth deciding up front because it is baked into every range: starpls **hard-codes
UTF-16** (`crates/starpls/src/convert.rs:52-53,83` — `WideEncoding::Utf16`) and never
negotiates. bazelrc-lsp reads the client's `positionEncodings` capability and stores
the selection (`src/language_server.rs:118-121`, `LspPositionEncoding`,
defaulting to UTF-16). Negotiate — Neovim and Zed both prefer UTF-8 and it removes a
whole class of off-by-N bugs with non-ASCII strings in BUILD files.

---

## Appendix: repo liveness (verified 2026-08-25)

| Repo | HEAD | Date | Verdict |
|---|---|---|---|
| `bazelbuild/buildtools` | `674b293` | 2026-08-24 | active |
| `facebookexperimental/starlark-rust` | `79bc31f` | 2026-08-24 | active |
| `JetBrains/hirschgarten` | `f2724ec` | 2026-08-24 | very active |
| `bazelbuild/vscode-bazel` | `84484e6` | 2026-08-19 | active |
| `bazelbuild/emacs-bazel-mode` | `0a5dec6` | 2026-08-10 | active |
| `salesforce-misc/bazelrc-lsp` | `2a971b8` | 2026-06-02 | maintained |
| `withered-magic/starpls` | `ac25eca` | 2025-12-03 | slowing (last release 2025-08-30) |
| `cameron-martin/bazel-lsp` | `48fead6` | 2025-07-20 | dormant (last human commit earlier; last release 2025-02-12) |
| `tilt-dev/starlark-lsp` | `5689e7e` | 2024-07-30 | **abandoned** |
| `stackb/bazel-stack-vscode` | `a7812c1` | 2023-08-06 | **abandoned** |
