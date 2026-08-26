# Survey of existing Bazel/Starlark IDE tooling

State as of **2026-08-25**. Every liveness claim below is backed by a commit SHA + date,
a release date, or an issue date. Local clones are at
`/Users/bruth/dev/bazel-language-server/upstream/<name>`; `/tmp` clones are noted.

## 0. Verdicts at a glance

| Project | Kind | Language | Last commit (default branch) | Last release | Verdict |
|---|---|---|---|---|---|
| withered-magic/starpls | LSP server | Rust | 2025-12-03 | v0.1.22, 2025-08-30 | **stalled** (~9 mo, bus factor 1, 20 PRs unreviewed) |
| facebook/starlark-rust `starlark_lsp` | LSP library | Rust | 2026-08-24 (repo) | v0.14.0, 2026-06-01 | repo active; **LSP crate maintenance-only** |
| cameron-martin/bazel-lsp | LSP server | Rust | 2025-07-20 (bot); **2025-07-13 human** | v0.6.4, 2025-02-12 | **abandoned** (13 mo no human commit) |
| tilt-dev/starlark-lsp | LSP server | Go | 2024-07-30 | never released | **abandoned** (25 mo) |
| bazel-contrib/vscode-bazel | VS Code ext (LSP *client*) | TypeScript | 2026-08-19 | v0.14.0, 2026-03-31 | **active**; moved org |
| stackb/bazel-stack-vscode + stackb/bzl | VS Code ext + closed-source server | TS + Go(binary) | 2023-08-06 / 2021-06-11 | 1.8.4, 2022-04-21 | **abandoned** (3 yr) |
| JetBrains/hirschgarten | IntelliJ plugin (native PSI, not LSP) | Kotlin | 2026-08-24 | Marketplace 2026.2.1.1, 2026-08-06 | **very active** — the state of the art |
| bazelbuild/intellij | IntelliJ/CLion plugin (legacy) | Java | 2026-08-14 | 2026.08.13.0, 2026-08-13 | **maintenance-only** (CLion); IntelliJ branch feature-frozen |
| bazel-contrib/buildtools | formatter/linter/rewriter | Go | 2026-08-24 | v8.5.1, 2026-01-30 | **active**; no LSP surface; moved org |
| bazel-contrib/bazel.el | Emacs mode (xref/flymake/capf) | Emacs Lisp | 2026-08-20 | v0.0.3, 2026-08-05 | **active**; moved org + renamed |
| salesforce-misc/bazelrc-lsp | LSP server (`.bazelrc` only) | Rust | 2026-06-02 (compliance); **2026-02-06 functional** | v0.2.6, 2026-02-06 | **maintenance-only** |
| M31-Labs/starlsp | LSP server | Go | 2026-08-16 | v0.3.0 (tag only) | **brand new, unproven** (9 days old, ★1) |
| zaucy/zed-starlark | Zed extension (downloads starpls) | Rust | 2026-03-01 | v0.4.1 | maintenance-only |
| tree-sitter-grammars/tree-sitter-starlark | grammar | JS/C | 2024-12-04 | 1.3.0 | **stale** (20 mo) |

Headline: **the only actively developed, feature-complete Bazel-file language
intelligence in existence is a closed-architecture IntelliJ plugin.** Every LSP-speaking
option is either stalled (starpls), abandoned (bazel-lsp, tilt-dev, stackb), non-Bazel
(starlark-rust core, tilt-dev, starlsp), or scoped to one file type (bazelrc-lsp).

Org moves confirmed by GitHub redirect (`gh api repos/OLD --jq .full_name`):

- `bazelbuild/vscode-bazel` → **`bazel-contrib/vscode-bazel`**
- `bazelbuild/buildtools` → **`bazel-contrib/buildtools`**
- `bazelbuild/emacs-bazel-mode` → **`bazel-contrib/bazel.el`** (renamed too)
- `withered-magic/starpls` has **not** moved: `bazelbuild/starpls` and `bazel-contrib/starpls`
  both return HTTP 404 as of 2026-08-25.

---

## 1. withered-magic/starpls

- Repo: <https://github.com/withered-magic/starpls> · Rust · dual Apache-2.0 / MIT
  (`LICENSE-APACHE` + `LICENSE-MIT`; GitHub reports Apache-2.0)
- Owner: `withered-magic`, an individual. Created 2023-12-19. ★217.
- `git log -1 --format='%H %ad %s'`:
  `ac25eca3dbbed6347fbca5fbf14d3a027d46bcab Wed Dec 3 02:54:57 2025 +0100 Accept 1/0 as bool (#414)`
  (identical to the GitHub `main` head → the local clone is current).
- 77 open issues+PRs = 57 issues + 20 PRs. Latest release **v0.1.22, 2025-08-30**
  (binaries for darwin amd64/arm64, linux amd64/aarch64, windows amd64).
- Contributors: `withered-magic` 383 commits; **every other contributor has exactly 1**
  (brentleyjones, Matir, Fil-Den, jboulter11, keith, patrickdoc, PeterCardenas). Bus factor 1.

### Liveness verdict: stalled

Last merged PR is **#414, merged 2025-12-03** — the head commit. Since then contributors
have opened PRs that sit unreviewed: #431/#432/#433/#434/#435 (2026-07-09/10),
#436/#437/#438 (2026-07-17..19), #421 (issue, 2026-08-03). The oldest open PR is **#280,
opened 2024-08-04**. Two forks carry the work the maintainer hasn't merged:

- `modular/starpls` — **10 commits ahead** of `main`, branches `trotta/fix-*`
  (Modular, the Mojo company, is effectively running a private fork)
- `Ahajha/starpls` — **7 ahead**, same `trotta/*` branch names
- `fmeum/starpls` (Fabian Meumertzheim, Bazel core) — at parity, has a
  `claude/wildcard-support-args-*` branch

### LSP surface

`crates/starpls/src/commands/server.rs:58-77` — `ServerCapabilities`:

```rust
completion_provider:      Some(CompletionOptions { trigger_characters: ['.','"','\'','/',':','@'] }),
declaration_provider:     Some(DeclarationCapability::Simple(true)),
definition_provider:      Some(OneOf::Left(true)),
document_symbol_provider: Some(OneOf::Left(true)),
hover_provider:           Some(HoverProviderCapability::Simple(true)),
references_provider:      Some(OneOf::Left(true)),
signature_help_provider:  Some(SignatureHelpOptions { trigger_characters: ['(',',',')'] }),
text_document_sync:       Some(Kind(TextDocumentSyncKind::INCREMENTAL)),
..Default::default()
```

Everything not listed is `None`: **no rename, no formatting, no code actions, no code
lens, no semantic tokens, no inlay hints, no workspace symbols, no folding, no
document highlight, no selection range.**

Dispatch table `crates/starpls/src/event_loop.rs:214-230`:

- requests: `textDocument/completion`, `textDocument/documentSymbol`,
  `textDocument/definition`, `textDocument/declaration`, `textDocument/hover`,
  `textDocument/references`, `textDocument/signatureHelp`
- notifications in: `textDocument/didOpen|didClose|didChange|didSave`
- notifications out: `textDocument/publishDiagnostics`, `$/progress`,
  `window/showMessage`; request out `window/workDoneProgress/create`
- custom (`crates/starpls/src/extensions.rs`): `starpls/showSyntaxTree`, `starpls/showHir`

Public IDE API (`crates/starpls_ide/src/lib.rs:353-397`): `completions`, `diagnostics`,
`document_symbols`, `find_references`, `goto_definition`, `hover`, `line_index`,
`show_hir`, `show_syntax_tree`, `signature_help`. ~30k LOC of Rust across 10 crates,
salsa-based, explicitly rust-analyzer-shaped (`starpls_lexer` → `starpls_parser` →
`starpls_syntax` → `starpls_hir` → `starpls_ide`).

### Label resolution

`crates/starpls_bazel/src/client.rs` — the `BazelClient` trait shells out to:

| Method | Command |
|---|---|
| `info` | `bazel info execution_root output_base release starlark-semantics workspace` |
| `build_language` | `bazel info build-language` |
| `dump_repo_mapping` | `bazel mod --enable_bzlmod dump_repo_mapping <repo>` |
| `query_all_workspace_targets` | `bazel query "kind('.* rule', ...)"` |
| `null_query_external_repo_targets` | `bazel query --keep_going @@<repo>//...` |
| `fetch_repo` | `bazel fetch --repo @@<repo>` |

- **bzlmod:** yes. `crates/starpls/src/bazel.rs:29-53` decides bzlmod on/off from the
  `release` string against a hardcoded prefix list `["development","release 7","release 8","release 9"]`
  ("Just hardcoding this for now since I'm lazy to parse the actual versions… Bazel 9
  isn't anywhere on the horizon" — it is; Bazel 9.2.0 is current), then overridden by
  `enable_bzlmod=true|false` found in `starlark-semantics`. Apparent→canonical mapping
  via `dump_repo_mapping`, cached per *from-repo* in a `RwLock<HashMap<..>>`
  (`client.rs:126-150`), never invalidated except by `clear_repo_mappings`.
- **External repos:** `<output_base>/external/<canonical_repo>`
  (`crates/starpls/src/server.rs:128`). Missing repos trigger a background
  `bazel fetch --repo` via `fetch_repo_sender`.
- **Label → target:** `crates/starpls/src/document.rs:385-437 resolve_path()`. If the
  resolved path is a file → `ResolvedPath::Source`. Otherwise it `read_dir`s the package
  directory for `BUILD`/`BUILD.bazel` → `ResolvedPath::BuildTarget`, and
  `crates/starpls_ide/src/goto_definition.rs:260-300` then scans the parsed BUILD file's
  **top-level call expressions** for a keyword arg `name = "<target>"`. Pure filesystem +
  syntax; **no `bazel query` per navigation** (good latency, but it cannot see targets
  produced by macros or `glob`).
- Load completion (`document.rs:520-660`) reads directories for packages/targets, and
  for `@`-prefixed repo completion uses `repo_mapping_keys("")` under bzlmod, else a
  `read_dir` of `external/`.
- File-kind → dialect/API-context map (`document.rs:748-780`):
  `BUILD`, `BUILD.bazel`, `*.BUILD`, `*.BUILD.bazel`, `REPO.bazel`, `VENDOR.bazel`,
  `MODULE.bazel`, `*.MODULE.bazel`, `WORKSPACE`, `WORKSPACE.bazel`, `WORKSPACE.bzlmod`,
  `*.bzl`, `*.cquery`, `*.query.bzl`, `tools/build_rules/prelude_bazel`.
  **Not handled:** `.bazelrc`, `MODULE.bazel.lock`, `.bazelignore`, BCR `source.json`/`metadata.json`.

### Where builtin docs come from

Three stacked sources:

1. `crates/starpls/src/builtin/builtin.pb` — **2,452,170 bytes, checked in**, loaded with
   `include_bytes!` at `server.rs:366`. This is Bazel's own
   `//src/main/java/com/google/devtools/build/lib:builtin.pb` (see §9). **It was last
   regenerated 2024-12-27** (`chore: update builtins proto (#365)`) — so globals, `ctx`,
   `attr`, `depset`, provider docs are frozen at roughly Bazel 7.4/8.0 and are ~20 months
   stale against Bazel 9.2.0.
2. Live `bazel info build-language` at startup (`server.rs:372 load_bazel_build_language`)
   → native rule classes and their attributes. This part *is* version-correct.
3. Hand-maintained patch JSON in `crates/starpls_bazel/data/`: `build.builtins.json`,
   `bzl.builtins.json`, `module-bazel.builtins.json`, `repo.builtins.json`,
   `vendor.builtins.json`, `workspace.builtins.json`, `cquery.builtins.json`,
   `commonAttributes.json`, `missingModuleFields.json`. `env.rs:79` explains why: *"The
   builtin.pb file is missing `module_extension`, `repository_rule` and `tag_class`."*

### Type inference

The distinguishing feature. `starpls_hir` does real inference: PEP-484 type comments
(`# type: (ctx) -> Unknown`), builtin provider types (`KNOWN_PROVIDER_TYPES` in
`crates/starpls_bazel/src/lib.rs`), custom `provider()` field typing, and two opt-in flags:

- `--experimental_infer_ctx_attributes` — infers `ctx.attr.<name>` types from the `rule(attrs=…)` dict
- `--experimental_use_code_flow_analysis` — union types across branches

Diagnostics produced (all from inference/lowering, `crates/starpls_hir/src/typeck/infer.rs`,
`def/lower.rs`): `Could not resolve symbol`, `Could not resolve module`,
`Detected circular import`, `Argument of type … cannot be assigned to parameter of type …`,
`Argument missing for attribute(s)/parameter(s)`, `Cannot access/assign to field`,
`Cannot index … with type`, `Cannot slice`, `Duplicate parameter`,
`Non-default parameter cannot follow default parameter`,
`Positional argument cannot follow keyword arguments`,
`Starlark does not allow top-level if/for statements`, `Tuple size mismatch`,
`Index N is out of range`, `Code is unreachable` (`DiagnosticTag::Unnecessary`),
deprecation (`DiagnosticTag::Deprecated`). **No buildifier lints.**

### Known limitations

- **#267 (2024-06)** `textDocument/formatting` unimplemented. PR **#401** implementing it
  has been open since **2025-08-11**, unmerged.
- **#125 (2024-03)** "Find references": the implementation that exists
  (`crates/starpls_ide/src/find_references.rs`) does a `memchr` scan of
  **`self.file.contents(db)` — the current file only** — and matches only `ast::Name` /
  `ast::NameRef`. There is no cross-file reference search and no label reference search.
- **#225 (2024-04)** multi-root workspaces (`parent/proj1`, `parent/proj2` in one VS Code
  window) do not work.
- **#379 (2025-03)** no custom builtin stubs; PR #426 for it was closed unmerged 2026-05-27.
- **#354 (2024-12)** symbolic macros unsupported.
- **#421 (2026-02)** `git_override` (and probably other MODULE.bazel functions) fields unresolved.
- **#418 (2026-01)** `bazel info` fails at init with "at most one key may be specified" on
  some Bazel versions — a hard startup failure, since `BazelContext::new` returns `Err` if
  `bazel info` fails.
- **#400 (2025-07)** VS Code cannot auto-install starpls; the user must download a binary
  and set `bazel.lsp.command` by hand.
- Startup requires `bazel` on `PATH` and acquires the Bazel lock — README: *"if your VSCode
  setup also has any tasks that run Bazel commands on open, those might temporarily block
  the server from starting up because of the Bazel lock."*

---

## 2. facebook/starlark-rust — the `starlark_lsp` crate

- Repo: <https://github.com/facebook/starlark-rust> · Rust · Apache-2.0 · Meta. ★1012, 39 open issues.
- `git log -1`: `79bc31f32ca08975660f31f7719c995513b6ea65 Mon Aug 24 16:20:28 2026 -0700 Remove the order-nondeterministic StrongHash impl for HashMap`
- crate `starlark_lsp` v0.14.2; workspace release **v0.14.0, 2026-06-01**
  (CHANGELOG: "nearly five hundred commits since the last release", `bytes` type, f-string
  expressions, 2x-faster recursive-descent parser, heap lifetime rework).
- Repo is highly active — but the *LSP crate* is not the focus. Its most recent touch in
  the shallow clone is the mechanical `lifetimes: Delete freeze_via_branded` (2026-08-13).

**Verdict:** repo active, LSP crate maintenance-only. Its real customer is Buck2
(`facebook/buck2` `app/buck2_server/src/lsp.rs`, `app/buck2_client/src/commands/lsp.rs`),
not Bazel.

### LSP surface (`starlark_lsp/src/server.rs:415-429`)

```rust
ServerCapabilities {
    text_document_sync:  Some(Kind(TextDocumentSyncKind::FULL)),
    definition_provider,               // gated on LspServerSettings.enable_goto_definition
    completion_provider: Some(CompletionOptions::default()),
    hover_provider:      Some(HoverProviderCapability::Simple(true)),
    ..Default::default()
}
```

Dispatch (`server.rs:1261-1281`): `GotoDefinition`, `Completion`, `HoverRequest`, custom
`StarlarkFileContentsRequest`; notifications `DidOpen`/`DidChange`/`DidClose`; emits
`PublishDiagnostics` and `LogMessage`. `symbols.rs` exists but only feeds completion —
**`textDocument/documentSymbol` is not advertised or handled.** No references, rename,
formatting, code lens, semantic tokens.

The extension point is the `LspContext` trait: `parse_file_with_contents`, `resolve_load`,
`render_as_load`, `resolve_string_literal`, `get_load_contents`, `get_environment`,
`get_url_for_global_symbol`, `get_string_completion_options`. Also notable:
`LspUri::{File, Starlark, Other}` — the `starlark:` scheme is how builtins get a synthetic
"file" to jump to.

### Bazel mode: `starlark --bazel --lsp`

Lives in the binary, not the library: `starlark_bin/bin/bazel.rs` (888 lines) +
`starlark_bin/bin/bazel/label.rs` (280 lines). `main.rs:301` carries the disclaimer
*"TODO: Remove this when extracting the Bazel binary to its own repository, after the
LspContext interface stabilizes."*

- `BazelContext::new` (`bazel.rs:219-262`) runs a bare `bazel info`, reads `execution_root`
  (basename → workspace name, `__main__` → `None`) and `output_base` → `<output_base>/external`.
- `resolve_folder` (`bazel.rs:400-475`) handles: empty repo from workspace root; empty repo
  from inside a known external repo; named repo == workspace; named repo == external dir.
- `get_repository_names` (`bazel.rs:479-499`) is a **`read_dir` of `<output_base>/external`**.
- **No bzlmod repo mapping at all.** `@rules_go` resolves only if a directory literally named
  `rules_go` exists under `external/` — true under WORKSPACE, false under bzlmod where the
  canonical name is `rules_go+` / `rules_go~0.61.1`.
- Builtin docs: `Globals::documentation()` → `starlark:<name>.bzl` URIs (`bazel.rs:211-218`).
  **Core Starlark only** — no Bazel rule classes, no `ctx`, no native rule attributes.

nvim-lspconfig's `lsp/starlark_rust.lua` sums it up: *"The LSP part of starlark-rust is not
currently documented, but the implementation works well for linting… does not support
refactorings."*

---

## 3. cameron-martin/bazel-lsp

- Repo: <https://github.com/cameron-martin/bazel-lsp> · Rust · Apache-2.0 · individual
  (Cameron Martin). ★82, 21 open issues. Created 2024-01-02.
- `git log -1 --format='%H %ad %s'`:
  `48fead628b45af5880a2df8aa8184b0c6ab0f0b9 Sun Jul 20 21:41:36 2025 +0000 chore(deps): update rust crate cc to v1.2.29 (#151)`
- Latest release **v0.6.4, 2025-02-12**. Release history: v0.6.3 2024-11-27, v0.6.2 2024-11-27,
  v0.6.1 2024-06-29, v0.6.0 2024-03-16.

### Liveness verdict: abandoned

`repos/cameron-martin/bazel-lsp.pushed_at` reads 2026-08-21, which is **renovate bot
branches only** — the branch list is `bzlmod`, `master`,
`release-please--branches--master--components--bazel-lsp`, and eleven `renovate/*`
branches. Filtering bots out of `git log`:

```
Sun Jul 13 21:16:21 2025 +0100 | Cameron Martin | ci: Update windows image (#153)
Thu Jun 26 21:50:27 2025 +0100 | Cameron Martin | ci: Update ubuntu runners (#138)
Sat Feb 15 19:09:13 2025 +0000 | Cameron Martin | feat: Add support for logging & tracing (#118)
```

**Last human commit 2025-07-13 (13 months); last feature commit 2025-02-15 (18 months);
last release 2025-02-12.** Eight renovate PRs sit open, the oldest since 2026-06-16.
`Cargo.toml:24-26` pins starlark/starlark_lsp/starlark_syntax to
`git = "https://github.com/facebook/starlark-rust.git", branch = "main"` — a *floating*
git dependency, so the build is not reproducible and will break against the 0.14 heap
lifetime rework.

### LSP surface

Exactly what `starlark_lsp` advertises (§2): **definition, completion, hover, diagnostics**,
plus the custom `starlark/fileContents`. No documentSymbol, references, rename, formatting,
code lens, semantic tokens. The project is a 2,410-LOC `LspContext` implementation
(`src/bazel.rs` 1,300 LOC, `src/builtin.rs`, `src/client.rs`, `src/workspace.rs`,
`src/label.rs`, `src/file_type.rs`).

### Label resolution — the best bzlmod story of any LSP here

`src/client.rs` shells out to: `bazel info` (`client.rs:84`),
`bazel mod dump_repo_mapping <repo>` (`client.rs:121`),
`bazel info build-language` (`client.rs:135`), and `bazel query <module>*`
(`src/bazel.rs:413 query_buildable_targets`) for target completion.

`src/bazel.rs:216-229 repo_mapping_for_file` resolves apparent→canonical per file, and
there is a `ProfilingClient` (`src/client.rs:145-185`) whose whole purpose is asserting how
many times `dump_repo_mapping` gets called. Tests that prove the bzlmod path:
`external_resolve_load_in_bzlmod_workspace` (`bazel.rs:859`, uses a
`rules_rust~0.36.2` external dir),
`test_completion_for_repositories_in_root_workspace_with_bzlmod` (`bazel.rs:896`),
`test_completion_for_packages_in_root_workspace_with_bzlmod` (`bazel.rs:932`).
External repos live at `<output_base>/external/<name>` (`src/workspace.rs:43,61,81,91`).

Goto-definition on a label lands on the **file**, not on the rule call inside a BUILD file
(`resolve_string_literal` / `resolve_load` return a URI plus an optional
`location_finder`; the Bazel context supplies a `find_call_name` finder for `load()` symbols).

### Where builtin docs come from

- Checked-in `src/builtin/builtin.pbtxt` (**521,414 bytes**) and
  `src/builtin/default_build_language.pbtxt` (**516,153 bytes**), converted pbtxt→pb by
  `src/builtin/pbtxt_to_pb.bzl` and embedded with `include_bytes!(env!("BUILTIN_PB"))` /
  `env!("DEFAULT_BUILD_LANGUAGE_PB")` (`src/bazel.rs:441-455`).
- Live `bazel info build-language` overrides the checked-in build-language when available.
- HTML in Bazel's docs is converted to Markdown with the `htmd` crate (the v0.6.4 fix,
  "convert html to markdown in docs (#66)").
- `src/builtin.rs:13-50` `MISSING_GLOBALS` — a hand-maintained list of ~25 globals absent
  from `builtin.pb`: all of the WORKSPACE globals (`bind`, `register_toolchains`, …), all
  of the MODULE globals (`bazel_dep`, `git_override`, `use_extension`, `use_repo`,
  `override_repo`, `inject_repo`, …), plus `module_extension`, `repository_rule`,
  `tag_class`, `exec_transition`, `package`, `repo_name`, `licenses`, `environment_group`,
  `distribs`.
- `src/bazel.rs:447`: *"TODO: builtins are also dependent on bazel version, but there is no
  way to obtain those, see bazel-contrib/vscode-bazel#1."*

### Known limitations

README lists the complete feature set as: *"Go to definition for identifiers & labels /
Autocomplete for identifiers & labels / Auto-import (currently only for open files)."*
Fixed lint bugs in 0.6.3: "Fix linting of global symbols (#51)", "Remove misplaced-load
lints from WORKSPACE files (#61)" — i.e. starlark-rust's linter fires false positives on
Bazel dialects and has to be muzzled case by case.

---

## 4. tilt-dev/starlark-lsp

- Repo: <https://github.com/tilt-dev/starlark-lsp> · Go · Apache-2.0 · Tilt (Docker). ★35, 6 open issues.
- `git log -1 --format='%H %ad %s'`:
  `5689e7e8a3aa8ab55eca07d215054a0f25dbc17c Tue Jul 30 17:15:32 2024 -0400 circleci: update golang (#58)`
- **Never made a GitHub release** (`releases/latest` → 404). `pushed_at` 2025-08-28 is the
  dependabot branch `dependabot/go_modules/gopkg.in/yaml.v3-3.0.1`.

**Verdict: abandoned.** 25 months since the last commit to `main`. Open issue **#62
(2026-07-26) "Add configurable load paths for load() resolution"** has no maintainer reply.
Older asks #43 "Code outline support" and #42 "Hover support for custom symbols" are open
since 2022-09.

Still relevant because it is bundled in Tilt as `tilt lsp` and drives the Tiltfile VS Code
extension; Helix wires it as the `tilt` language server; the Zed Starlark extension exposes
it as a `tilt` server option.

### LSP surface (`pkg/server/initialize.go:18-36`)

```go
Capabilities: protocol.ServerCapabilities{
    TextDocumentSync: protocol.TextDocumentSyncOptions{
        Change: FULL, OpenClose: true, Save: &SaveOptions{IncludeText: true}},
    SignatureHelpProvider:  &SignatureHelpOptions{Trigger: "(", Retrigger: ",", "="},
    DocumentSymbolProvider: true,
    CompletionProvider:     &CompletionOptions{TriggerCharacters: []string{"."}},
    HoverProvider:          true,
    DefinitionProvider:     true,
},
```

Handlers (`pkg/server/*.go`): `Initialize`, `Shutdown`, `Exit`, `Completion`, `Hover`,
`SignatureHelp`, `DocumentSymbol`, `DidOpen`, `DidChange`, `DidSave`, `DidClose`,
`publishDiagnostics`.

### Bazel awareness: none

`grep -rn "bazel\|BUILD" --include=*.go pkg/` returns nothing outside test fixtures. There
is no label parser, no `bazel` invocation, no external-repo concept.
`pkg/analysis/definition.go` only jumps to a symbol's already-recorded `Location`;
`load()` resolution is `document.Manager.Resolve` on plain filesystem paths with a
circular-load guard (`pkg/document/manager.go:76-220`).

Builtins come from `pkg/analysis/builtins.py` (`//go:embed`) — hand-written Python type
stubs — extendable per-invocation with `--builtin-paths file.py|dir` where a directory is
treated as Python modules. Generators live in `hack/starlark-builtins.{py,go}`.

Parser: `smacker/go-tree-sitter` (cgo, pinned to a 2022 commit) — which is precisely the
complaint that motivated starlsp (§12).

---

## 5. bazel-contrib/vscode-bazel (was bazelbuild/vscode-bazel)

- Repo: <https://github.com/bazel-contrib/vscode-bazel> · TypeScript · Apache-2.0 · bazel-contrib.
  ★294, 88 open issues. Marketplace `BazelBuild.vscode-bazel`.
- `git log -1 --format='%H %ad %s'`:
  `84484e6bea24d483bfc5c2ab0035a9f05e7f7d35 Wed Aug 19 13:48:52 2026 -0400 fix: silence workspace error in non-Bazel repos (#638)`
- Latest release **v0.14.0, 2026-03-31**. **Verdict: active.**

### It is a *client*, not a server — and it does not bundle starpls

`src/language_support/language_support_feature.ts:39-48` picks one of two mutually
exclusive branches at activation:

```ts
const lspCommand = getLspServerExecutablePath();
this.isUsingExternalLSP = !!lspCommand;
if (this.isUsingExternalLSP) return this.enableExternalLSP(context);
else                        return this.enableBuiltInSupport(context);
```

- **External LSP branch** (`src/language_support/language-server-client.ts`): spawns
  `bazel.lsp.command` with `bazel.lsp.args` and `bazel.lsp.env` over stdio;
  `documentSelector: [{scheme:"file", language:"starlark"}]`; `vscode-languageclient ^10.0.0`;
  command `bazel.lsp.restart`; context key `bazel.lsp.enabled`.
- **Built-in branch**: registers three providers, **all glob-restricted to `**/BUILD` and
  `**/BUILD.bazel`** (`language_support_feature.ts:126-146`) — `.bzl` and `MODULE.bazel`
  get nothing:
  - `BazelCompletionItemProvider` (triggers `/`, `:`) — label completion from a cached
    `bazel query`, refreshed by a `**/{BUILD,BUILD.bazel}` file watcher.
  - `BazelTargetSymbolProvider` — document symbols by running `bazel query` on the package
    and reading `rule.location` out of the `blaze_query` proto.
  - `BazelGotoDefinitionProvider` — `LABEL_REGEX = /"((?:@\w+)?\/\/|(?:.+\/)?[^:"]*(?::[^:"]+)?)"/`
    then `bazel query 'kind(rule, "<label>") + kind(file, "<label>")'`.
    **One `bazel query` subprocess per navigation.**

Everything else is unconditional and unrelated to the LSP:
`CodeLensFeature` (build/run/test/debug lenses over `bazel query` results, registered on
`**/BUILD` + `**/BUILD.bazel`), `BuildifierFeature` =
`registerDocumentFormattingEditProvider` + `BuildifierDiagnosticsManager`
(buildifier `--lint=warn --format=json`; globs `**/BUILD`, `**/*.bazel`, `**/WORKSPACE`,
`**/*.BUILD`, `**/*.bzl`, `**/*.sky` — i.e. **buildifier covers far more file types than
the completion/definition/symbol providers do**, and it handles MODULE.bazel since 0.13.0),
Starlark debug adapter, test explorer + lcov parser, workspace tree,
`bazel.goToLabel` / `goToBuildFile` / `copyLabelToClipboard`, URI handler.

README §"Using a language server (experimental)" names **bazel-lsp** and **starpls** and
tells the user to install a binary themselves. Issue **#371 "Auto-install tools" is open
since 2026-04-01**; starpls issue #400 is the mirror of it.

Historical marker: **issue #1, "Implement language server for Starlark", opened
2018-09-11, is still open on 2026-08-25.**

Label resolution is entirely delegated to `bazel query`, so it is trivially correct about
bzlmod and external repos — at the cost of a subprocess per request and total dependence
on the workspace being loadable.

---

## 6. stackb/bazel-stack-vscode and stackb/bzl

- `stackb/bazel-stack-vscode` · TypeScript · Apache-2.0 (`LICENSE.md`, "Copyright 2021
  Stack.Build LLC"; GitHub reports NOASSERTION) · ★70, 21 open issues.
  `git log -1 --format='%H %ad %s'`:
  `a7812c1931e5b885600512df071b0027037987b4 Sun Aug 6 19:19:51 2023 -0600 Prepare v1.9.8 (#129)`
  Latest GitHub release **1.8.4, 2022-04-21**.
- `stackb/bzl` · ★6 · `pushed_at` **2021-06-11** · 0 open issues · homepage <https://bzl.io>.

**Verdict: abandoned.** Three years since the last extension commit; five years since the
`bzl` repo was touched. `bzl.io`, `docs.stack.build` and the Marketplace listing all still
return 200. Open issue **#132 "failed to download bzl" (2026-06-17)** is unanswered.

### The server is closed source

`src/bezel/configuration.ts:255` — the extension downloads a Go binary from
`https://get.bzl.io` (`downloadBaseUrl`), plus an accounts service at
`grpcs://accounts.bzl.io:443`. The GitHub repo `stackb/bzl` is a distribution stub; **the
language server source has never been published**, so the exact LSP method set cannot be
verified. What can be verified is the client contract
(`src/bezel/lsp.ts:307-352`):

```ts
new LanguageClient('starlark', 'Starlark Language Server',
  { command: cfg.executable, args: cfg.command },
  { documentSelector: [{language:'starlark'},{language:'bazel'}],
    synchronize: { fileEvents: createFileSystemWatcher('**/BUILD.bazel') },
    progressOnInitialization: true,
    initializationOptions: { enableCodelenses, enableCodelensCopyLabel,
      enableCodelensCodesearch, enableCodelensBrowse, enableCodelensStarlarkDebug,
      enableCodelensBuild, enableCodelensTest, enableCodelensRun } })
```

plus a custom request shape `BuildFileLabelLocationParams { textDocument, label }`.

README-documented server features: code actions/lenses for `[LABEL]`, `build`, `test`,
`run`, `debug`, `codesearch`, `ui`; hover over rules/providers/aspects/attributes/functions;
completion for core Starlark and Bazel builtins; jump-to-definition on any label-shaped
string literal, "works with default and external workspaces". Crucially:
**"completion for third-party and custom starlark rules is available on a subscription
basis"** — the interesting half is paywalled.

Non-LSP parts of the extension: bazelrc hover + completion (`src/bazelrc/flags.ts`),
buildifier formatter (`src/buildifier/formatter.ts`), buildozer wizard, `bazeldoc` hover,
Starlark DAP debugger, LRU remote cache, codesearch UI.

nvim-lspconfig still ships `lsp/bzl.lua` → `cmd = { 'bzl', 'lsp', 'serve' }`.

---

## 7. JetBrains/hirschgarten — the Bazel plugin for IntelliJ IDEA

- Repo: <https://github.com/JetBrains/hirschgarten> · Kotlin · Apache-2.0 · JetBrains. ★148.
  Issues live in YouTrack project `BAZEL` (only 9 open on GitHub).
- `git log -1 --format='%H %ad %s'`:
  `f2724ec21d802ef8ab0b1faa4df102c6c131ca41 Mon Aug 24 18:10:34 2026 +0200 BAZEL-3342: Go strict dependencies inspection & quick-fix`
- Default branch is `262`; branches `252`, `253`, `261`, `262` track IDE release trains.
  No GitHub releases; ships to JetBrains Marketplace plugin **22977 "Bazel"**, vendor
  JetBrains, **1,613,351 downloads**, latest build **2026.2.1.1 published 2026-08-06**
  (also 2026.1.5 and 2026.2.1 on 2026-07-27).

**Verdict: by far the most actively developed Bazel-file language intelligence that
exists.** Daily commits from a funded team.

**It is not an LSP.** It is a native IntelliJ PSI language implementation; nothing is
reusable over the wire. (It speaks BSP northbound to `intellij.bazel.backend` for the
project model — out of scope per the brief.) It is nonetheless the correct feature
benchmark.

### Where the Starlark support lives

`intellij.bazel.core/src/org/jetbrains/bazel/languages/starlark/` — **259 Kotlin files**,
subpackages: `lexer parser psi elements references completion documentation inspection
quickFixes findusages rename folding formatting highlighting indentation matching globbing
index annotation repomapping utils actions bazel`. Registered in
`intellij.bazel.core/resources/intellij.bazel.core.xml`. Sibling languages in the same
plugin: **`bazelrc`, `bazelquery` (+ query flags), `bazelversion` (`.bazelversion`),
`projectview` (`.bazelproject`)**.

### References / resolve

`languages/starlark/references/`:

| File | Role |
|---|---|
| `BazelLabelReference.kt` | resolves any `StarlarkStringLiteralExpression` that parses as a `Label`; `acceptOnlyFileTarget` inside `load()`; implements `getVariants()` (file / target / load-filename / `use_extension` completion), `bindToElement()` (rewrites labels when a file moves) and `handleElementRename()` |
| `StarlarkLoadReference.kt` | resolves the *symbol* inside the loaded `.bzl` PSI file via `SearchUtils.searchInFile` |
| `BazelGlobalFunctionReference.kt`, `BazelGlobalFunctionArgumentReference.kt` | rule/attribute name → bundled signature |
| `StarlarkGlobReference.kt` | `glob()` patterns → matched files |
| `StarlarkVisibilityReference.kt`, `StarlarkNamedArgumentReference.kt`, `StarlarkArgumentReference.kt`, `StarlarkLocalVariableReference.kt`, `StarlarkQualifiedReferenceExpressionReference.kt`, `StarlarkFunctionCallReference.kt` | the rest |
| `LabelResolveUtils.kt`, `SearchUtils.kt` | shared |

Backed by real indices: `index/StarlarkLoadsIndexExtension` and
`index/StarlarkLoadEdgesIndexExtension` — a persisted **load graph**.

### Repo mapping / bzlmod

`languages/starlark/repomapping/BazelRepoMappingSyncHook.kt` is a `ProjectSyncHook` that
copies `BzlmodRepoMapping.apparentRepoNameToCanonicalName` and
`.canonicalRepoNameToPath` into a `PersistentStateComponent`;
`project.canonicalRepoNameToPath` is what `BazelLabelReference` consults, and
`Label.toShortString(project)` renders canonical→apparent for display. **Fully bzlmod- and
external-repo-aware, but only after a project sync** — no `bazel` subprocess per keystroke,
which is the opposite trade from vscode-bazel and starpls.

Beyond that: `languages/starlark/bazel/bzlmod/` contains `BazelModuleRegistryService`,
`resolver/BazelCentralRegistryModuleResolver`, `resolver/BazelModuleBcrCacheService`,
`RefreshBcrModulesAction`, and `completion/BazelDepCompletionContributor` — which completes
**`bazel_dep(name = …, version = …)` directly out of the Bazel Central Registry**. Nothing
else in this survey does this.

### Where builtin/rule docs come from

`languages/starlark/bazel/`, both feeding the
`org.jetbrains.bazel.starlarkGlobalFunctionProvider` extension point (so third parties can
add rulesets):

- `BazelBuiltinFunctionProvider` reads
  `intellij.bazel.core/resources/bazelSignatures/builtins/builtins@{version}.json`.
  **Fourteen Bazel versions are shipped**: 7.5.0, 7.6.0, 7.7.0, 8.0.0, 8.1.0, 8.2.0, 8.3.0,
  8.4.0, 8.4.1, 8.5.0, 8.6.0, 9.0.0, 9.0.1, 9.1.0. It selects the closest version ≤ the
  project's Bazel version (from `BazelProjectContextService`, falling back to
  `.bazelversion` before the first sync, else the newest). Generated offline by
  `//plugins/bazel/tools/bazel_signatures/builtins:generator -- --version $version`.
- `BazelStardocFunctionProvider` reads `resources/bazelSignatures/rules_kotlin@v2.4.0.json`
  and `rules_go@v0.61.1.json`, generated offline by
  `//plugins/bazel/tools/bazel_signatures/stardoc:generator --repo-url … --ref … --bzl-file … --dep …`
  i.e. **real stardoc extraction from real rulesets, pinned per version**.
- Model: `BazelGlobalFunction(name, doc, environment: List<Environment{BZL,BUILD,MODULE,REPO,VENDOR}>, params, returnType)`.

### The rest of the feature set

- **13 inspections** (`languages/starlark/inspection/`): BindingConflict,
  ControlFlowContext, FunctionCallValidation, FunctionDeclaration,
  FunctionParameterValidation, InvalidAssignmentTarget, LoadCycle, LoadParameters,
  LoadPlacement, LoadPrivateSymbol, Recursion, StatementContainerPlacement,
  VariableUsage — plus annotators (Declaration / Function / String / Load / Glob).
- **Formatting**: `formatting/StarlarkFormattingService.kt`, an
  `AsyncDocumentFormattingService` that shells out to **buildifier** (path from
  `bazelProjectSettings.getBuildifierPathString`), plus `StarlarkFormattingActionOnSave`.
- **Find usages**: `findusages/` (9 files, incl. `BazelTargetReferenceSearcher`,
  `StarlarkStringUsageSearcher`, `StarlarkUseScopeOptimizer`, `StarlarkFileUseScopeEnlarger`).
- **Rename**: `rename/` — `BazelTargetRenameInputValidator`,
  `StarlarkRenamePsiElementProcessor`, `StarlarkStringLiteralManipulator` →
  renaming a target rewrites every referring label.
- **Workspace symbols**: `ui/widgets/LabelSearchEverywhereContributor$Factory` +
  `target/TargetStorage.kt` (`targetsForPath`).
- **Code-lens equivalent**: `ui/gutters/StarlarkRunLineMarkerContributor`.
- **Documentation**: `documentation/` — `BazelGlobalFunctionDocumentationTarget`,
  `BazelGlobalFunctionArgumentDocumentationTarget`, `BazelTargetDocumentationProvider`.
- Folding, brace matching, quote handling, commenter, colour settings page,
  `StarlarkCompletionConfidence`, glob evaluation (`globbing/`), `srcs` list evaluation
  (`utils/StarlarkSrcsListEval.kt`).

### Fate of the old bazelbuild/intellij plugin

`bazelbuild/intellij` README, first line: *"As of July 1, 2025, the Bazel for IntelliJ and
Bazel for CLion plugins are maintained by JetBrains. These plugins are not offered by nor
affiliated with Google."* The repo's GitHub description is now literally
**"CLion plugin for Bazel projects"**. ★821, 227 open issues; HEAD
`ce93830dff25 2026-08-14 Update intellij_aspect_sdk digest`; latest release tag
`2026.08.13.0`, 2026-08-13.

- `master` = Bazel for CLion (actively released).
- `ijwb` branch = Bazel for IntelliJ — *"maintenance for the IntelliJ plugin is limited to
  ensuring compatibility with new versions of JetBrains IDEs. No new features or bug fixes
  will be provided."*
- Android Studio's ASwB moved to AOSP; the `google`/`aswb` branches are deprecated/stale;
  *"Due to multiple regressions caused by picks from the AOSP, cherry-picks have been
  halted for the time being."*

Its BUILD-file language still exists at
`base/src/com/google/idea/blaze/base/lang/buildfile/{completion,documentation,editor,findusages,formatting,globbing,highlighting,language,lexer,livetemplates,parser,psi,quickfix,refactor,references,search,stubs,sync,validation,views}`
with references `LabelReference`, `TargetReference`, `PackageReferenceFragment`,
`ExternalWorkspaceReferenceFragment`, `LoadedSymbolReference`, `GlobReference`,
`KeywordArgumentReference`, `VisibilityReference`, `FuncallReference`, `ArgumentReference`,
`AttributeSpecificStringLiteralReferenceProvider`, `IncludeReference`, `LabelUtils`,
`BuildReferenceManager`. This is the original Google implementation and is a worthwhile
prior-art read, but it is feature-frozen for IntelliJ and superseded by hirschgarten.

---

## 8. bazel-contrib/buildtools (buildifier / buildozer)

- Repo: <https://github.com/bazel-contrib/buildtools> (redirect from `bazelbuild/buildtools`)
  · Go · Apache-2.0. ★1189, 121 open issues.
- `git log -1 --format='%H %ad %s'`:
  `674b29349bf1d98d19b5a0e611bba704bb67bc0f Mon Aug 24 09:36:17 2026 -0700 perf: preallocate sortStringExprs chunk slice size (#1475)`
- Latest release **v8.5.1, 2026-01-30** (matches the nixpkgs pin in the brief). Prior:
  v8.2.1 2025-06-10, v8.2.0 2025-04-30. **Verdict: active.**

**No LSP surface whatsoever.**
`grep -rln "lsp\|languageserver\|jsonrpc" --include=*.go .` → no matches. There is no
server mode, no JSON-RPC, no `textDocument/`.

What it is:

- `buildifier` — formatter + linter. **194 warning categories** catalogued in
  `WARNINGS.md` (`allowed-symbol-load-locations`, `attr-cfg`, `bzl-visibility`,
  `canonical-repository`, `constant-glob`, `depset-iteration`, `duplicated-name`,
  `external-path`, `function-docstring-*`, `load-on-top`, `native-*`, `unsorted-dict-items`,
  `unused-variable`, …). CLI: `--lint=warn|fix`, `--warnings=+a,-b`, `--mode=check`,
  `--format=json`, `--type=build|bzl|workspace|module|default`.
- `buildozer` — scripted BUILD rewriting (`new`, `add`, `remove`, `set`, `rename`, `copy`,
  `move`, `print`, …) addressed by label, including `//WORKSPACE:all`-style pseudo-labels.
- `unused_deps`, plus Go libraries `build` (parser + printer with comment fidelity),
  `edit`, `labels`, `tables`, `warn`, `bzlenv`, `differ`, `wspace`.

Its real role here: it is the **formatting and lint backend every other tool shells out
to** — vscode-bazel (`BuildifierFeature`), hirschgarten (`StarlarkFormattingService`),
bazel.el (`bazel-buildifier`, `bazel-mode-flymake`), Helix (`formatter = {command = "buildifier"}`),
and the unimplemented starpls issue #267. The `build` + `edit` + `labels` packages are the
only mature, comment-preserving BUILD-file surgery libraries in existence.

---

## 9. Bazel itself, stardoc, and the doc pipeline

- **`bazel info build-language`** — the runtime source of native rule classes and their
  attributes (`blaze_query.BuildLanguage` proto,
  `crates/starpls_bazel/data/build.proto` / `bazel-lsp/prost`). Every serious tool calls it.
- **`//src/main/java/com/google/devtools/build/lib:builtin.pb`** —
  `src/main/java/com/google/devtools/build/lib/BUILD:305`, a genrule that runs
  `docgen:api_exporter` over `BazelRuleClassProvider` plus the Build Encyclopedia stardoc
  protos (`gen_be_{proto,java,cpp,objc,python,shell}_stardoc_proto`) and `docs_embedded_in_sources`.
  This one artifact is the upstream of both starpls's and bazel-lsp's checked-in blobs.
  It is **not published as a release artifact** — each tool has to build Bazel or scrape it,
  which is exactly why both copies are stale.
- Bazel ships **no** LSP:
  `grep -rl "languageserver\|textDocument/" --include=*.java src/main/java` → nothing.
  It does ship the Starlark **debug** protocol
  (`src/main/protobuf/starlark_debugging.proto`, `--experimental_skylark_debug`), which
  vscode-bazel and bazel-stack-vscode both drive.
- **bazelbuild/stardoc** · Apache-2.0 · HEAD `bd575db06a38 2026-06-23`, release **0.8.1
  2026-01-28**. Now just a Velocity template over `native.starlark_doc_extract` output.
  Key line in its README: *"Modules published to the Bazel Central Registry do not need to
  use Stardoc. They can simply publish the `starlark_doc_extract` outputs as a release
  artifact"* (see `bazel-central-registry/docs/stardoc.md`). **There is therefore a
  standard, machine-readable, per-ruleset documentation artifact reachable from the BCR** —
  hirschgarten already exploits this offline; no LSP does.
- **bazelbuild/starlark** (the spec) · ★3074 · HEAD `c0af0ea03dc9 2026-02-06 Remove invalid example (#325)` · 96 open issues. Slow but not dead.
- **google/starlark-go** · BSD-3-Clause · ★2752 · HEAD `5395d018f003 2026-07-08 syntax: reject excessively nested bracketed expressions (#644)`. No LSP; it is the `syntax`/`resolve` substrate under starlsp and Tilt-adjacent tooling.
- **tree-sitter-grammars/tree-sitter-starlark** · HEAD `a453dbf3ba43 2024-12-04 (tag 1.3.0)` — **20 months stale**. Consumed by Zed (pinned commit `b31a616a`) and Neovim.

---

## 10. Editor integrations

### Emacs — bazel-contrib/bazel.el (was bazelbuild/emacs-bazel-mode)

- Apache-2.0 · ★92 · 12 open issues · release **v0.0.3, 2026-08-05** ·
  `git log -1`: `0a5dec6508afdbcc79d98300717c5be530f12497 Mon Aug 10 03:00:51 2026 +0200 Convert external repository root to directory name`.
  **Verdict: active.**
- **No LSP.** It hooks native Emacs facilities from `bazel-mode` (`bazel.el:381-401`):
  - `flymake-diagnostic-functions` → `bazel-mode-flymake`, which runs
    `bazel-buildifier-command` (`buildifier`) in check mode and parses its JSON output
    (`bazel.el:756-890`).
  - `xref-backend-functions` → `bazel-mode-xref-backend`;
    `xref-backend-definitions` resolves `//pkg:target` and `@repo//pkg:target` by path
    arithmetic (`bazel--canonical`, `bazel--external-repository`), **not** `bazel query`.
  - `completion-at-point-functions` → `bazel-completion-at-point` — label completion inside
    string literals, deliberately filesystem-based and `non-essential`-aware so idle
    (company) completion doesn't stat remote files (`bazel.el:1100-1140`).
  - `imenu-create-index-function` → `bazel-mode-create-index` (document symbols).
  - `bazel-buildifier` (`C-c C-f`) formats; `bazel-buildifier-before-save` for format-on-save;
    per-mode `--type` selection (`build`/`workspace`/`module`/`bzl`/`default`).
  - `project.el` integration: `project-external-roots` → `bazel--external-repository-roots`.
- **Widest file-type coverage of anything surveyed** — derived modes:
  `bazel-build-mode`, `bazel-workspace-mode` (`WORKSPACE`, `WORKSPACE.bazel`,
  `WORKSPACE.bzlmod`), `bazel-module-mode` (`MODULE.bazel`, `*.MODULE.bazel`),
  `bazel-repo-mode` (`REPO.bazel`), `bazel-vendor-mode` (`VENDOR.bazel`),
  `bazel-starlark-mode` (`.bzl`), `bazelrc-mode`, `bazelignore-mode`, `bazeliskrc-mode`,
  `bazelproject-mode`, and `MODULE.bazel.lock` → `js-json-mode`.
- External repos are found via the `bazel-out` symlink →
  `$(readlink ROOT/bazel-out)/../../../external/REPOSITORY` (`bazel.el:2198-2231`), i.e.
  **canonical directory names only — no `bazel mod dump_repo_mapping`, so apparent
  bzlmod repo names do not resolve.**
- Neither **lsp-mode** nor **Eglot** ships a Starlark/Bazel client:
  `repos/emacs-lsp/lsp-mode/contents/clients` has 141 files, none matching `star|bazel|bzl`.

### Neovim

`neovim/nvim-lspconfig` ships four relevant configs under `lsp/`:

| File | `cmd` | filetypes | root markers |
|---|---|---|---|
| `starpls.lua` | `starpls` | `bzl` | `WORKSPACE`, `WORKSPACE.bazel`, `MODULE.bazel` |
| `starlark_rust.lua` | `starlark --lsp` | `star`, `bzl`, `BUILD.bazel` | `.git` |
| `bzl.lua` | `bzl lsp serve` | `bzl` | `WORKSPACE`, `WORKSPACE.bazel` |
| `bazelrc_lsp.lua` | `bazelrc-lsp lsp` | `bazelrc` (must register the filetype yourself) | `WORKSPACE`, `WORKSPACE.bazel`, `MODULE.bazel` |

`lsp/starpls.lua` last touched 2025-08-20. **There is no nvim-lspconfig entry for
cameron-martin/bazel-lsp or tilt-dev/starlark-lsp.** Note `filetypes = {'bzl'}` for
starpls — Neovim's built-in `filetype.lua` maps `BUILD`/`BUILD.bazel`/`WORKSPACE`/
`MODULE.bazel`/`*.bzl` to `bzl`, so this does cover BUILD files.

### Zed — zaucy/zed-starlark

MIT · ★33 · `git log -1`: `7c04309e 2026-03-01 chore: use zed maintainers preferred pr title` · v0.4.1.
`extension.toml` declares one language (`Starlark`) with **three** servers:
`starpls`, `buck2-lsp`, `tilt`. `src/starpls.rs` calls
`zed::latest_github_release("withered-magic/starpls", {pre_release:false})` and downloads
`starpls-{os}-{arch}` — so Zed users are pinned to whatever the last starpls release is,
today **v0.1.22 (2025-08-30)**, which is 3 months *behind* even starpls's own `main`.
Grammars: `tree-sitter-grammars/tree-sitter-starlark` @ `b31a616a`, plus
`zaucy/tree-sitter-bazelrc`.

### Helix

`languages.toml` (master, fetched 2026-08-25):

- `[language-server.starpls] command = "starpls"` (line 151)
- the `starlark` language: `file-types = ["bzl","bazel","star","bxl", {glob="BUILD"},
  {glob="BUCK"}, {glob="BUILD.*"}, {glob="WORKSPACE"}, {glob="WORKSPACE.bzlmod"},
  {glob="PACKAGE"}]`, `language-servers = ["starpls", "buck2"]`, and
  **`grammar = "python"`** — Helix does not use the Starlark tree-sitter grammar at all.
  Note the absence of `MODULE.bazel` from the glob list.
- `bazelrc` is merely a file-type of the `bash` language — **no `bazelrc-lsp` wiring**.
- `tilt` language → `language-servers = ["tilt"]`, `formatter = {command = "buildifier"}`,
  `auto-format = true`.

### Sublime Text

Nothing. No LSP-* package for Bazel/Starlark exists on Package Control. Only syntax and
shell-out formatters: `niosus/BazelSyntax` (2021-02-06), `mfilippov/sublime-buildifier`
(2026-03-09), `heethesh/BazelFormatter` (2022-09-21),
`abergmeier/sublime-bazel-buildtools` (2017-06-04).

---

## 11. salesforce-misc/bazelrc-lsp

- Repo: <https://github.com/salesforce-misc/bazelrc-lsp> · Rust · Apache-2.0 · Salesforce
  (a `-misc` org — i.e. not a supported product). ★30, 5 open issues. Created 2024-06-10.
- `git log -1`: `2a971b8532 2026-06-02 Upload required SECURITY.md file for compliance`;
  the last *functional* commit is `7b49d038d9 2026-02-06 Bump to version 0.2.6`.
  Latest release **v0.2.6, 2026-02-06**. **Verdict: maintenance-only / functionally frozen.**
- Scope: **`.bazelrc` only**. Directly in scope per the brief's file list, and the only
  tool that treats bazelrc as a first-class language other than hirschgarten and
  bazel-stack-vscode.
- Capabilities (`src/language_server.rs:133-169`): `semantic_tokens_provider`,
  `completion_provider`, `hover_provider`, `document_formatting_provider`,
  `document_range_formatting_provider`, `document_link_provider`, `definition_provider`,
  plus `publishDiagnostics`.
- Flag metadata: pre-generated flag dumps for a range of Bazel versions are baked in
  (`build.rs`); the version is auto-detected using **Bazelisk's algorithm**
  (`USE_BAZEL_VERSION`, `.bazeliskrc`, `.bazelversion`), or it can invoke Bazel live via
  `BAZELRC_LSP_RUN_BAZEL_PATH` → `bazel help flags-as-proto`.
- Diagnostics: unknown flags, abbreviated flag names, deprecated flags, missing `import`ed
  files, `config` on `startup`/`import`/`try-import`, empty/invalid config names, custom
  `--//my/package:setting` flags.
- Documented gaps (README backlog): rename for config names, find-references for config
  names/flags, config-name completion, flag-value completion, fix-its for
  abbreviated/deprecated/repeated flags, default-value display (blocked on
  bazelbuild/bazel#25169).
- Strategic note, verbatim: *"Long-term, I am considering to integrate this functionality
  into the official VSCode Bazel extension. This is also why this extension is not
  published to the VS Code Marketplace as a standalone extension."*

---

## 12. M31-Labs/starlsp (new, 2026-08-16)

- Repo: <https://github.com/M31-Labs/starlsp> · Go · Apache-2.0 · created **2026-08-16**,
  last commit **2026-08-16** (`68d1ee968b add(build): add build tooling and version
  consistency checks`), ★1, 0 issues, version 0.3.0. `go install github.com/M31-Labs/starlsp/cmd/starlsp@latest`.
- **Verdict: nine days old, single-author burst, entirely unproven.** The README reads as
  marketing copy (benchmark tables, "21,740 positional requests", "Six become fourteen").
  Treat all of its claims as unverified. Included because it is the only *new* entrant and
  because its capability list is genuinely the broadest.
- Capabilities (`server.go:252-265`): completion (`.`, `"`, `'`), hover, signatureHelp,
  definition, **references**, **documentHighlight**, **rename + prepareRename**,
  documentSymbol, **workspaceSymbol**, **foldingRange**, **selectionRange**,
  **documentLink**, **semanticTokens**. Fourteen, versus starpls's seven.
- Design: two parsers — `gotreesitter` (**pure Go, no cgo**) for the error-tolerant tree
  driving outline/folding/semantic tokens/completion context, and `go.starlark.net`
  `syntax`+`resolve` for correctness (diagnostics, binding, scope). Dialects supply
  `syntax.FileOptions` so Tiltfiles' top-level `if`/`for` don't produce false errors.
  Also runs as a linter: `starlsp --dialect tilt --check Tiltfile`.
- **Bazel awareness is cosmetic.** `dialects/bazel/bazel.go` is 239 lines of
  **hard-coded global signature strings** (`{"repository_rule", "repository_rule(implementation, attrs=None, …)", "callable", "Define an external repository rule."}`).
  No `bazel` subprocess, no `output_base`, no external repo directory, no repo mapping, no
  label parser, no BUILD-target resolution — `Host.ResolveLoad(req) (path string, ok bool)`
  is a path mapper. Its references/rename/workspace-symbols therefore operate on **Starlark
  identifiers, not Bazel labels**.
- Its README's stated reason for existing is a fair critique of tilt-dev/starlark-lsp:
  cgo tree-sitter pinned to 2022, full-document sync, hand-maintained Python stubs,
  fixed dialect, last code commit July 2024.

---

## 13. Feature matrix

Rows are implementations; columns are the features from the brief.
`yes` / `partial` / `no`; every `partial` carries a numbered footnote.

| | goto-def (load) | goto-def (label) | completion (attrs) | completion (labels) | hover (rule docs) | find-refs | rename | workspace symbols | diagnostics | formatting | code lens | bzlmod-aware | external-repo-aware | type inference |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **starpls** | yes | yes | yes | partial [1] | yes | partial [2] | no | no | partial [3] | no | no | yes | yes | yes |
| **starlark-rust `starlark_lsp`** (generic ctx) | partial [4] | no | no | no | partial [5] | no | no | no | yes | no | no | no | no | partial [6] |
| **starlark-rust `--bazel --lsp`** | yes | partial [7] | no | partial [8] | no | no | no | no | yes | no | no | **no** [9] | partial [10] | partial [6] |
| **cameron-martin/bazel-lsp** | yes | partial [7] | yes | yes | yes | no | no | no | partial [11] | no | no | yes | yes | partial [6] |
| **tilt-dev/starlark-lsp** | partial [12] | no | partial [13] | no | partial [13] | no | no | no | partial [14] | no | no | no | no | partial [15] |
| **M31-Labs/starlsp** | partial [16] | no | partial [17] | no | partial [17] | partial [18] | partial [18] | partial [18] | yes | no | no | no | no | partial [19] |
| **vscode-bazel** (built-in providers) | no [20] | yes | no | yes | no | no | no | partial [21] | yes | yes | yes | yes [22] | yes [22] | no |
| **stackb `bzl` LSP** (closed source) | yes | yes | partial [23] | partial [23] | yes | no | no | no | partial [24] | yes [24] | yes | no [25] | partial [25] | no |
| **JetBrains/hirschgarten** (not LSP) | yes | yes | yes | yes | yes | yes | yes | yes | yes | yes | yes [26] | yes | yes [27] | partial [28] |
| **bazelbuild/intellij** `ijwb` (legacy, not LSP) | yes | yes | yes | yes | partial [29] | yes | yes | yes | yes | yes | yes | partial [30] | yes | no |
| **bazel-contrib/bazel.el** (not LSP) | partial [31] | yes | no | yes | no | no | no | partial [32] | yes | yes | no | **no** [33] | partial [33] | no |
| **salesforce-misc/bazelrc-lsp** (`.bazelrc` only) | n/a [34] | n/a [34] | n/a [34] | n/a [34] | n/a [34] | no | no | no | yes | yes | no | n/a [34] | n/a [34] | n/a [34] |
| **bazel-contrib/buildtools** (CLI only) | no | no | no | no | no | no | no [35] | no | yes [36] | yes | no | no | no | no |

### Footnotes

1. **starpls, completion(labels)** — package/target path completion works always, by
   `read_dir` of the package directory (`document.rs:520-660`). A full list of workspace
   targets is only available behind `--experimental_enable_label_completions`, which runs
   one `bazel query "kind('.* rule', ...)"` at startup and again on refresh
   (`server.rs:104-140, 332-350`). Macro-generated targets are invisible without it.
2. **starpls, find-refs** — `crates/starpls_ide/src/find_references.rs` `memchr`-scans
   **only the current file's contents** and matches only `ast::Name`/`ast::NameRef`. No
   cross-file search, no label references. Issue #125 open since 2024-03-30.
3. **starpls, diagnostics** — rich parse + type diagnostics (see §1), but no buildifier
   lints, no unknown-attribute check against `build-language` beyond what inference
   catches, no unresolved-label diagnostic.
4. **starlark-rust generic, goto-def(load)** — the whole of load resolution is delegated to
   `LspContext::resolve_load`; the stock non-Bazel `Context` in `starlark_bin` resolves
   plain relative paths only.
5. **starlark-rust generic, hover(rule docs)** — hover renders
   `Globals::documentation()` for core Starlark builtins via `starlark:<name>.bzl` URIs.
   No Bazel rules exist in that namespace.
6. **starlark-rust family, type inference** — `starlark::typing::Ty` is used to build
   `DocFunction`/`DocParam` for hover, signature and completion, but the LSP never reports
   type errors and there is no inference across `load()` boundaries or into `ctx`.
7. **goto-def(label), starlark-rust `--bazel` and bazel-lsp** — resolves a label to the
   **file** it denotes (or the package's BUILD file) and, for `load()`, to the symbol via
   `find_call_name`. It does not locate an arbitrary `//pkg:target` rule *call* inside a
   BUILD file the way starpls's `goto_definition.rs:260-300` does.
8. **starlark-rust `--bazel`, completion(labels)** — `get_filesystem_entries` walks the
   filesystem for packages/targets and `read_dir`s `<output_base>/external` for repo names
   (`bazel.rs:479-520`). No `bazel query`, so nothing macro-generated appears.
9. **starlark-rust `--bazel`, bzlmod** — there is no `bazel mod dump_repo_mapping` call
   anywhere in `starlark_bin/bin/bazel.rs`. Apparent repo names resolve only when a
   directory of that exact name happens to exist under `external/`.
10. **starlark-rust `--bazel`, external repos** — resolvable only through
    `<output_base>/external/<literal name>` (`get_repository_path`, `bazel.rs:394-398`).
    Correct under WORKSPACE, wrong under bzlmod's canonical names.
11. **bazel-lsp, diagnostics** — starlark-rust parse errors plus `AstModuleLint` warnings,
    which are Buck-flavoured and had to be selectively disabled for Bazel dialects
    (v0.6.3: "Fix linting of global symbols", "Remove misplaced-load lints from WORKSPACE
    files"). No buildifier, no Bazel-specific checks.
12. **tilt-dev, goto-def(load)** — `document.Manager.Resolve` maps a load string to a
    `file://` URI by plain path arithmetic (`pkg/document/manager.go:76-180`), with a
    circular-load guard. No label syntax, no repos.
13. **tilt-dev, completion(attrs)/hover** — sourced from the embedded
    `pkg/analysis/builtins.py` Python type stubs, or user-supplied `--builtin-paths`.
    These describe core Starlark and Tilt, not Bazel rules; no rule attribute is present
    unless someone hand-writes a stub.
14. **tilt-dev, diagnostics** — tree-sitter parse errors only; there is no resolver or
    type checker.
15. **tilt-dev, type inference** — `query.Type`/`query.Signature` read declared types out
    of the stub files. Nothing is inferred.
16. **starlsp, goto-def(load)** — `Host.ResolveLoad(req) (path string, ok bool)` is a
    string→path hook; the bundled Bazel host does not implement label semantics.
17. **starlsp, completion(attrs)/hover** — from 239 lines of hard-coded signature strings
    in `dialects/bazel/bazel.go` plus runtime introspection of `starlark.Universe`. No
    `bazel info build-language`, so real rule attributes are absent.
18. **starlsp, find-refs / rename / workspace symbols** — built on a reference index over
    `go.starlark.net/resolve` bindings, i.e. **Starlark identifiers**. Renaming a Bazel
    target or a label does not happen.
19. **starlsp, type inference** — method sets are introspected from live
    `starlark.Value` runtime types; there is no user-code inference and no `ctx` model.
20. **vscode-bazel, goto-def(load)** — the definition provider is registered only for
    `**/BUILD` and `**/BUILD.bazel` (`language_support_feature.ts:140-143`), so `.bzl`
    files get nothing; and it resolves via `bazel query kind(rule|file, …)` rather than
    load semantics.
21. **vscode-bazel, workspace symbols** — `BazelTargetSymbolProvider` is a *document*
    symbol provider (one `bazel query` per BUILD file). No
    `workspace/symbol` equivalent is registered.
22. **vscode-bazel, bzlmod / external repos** — correct only because every lookup is a
    `bazel query` subprocess; the extension itself has no repo-mapping model. Cost:
    one Bazel invocation per navigation, and total failure when the workspace won't load.
23. **stackb, completion** — README: builtins are free; *"completion for third-party and
    custom starlark rules is available on a subscription basis."*
24. **stackb, diagnostics/formatting** — buildifier lint + format are provided by the
    **extension** (`src/buildifier/formatter.ts`), not by the `bzl` language server.
25. **stackb, bzlmod / external repos** — the server predates bzlmod (last shipped 2022–23);
    README claims label jumps work "with default and external workspaces", which is
    WORKSPACE-era `external/<name>` behaviour. Unverifiable — the source is closed.
26. **hirschgarten, code lens** — IntelliJ has no code lens; the equivalent is
    `ui/gutters/StarlarkRunLineMarkerContributor` (run/test gutter icons) plus
    `BazelJumpToBuildFileAction` / `CopyTargetIdAction`.
27. **hirschgarten, external repos** — fully correct, but only after a project **sync**
    populates `canonicalRepoNameToPath` via `BazelRepoMappingSyncHook`. Editing a file in a
    repo that appeared since the last sync will not resolve.
28. **hirschgarten, type inference** — there is argument/parameter validation against
    bundled signatures (`StarlarkFunctionCallValidationInspection`,
    `StarlarkFunctionParameterValidationInspection`) and provider/attr awareness, but no
    general Starlark type inference engine comparable to `starpls_hir`. No `ctx.attr.*`
    typing.
29. **bazelbuild/intellij, hover(rule docs)** — the `lang/buildfile/documentation` package
    exists, but its rule/attribute doc source is the Google-era bundled data and the plugin
    is feature-frozen for IntelliJ as of 2025-07-01. Not source-verified in depth here.
30. **bazelbuild/intellij, bzlmod** — `references/ExternalWorkspaceReferenceFragment.java`
    is WORKSPACE-shaped; bzlmod support in this codebase lags and receives no new features
    per the README's own statement.
31. **bazel.el, goto-def(load)** — `xref-backend-definitions` handles labels including
    `.bzl` targets, landing on the file; it does not jump to the loaded *symbol* inside
    that file.
32. **bazel.el, workspace symbols** — `imenu` provides per-buffer symbols only; there is no
    project-wide target index (`project-external-roots` exists but only feeds `project.el`
    search).
33. **bazel.el, bzlmod / external repos** — external repository roots are found by reading
    the `bazel-out` symlink and walking to `…/external/REPOSITORY`
    (`bazel.el:2198-2231`, and the 2026-08-10 commit "Convert external repository root to
    directory name"). That is a **canonical** directory name; there is no
    `bazel mod dump_repo_mapping`, so apparent names like `@rules_go` under bzlmod do not
    resolve.
34. **bazelrc-lsp** — the columns are about BUILD/`.bzl` semantics; this server only
    handles `.bazelrc`. Within its own domain it does have goto-definition and
    document links for `import`/`try-import` targets, completion of command and flag names,
    and hover docs for flags and commands.
35. **buildtools, rename** — `buildozer` can perform `rename` and label rewriting from the
    command line, but there is no interactive/LSP surface; it is a batch tool.
36. **buildtools, diagnostics** — `buildifier --lint=warn --format=json` emits the 194
    warnings in `WARNINGS.md`. It is a CLI; every diagnostic cell marked "yes" elsewhere in
    this table for a non-starpls tool is ultimately this.

---

## 14. Things worth carrying into the design

- **No LSP-speaking implementation is both alive and Bazel-correct.** starpls is the only
  one with the right architecture (salsa, HIR, inference, incremental sync) and it has been
  unmaintained since 2025-12-03 with 20 PRs queued and a bus factor of 1. Two companies
  (Modular, and whoever `Ahajha` is) already run private forks.
- **`bazel mod dump_repo_mapping` is the dividing line.** starpls and bazel-lsp call it;
  starlark-rust `--bazel`, bazel.el and starlsp do not, and are therefore wrong on every
  bzlmod repo. hirschgarten gets it from the sync's `BzlmodRepoMapping` instead.
- **Nobody streams builtin docs correctly.** starpls's `builtin.pb` is frozen at 2024-12-27;
  bazel-lsp's `builtin.pbtxt` is a 2024 snapshot with a hand-written `MISSING_GLOBALS`
  patch list; hirschgarten is the only project that ships **14 version-keyed builtin JSONs
  (7.5.0 … 9.1.0)** plus real stardoc extracts for `rules_go`/`rules_kotlin`. The
  BCR-published `starlark_doc_extract` artifacts are an untapped source for a new server.
- **Two opposite latency strategies exist and both are used in production**: subprocess
  `bazel query` per request (vscode-bazel, stackb) vs. a filesystem+syntax model refreshed
  on sync (hirschgarten, starpls). starpls's `--experimental_enable_label_completions` is a
  half-way point that costs one query at startup.
- **Rename of a target with label rewriting exists exactly once**, in hirschgarten
  (`StarlarkStringLiteralManipulator` + `BazelTargetReferenceSearcher` +
  `BazelLabelReference.bindToElement`). No LSP has it.
- **Formatting is universally delegated to buildifier** and starpls still doesn't do even
  that (issue #267 open since 2024-06; PR #401 open since 2025-08-11).
- **`.bazelrc`, `.bazelversion`, `.bazelproject`, `MODULE.bazel.lock`, `.bazelignore`,
  BCR `source.json`/`metadata.json`** are covered only by hirschgarten (first three),
  bazelrc-lsp (first one), and bazel.el (all but the BCR files, mostly as font-lock modes).
  No LSP covers them together.
- **A vacancy in the ecosystem is explicitly on record**: vscode-bazel issue #1
  ("Implement language server for Starlark") has been open since 2018-09-11, its README
  still points users at two third-party binaries they must install by hand, and the
  bazelrc-lsp author has publicly stated an intent to fold into vscode-bazel.
