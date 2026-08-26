# Research brief — shared context for all subagents

Today is **2026-08-25**. Bazel **9.2.0** is the current release (confirmed: `bazelisk`
downloads it as `latest`). nixpkgs pins `bazel_8` = 8.7.0, `bazel_7` = 7.6.0,
`bazelisk` = 1.29.0, `bazel-buildtools` (buildifier/buildozer) = 8.5.1.

## Goal

We are researching in order to build a **new language server for Bazel build files**.
This phase is research only.

## Scope — IN

LSP features *for the Bazel/Starlark files themselves*:

- File types: `BUILD`, `BUILD.bazel`, `*.bzl`, `MODULE.bazel`, `MODULE.bazel.lock`,
  `WORKSPACE`, `WORKSPACE.bazel`, `REPO.bazel`, `*.bazelrc`, `.bazelignore`,
  `*.bzlmod`, `vendor` manifests, BCR `source.json`/`metadata.json`.
- Goto-definition on a label (`//foo:bar`, `:bar`, `@rules_go//go:def.bzl`) landing on
  the declaring `BUILD` file / `.bzl` file / `MODULE.bazel` `bazel_dep`.
- Goto-definition on `load()` symbols, following into external repos.
- Completion: labels, target names, rule names, attribute names, attribute values
  (enums, `select()` keys, visibility, tags), `load()` symbol lists, module deps.
- Hover: rule docs, attribute docs, provider fields, resolved label -> path.
- Find references / rename of a target (and rewriting every referring label).
- Document symbols, workspace symbols (all targets in the repo).
- Diagnostics: buildifier lints, unresolved labels, unknown attributes, type errors
  in `.bzl`, cycle detection, missing `load`.
- Formatting (buildifier), semantic tokens, inlay hints, code lens
  (`build`/`test` this target), code actions (add dep, fix visibility).

## Scope — OUT (do not spend time here)

- Making *other* languages' language servers (clangd, rust-analyzer, gopls, jdtls,
  pyright) work inside a Bazel-managed repo.
- `compile_commands.json` generation, `hedron_compile_commands`, `rules_*` IDE support.
- Build Server Protocol *as a transport for non-Bazel LSPs*. BSP is only relevant
  insofar as it overlaps with editing Bazel files (e.g. JetBrains' Starlark support).
- Remote execution, caching, CI, build performance — except where it bears on
  whether an LSP can afford to call Bazel interactively.

## Working rules

- Repos are already shallow-cloned at `/Users/bruth/dev/bazel-language-server/upstream/<name>`:
  `starpls`, `starlark-rust`, `bazel-lsp`, `starlark-lsp`, `buildtools`, `starlark`,
  `starlark-go`, `vscode-bazel`, `bazel-stack-vscode`, `tree-sitter-starlark`,
  `stardoc`, `bazel` (depth 1), `hirschgarten` (depth 1).
  **Do not clone into that directory.** If you need another repo, clone into `/tmp`.
- Most published material is stale. Always establish the *current* state: check
  `git log -1`, GitHub releases, issue dates. State explicitly when a project is
  abandoned or stale, **with the date of its last activity**.
- Be dense and specific: exact file paths, type/function names, commands, version
  numbers, dates, URLs. Quote code where it proves a point.
- No filler, no restating the prompt, no concluding pleasantries.
