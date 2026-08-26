# Rust LSP server frameworks — state of the world, 2026-08-25

All version/date/download figures pulled from the crates.io API and the upstream
git repositories on 2026-08-25/26. Both code skeletons at the end were compiled
against real crates with `rustc 1.98.0` (stable, aarch64-apple-darwin).

## 1. The candidate set

| crate | latest | published | repo last push | src LOC | total dl | dl/90d | rev deps | stars | open issues |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `lsp-server` | 0.10.0 | 2026-07-16 | 2026-08-25 | 1,129 | 14,500,586 | 3,596,652 | 194 | (in r-a) | (in r-a) |
| `tower-lsp-server` | 0.23.0 | 2025-12-07 | 2026-08-15 | 5,770 | 1,622,458 | 918,303 | 73 | 219 | 5 |
| `async-lsp` | 0.2.4 | 2026-04-24 | 2026-08-09 | 2,954 | 1,266,949 | 480,650 | 31 | 175 | 4 |
| `tower-lsp` | 0.20.0 | **2023-08-11** | **2024-08-15** | — | 7,066,831 | 1,839,025 | 367 | 1,359 | 41 |

Ruled out, with reasons:

- **`tower-lsp` 0.20.0** — no release in 3 years, no commit in 2 years, 41 open
  issues including the ordering bug (#284) and a live panic bug (#417). Dead.
  Its 1.84M downloads/90d are legacy consumers (Turborepo still pins it).
- **`lspower` 1.5.0** (2021-12-07) — abandoned fork of tower-lsp; upstream
  merged back into tower-lsp via ebkalderon/tower-lsp#308/#309 in 2022.
- **`lsp-async-stub` 0.7.0** (2025-05-23, 167k dl/90d) — taplo's internal stub,
  published but undocumented and not intended as a general framework.
- **`deno_tower_lsp` 0.5.0** (2026-02-11) — Deno's hard fork, self-described as
  "at the moment only floating patches". Single-consumer.
- **`sync-ls` 0.15.4-rc3** (2026-08-25, 787 dl/90d) — tinymist's in-tree
  framework, "inspired by async-lsp, primarily for tinymist". Perpetual rc,
  single-consumer.
- **`microsoft/lsprotocol` Rust bindings** (`lsprotocol` 1.0.0-alpha.3,
  2025-06-18, **32 downloads in 90 days**) — Microsoft's official codegen has a
  Rust target, but it is alpha, unused, and types-only. tower-lsp-server
  evaluated and rejected it (tower-lsp-community/tower-lsp-server#64, closed
  2025-08-12).

So there are exactly three live options: `lsp-server`, `tower-lsp-server`,
`async-lsp`.

## 2. `lsp-server` 0.7.9 → 0.10.0: what actually changed

Three "major" 0.x bumps in 22 days is alarming on the surface. It is not a
redesign and it is not churn in any meaningful sense. I diffed the published
`.crate` tarballs. Here is the **complete** public-API delta across all three
releases:

**0.7.9 (2025-08-06) → 0.8.0 (2026-06-24)** — purely additive:
```rust
impl<I, O> ReqQueue<I, O> { pub fn has_pending(&self) -> bool }   // new
impl<I>    Incoming<I>    { pub fn has_pending(&self) -> bool }   // new
impl<O>    Outgoing<O>    { pub fn has_pending(&self) -> bool }   // new
```
plus a better error message on malformed payloads (now includes the offending
text) and removal of dev-dependencies from the published manifest. Nothing
breaking. The major bump was conservatism, not necessity.
(rust-analyzer commits `86c523fa5`, `ae8b69f33`.)

**0.8.0 → 0.9.0 (2026-07-11)** — one struct, reshaped for JSON-RPC correctness:
```rust
-pub struct Response { pub id: RequestId,
-    #[serde(skip_serializing_if="Option::is_none", default)] pub result: Option<Value>,
-    #[serde(skip_serializing_if="Option::is_none", default)] pub error:  Option<ResponseError>, }
+pub struct Response { pub id: RequestId, #[serde(flatten)] pub response_kind: ResponseKind }
+pub enum ResponseKind { Ok { result: Value }, Err { error: ResponseError } }
```
Motivation (commit `87d1736bf`, Riley Bruins, 2026-07-09): the old type could
express `{result: None, error: None}` and `{result: Some, error: Some}`, both of
which JSON-RPC 2.0 forbids.

**0.9.0 → 0.10.0 (2026-07-16)** — the same fix, done better:
```rust
-pub enum ResponseKind { Ok { result: Value }, Err { error: ResponseError } }
+#[serde(flatten, with = "ResponseResult")]
+pub response_result: Result<serde_json::Value, ResponseError>
```
Commit `99d657170` (2026-07-14): *"We don't need a new type here thanks to
serde's `remote`."* `ResponseKind` was removed from the public API 5 days after
being added.

**Verdict on churn.** Total migration cost from 0.7.9 to 0.10.0 is: add
`response_result: Ok(..)` / `Err(..)` at any site that constructs or destructures
`Response` by field. Anyone using `Response::new_ok` / `Response::new_err`
(the normal path, and the only path used in the shipped example) changes
**nothing**. 0.9.0 was a five-day mistake; 0.10.0 is the settled shape. This is
a small, correct, low-velocity crate having one awkward month, not a redesign.

**The one real caveat, and it is a live one.** rust-analyzer does not ship what
it publishes. `Cargo.toml:42` has `# lsp-server = { path = "lib/lsp-server" }`
commented out inside `[patch.'crates-io']`, and `Cargo.toml:101` reads:
```toml
lsp-server = { version = "0.7.9" }
```
`lib/README.md` states the intent — *"We use these crates from crates.io, not
the local copies because we want to ensure that rust-analyzer works with the
versions that are published"* — but the pin has not been moved. At master
(`014d54b68`, 2026-08-25) `crates/rust-analyzer/src/handlers/dispatch.rs:271`
still constructs `Response { id, result: None, error: Some(error) }`, i.e. the
0.7.x layout. **The rust-analyzer binary today links `lsp-server` 0.7.9. The
0.8/0.9/0.10 API has never run rust-analyzer's main loop.**

Download split for the last 90 days confirms this is where the mass is:

| version | dl/90d |
| --- | --- |
| 0.7.9 | 1,410,874 |
| 0.7.8 | 1,025,940 |
| 0.10.0 | 199,393 |
| 0.8.0 | 148,225 |
| 0.9.0 | 17,254 |

0.10.0's ~199k in 41 days is real adoption, not zero — **ruff** and
**wgsl-analyzer** both pin `lsp-server 0.10.0` + `gen-lsp-types 0.11.0` at HEAD.
So it is dogfooded, just not by rust-analyzer itself.

## 3. The types layer — the more consequential decision

`lsp-types` 0.97.0 (gluon-lang) was published **2024-06-04**; last repo push
2024-07-09; 47 open issues. It is stale by 26 months and targets LSP 3.17.
Everyone has moved or is moving off it.

Three successors exist, and they collapsed into one during 2026:

| crate | latest | published | status |
| --- | --- | --- | --- |
| `gen-lsp-types` | 0.11.0 | 2026-07-28 | **the convergence point** |
| `ls-types` | 0.0.6 | 2026-03-08 | tower-lsp-community's fork; repo **archived 2026-08-15** |
| `helix-lsp-types` | 0.95.1 | — | Helix's private client-side fork |

`ls-types` was tower-lsp-community's hand-patched fork of `lsp-types`, shipped
in tower-lsp-server 0.23.0. Their CHANGELOG said the long-term plan was
metamodel codegen. They then gave up and adopted `gen-lsp-types` instead —
commit `d8aae82` *"fix!: switch to gen-lsp-types"* on tower-lsp-server main,
2026-08-15, currently **unreleased** — and archived the `ls-types` repo the same
day (18 stars, 13 open issues left unresolved). rust-analyzer switched on
2026-06-19 (`0625d0f14`).

`async-lsp` is the holdout. ribru17 opened
[oxalica/async-lsp#27](https://github.com/oxalica/async-lsp/pull/27),
*"fix!: switch to gen-lsp-types"*, on 2026-04-23; it was **closed the same day
without merging**. `async-lsp` HEAD still reads `lsp-types = "0.95.0"`.

### Is `gen-lsp-types` one person's project?

Substantially yes, and you should price that in.

- 131 commits total. **Riley Bruins (ribru17): 94. Dependabot: 32.** Three other
  humans contributed 8 commits between them.
- 28 GitHub stars. 9 reverse dependencies on crates.io.
- First release 2026-04-19; 12 releases in 100 days.

Against that, the mitigating facts are strong:

- **It is generated, not written.** `xtask` runs `typify` over a vendored copy
  of Microsoft's `metaModel.json` (currently `{"version": "3.18.0"}`: 69
  requests, 26 notifications, 387 structures, 40 enumerations). 18,179 of its
  19,495 lines are under `src/generated/`. The bus factor on a code generator
  fed by an upstream JSON schema is much better than the bus factor on 19k
  hand-written lines — a fork is a `cargo xtask` away.
- **Two independent major consumers have committed.** rust-analyzer (in-tree,
  since 2026-06-19) and tower-lsp-server (main, since 2026-08-15). The same
  person also drove the recent `lsp-server` 0.9/0.10 changes, which is a
  concentration risk, but it also means the two crates are being co-designed.
- 324,849 of its 330,822 lifetime downloads landed in the last 90 days.
- Zero open issues.
- It fixes a real correctness bug class `lsp-types` has: it distinguishes
  `null` from absent, which the spec requires and `Option<T>` cannot express.

### Breaking-change cadence — real, but decelerating

Breaking commits are marked with conventional-commit `!`:

```
2026-04-21  feat!: more succinct response type names            (0.3.0)
2026-04-22  fix!: rename request and notification method enums  (0.4.0)
2026-05-11  fix!: better Lsp(Request|Notification)Method ergonomics
2026-05-21  feat!: bump metamodel                               (0.6.0)
2026-06-07  feat!: bump LSP metamodel to officially released 3.18
2026-06-07  fix!: type LspObject as serde_json::Map rather than HashMap
2026-06-07  fix!: encode arbitrary additional properties on MessageActionItem
2026-06-07  fix!: generalize SemanticTokensEdit data property   (0.8.0/0.9.0)
2026-06-23  chore!: bump metamodel                              (0.10.0)
2026-07-27  fix!: allow all enums to accept custom values       (0.11.0)
```

Nine breaking releases April–July. But nothing breaking has landed since
2026-07-27 — the last four weeks are dependabot bumps and one additive
`WithUri` trait (`a2e1b70`). The 3.18 metamodel is now officially released, so
the largest single source of churn is spent. Still: budget for a breaking bump
every ~2 months for the next while, and pin exactly.

Two dependency footguns worth knowing:

```
lsp-server + gen-lsp-types, features = ["fluent-uri"]  →  20 crates
lsp-server + gen-lsp-types, features = ["url"]         →  45 crates  (idna + full icu chain)
```
The `url` feature drags in `icu_normalizer`, `icu_properties`, `idna`,
`zerovec`, `yoke`, and friends. `fluent-uri` is the right choice unless you
specifically need `url::Url` semantics. (rust-analyzer uses `features = ["url"]`.)

## 4. Threading model, and what async actually buys us

### `lsp-server` — synchronous, you own the concurrency

`Connection::stdio()` spawns exactly two threads (a reader and a writer) and
hands you `crossbeam_channel::{Sender, Receiver}<Message>`. That is the entire
threading model. There is no runtime, no executor, no `Send + 'static` bound on
anything, no task scheduler. The public API is 1,129 lines:

```
Connection::{stdio, connect, listen, memory, initialize*, handle_shutdown}
Message / Request / Response / Notification / RequestId / ErrorCode
ReqQueue { incoming: Incoming<I>, outgoing: Outgoing<O> }
IoThreads::join
```

Everything else you write. In practice that is a worker pool and a `Task` enum.
rust-analyzer's is `task_pool.rs`, **55 lines** (plus `stdx::thread`, 553 lines
that are almost entirely macOS/Linux thread-QoS plumbing we do not need).
starpls — the existing Bazel/Starlark LSP — has a **73-line** `task_pool.rs`
that is just a `rayon::ThreadPool` plus a `crossbeam` sender.

The resulting shape, which is exactly what rust-analyzer does:

- **All notifications on the main thread, `&mut State`, in receive order.**
  rust-analyzer's `NotificationDispatcher` exposes *only* `on_sync_mut` — there
  is no concurrent variant (`handlers/dispatch.rs:393-411`,
  `main_loop.rs:1443-1480`). `didOpen`/`didChange`/`didClose`/
  `didChangeWatchedFiles` cannot race. No locks, because there is no sharing.
- **Read-only requests on a worker pool** against an immutable snapshot, wrapped
  in `catch_unwind` (`on`, `on_with_thread_intent`).
- **Latency-sensitive requests inline on the main thread** (`on_sync`).
- **State-mutating requests inline with `&mut`** (`on_sync_mut`), explicitly
  *not* `catch_unwind`-guarded, with the comment *"please, don't make bugs :-)"*.

### `tower-lsp-server` — tokio, `&self` everywhere, `buffer_unordered(4)`

`LanguageServer` is a trait of `async fn`s taking `&self`. Since 0.21.0 it uses
native AFIT (no `#[async_trait]`), which also made it non-dyn-compatible.
Every handler future must be `Send + 'static`, so all mutable state lives behind
`Arc<RwLock<..>>` / `DashMap`.

The scheduler is in `src/transport.rs`:
```rust
const DEFAULT_MAX_CONCURRENCY: usize = 4;
let process_server_tasks = server_tasks_rx
    .buffer_unordered(self.max_concurrency)     // line 120
    .filter_map(future::ready)
    .map(|res| Ok(Message::Response(res)))
    .forward(responses_tx.clone());
```
Crucially, `jsonrpc::Request` carries `id: Option<Id>` — **a notification is a
`Request` with no id** (`src/jsonrpc/request.rs:29`). `read_input` pushes
requests *and notifications* into the same `server_tasks_tx`. So up to four
notifications and requests execute concurrently and complete out of order.
`concurrency_level(1)` forces serialisation, but the doc comment says doing so
*"implicitly disabl[es] support for the `$/cancelRequest` notification"*.

### `async-lsp` — tokio/async-std/smol, tower `Layer`s, `&mut self`

The one design here that took the ordering problem seriously. Handlers take
`&mut self`; a request handler returns a `Future` that does **not** borrow
`self`, so the *preparation* phase (snapshotting) is serialised and mutable
while the *computation* runs concurrently. Notification handlers are
synchronous `&mut self` fns returning `ControlFlow`, so they cannot interleave
by construction. Concurrency, panic recovery, lifecycle, client-process
monitoring and tracing are separate composable `tower::Layer`s
(`concurrency.rs`, `panic.rs`, `server.rs`, `client_monitor.rs`, `tracing.rs`).

Architecturally this is the best of the three. Its problem is elsewhere:
`Cargo.toml` pins `lsp-types = "0.95.0"` (a 2024-03-18 release, two majors
behind the already-stale 0.97), `edition = "2021"`, `rust-version = "1.66"`.
158 of 161 commits are by one person (oxalica), and the gen-lsp-types migration
PR (#27) was closed unmerged. Dependency count for a minimal server:
**86 crates**. It is not stagnant — issue #28 (2026-08-12, *"Lifecycle forwards
pre-initialize notifications and treats premature exit as success"*) and PRs
#29/#30 are open and recent — but the types layer is not moving.

### What does async buy *us*?

Honest case **for** async, applied to this program specifically:

1. We will run `bazel query` subprocesses that take **0.82s warm and 16.76s
   cold**, and up to **4.31s** when a build holds the base. Under async these
   are `tokio::process::Command` + `.await`, with cancellation for free via
   dropping the future, and `tokio::select!` gives clean timeout/debounce
   composition. Under `lsp-server` we write a dedicated thread that does
   `Command::spawn()` + `wait_with_output()` and posts the result back over a
   channel — plus explicit kill-on-cancel.
2. Streaming 52–165 MB of `--output=streamed_proto` is genuinely IO-shaped, and
   `tokio::io::AsyncRead` over a child's stdout is a nicer decoder loop than a
   blocking `BufReader` on a worker thread. Marginally.
3. If we ever add a server-to-client request *inside* a handler (e.g.
   `workspace/configuration`, or `window/showMessageRequest` for "run `bazel
   fetch`?"), async makes the round-trip an `.await`. Synchronously this is a
   correlation-id dance through `ReqQueue::outgoing` and the main loop. This is
   the single strongest pro-async argument, and it is exactly the case
   ebkalderon and samscott89 argued in tower-lsp#284.

Honest case **against**, applied to this program specifically:

1. CONTEXT.md commitment #1 is *"Never make a Bazel call in the request path."*
   That deletes argument (1) from the request path entirely. The subprocess
   lives in a background actor that owns the index. One long-lived thread with
   a channel is not harder than one long-lived task with a channel — it is the
   same program, minus a runtime.
2. `~1 subprocess` and, per matklad, *"dozens requests per second at most"*.
   Async's value is amortising thousands of concurrent waits over few threads.
   We have single-digit concurrency. There is nothing to amortise.
3. The workload is **CPU-bound**: 110.8 MB/s parsing, symbol extraction,
   completion filtering. Async does not make CPU work concurrent; a thread pool
   does. Under tokio you must additionally remember `spawn_blocking` for every
   CPU-bound handler or you stall the reactor — a footgun that does not exist
   in the synchronous design.
4. **`&mut self` vs `&self` is the real cost.** With `lsp-server` the index and
   the document store are plain `&mut` fields on `GlobalState`, mutated only
   from the main loop, snapshotted (`Arc`-cloned) for workers. With
   `tower-lsp-server` every one of them becomes `Arc<RwLock<_>>` because the
   trait hands you `&self`. Given we are building a two-tier index with an LRU
   of CSTs — i.e. a mutable cache read on the request path — that difference is
   not cosmetic. It is the difference between "no locks" and "an async RwLock
   around the hot path".
5. Dependency and build cost, measured cold on this machine:

   | stack | external crates | `cargo check` cold |
   | --- | --- | --- |
   | `lsp-server` + `gen-lsp-types[fluent-uri]` | 20 | 6.0 s |
   | `lsp-server` + `gen-lsp-types[url]` | 45 | — |
   | `tower-lsp-server` + minimal tokio | 54 | — |
   | `tower-lsp-server` + `tokio[full]` | 61 | 7.3 s |
   | `async-lsp` + `lsp-types` + `tower` + tokio | 86 | — |

## 5. `$/cancelRequest`

| | mechanism | automatic? |
| --- | --- | --- |
| `lsp-server` | `ReqQueue::incoming.cancel(id)` removes the pending entry and returns a `Response` with `ErrorCode::RequestCanceled` (-32800). | **No.** You must route `$/cancelRequest` yourself and decide whether to actually stop the work. |
| `tower-lsp-server` | `Pending(Arc<DashMap<Id, AbortHandle>>)`; every handler is wrapped in `future::abortable`. `$/cancelRequest` is pre-registered in the `LanguageServer` macro (`src/server.rs:64-72`) and calls `pending.cancel(&id)`, which aborts the future and replies -32800. | **Yes**, and the handler future is genuinely dropped mid-flight. |
| `async-lsp` | `ConcurrencyLayer` keeps `AbortHandle`s per request; on cancel it aborts and returns `ErrorCode::REQUEST_CANCELLED` (-32800) (`src/concurrency.rs:113,140-158`). | **Yes.** Note 0.2.4 (2026-04-24) exists *because* of this: *"Missed `abort()` of ongoing tasks on cancellation (#26)"* — the abort was silently broken before 2026-03-16. |

`lsp-server`'s "no" is less bad than it reads. Aborting a future at an `.await`
point is easy; aborting a synchronous CPU-bound computation on a worker thread
is not something any of these frameworks can do — `tower-lsp-server` and
`async-lsp` can only abort at yield points, which a parse loop does not have.
Real cancellation of CPU work needs cooperative checks, which is precisely what
salsa does.

### How rust-analyzer's panic-based cancellation interacts

rust-analyzer runs **two independent cancellation mechanisms**, and it is worth
being precise about which is which:

**(a) Client-initiated, `$/cancelRequest`.** Purely bookkeeping.
`handle_cancel` → `GlobalState::cancel(id)` → `req_queue.incoming.cancel(id)`
→ send -32800. The worker thread computing that request **keeps running to
completion**; its result is later dropped because `Incoming::complete()` no
longer has the id (`global_state.rs:623-641`). Nothing is aborted.

**(b) Server-initiated, salsa `Cancelled`.** When the main thread applies a
change (`analysis_host.apply_change`), salsa sets a cancellation flag and every
in-flight query **panics** with a `salsa::Cancelled` payload at its next
`maybe_changed_after` check. The dispatcher catches it:

```rust
// handlers/dispatch.rs:261-274
let result = panic::catch_unwind(move || { let _pctx = DbPanicContext::enter(pc); f(world, params) });
match thread_result_to_response::<R>(req.id.clone(), result) {
    Ok(response)                        => Task::Response(response),
    Err(_cancelled) if ALLOW_RETRYING   => Task::Retry(req),
    Err(_cancelled)                     => Task::Response(/* -32801 ContentModified */),
}
```
and `thread_result_to_response` downcasts the panic payload to `Cancelled`
(`dispatch.rs:353`). `Task::Retry` re-dispatches the *original request* against
the *new* snapshot, gated on the request not having been cancelled meanwhile:

```rust
// main_loop.rs:859-861
// Only retry requests that haven't been cancelled. Otherwise we do unnecessary work.
Task::Retry(req) if !self.is_completed(&req) => self.on_request(req),
Task::Retry(_) => (),
```

So (a) and (b) compose: `$/cancelRequest` marks the id complete, and the salsa
retry loop then declines to redo the work. Requests that cannot retry get
`ContentModified` (-32801), which every editor treats as "ask again".

**Implications for framework choice:**

- The mechanism is `panic::catch_unwind` + downcast. It is *only* available if
  handlers run in a context you can `catch_unwind`. `lsp-server` imposes nothing
  and rust-analyzer wraps each worker closure directly. `async-lsp` ships
  `CatchUnwindLayer` (`src/panic.rs:56,110` — `catch_unwind(AssertUnwindSafe(..))`
  around both `call` and each `poll`), which is compatible.
  **`tower-lsp-server` has no panic recovery at all**: the only relevant line is
  `src/service/pending.rs:116` — `handler_fut.await.expect("task panicked")`.
  A salsa-style cancellation panic escaping a handler would take down the
  service, not produce a -32801.
- It requires `panic = "unwind"`. Anything in our release profile that sets
  `panic = "abort"` breaks it outright.
- We are not committed to salsa. But if the two-tier index ever becomes an
  incremental query system (and for 74,000 packages it plausibly should), this
  is the design we would land on, and it wants `catch_unwind` around handlers.

## 6. tower-lsp's documented ordering/deadlock issues

Specific issues, with numbers and dates:

| # | title | opened | state today | fixed in tower-lsp-server? |
| --- | --- | --- | --- | --- |
| [#284](https://github.com/ebkalderon/tower-lsp/issues/284) | Consider ditching concurrent handler execution | 2021-05-23 | **open**, 42 comments, last activity 2024-08-21 | **No** — reopened as tower-lsp-server#36 |
| [#386](https://github.com/ebkalderon/tower-lsp/issues/386) | Concurrent requests without `Send` for single-threaded runtimes | 2023-03-15 | **open** | No |
| [#399](https://github.com/ebkalderon/tower-lsp/issues/399) | Server may not be exiting correctly after `exit` | 2023-10-10 | **open** | **Yes** — tower-lsp-server#12, "Close transport 1s after exit notification" (0.21.0), plus the `will_exit` early-break in `transport.rs:142` |
| [#417](https://github.com/ebkalderon/tower-lsp/issues/417) | Panic on future cancellation (`tx.send(r).expect("receiver already dropped")`) | 2024-04-29 | **open** | **Yes** — tower-lsp-server#37, "Do not panic when receiving a response whose request has been cancelled" (0.21.1, 2025-04-07); `src/service/client/pending.rs:35` now ignores the send error |
| [#183](https://github.com/ebkalderon/tower-lsp/issues/183) | Client request routing is broken (deadlock) | 2020-04-30 | closed | pre-existing fix |

### #284 in detail — this is the one that matters

ebkalderon's own diagnosis (2021-06-19): `Stream::buffered` is built on
`FuturesOrdered`, which *races futures in parallel* and merely yields results in
order. So responses are ordered but **handler execution is not**. The spec
requires `didChange` notifications to be applied in receive order; tower-lsp
cannot guarantee it, and as a generic framework *"we cannot guarantee that all
downstream `LanguageServer` trait implementers will behave sanely when multiple
changes are processed concurrently."*

Downstream symptoms, cited in the thread:
- denoland/deno#10437 — state drift from concurrent handler execution.
- micahscopes (2024-01-29): `did_close`/`did_open`/`did_change_watched_file`
  arriving in a *different order* under tower-lsp than under `lsp-server` after
  a VS Code file rename.
- fda-odoo (2024-06-19): *"reordering `did_change` notification is totally
  forbidden, especially if we use incremental file cache"* — then, 2024-08-21,
  reported migrating off tower-lsp to `lsp-server` + a hand-rolled message
  manager in 2 days and ~500 lines: *"we dropped all the tower-lsp and tokio
  dependencies, and we are no more using async keyword."*
- posit-dev/ark (lionel-, 2024-08-21): kept tower-lsp but forwards **every**
  handler into their own event loop to regain ordering control.

ebkalderon prototyped a fix on the `support-mutable-methods` branch (2023-03-01)
using `&mut self` receivers, collected positive feedback, and then stalled on
three stated blockers (2023-09-03): no downstream consensus, a competing demand
for configurable concurrency (#386), and a suspicion the monolithic
`LanguageServer` trait needed replacing anyway. It never merged. The repo's last
commit is 2024-08-15.

### Did the fork fix it? No.

`tower-lsp-community/tower-lsp-server#36` — *"How serial should notifications
be?"*, opened 2025-03-19, explicitly *"a continuation of
ebkalderon/tower-lsp#284"*. It is **open with zero comments** 17 months later.
The code confirms it: 0.23.0 and main still route notifications through
`buffer_unordered(DEFAULT_MAX_CONCURRENCY = 4)`, and `Request.id: Option<Id>`
means notifications and requests share that queue. The fork's 0.21.0 change from
`buffered` to `stream_select!`/`buffer_unordered` (#9, "avoid hanging behaviour")
arguably made ordering *more* visible, not less.

What the fork *did* fix, credit where due: the exit hang (#12), the cancellation
panic (#37), `null` params (#41), several `UriExt` path-conversion bugs (#53,
#56, #61, #65), `workspace/symbol` return type (#49), and notebook support
(#15). It has a CHANGELOG, a CoC, a `FEATURES.md` (86 green / 2 yellow / 10 red
across the spec), and a responsive maintainer (mrnossiom). It is a real,
healthy fork. It just did not touch the architectural problem.

Also note the pending disruption: main switched from `ls-types` to
`gen-lsp-types` on 2026-08-15 and is **unreleased**. Anyone adopting 0.23.0
today writes against `tower_lsp_server::ls_types::*` and will have to migrate.

## 7. Why rust-analyzer does not use tower-lsp

Directly stated, twice, by matklad:

**rust-analyzer#3421**, "Should we look at async for rust-analyzer?" (opened
2020-03-03 by kjeremy, linking tower-lsp). matklad, same day:

> No, we have dozens requests per second at most, there's no reason to not use
> blocking APIs.

ebkalderon replied (2020-03-08) that tower-lsp is async chiefly to be
transport-generic (stdio *and* TCP), and *"If `rust-analyzer` aims to stick with
a `stdio` transport for the foreseeable future, then there is indeed no reason
to adopt a non-blocking API."* matklad:

> Yeah, rust-analyzer is a client, and we deliberately choose to work with
> stdio. But even if we chose tcp, I think that wouldn't be different? Like, we
> would just do blocking calls on the socket.

ebkalderon conceded the point: *"You can also achieve the exact same level of
responsiveness with no async/await using a few dedicated threads, no problems
there either."*

**rust-analyzer/lsp-server#26**, "Async version of the server" (bartlomieju,
Deno). matklad: *"Yup, it's design choice that this lib is synchronous,
tower-lsp would be your best bet!"* — and Deno went to tower-lsp, and is now on
its own hard fork (`deno_tower_lsp`).

The architectural corollary matklad gave in rust-analyzer#2879 (2020-01-22):

> rust-analyzer is architectured in such a way that it drains the messages
> stream as fast as possible. In particular, the main loop does not block on
> anything else, all real work is done in the background. *If* we observe that
> the main loop gets stuck, that means we have a bug.

That is the whole design: a non-blocking main loop over a channel, plus a
worker pool. Async is a way to get that; threads are another; for dozens of
requests per second the threads are cheaper.

Two independent confirmations that this is the correct read, from the tower-lsp
side: async-lsp's README says *"One of the reasons async-lsp was made was to
resolve the pain point of concurrent handling not respecting the expected order
of execution"* and *"tower-lsp handles notifications asynchronously, which is
semantically incorrect"*. And ebkalderon himself, in #284, cites rust-analyzer's
sequential `on_notification` as the reference behaviour.

## 8. Adoption, with evidence

Determined by parsing each project's top-level `Cargo.lock` at HEAD on
2026-08-25.

| project | framework | types | notes |
| --- | --- | --- | --- |
| **rust-analyzer** | `lsp-server` **0.7.9** (registry pin) | `gen-lsp-types` 0.11.0 | publishes 0.10.0 from `lib/lsp-server`, ships 0.7.9 |
| **ruff** (astral-sh) | `lsp-server` **0.10.0** | `gen-lsp-types` 0.11.0 | at the bleeding edge; 49.3k stars |
| **wgsl-analyzer** | `lsp-server` **0.10.0** | `gen-lsp-types` 0.11.0 | |
| **texlab** | `lsp-server` **0.8.0** | `lsp-types` 0.97.0 | |
| **starpls** | `lsp-server` 0.7.5 | `lsp-types` 0.94.1 | **the existing Bazel/Starlark LSP**; last push 2025-12-03 |
| **taplo** | `lsp-async-stub` 0.7.0 (own) | `lsp-types` 0.93.2 | own async stub, not a listed framework |
| **tinymist** | `sync-ls` 0.15.4-rc3 (own) | `lsp-types` 0.95.0 | own framework, "inspired by async-lsp" |
| **nil** | **`async-lsp` 0.2.4** | `lsp-types` 0.95.1 | same author as async-lsp (oxalica) |
| **harper-ls** (Automattic) | `tower-lsp-server` 0.22.1 | `lsp-types` 0.97.0 | 14.7k stars |
| **typos-lsp** (tekumara) | `tower-lsp-server` 0.23.0 | `ls-types` 0.0.2 | |
| **oxc** | `tower-lsp-server` 0.23.0 | `ls-types` 0.0.3 | 22.5k stars |
| **biome** | `tower-lsp-server` 0.23.0 | `ls-types` 0.0.2 | 25.6k stars |
| **ark** (posit-dev, R) | `tower-lsp-server` 0.23.0 | `ls-types` 0.0.6 | forwards all handlers to its own event loop |
| **Deno** | `deno_tower_lsp` 0.5.0 (hard fork) | `ls-types` 0.0.3 | left upstream |
| **Turborepo** | `tower-lsp` **0.20.0** | `lsp-types` 0.94.1 | still on the dead crate |
| **marksman** | — | — | **F#/.NET**, not Rust. No bearing. |
| Helix (client) | own `helix-lsp` | `helix-lsp-types` 0.95.1 (fork) | client side |

Read of the table:

- `lsp-server`'s users are the *analysis-heavy, incremental, index-owning*
  servers: rust-analyzer, ruff, wgsl-analyzer, texlab, starpls.
- `tower-lsp-server`'s users are the *stateless-ish linter/formatter* servers:
  biome, oxc, typos, harper. Their per-request work is "run a rule set over one
  file", which is exactly the workload where handler-ordering does not bite.
- The projects that outgrew tower-lsp did not migrate to another framework —
  they forked (Deno) or built their own (taplo, tinymist, odoo-ls) or wrapped it
  in a private event loop (ark).
- **starpls is the single most relevant precedent.** It is a Bazel language
  server, and its architecture is `lsp_server::Connection` + `ReqQueue<(),()>` +
  a 73-line rayon `TaskPool` + a `BazelClient` shelling out to the `bazel` CLI +
  an `AnalysisDebouncer`. That is, almost exactly the design CONTEXT.md
  describes. It has been idle since 2025-12-03, which is an opportunity rather
  than a warning.

## 9. Skeletons — `initialize` + `textDocument/definition`

Both of these were compiled. Candidate A: `cargo build` clean. Candidate B:
`cargo check` clean (`cargo build` fails only on a `-liconv` linker issue in
this nix devshell, unrelated to the code).

### A. `lsp-server` 0.10.0 + `gen-lsp-types` 0.11.0 — 20 crates, 6.0 s cold check

```toml
[dependencies]
lsp-server = "0.10.0"
lsp-types = { version = "0.11.0", package = "gen-lsp-types", features = ["fluent-uri"] }
serde_json = "1"
crossbeam-channel = "0.5"
```

```rust
use std::error::Error;

use lsp_server::{Connection, ExtractError, Message, Request, RequestId, Response};
use lsp_types::{
    Definition, DefinitionParams, DefinitionProvider, DefinitionRequest, DefinitionResponse,
    InitializeParams, Location, Position, Range, ServerCapabilities, TextDocumentSync,
    TextDocumentSyncKind,
};

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    let (conn, io_threads) = Connection::stdio();

    let caps = ServerCapabilities {
        text_document_sync: Some(TextDocumentSync::Kind(TextDocumentSyncKind::Incremental)),
        definition_provider: Some(DefinitionProvider::Bool(true)),
        ..Default::default()
    };
    let init_params = conn.initialize(serde_json::json!({ "capabilities": caps }))?;
    let _init: InitializeParams = serde_json::from_value(init_params)?;

    main_loop(&conn)?;
    io_threads.join()?;
    Ok(())
}

fn main_loop(conn: &Connection) -> Result<(), Box<dyn Error + Sync + Send>> {
    for msg in &conn.receiver {
        match msg {
            Message::Request(req) => {
                if conn.handle_shutdown(&req)? {
                    return Ok(());
                }
                match cast::<DefinitionRequest>(req) {
                    Ok((id, params)) => {
                        let result = goto_definition(&params);
                        conn.sender.send(Message::Response(Response::new_ok(id, result)))?;
                    }
                    Err(ExtractError::MethodMismatch(req)) => {
                        conn.sender.send(Message::Response(Response::new_err(
                            req.id,
                            lsp_server::ErrorCode::MethodNotFound as i32,
                            format!("unhandled method {}", req.method),
                        )))?;
                    }
                    Err(ExtractError::JsonError { .. }) => {}
                }
            }
            Message::Notification(_) | Message::Response(_) => {}
        }
    }
    Ok(())
}

fn goto_definition(params: &DefinitionParams) -> Option<DefinitionResponse> {
    let pos = &params.text_document_position_params;
    Some(DefinitionResponse::Definition(Definition::Location(Location {
        uri: pos.text_document.uri.clone(),
        range: Range::new(Position::new(0, 0), Position::new(0, 0)),
    })))
}

fn cast<R>(req: Request) -> Result<(RequestId, R::Params), ExtractError<Request>>
where
    R: lsp_types::Request,
{
    req.extract(R::METHOD.as_str())
}
```

Visible boilerplate: the `cast` helper (7 lines, written once), the `match` arm
per method, and the fact that dispatch is a hand-written chain. `DefinitionParams`
uses `#[serde(flatten)]` composition, so the position is at
`params.text_document_position_params`, not `params`. Note `R::METHOD` is
`LspRequestMethod<'static>`, not `&str` — hence `.as_str()`.

Realistically the `match` becomes rust-analyzer's builder chain:
```rust
RequestDispatcher { req: Some(req), state: self }
    .on_sync_mut::<Shutdown>(handle_shutdown)
    .on_sync::<SelectionRangeRequest>(handle_selection_range)   // main thread, low latency
    .on::<DefinitionRequest>(handle_goto_definition)            // worker pool + catch_unwind
    .finish();
```
That dispatcher is ~150 lines you write and then never touch.

### B. `tower-lsp-server` 0.23.0 — 54 crates, 7.3 s cold check

```toml
[dependencies]
tower-lsp-server = "0.23.0"
tokio = { version = "1", features = ["io-std", "rt-multi-thread", "macros"] }
```

```rust
use tower_lsp_server::ls_types::{
    GotoDefinitionParams, GotoDefinitionResponse, InitializeParams, InitializeResult, Location,
    OneOf, Position, Range, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
};
use tower_lsp_server::{Client, LanguageServer, LspService, Server, jsonrpc::Result};

struct Backend {
    client: Client,
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                definition_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let pos = params.text_document_position_params;
        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: pos.text_document.uri,
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
        })))
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let (service, socket) = LspService::new(|client| Backend { client });
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service).await;
}
```

Genuinely less boilerplate — dispatch, `$/cancelRequest`, and the lifecycle are
free. What is invisible in this snippet and shows up on line 200 of the real
server: `&self` means the moment `Backend` holds an index, every field becomes
`Arc<RwLock<_>>` or `DashMap`, and every CPU-bound handler needs
`spawn_blocking`. Also note `ls_types`, not `gen_lsp_types` — that rename is on
main and unreleased.

### C. `async-lsp` 0.2.4, for contrast — 86 crates

```rust
let (server, _) = async_lsp::MainLoop::new_server(|client| {
    let mut router = Router::new(ServerState { client, index: Index::new() });
    router
        .request::<request::Initialize, _>(|_, params| async move { .. })
        .request::<request::GotoDefinition, _>(|st, params| {
            let snap = st.index.snapshot();          // &mut self, serialised
            async move { Ok(snap.goto_definition(params)) }   // concurrent, no borrow
        })
        .notification::<notification::DidChangeTextDocument>(|st, p| {
            st.index.apply(p);                        // &mut self, strictly ordered
            ControlFlow::Continue(())
        });

    ServiceBuilder::new()
        .layer(TracingLayer::default())
        .layer(CatchUnwindLayer::default())
        .layer(ConcurrencyLayer::default())
        .layer(ClientProcessMonitorLayer::new(client))
        .service(router)
});
```
The split between the synchronous `&mut self` prologue and the borrow-free
concurrent future is the right idea, and it is the only framework of the three
that has it. It is stranded on `lsp-types` 0.95.0.

---

## RECOMMENDATION

**`lsp-server` 0.10.0 + `gen-lsp-types` 0.11.0 (aliased as `lsp-types`, feature
`fluent-uri`), with a hand-written dispatcher and a rayon/crossbeam worker
pool modelled on rust-analyzer's.**

Pin exactly: `lsp-server = "=0.10.0"`, `lsp-types = { version = "=0.11.0",
package = "gen-lsp-types", features = ["fluent-uri"] }`.

The case, in order of weight:

1. **CONTEXT.md commitment #1 removes the only strong reason to be async.** No
   Bazel call is in the request path; the slow IO lives in a background actor.
   What remains on the request path is CPU-bound work over an index — thread-pool
   work, not reactor work.
2. **`&mut self` on the main loop is worth more than free dispatch.** A two-tier
   index plus an LRU of CSTs is mutable shared state read on the hot path.
   `lsp-server` lets it be a plain field mutated only from the main loop and
   `Arc`-snapshotted for workers. `tower-lsp-server`'s `&self` forces a lock
   around it; `tower-lsp`#284 is 5 years of evidence that this is where the bugs
   are.
3. **Notification ordering is a correctness requirement here.** With
   `MODULE.bazel` edits invalidating repo resolution and `didChangeWatchedFiles`
   invalidating package indices, an out-of-order `didClose`/`didOpen` pair
   (micahscopes' exact report) corrupts the index. `lsp-server` gives ordering by
   construction. `tower-lsp-server` does not, and its issue tracking that has
   zero comments in 17 months.
4. **1,129 lines and 20 dependencies.** Matches the stated "easy to use,
   minimal" preference literally, and it is small enough to read end-to-end and
   vendor if it ever goes bad.
5. **The precedents line up.** starpls (Bazel, `lsp-server` + rayon + BazelClient)
   is our architecture already. rust-analyzer, ruff, texlab and wgsl-analyzer are
   the index-owning servers, and they are all here. The author's
   `vimdoc-language-server` already uses this pair, so the migration is
   `lsp-types` → `gen-lsp-types` and `Response{result,error}` →
   `Response{response_result}`.
6. **`gen-lsp-types` is where the ecosystem converged in 2026.** rust-analyzer
   adopted it 2026-06-19; tower-lsp-server abandoned its own fork for it
   2026-08-15. Whichever framework we picked, this is the types crate.

Cost accepted: ~150 lines of dispatcher, ~70 lines of task pool, and manual
`$/cancelRequest` bookkeeping through `ReqQueue`. Roughly 250 lines, written
once, copied from two working references.

### The two strongest arguments against

**1. The types crate is one person, moving fast, and it is not optional.**
`gen-lsp-types` is 94/131 commits from Riley Bruins, 28 stars, 9 reverse
dependencies, and it shipped **nine breaking releases between 2026-04-21 and
2026-07-27**. The same person also authored the `lsp-server` 0.9.0 `ResponseKind`
change and its 0.10.0 reversal five days later. So the single-maintainer risk is
not diversified across the two crates — it is the *same* maintainer on both, and
the recommendation is "adopt both at their newest versions". If he stops, we
inherit a code generator and a vendored metamodel. That is a recoverable
position (regenerate, or fork), but it is a real, currently-active tax, and it
is worse than it looks because there is no alternative: `lsp-types` 0.97.0 is 26
months stale and `ls-types` was just abandoned by its own authors.

The specific version we are recommending, `lsp-server` 0.10.0, is additionally
**not the version rust-analyzer runs**. rust-analyzer pins 0.7.9 from crates.io
and `handlers/dispatch.rs:271` still uses the 0.7 field layout at master. We
would be relying on ruff and wgsl-analyzer for real-world validation of 0.10.0,
not on rust-analyzer.

**2. We are choosing to write the parts that the other two frameworks give us,
and one of them we might get wrong.** `tower-lsp-server` gives working
`$/cancelRequest` with actual future abortion, `window/workDoneProgress/create`,
`$/progress` helpers, `UriExt` path conversion (which took them four bug fixes
across #53/#55/#56/#65 to get right on Windows and WSL2), notebook documents,
and 86-of-98 spec coverage documented in `FEATURES.md`. `lsp-server` gives us a
`HashMap` and a `Response`. Every one of those we will re-implement, and the
`UriExt` history is a fair warning about how much detail hides in "just convert
a URI to a path". If the server ever grows a genuinely concurrent interaction —
a `workspace/configuration` round-trip inside a handler, or forwarding requests
to a second process — the synchronous design makes that a correlation-id state
machine through `ReqQueue::outgoing` rather than an `.await`, and that is
precisely the case (samscott89, tower-lsp#284) where async is unambiguously
better.

If either of these lands harder than expected, the fallback is **`async-lsp`**,
not `tower-lsp-server`: it is the only one of the three that gets `&mut self`
and notification ordering right. Its blocker is that it pins `lsp-types` 0.95.0
and closed the migration PR (#27) unmerged; if it adopts `gen-lsp-types`,
reconsider.

---

### Where I am uncertain

- **Whether `lsp-server` 0.10.0 is final.** Two of the three recent releases
  reshaped the same struct. I have no evidence of further planned changes, but
  the fact that rust-analyzer's own pin is still 0.7.9 means there is no
  migration pressure forcing the API to settle. Pin exactly and read the diff
  before bumping.
- **Why `async-lsp#27` was closed.** GitHub rate-limited me before I could read
  the close comment. The fact of the close (same day, unmerged, by the repo
  owner) is verified; the reason is not.
- **`gen-lsp-types` completeness against LSP 3.18.** I verified the vendored
  `metaModel.json` reports `{"version": "3.18.0"}` with 69 requests, 26
  notifications, 387 structures, 40 enumerations, and that the generator emits
  all of them. I did not diff the generated output against a client's actual
  wire traffic. "100% accurate to the protocol" is the project's claim, not a
  measurement of mine.
- **tower-lsp-server's real-world ordering exposure.** I established that
  notifications and requests share a `buffer_unordered(4)` queue by reading
  `transport.rs` and `jsonrpc/request.rs`. I did not write a test that
  demonstrates a reordered `didChange` in practice. biome, oxc and harper ship on
  it without visible complaint — which is consistent with the theory that the bug
  only bites servers with mutable cross-request state, but is not proof.
