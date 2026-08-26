# Stack research context

Today is 2026-08-25. We are choosing the Rust stack for a **Bazel language
server** (BUILD, *.bzl, MODULE.bazel, .bazelrc). Research only; no code.

## What already exists

`starlark-cst` (github.com/barrettruth/starlark-cst): a lossless rowan CST for
Starlark. Hand-written lexer + recursive-descent parser, typed AST accessors,
error recovery. Measured in release over 2,543 real Bazel files:
**110.8 MB/s, 26,000 files/s; largest real file (280 KB) reparses in 1.22 ms.**
Parsing is NOT a bottleneck and will not become one.

## Measured constraints that drive the design

At 20,000 packages / 60,000 targets (synthetic, Bazel 8.7.0):

| operation | time |
| --- | --- |
| cold `bazel query //...` | 16.76 s |
| warm `bazel query //...` | 0.82 s |
| warm `rdeps(//..., //x:a)` | 0.64 s |
| warm `query --output=streamed_proto --proto:rule_classes` | 0.95 s, **52 MB** of proto |
| Bazel server RSS | ~991 MB (a second server adds ~1.2 GB) |
| query on a base held by a running build | 4.31 s, bounded only by the build's remaining time |

Real monorepos reach **74,000 packages / 189,000 targets** — so ~165 MB of
proto per full index refresh, and keeping rowan trees for every file would be
1.6-3.3 GB.

## Architectural commitments already made

1. **Never make a Bazel call in the request path.** Bazel calls are background;
   requests answer from an index or degrade. This is the one rule that cannot
   be retrofitted.
2. **Two-tier index**: a light `(name, kind, file, offset)` table for the whole
   repo; full CSTs only for open files plus a small LRU.
3. Never eagerly index `//...`; lazy and per-package.
4. Degrade loudly — an unresolved label because a repo is unfetched must be a
   diagnostic, never a silent empty result.

## The shape of the work

- CPU-bound and fast: parsing, symbol extraction, completion filtering.
- IO-bound and slow: a handful of `bazel` subprocesses, seconds each, cancellable.
- There is no high-concurrency network IO anywhere in this program.

## House conventions

Rust 2024, `unsafe_code = "forbid"`, `clippy::pedantic`, `just ci`, nix flake
devshell. The author's other language server (`vimdoc-language-server`) uses
`lsp-server` + `lsp-types`. Preference stated: "easy to use, minimal", but it
must scale to the repo sizes above.

## Rules for your report

Be concrete: exact crate versions, download counts, last-release dates, open
issue counts, real API snippets. Establish CURRENT state (2026-08-25) — say
when something is stale or abandoned and give the date. Prefer measured or
cited facts over impressions. Say plainly where you are uncertain.
No filler, no restating the prompt.
