# Measurements and decisions

Everything here was measured on this machine (darwin aarch64, 18 cores) unless
marked otherwise. Recorded before the scaffold so the numbers survive the
session that produced them.

## 1. Hard data

### starlark-cst, release build

| measurement | value |
| --- | --- |
| corpus throughput (2,543 real files, 10.8 MB) | **110.8 MB/s**, 26,000 files/s |
| largest real file (280 KB), single reparse | **1.22 ms** |
| nodes produced across the corpus | 681,152 |

### Static index (parse + extract every `name =` target), 18 threads

| corpus | files | bytes | walk | index | targets | throughput |
| --- | --- | --- | --- | --- | --- | --- |
| real Bazel files | 5,115 | 21.7 MB | 0.40 s | **0.10 s** | 16,011 | **219 MB/s** |
| synthetic 20k pkgs | 20,001 | 3.2 MB | 1.19 s | 0.59 s | 60,001 | 34,000 files/s |

- **69 bytes per target** with no interning (`Box<str>` name + rule + u32 file + u32 offset).
- Parallel speedup only **1.8× on 18 threads** — indexing small files is I/O bound,
  not CPU bound. The directory walk (0.40 s) is comparable to the parse.
- Extrapolated to a real monorepo (74k packages, ~4.2 KB per BUILD file, 189k
  targets): **~1.4 s to index cold, ~13 MB resident.**

### Bazel, 20,000 packages / 60,000 targets (Bazel 8.7.0)

| operation | time |
| --- | --- |
| cold `query //...` | **16.76 s** |
| warm `query //...` | 0.82 s |
| warm `rdeps(//..., //x:a)` | 0.64 s |
| warm `query --output=streamed_proto --proto:rule_classes` | 0.95 s, **52 MB** |
| query while a build holds the same output base | **4.31 s** |
| query on a separate output base during that build | 0.37 s |

| resource | value |
| --- | --- |
| Bazel server RSS, one server | **991 MB** |
| second server on a private output base | **+1,225 MB** |
| output base on disk | 154 MB / 109 MB |

At 189k targets the proto stream extrapolates to **~165 MB per full refresh**.

### The ratio that drives everything

Our entire static tier costs **~1.4 s and ~13 MB** at real-monorepo scale. One
cold `bazel query` costs **16.76 s and ~1 GB** at a third that scale. Bazel is
~20× more expensive than everything we do ourselves, and it is the only thing
worth optimising, caching, or backgrounding.

### Stack facts

- `lsp-server` 0.10.0 (2026-07-16). Went 0.7.9 → 0.8.0 → 0.9.0 → 0.10.0 between
  2026-06-24 and 2026-07-16. Real redesign, not churn: `Response` moved from
  nullable `result`/`error` to `response_result: Result<Value, ResponseError>`.
  Deps are only serde, serde_json, log, crossbeam-channel.
- `lsp-types` 0.97.0 has been stale since **2024-06-04**. rust-analyzer replaced
  it with **`gen-lsp-types`** 0.11.0 (`ribru17/gen-lsp-types`, generated from the
  official LSP metaModel), aliased in Cargo.toml as `lsp-types`.
  **It is not a drop-in**: `DocumentSymbolProvider::Bool`, `TextDocumentSync::Kind`,
  `LspRequestMethod`, `Contents`. Differs from `vimdoc-language-server`'s idiom.
- rust-analyzer HEAD (2026-08-25) uses `salsa` 0.28.2, `rowan` 0.17.0,
  `dashmap` pinned `=6.2.1`, `triomphe`.
- A working vertical slice — `lsp-server` + `gen-lsp-types` + `starlark-cst` +
  `arc-swap`, serving real `documentSymbol` over stdio — took **~160 lines**.

## 2. Bazel as an optional, separately-configured subsystem

**Decision: Bazel is off-by-default-able and never required to start.**

The static tier needs no Bazel at all, and it is most of the daily value. So the
server must run, and be useful, with `bazel` absent, broken, or disabled.

```jsonc
{
  // Turn the whole Bazel subsystem off. Static tier only.
  "bazel.enable": true,
  // Binary to invoke. `bazelisk` also works.
  "bazel.path": "bazel",
  // Use a private --output_base so LSP queries never queue behind the user's
  // build. Costs a second Bazel server: measured +1.2 GB at 20k packages.
  // Default false — see below.
  "bazel.privateOutputBase": false,
  // Extra startup/command flags.
  "bazel.args": [],
  // Refuse to spend longer than this on any single invocation.
  "bazel.timeoutSeconds": 120
}
```

### Feature matrix by mode

| feature | no Bazel | with Bazel |
| --- | --- | --- |
| syntax diagnostics | yes | yes |
| `documentSymbol` | yes | yes |
| formatting (buildifier) | yes | yes |
| goto-def on `load()` in the main repo | yes | yes |
| goto-def on `//pkg:target` in the main repo | yes | yes |
| goto-def into `@repo//...` | **no** | yes |
| `workspace/symbol` | degraded (static scan) | yes |
| find-references | **no** | yes |
| unresolved-label diagnostics | **no** | yes |
| rule/attribute completion and hover docs | **no** | yes |

Everything in the "no" column is graph-derived and genuinely impossible without
Bazel. Everything else must keep working, which is what makes the flag honest
rather than a crippled mode.

### Why `privateOutputBase` defaults to false

Earlier reasoning said always use a private output base, to dodge the lock. The
measurements say otherwise: a second server costs **+1.2 GB at 20k packages**,
and a real repo would be several GB. The lock only hurts if a request *waits* on
Bazel — and it never does (see invariant 1). A build holding the lock just means
the index refresh is late, and late is invisible. So it is a tunable for people
with RAM to spare and constant builds, not a default.

## 3. Threading

Three roles. Everything slow is off the main thread, and the index is published
rather than shared mutably.

```
main thread          owns the LSP connection, the open-document map and the VFS.
  (single)           Applies edits. Dispatches. NEVER blocks on Bazel or on IO
                     beyond reading the next message.

worker pool          N = available_parallelism. CPU-bound request handling
  (threads)          (parse, symbol extraction, completion filtering) against an
                     immutable index snapshot.

bazel actor          exactly one thread, owning the subprocess. Serialises all
  (1 thread)         Bazel invocations, decodes proto incrementally, builds a new
                     index, and publishes it. Cancellable; a superseded refresh
                     is dropped rather than queued.
```

- The index is `ArcSwap<Index>`. Readers `load()` an `Arc` and hold a consistent
  snapshot for the life of the request; the writer `store()`s a freshly built
  one. Verified in the prototype: a reader holding a stale snapshot is unaffected
  by a concurrent swap. No reader ever blocks, and there is no torn state.
- **One** Bazel thread, not a pool. Bazel serialises on the output base anyway,
  so concurrent invocations would only contend. A single actor also gives one
  obvious place to implement cancellation and backpressure.
- Requests do not need `salsa`-style cancellation to be correct, because they are
  milliseconds long. `$/cancelRequest` is honoured by dropping the response, not
  by unwinding.

## 4. Consequences for the design

1. **No salsa at v1.** Salsa avoids recomputation; recomputing our whole static
   index costs 1.4 s and one file costs 1.22 ms. Revisit only if profiling on a
   real monorepo says otherwise. Migrating later means replacing the index
   builder, not the architecture.
2. **No persistence at v1.** A cold static rebuild is faster than the
   invalidation bugs a cache would buy. If anything is ever cached to disk it is
   the Bazel-derived tier, keyed on `bazel info` plus BUILD-file mtimes.
3. **Optimise traversal before compute.** The walk is as expensive as the parse.
4. **Never print to stdout.** stdout is the LSP transport. Human-readable output
   goes to stderr; a separate demo subcommand may use stdout because it is not
   speaking LSP.
