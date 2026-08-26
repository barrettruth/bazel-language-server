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

At 189k targets the proto stream extrapolates to ~165 MB per full refresh — but
see §1.1: pruning attributes cuts that by 10×.

### 1.1 Proto stream, measured end to end

`--output=streamed_proto` off the pipe, 60,000 targets:

| variant | bytes | wall | peak RSS |
| --- | --- | --- | --- |
| default | 53.7 MB | 0.89 s | — |
| `--proto:output_rule_attrs= --noproto:rule_inputs_and_outputs` | **5.4 MB** | **0.42 s** | — |
| incremental decode, 205 MiB / 240k targets | — | 0.622 s | **34 MB** |
| slurped instead, same input | — | 0.641 s | **442 MB** |

- Attribute pruning is a **10× reduction** and still carries name, rule class and
  location — everything the index needs. The 189k-target refresh is therefore
  **~16 MB, not 165 MB**.
- Incremental and slurped decode run at the same speed; slurping costs 13× the
  memory for nothing. Decode must stream.
- Throughput is Bazel's write rate (52–56 MB/s), not the decoder's: prost
  manages 324 MB/s. Decoder choice is not a performance decision.
- `prost-build` hard-requires `protoc` at build time. **`protox` 0.9.1 removes
  that**, which matters for packaging.

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

## 2.1 Cancelling Bazel — do not use `kill()`

Measured, and it is a trap:

| signal to the client | result |
| --- | --- |
| `SIGKILL` | the **server-side command keeps running to completion** and holds the command lock under a dead PID. Later invocations fail with `Another command (pid=…) is running`. |
| `SIGINT` / `SIGTERM` | clean: lock released in **13–42 ms** |

`Child::kill()` in std sends `SIGKILL`, so the obvious call is the wrong one.
The client turns SIGINT into a `Cancel` RPC (`blaze_util_posix.cc:144`).
Under `unsafe_code = "forbid"` the way to send a different signal is
`shared_child`'s `unix::SharedChildExt::send_signal`.

Two further limits:

- **Bazel's query output-serialisation phase is uncancellable.** Cancelling at
  254 ms exits in 7 ms; at 352 ms the command runs to completion. So a
  superseded refresh cannot always be stopped — it must be *discarded*, which
  the single-actor design already does.
- **Do not ping to keep the server warm.** `--max_idle_secs` defaults to 10800
  and a ping does reset the timer, but it also suppresses Bazel's 10 s idle GC.
  Eviction costs 3.09 s against 0.99 s warm; that is not worth a permanently
  inflated heap.

**gRPC command server: not for v1.** A working tonic client saves a measured
106 ms median per call and gets a real `Cancel` (acked in 2.3 ms), but costs 89
crates against 34, and tonic has just moved to `grpc/grpc-rust` with breaking
changes in preparation. Revisit when per-call latency actually matters.

## 2.2 Watching — one recursive watch, never per-directory

rust-analyzer calls `watcher.watch(dir, Recursive)` once **per directory**
(`crates/vfs-notify/src/lib.rs:329-331`). Measured with notify 8.2.0 on macOS:

| directories watched individually | result |
| --- | --- |
| 4,096 | 72 s setup, quadratic — 8.2.0 rebuilds the whole `FSEventStream` per call |
| **4,100** | every `watch()` returns `Ok(())` and **zero events are delivered, ever** |

Silent and total. FSEvents burns a file descriptor per stream path, so it is
really `RLIMIT_NOFILE` (default 256) in disguise; notify 9.0.0-rc.4 at least
reports the failure, and notify `main` added an `RLIMIT_NOFILE/12` budget with
the note that past ~`/10` "FSEvents closes fd 0, which this process owns."

**One recursive watch on the 74k-package root instead: 0.002 s setup, 15.8 ms
to the deepest package, 2,000 writes → 4,048 events, no rescan flags.** The
problem is the shape of the call, not the crate.

**Own the watcher rather than using `workspace/didChangeWatchedFiles`.**
`DidChangeWatchedFilesRegistrationOptions` has watchers and no exclude field, so
"not under `bazel-out`" is inexpressible, and VS Code's default
`files.watcherExclude` has no Bazel entries.

### The symlink trap, measured

`walkdir` with `follow_links(true)` and no pruning finds **94,118 BUILD files in
a tree that has 74,001**. The `bazel-<workspace>` convenience symlink points at
the execroot, whose symlink forest re-enters the source tree. Neither walkdir's
ancestor-loop detection nor rust-analyzer's `path_might_be_cyclic` catches it —
rust-analyzer walks Bazel workspaces twice. `bls_index::build_static` sets
`follow_links(false)` and prunes `bazel-*`; `bazel_symlinks_are_not_followed`
pins it.

### Traversal and memory

- `walkdir`, `ignore` and `jwalk` are identical serially (~37 K entries/s — the
  filesystem is the ceiling). `ignore`'s parallel walker reaches 69 K/s at ×18.
  Its gitignore machinery costs 2.27× **and is wrong**: Bazel does not honour
  `.gitignore`. Use it with gitignore off, or stay on walkdir.
- `jwalk` 0.9.0 is deprecated by its own author.
- Real RSS for the light index over 74 K paths + 189 K entries: **15.58 MiB**
  with `lasso`, 26.00 MiB if a name lookup is an `FxHashMap`, ~16.6 MiB if that
  is flattened to CSR. `ThreadedRodeo` costs 2.8× `Rodeo`; `ustr` is worse than
  plain `String`. This confirms the ~13 MB estimate in §1.
- **`bincode` 3.0.0 is `compile_error!("https://xkcd.com/2347/")`** and `sled`
  has not shipped since 2024-10-11. If persistence ever happens, neither is a
  candidate.

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
