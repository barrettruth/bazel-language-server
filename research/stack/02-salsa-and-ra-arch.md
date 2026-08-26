# salsa, and rust-analyzer's threading/snapshot architecture

Probes used: `/tmp/ra-probe` (rust-analyzer HEAD `014d54b`, 2026-08-25),
`/tmp/salsa-probe` (salsa-rs/salsa, full history, checked out at tag
`salsa-v0.28.2`), `/tmp/comemo-probe` (typst/comemo `5944487`, 2026-03-13),
`/tmp/salsa-min` (a throwaway crate I built and ran). Machine: darwin aarch64,
18 cores, `cargo 1.98.0` / `rustc 1.98.0` (stable).

---

## 1. salsa today

### Version and identity

| fact | value |
| --- | --- |
| latest | **0.28.2**, published **2026-08-03T08:02Z** (changelog dates the release 2026-08-01) |
| downloads | 7,528,097 all-time; **2,144,782** in the last 90 days; 149,095 for 0.28.2 alone |
| repo | github.com/salsa-rs/salsa — 2,945 stars, **70 open issues** (91 open issues+PRs), last push 2026-08-21 |
| license | Apache-2.0 OR MIT |
| MSRV | 1.85 |
| self-description | *"A generic framework for on-demand, incrementalized computation (**experimental**)"* |
| reverse deps | 113 crates. Notable: all of `ra_ap_*`, all of `cairo-lang-*` (Starkware), `ruff_python_ast` (Astral's `ty`), `pilota-build` |

Feature list as published (`crates.io/api/v1/crates/salsa/0.28.2`), plus the
implicit optional-dependency features from `Cargo.toml`:

```toml
default        = ["salsa_unstable", "rayon", "macros", "inventory", "accumulator"]
salsa_unstable = []          # gates db.memory_usage() / IngredientInfo / PageInfo
macros         = ["dep:salsa-macros"]
inventory      = ["dep:inventory"]        # static ingredient auto-registration
accumulator    = ["salsa-macro-rules/accumulator"]
persistence    = ["dep:serde", "dep:erased-serde", ...]
detailed-trace = []
shuttle        = ["dep:shuttle"]
# implicit, from `optional = true` deps:
rayon, triomphe, compact_str, ordermap
```

`triomphe` is an implicit feature: `/tmp/salsa-probe/Cargo.toml` has
`triomphe = { version = "0.1", optional = true }`, and the only thing it does is
`unsafe impl<T: ?Sized + SalsaValue> SalsaValue for triomphe::Arc<T> {}`
(`src/salsa_value.rs:147-149`). That is the entire feature. rust-analyzer enables
it because `base-db` stores `triomphe::Arc<str>` file texts inside salsa inputs.

`salsa_unstable` is likewise narrow: it gates `<dyn Database>::memory_usage()`
and the `IngredientInfo`/`PageInfo` types (`src/database.rs:400-420`,
`src/lib.rs:294`). rust-analyzer's "Memory Usage" command is the consumer.
Note the comment in salsa's own `Cargo.toml`: `# FIXME: remove salsa_unstable
before 1.0.`

### Which API is current, and how much churn

There have been two salsas.

- **Old salsa** (`#[salsa::query_group]`, `#[salsa::database(...)]`,
  `Snapshot<DB>`, `db.snapshot()`): last release `0.17.0-pre.2`, **2021-10-06**.
  Anything you read on the internet using `query_group` is at least four years
  stale.
- **New salsa** (`#[salsa::db]`, `#[salsa::input]`, `#[salsa::tracked]`,
  `#[salsa::interned]`): developed in-tree under the name `salsa-2022`, renamed
  by commit `c7851112` *"Rename `salsa-2022` to `salsa`"* on **2024-06-18**,
  first published to crates.io as **0.18.0 on 2025-02-20**. There is no
  `salsa-2022` crate on crates.io — it never escaped the workspace.

The current API is unambiguously the second one. Churn since it went public:

| version | date | notable breaking change |
| --- | --- | --- |
| 0.18.0 | 2025-02-20 | first public release of the redesign |
| 0.19.0 | 2025-03-10 | |
| 0.20.0 | 2025-04-22 | |
| 0.21.0 | 2025-04-29 | `values_equal` signature fixed |
| 0.22.0 | 2025-05-23 | **`return_ref` → `returns(as_ref)` / `returns(cloned)`** (#772); default `PartialOrd`/`Ord` derives removed from salsa structs (#868) |
| 0.23.0 | 2025-06-27 | MSRV → 1.85; generational tracked-struct IDs; interned LRU GC |
| 0.24.0 | 2025-10-05 | `accumulator` moved behind a feature flag (#946); `entries` API refactor (#987); `inventory` registration (#934) |
| 0.25.0 | 2025-12-16 | interned fields must be `Update` (#1036); `parallel` feature removed (#1013); cycle-recovery signature changed twice (#1012, #1015) |
| 0.26.0 | 2026-02-07 | database forking removed (#1049); compile-time LRU opt-out (#1051); `cycle_fallback` → `cycle_result` |
| 0.27.0 | 2026-06-04 | |
| 0.28.0 | 2026-07-12 | **`Update` trait → `SalsaValue` + `PartialEq`** (#1217); **references returned by default** (#1216) |
| 0.28.2 | 2026-08-03 | current |

**Eleven minor releases in 17.5 months**, i.e. a breaking bump roughly every six
weeks, and at least four of them renamed or re-shaped core surface that every
call site touches. Concretely: rust-analyzer HEAD carries **60 `SalsaValue`
derive/import sites** across `hir-def`, `hir-ty`, `hir-expand`, `base-db` — every
one of which said `Update` two months ago.

That is the honest cost picture: salsa is actively, competently maintained, and
it will keep moving under you. It is pre-1.0 and says so.

### A minimal working example — actually compiled

This is the literal contents of `/tmp/salsa-min/src/lib.rs`. I ran
`cargo test` against `salsa = "0.28.2"` on stable 1.98.0 and **it compiles and
the test passes**, including every `assert_eq!` on the execution counters. The
counters are the point: they *prove* the memoisation and backdating claims rather
than asserting them.

```rust
#![forbid(unsafe_code)]
//! Minimal salsa 0.28.2 example: one input, two tracked fns, LRU, cancellation.

use std::sync::atomic::{AtomicUsize, Ordering};

pub static PARSES: AtomicUsize = AtomicUsize::new(0);
pub static COUNTS: AtomicUsize = AtomicUsize::new(0);

/// An *input*: the only thing you `set`. Owned by the database, versioned.
#[salsa::input(debug)]
pub struct SourceFile {
    pub path: String,
    #[returns(ref)]
    pub text: String,
}

/// A *tracked fn* is a memoized query; dependencies are recorded automatically.
/// `lru = 128` bounds retained memos; eviction runs at the start of a revision.
#[salsa::tracked(returns(ref), lru = 128)]
pub fn parse(db: &dyn salsa::Database, file: SourceFile) -> Vec<String> {
    db.unwind_if_revision_cancelled();
    PARSES.fetch_add(1, Ordering::SeqCst);
    file.text(db).split_whitespace().map(str::to_owned).collect()
}

/// A derived query over another query. Re-runs only if `parse`'s value changed.
#[salsa::tracked(returns(copy))]
pub fn word_count(db: &dyn salsa::Database, file: SourceFile) -> usize {
    COUNTS.fetch_add(1, Ordering::SeqCst);
    parse(db, file).len()
}

/// A concrete database. `salsa::Storage<Self>` is the whole implementation.
#[salsa::db]
#[derive(Default, Clone)]
pub struct Db {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Db {}

#[cfg(test)]
mod tests {
    use super::*;
    use salsa::Setter as _; // brings `.to(..)` into scope

    #[test]
    fn incremental() {
        let mut db = Db::default();
        let f = SourceFile::new(&db, "BUILD".to_owned(), "a b c".to_owned());
        let seen = || (PARSES.load(Ordering::SeqCst), COUNTS.load(Ordering::SeqCst));

        assert_eq!(word_count(&db, f), 3);
        assert_eq!(seen(), (1, 1));

        // Same revision: pure memo hit, neither body runs.
        assert_eq!(word_count(&db, f), 3);
        assert_eq!(seen(), (1, 1));

        // Edit the text -> new revision; `parse` differs, so both re-run.
        f.set_text(&mut db).to("a b c d".to_owned());
        assert_eq!(word_count(&db, f), 4);
        assert_eq!(seen(), (2, 2));

        // Edit `path`, which `parse` never read: `parse` is not re-run at all,
        // so neither is `word_count`. Per-field dependency tracking.
        f.set_path(&mut db).to("BUILD.bazel".to_owned());
        assert_eq!(word_count(&db, f), 4);
        assert_eq!(seen(), (2, 2));

        // Edit the text to something that parses equal: `parse` re-runs, salsa
        // *backdates* the equal result, and `word_count` is not re-run.
        f.set_text(&mut db).to("a  b   c    d".to_owned());
        assert_eq!(word_count(&db, f), 4);
        assert_eq!(seen(), (3, 2));

        // Runtime LRU tuning; generated only for fns declared with `lru = N`.
        parse::set_lru_capacity(&mut db, 8);

        // A "snapshot" is a clone of the db handle. `&Db` is Send + Sync; the
        // next `&mut Db` blocks until every clone is dropped (see cancellation).
        let snap = db.clone();
        let h = std::thread::spawn(move || word_count(&snap, f));
        assert_eq!(h.join().unwrap(), 4);
    }
}
```

```
running 1 test
test tests::incremental ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Three things this establishes that the docs do not make obvious:

1. **`#![forbid(unsafe_code)]` is fine.** I also verified separately that
   `#[salsa::interned]`, `#[salsa::interned(unsafe(no_lifetime), revisions = usize::MAX)]`
   and `#[salsa::tracked] struct` all compile under `forbid`. The `unsafe(...)`
   in the attribute is a macro token, not an `unsafe` block in your crate. No
   conflict with the house convention.
2. **`clippy::pedantic` is fine.** `cargo clippy --all-targets -- -W clippy::pedantic`
   on this crate produced exactly one salsa-adjacent warning
   (`elided_lifetimes` on my own `pkg_of<'db>` signature); everything else came
   from my benchmark's float casts. salsa squelches lints in generated code
   (#809).
3. **`Setter` must be in scope at the call site** for `.to(..)`. `use salsa::Setter as _;`
   in a parent module does *not* propagate through `use super::*`. Minor, but it
   is the first thing that fails to compile.

---

## 2. What salsa buys, what it costs

### Buys

- **Automatic, per-field dependency tracking.** Proved above: mutating `path`
  did not re-run `parse`, because `parse` only ever read `text`.
- **Backdating.** A recomputed value equal to the old one does not propagate.
  Proved above: `PARSES` went 2→3 while `COUNTS` stayed at 2. This requires
  `PartialEq` on every memoised value (since 0.28.0, via `SalsaValue`).
- **Durability tiers** (`src/durability.rs`): `LOW` / `MEDIUM` / `HIGH` /
  `NEVER_CHANGE`. If only low-durability inputs changed, queries that read only
  medium-or-higher inputs skip dependency enumeration entirely. This is what
  makes rust-analyzer's stdlib not get revalidated on every keystroke.
- **Cancellation** as a first-class protocol (§3).
- **Interning** with garbage collection of low-durability interned values.
- **Cycle recovery** (fixpoint iteration) — irrelevant to us; Starlark `load()`
  cycles are an error, not something to converge.
- **Memory introspection** under `salsa_unstable`.

### Costs

- **33 transitive crates** at the minimum useful feature set
  (`default-features = false, features = ["macros","inventory"]`), measured with
  `cargo tree`. With defaults (adds `rayon`, `accumulator`, `salsa_unstable`) it
  is 38. Clean release build of that tree: **28.4 CPU-seconds / 5.8 s wall on 18
  cores** (`cargo build --timings`; salsa itself 1.33 s, `syn 3` 1.30 s).
- **Every memoised value must be `SalsaValue` + `PartialEq`.** `SalsaValue` is an
  `unsafe` marker trait about in-place updatability; you derive it, but it is a
  bound that propagates through your whole type graph. rowan's `SyntaxNode` is
  `!Send` and can never live in the db — only `GreenNode` can, which is exactly
  why rust-analyzer stores `syntax::Parse<T> { green: Option<GreenNode>, errors: Option<Arc<[SyntaxError]>> }`
  (`crates/syntax/src/lib.rs:75-79`) rather than a `SyntaxNode`.
- **Per-memo metadata is not free.** From salsa's own
  `tests/memory-usage.rs` expectations (64-bit): `MyInput` 96 bytes of metadata
  for 3 inputs (**32 B/input**); query `input_to_string` 32 B for 1 memo;
  `input_to_interned` 144 B for 3 (**48 B/memo**); `input_to_tracked` 240 B for 2
  (**120 B/memo**). So **~32–120 bytes of pure bookkeeping per memoised entry**,
  before the value.
- **Writes are exclusive and block on readers** (§3). Every revision bump, and
  every LRU eviction, requires `&mut Database`, which stalls until every
  outstanding snapshot is dropped.
- The churn treadmill from §1.

### LRU eviction — yes, 0.28 still has it

This matters for us specifically ("full CSTs for open files only plus a small
LRU"), so here is the whole mechanism.

**Declaration.** `components/salsa-macros/src/lib.rs:443-446`:

> `lru = INTEGER` bounds the number of memoized values retained by the function
> and sets the initial capacity used by `FUNCTION::set_lru_capacity`.

```rust
#[salsa::tracked(lru = 128)]
fn parse(db: &dyn Db, input: SourceFile) -> Ast { .. }

parse::set_lru_capacity(&mut db, 256);   // requires &mut db
```

`set_lru_capacity` is generated **only** for functions declared with `lru = N`
(`components/salsa-macro-rules/src/setup_tracked_fn.rs:501-507`, bounded on
`Eviction: HasCapacity`). `lru` and `specify` are mutually exclusive
(`tracked_fn.rs:153-156`).

**Zero cost when unused.** 0.26.0 (#1051) made eviction a compile-time-selected
policy: `src/function/eviction.rs` defines `trait EvictionPolicy`, with `Lru` and
`NoopEviction` impls. Without `lru = N` you get `NoopEviction`, which the
optimiser removes.

**The implementation** (`src/function/eviction/lru.rs`) is a
`Mutex<FxLinkedHashSet<Id>>`; `record_use` inserts on each fetch;
`for_each_evicted` pops from the front while `len() > capacity`.

**The catch, and it is a real one for us.** Eviction only ever runs from
`Ingredient::reset_for_new_revision` (`src/function.rs:497-507`), and that is
called from exactly two places in `src/zalsa.rs`, both requiring `&mut Zalsa`:

```rust
// src/zalsa.rs:455-476
    let new_revision = self.runtime.new_revision();
    for ingredient in &self.ingredients_requiring_reset {
        self.ingredients_vec[ingredient.as_u32() as usize]
            .reset_for_new_revision(self.runtime.table_mut());
    }
    new_revision
}

/// **NOT SEMVER STABLE**
#[doc(hidden)]
pub fn evict_lru(&mut self) {
    for ingredient in &self.ingredients_requiring_reset {
        self.ingredients_vec[ingredient.as_u32() as usize]
            .reset_for_new_revision(self.runtime.table_mut());
    }
}
```

And the public entry point warns explicitly (`src/database.rs:38-47`):

```rust
    /// Enforces current LRU limits, evicting entries if necessary.
    ///
    /// **WARNING:** Just like an ordinary write, this method triggers
    /// cancellation. If you invoke it while a snapshot exists, it
    /// will block until that snapshot is dropped -- if that snapshot
    /// is owned by the current thread, this could trigger deadlock.
    fn trigger_lru_eviction(&mut self) { .. }
```

Consequences:

- **Memory is never reclaimed while the server is idle.** If a "find references"
  sweep parses 5,000 files and then the user stops typing, nothing is evicted
  until the next revision bump. rust-analyzer papers over this with
  `synthetic_write(Durability::LOW)` after cache priming.
- **Reclaiming memory cancels every in-flight request.**
- LRU evicts the *value*, not the memo. Per `book/src/tuning.md`: *"LRU evicts
  memoized values, not query keys or dependency metadata."* The 32–120 B/entry
  bookkeeping is monotonic for the life of the database.

**And rust-analyzer's runtime LRU configuration is currently dead code.**
`crates/ide-db/src/lib.rs:219-253`:

```rust
    pub fn update_base_query_lru_capacities(&mut self, _lru_capacity: Option<u16>) {
        // let lru_capacity = lru_capacity.unwrap_or(base_db::DEFAULT_PARSE_LRU_CAP);
        // base_db::FileTextQuery.in_db_mut(self).set_lru_capacity(DEFAULT_FILE_TEXT_LRU_CAP);
        // base_db::ParseQuery.in_db_mut(self).set_lru_capacity(lru_capacity);
        ...
    }

    pub fn update_lru_capacities(&mut self, _lru_capacities: &FxHashMap<Box<str>, u16>) {
        // FIXME(salsa-transition): bring this back; allow changing LRU settings at runtime.
        ...
    }
```

The `rust-analyzer.lru.capacity` and `rust-analyzer.lru.query.capacities`
settings still exist in `config.rs:376-379` and are plumbed through
`reload.rs:95-99` — into a no-op. Only the compile-time constants are live:
`#[salsa::tracked(lru = 128)]` on `EditionedFileId::parse`
(`base-db/src/editioned_file_id.rs:22`), `lru = 512` on `body_with_source_map`
(`hir-def/src/expr_store/body.rs:89`), `lru = 250`/`50` on four attribute
queries. Likewise `AnalysisHost::trigger_garbage_collection`
(`ide/src/lib.rs:215-223`) has `self.db.trigger_lru_eviction()` commented out
with *"currently bugged wrt to cancellation"*, substituting a synthetic write.

That is 18 months post-migration, in the flagship consumer, by the people who
maintain both crates. Treat it as the realistic estimate of how much salsa
integration polish costs.

---

## 3. rust-analyzer's use: `base_db`, `SourceDatabase`, `Cancelled`, snapshots

### `base_db` — inputs live in a hand-rolled map, not in salsa

`crates/base-db/src/lib.rs`. The salsa inputs are tiny wrappers:

```rust
// base-db/src/lib.rs:215-232
#[salsa::input(debug)]
pub struct FileText {
    #[returns(ref)]
    pub text: Arc<str>,        // triomphe::Arc
    pub file_id: vfs::FileId,
}

#[salsa::input(debug)]
pub struct FileSourceRootInput { #[returns(copy)] pub source_root_id: SourceRootId }

#[salsa::input(debug)]
pub struct SourceRootInput { #[returns(clone)] pub source_root: Arc<SourceRoot> }
```

and the *mapping* from `FileId` to those inputs is a plain `DashMap` outside
salsa (`base-db/src/lib.rs:70-75`):

```rust
pub struct Files {
    files: Arc<DashMap<vfs::FileId, FileText, BuildHasherDefault<FxHasher>>>,
    source_roots: Arc<DashMap<SourceRootId, SourceRootInput, ...>>,
    file_source_roots: Arc<DashMap<vfs::FileId, FileSourceRootInput, ...>>,
}
```

This is the *"on-demand inputs"* pattern from salsa's book. It is worth noting
because it is the seam: **the file→text map is ordinary concurrent-map code that
salsa never sees.** Only the versioned cell is salsa's.

`SourceDatabase` (`base-db/src/lib.rs:234-281`) is a hand-written trait
(`#[salsa::db] pub trait SourceDatabase: salsa::Database + Debug`), not a
generated query group — the `query_group` macro no longer exists in the new
salsa; rust-analyzer publishes a compat shim as `ra_ap_query-group-macro`
(0.0.343, 2026-07-20, 740k downloads) but its own tree does not contain that
crate at HEAD. Methods are plain trait methods that delegate to the `DashMap`
or to salsa:

```rust
#[salsa::db]
pub trait SourceDatabase: salsa::Database + std::fmt::Debug {
    fn file_text(&self, file_id: vfs::FileId) -> FileText;
    fn set_file_text(&mut self, file_id: vfs::FileId, text: &str);
    fn set_file_text_with_durability(&mut self, file_id: vfs::FileId, text: &str, durability: Durability);
    fn source_root(&self, id: SourceRootId) -> SourceRootInput;
    fn file_source_root(&self, id: vfs::FileId) -> FileSourceRootInput;
    ...
    fn nonce_and_revision(&self) -> (Nonce, salsa::Revision);
}
```

The real derived queries hang off interned keys. The parse query is the canonical
one (`base-db/src/editioned_file_id.rs:13-39`):

```rust
#[salsa::interned(debug, constructor = from_span_file_id, unsafe(no_lifetime), revisions = usize::MAX)]
#[derive(PartialOrd, Ord)]
pub struct EditionedFileId {
    #[returns(copy)]
    field: span::EditionedFileId,
}

#[salsa::tracked]
impl EditionedFileId {
    #[salsa::tracked(lru = 128, returns(clone))]
    pub fn parse(self, db: &dyn SourceDatabase) -> syntax::Parse<ast::SourceFile> {
        let (file_id, edition) = self.unpack(db);
        let text = db.file_text(file_id).text(db);
        ast::SourceFile::parse(text, edition)
    }

    // firewall query
    #[salsa::tracked(returns(as_deref))]
    pub fn parse_errors(self, db: &dyn SourceDatabase) -> Option<Box<[SyntaxError]>> { .. }
}
```

Note `parse_errors` labelled "firewall query": a cheap projection whose *value*
rarely changes, so that consumers of syntax errors don't get invalidated every
time the tree changes shape. That idiom is the main day-to-day skill salsa
demands.

### `RootDatabase` — the concrete database, and what a clone is

`crates/ide-db/src/lib.rs:82-115`:

```rust
pub struct RootDatabase {
    // We use `ManuallyDrop` here because every codegen unit that contains a
    // `&RootDatabase -> &dyn OtherDatabase` cast will instantiate its drop glue
    // in the vtable, which duplicates `Weak::drop` and `Arc::drop` tens of
    // thousands of times, which makes compile times of all `ide_*` and
    // downstream crates suffer greatly.
    storage: ManuallyDrop<salsa::Storage<Self>>,
    files: Arc<Files>,
    crates_map: Arc<CratesMap>,
    nonce: Nonce,
}

impl std::panic::RefUnwindSafe for RootDatabase {}

impl Clone for RootDatabase {
    fn clone(&self) -> Self {
        Self { storage: self.storage.clone(), files: self.files.clone(),
               crates_map: self.crates_map.clone(), nonce: self.nonce }
    }
}
```

(That `ManuallyDrop` comment is itself a data point about what a salsa database
does to compile times at scale.)

### The snapshot model

There is no `Snapshot<DB>` type any more. **A snapshot is a clone of the
database handle.** `crates/ide/src/lib.rs:192-245`:

```rust
    /// Returns a snapshot of the current state, which you can query for
    /// semantic information.
    pub fn analysis(&self) -> Analysis {
        Analysis { db: self.db.clone() }
    }

    /// Applies changes to the current state of the world. If there are
    /// outstanding snapshots, they will be canceled.
    pub fn apply_change(&mut self, change: ChangeWithProcMacros) -> Duration {
        self.db.apply_change(change)
    }
...
/// Analysis is a snapshot of a world state at a moment in time. It is the main
/// entry point for asking semantic information about the world. When the world
/// state is advanced using `AnalysisHost::apply_change` method, all existing
/// `Analysis` are canceled (most method return `Err(Canceled)`).
#[derive(Debug)]
pub struct Analysis { db: RootDatabase }
```

`Analysis` is `Send` (asserted by a test at `ide/src/lib.rs:979-983`), so it
ships to a worker thread. Every public `Analysis` method funnels through:

```rust
// ide/src/lib.rs:958-976
    /// Performs an operation on the database that may be canceled.
    ///
    /// rust-analyzer needs to be able to answer semantic questions about the
    /// code while the code is being modified. A common problem is that a
    /// long-running query is being calculated when a new change arrives.
    ///
    /// We can't just apply the change immediately: this will cause the pending
    /// query to see inconsistent state (it will observe an absence of
    /// repeatable read). So what we do is we **cancel** all pending queries
    /// before applying the change.
    ///
    /// Salsa implements cancellation by unwinding with a special value and
    /// catching it on the API boundary.
    fn with_db<F, T>(&self, f: F) -> Cancellable<T>
    where
        F: FnOnce(&RootDatabase) -> T + std::panic::UnwindSafe,
    {
        hir::attach_db_allow_change(&self.db, || Cancelled::catch(|| f(&self.db)))
    }
```

with `pub type Cancellable<T> = Result<T, Cancelled>;` (`ide/src/lib.rs:154`).

### How consistency is actually enforced

The mechanism is a refcount and a condvar, in `salsa/src/storage.rs:152-191`.
This is the single most important 30 lines in the whole design:

```rust
    // ANCHOR: cancel_other_workers
    /// Sets cancellation flag and blocks until all other workers with access
    /// to this storage have completed.
    ///
    /// This could deadlock if there is a single worker with two handles to the
    /// same database!
    ///
    /// Needs to be paired with a call to `reset_cancellation_flag`.
    fn cancel_others(&mut self) -> &mut Zalsa {
        debug_assert!(
            self.zalsa_local.try_with_query_stack(|stack| stack.is_empty()) == Some(true),
            "attempted to cancel within query computation, this is a deadlock"
        );
        {
            let _cancellation_flag = CancellationFlagGuard::new(&self.handle.zalsa_impl);

            self.handle.zalsa_impl.event(&|| Event::new(EventKind::DidSetCancellationFlag));

            let mut clones = self.handle.coordinate.clones.lock();
            while *clones != 1 {
                clones = self.handle.coordinate.cvar.wait(clones);
            }
        }

        // The ref count on the `Arc` should now be 1
        let zalsa = Arc::get_mut(&mut self.handle.zalsa_impl).unwrap();
        let overflow = zalsa.runtime_mut().bump_cancellation_count();
        if overflow { zalsa.new_revision(); }
        zalsa
    }
    // ANCHOR_END: cancel_other_workers
```

`zalsa_mut()` (line 249-250) is just `self.storage_mut().cancel_others()`, and
*every* `&mut Database` operation goes through it. So:

- A snapshot is a `StorageHandle` clone; the clone increments
  `Coordinate.clones`.
- A write sets a global cancellation flag, then **blocks on a condvar until the
  clone count drops back to 1**.
- Readers notice the flag at the next query boundary and unwind with
  `Cancelled::throw()`, which uses `panic::resume_unwind` specifically to avoid
  running the panic hook (`src/cancelled.rs:26-29`).
- `Cancelled` has three variants (`src/cancelled.rs:12-21`): `Local`,
  `PendingWrite`, `PropagatedPanic`.
- `Cancelled::catch` downcasts the payload; anything else is re-raised
  (`src/cancelled.rs:31-43`).

Queries that loop without calling other queries must poll manually:
`db.unwind_if_revision_cancelled()` (`src/database.rs:111-117`). rust-analyzer's
cache-priming worker loop calls it every iteration
(`ide-db/src/prime_caches.rs`), and wraps each unit of work in
`Cancelled::catch`.

rust-analyzer's own summary, `docs/book/src/contributing/architecture.md:377-388`:

> The salsa database maintains a global revision counter. When applying a
> change, salsa bumps this counter and waits until all other threads using salsa
> finish. If a thread does salsa-based computation and notices that the counter
> is incremented, it panics with a special value (see `Canceled::throw`). That
> is, rust-analyzer requires unwinding. `ide` is the boundary where the panic is
> caught and transformed into a `Result<T, Cancelled>`.

**This is the architectural crux.** rust-analyzer does *not* get lock-free
readers. It gets a single-writer/many-reader scheme where the writer stalls until
readers evacuate, and readers evacuate by panicking. The whole cancellation
apparatus exists because salsa's storage is mutated in place rather than
replaced.

---

## 4. The main loop, concretely

Files: `crates/rust-analyzer/src/main_loop.rs` (1,480 lines),
`global_state.rs` (1,008), `handlers/dispatch.rs` (443),
`task_pool.rs` (55), `crates/stdx/src/thread/`.

### Threads

Created in `GlobalState::new` (`global_state.rs:233-256`):

| pool | size | purpose |
| --- | --- | --- |
| main loop | 1 | owns `GlobalState`; on Windows it raises its own thread priority to avoid being displaced by worker QoS boosts (`main_loop.rs:44-61`) |
| `task_pool` | `config.main_loop_num_threads()` | all async request handling |
| `fmt_pool` | **1** | rustfmt only, so formatting can never be stuck behind analysis |
| `cancellation_pool` | **1** | exists solely to overlap `trigger_cancellation` with other work |
| `loader` | vfs-notify's own | filesystem watching |
| flycheck / test-runner / discover | ad-hoc | `cargo check` etc. |

`stdx::thread::Pool` carries a `ThreadIntent` (`stdx/src/thread/intent.rs`):
`Worker` or `LatencySensitive`, mapped to Darwin QoS classes / Linux nice
values. It is a scheduler hint, not a queue.

### The event loop

`main_loop.rs:203-215`, `274-313`:

```rust
        while let Ok(event) = self.next_event(&inbox) {
            let Some(event) = event else {
                anyhow::bail!("client exited without proper shutdown sequence");
            };
            if matches!(&event, Event::Lsp(Message::Notification(Notification { method, .. }))
                        if method == lsp_types::ExitNotification::METHOD.as_str())
            { return Ok(()); }
            self.handle_event(event);
        }
...
    fn next_event(&mut self, inbox: &Receiver<lsp_server::Message>)
        -> Result<Option<Event>, crossbeam_channel::RecvError>
    {
        // Make sure we reply to formatting requests ASAP so the editor doesn't block
        if let Ok(task) = self.fmt_pool.receiver.try_recv() {
            return Ok(Some(Event::Task(task)));
        }

        select! {
            recv(inbox)                        -> msg  => return Ok(msg.ok().map(Event::Lsp)),
            recv(self.task_pool.receiver)      -> task => task.map(Event::Task),
            recv(self.deferred_task_queue.receiver) -> task => task.map(Event::DeferredTask),
            recv(self.fmt_pool.receiver)       -> task => task.map(Event::Task),
            recv(self.loader.receiver)         -> task => task.map(Event::Vfs),
            recv(self.flycheck_receiver)       -> task => task.map(Event::Flycheck),
            recv(self.test_run_receiver)       -> task => task.map(Event::TestResult),
            recv(self.discover_receiver)       -> task => task.map(Event::DiscoverProject),
            recv(self.fetch_ws_receiver.as_ref().map_or(&never(), |(chan, _)| chan)) -> _instant => {
                Ok(Event::FetchWorkspaces(self.fetch_ws_receiver.take().unwrap().1))
            },
        }
        .map(Some)
    }
```

Plain `crossbeam_channel::select!` over eight receivers, no async runtime
anywhere. Each arm then **coalesces**: after handling one event of a kind, it
`try_recv`s more of the same kind while `loop_start.elapsed() < 50 ms`
(`main_loop.rs:335-339, 347-351, 448-452, 467-471, 480-484, 489-493`). That is
the entire batching strategy: a 50 ms drain window per loop turn.

### Request dispatch

`handlers/dispatch.rs:22-35` states the taxonomy:

```rust
/// A visitor for routing a raw JSON request to an appropriate handler function.
///
/// Most requests are read-only and async and are handled on the threadpool
/// (`on` method).
///
/// Some read-only requests are latency sensitive, and are immediately handled
/// on the main loop thread (`on_sync`). These are typically typing-related
/// requests.
///
/// Some requests modify the state, and are run on the main thread to get
/// `&mut` (`on_sync_mut`).
///
/// Read-only requests are wrapped into `catch_unwind` -- they don't modify the
/// state, so it's OK to recover from their failures.
```

The `on_request` body (`main_loop.rs:1322`+) is one long builder chain; each
`.on::<RETRY, R>(handler)` peels off the request if the method matches. The
worker path is `dispatch.rs:247-277`:

```rust
        let world = self.global_state.snapshot();
        if RUSTFMT { &mut self.global_state.fmt_pool.handle }
        else       { &mut self.global_state.task_pool.handle }
        .spawn(intent, move || {
            let result = panic::catch_unwind(move || {
                let _pctx = DbPanicContext::enter(panic_context);
                f(world, params)
            });
            match thread_result_to_response::<R>(req.id.clone(), result) {
                Ok(response) => Task::Response(response),
                Err(_cancelled) if ALLOW_RETRYING => Task::Retry(req),
                Err(_cancelled) => {
                    let error = on_cancelled();
                    Task::Response(Response { id: req.id, result: None, error: Some(error) })
                }
            }
        });
```

So: snapshot on the main thread → move to a worker → run under `catch_unwind` →
send a `Task` back through the channel. On cancellation, either the request is
**retried** (re-dispatched from `handle_task`, `main_loop.rs:859-861`:
`Task::Retry(req) if !self.is_completed(&req) => self.on_request(req)`) or
answered with LSP `ContentModified` (`dispatch.rs:304-310`). Completion,
document-symbol, folding-range and semantic-tokens are `RETRY`; goto-definition
and inlay-hints are `NO_RETRY`.

### Where the write lock is taken

Exactly one place, and it is at the **end of every loop turn**, not in the
handlers. `main_loop.rs:499-512`:

```rust
        let event_handling_duration = loop_start.elapsed();
        let ((state_changed, changes_cancellation_time), memdocs_added_or_removed) =
            if self.vfs_done {
                if let Some(cause) = self.wants_to_switch.take() {
                    cancellation_time = ...self.switch_workspaces(cause)...;
                }
                (self.process_changes(), self.mem_docs.take_changes())
            } else {
                ((false, None), false)
            };
```

`GlobalState::process_changes` (`global_state.rs:340-449`) is where VFS deltas
become salsa writes, and it is carefully choreographed to hide the stall:

```rust
        let mut change = ChangeWithProcMacros::default();
        let mut guard = self.vfs.write();
        let changed_files = guard.0.take_changes();
        if changed_files.is_empty() { return (false, None); }

        let (change, modified_rust_files, workspace_structure_change) =
            self.cancellation_pool.scoped(|s| {
                // start cancellation in parallel,
                // allowing us to do meaningful work while waiting
                let analysis_host = AssertUnwindSafe(&mut self.analysis_host);
                s.spawn(thread::ThreadIntent::LatencySensitive, || {
                    { analysis_host }.0.trigger_cancellation()
                });

                // downgrade to read lock to allow more readers while we are normalizing text
                let guard = RwLockWriteGuard::downgrade_to_upgradable(guard);
                let vfs: &Vfs = &guard.0;
                ...
                for file in changed_files.into_values() {
                    ...
                    let text = ...String::from_utf8(v).ok().map(|text| {
                        let (text, line_endings) = LineEndings::normalize(text);
                        (text, line_endings)
                    })...;
                    // delay `line_endings_map` changes until we are done normalizing the text
                    // this allows delaying the re-acquisition of the write lock
                    bytes.push((file.file_id, text));
                }
                let (vfs, line_endings_map) = &mut *RwLockUpgradableReadGuard::upgrade(guard);
                bytes.into_iter().for_each(|(file_id, text)| { ... change.change_file(file_id, text); });
                if has_structure_changes { change.set_roots(self.source_root_config.partition(vfs)); }
                (change, modified_rust_files, workspace_structure_change)
            });

        let cancellation_time = self.analysis_host.apply_change(change);
```

Three separate locks, deliberately layered:

1. `self.vfs: Arc<RwLock<(Vfs, FxHashMap<FileId, LineEndings>)>>` — a
   `parking_lot` RwLock shared with every snapshot. Taken **write**, immediately
   downgraded to **upgradable-read** for the CPU-bound UTF-8 + line-ending
   normalisation, then upgraded back to **write** only to install the results.
2. The salsa exclusive lock, acquired by
   `AnalysisHost::trigger_cancellation()` — which, per `ide/src/lib.rs:208-214`,
   is currently implemented as `self.db.synthetic_write(Durability::LOW)`
   because `trigger_cancellation` "is currently bugged wrt to cancellation".
   This is **spawned on the dedicated 1-thread `cancellation_pool`** so it blocks
   *that* thread on the condvar while the main thread normalises text.
3. `apply_change` (`ide-db/src/apply_change.rs:11-19`) then does the real write:

```rust
    pub fn apply_change(&mut self, change: ChangeWithProcMacros) -> Duration {
        let now = Instant::now();
        self.trigger_cancellation();
        let elapsed = now.elapsed();
        change.apply(self);
        elapsed
    }
```

The returned `Duration` is *how long cancellation took* — rust-analyzer measures
its own stall on every keystroke and reports it. That tells you how much this
costs in practice.

And `snapshot()` itself is cheap (`global_state.rs:574-588`): six `Arc::clone`s,
one `RootDatabase::clone`, one `MemDocs::clone`. The VFS is shared by `Arc<RwLock<..>>`
— so a snapshot's file *text* view is not actually frozen; only the salsa side is
revision-consistent.

---

## 5. Is salsa overkill for us? Yes, on the measurements.

### What the numbers say

I measured salsa's overhead directly (release build, `/tmp/salsa-min`, 2,000
inputs, `black_box`ed):

| operation | cost |
| --- | --- |
| salsa cached tracked-fn fetch, same revision | **7.3 ns** |
| `std::HashMap::get` | 3.5 ns |
| `ArcSwap::load` once + `HashMap::get` | 3.7 ns |
| `ArcSwap::load` per lookup | 4.7 ns |
| 1 input edit + re-query all 2,000 memos | **87.5 µs** → **~44 ns/memo** cross-revision revalidation |

Now the other side of the ledger, from `05-measurements-and-decisions.md`:

- 2,543 real files, 110.8 MB/s, 26,000 files/s → **mean file 4.26 KB, mean
  reparse 38.5 µs**. Largest real file 1.22 ms.
- Full static index of 189k targets: **~1.4 s cold, ~13 MB resident, 69 bytes per
  target.**

Put those together and the conclusion is not close:

1. **Salsa's revalidation is on the same order as our recomputation.** Re-verifying
   a memo across a revision costs ~44 ns. Parsing an entire mean-sized BUILD file
   costs 38.5 µs — 875× more, yes, but the absolute number is 38 microseconds.
   You do not build an incremental-computation framework to avoid 38 µs.
2. **Salsa's bookkeeping is comparable to our entire payload.** Our index is
   69 bytes/target. Salsa's *metadata alone* is 32–120 bytes per memoised entry.
   Interning 189,000 labels as `#[salsa::interned]` would cost ~9 MB of metadata
   against a 13 MB total index. We would roughly double our memory to buy
   incrementality on work that takes 1.4 seconds to redo from scratch.
3. **Our expensive tier is not a pure function of our inputs.** `bazel query`
   depends on the whole repo, `.bazelrc`, `MODULE.bazel.lock`, fetched external
   repos, `--config` selection, environment variables, and the state of the
   output base. Modelling it as a `#[salsa::tracked]` fn means either calling
   `db.report_untracked_read()` — which forces re-execution on *every* revision,
   destroying the point — or lying to salsa about its dependencies, which
   produces stale answers that are indistinguishable from correct ones. Salsa
   offers nothing for a 0.82 s subprocess whose invalidation condition is
   "something in the build graph moved". You cancel a subprocess by killing the
   child, not by unwinding a query stack.

### The arc-swap alternative, and what it actually loses

`ArcSwap<Index>` + reparse-on-edit gives snapshot consistency **without any
cancellation machinery at all**, because the reader owns an `Arc` that the writer
cannot touch. This is not merely simpler than salsa's model — it is *strictly
better on the axis rust-analyzer cares most about*: the writer never blocks, so
there is no per-keystroke stall to measure and no `cancellation_pool` needed to
hide it.

The cost is that a reader's view can be stale. rust-analyzer cannot tolerate that
(a stale goto-definition is a wrong answer). We can, and already decided to: our
Bazel tier is *definitionally* stale — the index reflects a `bazel query` from
some seconds ago — and commitment #4 says degrade loudly. There is no
repeatable-read invariant for salsa to protect, because there is no moment at
which our index is "correct".

What we would genuinely give up:

| lost | replacement cost |
| --- | --- |
| automatic fan-out invalidation (edit `defs.bzl`, invalidate its `load()`ers) | a reverse map `bzl → dependents`, ~50 lines. But see below: we mostly don't need it. |
| backdating | hash the file text before replacing the entry; ~5 lines and it is *cheaper* than `PartialEq` on a parse tree |
| per-query LRU | `lru` 0.18.2 (2026-08-03, 81M downloads/90d) or `hashlink` 0.12.1 (84M/90d). ~40 lines. **And it evicts on demand rather than only on revision bumps** — strictly better for our memory constraint |
| cancellation protocol | we don't want it: our requests are milliseconds; `$/cancelRequest` = drop the response |
| durability tiers | not needed; we have exactly two tiers and they are already separate objects |
| `db.memory_usage()` | `jemalloc`/`dhat` or just `size_of` accounting over an index we fully own |
| cycle recovery | `load()` cycles are an error to report, not a fixpoint to iterate |

The fan-out point deserves care, because it is the only real one. Under
commitment #2 the whole-repo tier is a flat `(name, kind, file, offset)` table.
Editing `defs.bzl` **does not change any BUILD file's own symbol table** — it
changes *resolution*, which is a lookup against the table, not stored derived
data. So the classic salsa win (one edit invalidating a 5,000-node derived
subgraph) does not arise, *because we deliberately chose not to materialise that
subgraph*. Commitment #3 ("never eagerly index `//...`; lazy and per-package")
is the same decision from the other side.

### At what repo size does salsa start paying?

Repo size is the wrong parameter. The right one is **fan-out × recompute cost of
eagerly-maintained derived data**.

The break-even condition, with our measured constants:

```
salsa pays when   D × 38.5 µs  >  N × 44 ns  +  (integration + churn tax)
```

where `D` = files whose derived data must be recomputed per edit, `N` = memos in
the database. Ignoring the tax entirely (generous), at `N` = 189,000:

- salsa's revalidation floor: 189,000 × 44 ns = **8.3 ms** to walk the whole db.
  (In practice salsa is demand-driven and durability-gated, so you only pay for
  what you query — but that floor is the honest upper bound, and it is already
  half a 16 ms frame.)
- to beat it you need `D` > ~215 files recomputed per edit.

So: **salsa starts paying at a fan-out of a few hundred eagerly-recomputed files
per edit.** That is entirely achievable — a widely-`load()`ed macro file in a
74k-package monorepo has thousands of dependents — *but only if we choose to
eagerly maintain per-file derived analysis*. Two futures make that true:

1. **Starlark type/effect inference across `load()` boundaries.** If we ever
   compute per-symbol signatures and propagate them transitively, recompute
   depth goes from 2 to unbounded and salsa becomes the right tool.
2. **Eager diagnostics for unopened files** (publish diagnostics for the whole
   workspace, not just open buffers). That turns lazy per-package work into
   eagerly-maintained derived data with real fan-out.

Neither is v1. Both are legitimate v3 features.

### Where salsa would actively hurt us

- LRU only fires on `&mut Database` (§2). Our stated memory strategy is "full
  CSTs for open files plus a small LRU". Under salsa, closing 200 files reclaims
  nothing until the next edit, and reclaiming it cancels every in-flight request.
  Under `ArcSwap` + an `lru::LruCache`, eviction is `O(1)` on insert and blocks
  nobody.
- 991 MB of Bazel server RSS is already the dominant memory line. Adding 32–120
  bytes/memo of monotonic salsa metadata over 189k targets is the wrong
  direction.
- Cancellation-by-unwinding requires the entire handler graph to be panic-safe
  and `UnwindSafe`. rust-analyzer needed `DbPanicContext`, `AssertUnwindSafe` at
  a dozen sites, and a custom panic hook (`base-db/src/lib.rs:393-437`) to make
  the resulting backtraces legible.
- 33–38 extra crates and ~28 CPU-seconds of clean build against a project whose
  measured vertical slice is 160 lines.

---

## 6. Alternatives

### `comemo` 0.5.1 — typst's constrained memoization

| fact | value |
| --- | --- |
| latest | **0.5.1, 2026-01-29** (0.5.0 2025-07-31, 0.4.0 2024-03-07) |
| downloads | 2,539,729 all-time; **1,034,014** in 90 days |
| repo | github.com/typst/comemo — 631 stars, **0 open issues**, last push 2026-03-13 |

Model: `#[memoize]` on a function, `#[track]` on an impl block, and arguments
wrapped in `Tracked<T>`. Instead of a dependency graph, comemo records a
*constraint*: the set of `(method, args, result-hash)` calls the memoized
function made through tracked handles. A cached result is reused iff replaying
those calls against the new input yields the same answers.

```rust
use comemo::{memoize, track, Tracked};

#[memoize]
fn evaluate(script: &str, files: Tracked<Files>) -> i32 { .. }

#[track]
impl Files {
    fn read(&self, path: &str) -> String { .. }
}
```

Differences that matter:

- **Global static caches.** `src/memoize.rs`: `static EVICTORS: RwLock<Vec<fn(usize)>>`,
  and each memoized fn gets a `Cache<C, Out>(LazyLock<RwLock<CacheData<C, Out>>>)`.
  There is no database object, no revisions, no snapshots. That is fine for a
  batch compiler; for a long-lived server it means one global mutable cache with
  no per-request consistency story.
- **Age-based eviction, not LRU.** `comemo::evict(max_age)` — *"removes all
  memoized results whose age is larger than or equal to `max_age`. The age of a
  result grows by one during each eviction and is reset to zero when the result
  produces a cache hit."* Typst calls it once per compilation. It does not bound
  memory by count, which is exactly what our "full CSTs for open files plus a
  small LRU" requirement asks for.
- **No cancellation, no durability, no interning.**
- Much smaller conceptual surface than salsa; much less power.

Verdict: comemo is a better fit than salsa *for a batch pipeline* — but our
problem is a server, and comemo's global-cache-plus-age-eviction model is the
one thing we would have to fight.

### `adapton` 0.3.31 — dead

Last release **2019-12-22**. Last repo push **2022-03-24**. 366 stars, 406
downloads in the last 90 days. It is a research artifact (nominal adapton,
demanded computation graphs) that never got a stable, ergonomic Rust API. Do not
consider it.

### Hand-rolled

What we would actually assemble:

| piece | crate | version / date | 90-day downloads |
| --- | --- | --- | --- |
| published immutable index | `arc-swap` | 1.9.2, 2026-06-28 | 74,905,875 |
| concurrent open-doc map (if needed) | `dashmap` | 6.2.1 / 7.0.0-rc2 | 78,009,077 |
| bounded CST cache | `lru` | 0.18.2, 2026-08-03 | 81,363,711 |
| or ordered map for LRU | `hashlink` | 0.12.1, 2026-07-06 | 84,242,241 |
| cheap `Arc` | `triomphe` | 0.1.16, 2026-06-29 | 14,379,236 |

All boringly maintained, all already in our transitive graph via rowan/salsa
anyway. Total new code to replace what salsa gives us and we would actually use:
an index struct, a builder, an `ArcSwap` handle, an `LruCache<FileId, Parse>`,
and a text hash for backdating. On the order of 200 lines, and every line is
about our problem rather than about salsa's.

There is nothing else worth naming. Searching crates.io for incremental
computation by recent downloads returns salsa (2.1M/90d), comemo (1.0M/90d), and
then nothing in the category.

---

## Verdict: **salsa later — with the honest expectation of never**

Not now. The measurements say salsa's per-memo bookkeeping (44 ns to revalidate,
32–120 B to store) is the same order as the work it would avoid (38.5 µs to
reparse a mean BUILD file, 69 B to store a target), and the one genuinely
expensive thing in the system — `bazel query` — is a subprocess that salsa cannot
model without lying about its inputs. Meanwhile the specific feature we need most
(bounded CST retention) is salsa's weakest: LRU eviction requires the exclusive
write lock, so reclaiming memory cancels every in-flight request, and rust-analyzer
has had the runtime knob commented out for 18 months.

Adopt salsa if and only if one of these becomes true:

- we start eagerly maintaining per-file derived analysis with fan-out above a few
  hundred files per edit (transitive Starlark signature inference; workspace-wide
  eager diagnostics), **or**
- profiling on a real 74k-package monorepo shows recomputation, not Bazel, as the
  latency floor. Given 1.4 s to rebuild the *entire* static index versus 16.76 s
  for one cold `bazel query`, that is not where the money is.

### Migration cost from an `ArcSwap` index

Bounded, if one discipline is kept. **Write every analysis as a free function
`fn(&Snapshot, Key) -> Value` with no hidden state, no interior mutability, and
no ambient globals.** That is the entire precondition. If it holds:

| step | cost | note |
| --- | --- | --- |
| inputs → `#[salsa::input]` | small | keep your own `DashMap<FileId, FileText>` of salsa input handles, exactly as `base-db/src/lib.rs:70-75` does. The surrounding VFS code does not change. |
| each `fn(&Snapshot, K) -> V` → `#[salsa::tracked] fn(db: &dyn Db, k: K) -> V` | mechanical, per function | this is why the discipline matters |
| derive `SalsaValue` + `PartialEq` on every stored type | mechanical, wide | rust-analyzer has 60 such sites |
| thread `db: &dyn Db` through handler signatures | pervasive, mechanical | the single most annoying part |
| **handlers become `UnwindSafe`, return `Result<T, Cancelled>`** | real design work | this is irreducible |
| **dispatcher grows cancel/retry/`ContentModified` plumbing** | real design work | rust-analyzer's `dispatch.rs` is 443 lines and is mostly this |
| **writer must stall on readers; measure and hide it** | real design work | rust-analyzer needed a dedicated `cancellation_pool` and returns the stall duration from `apply_change` |

The first four rows are a mechanical refactor — days. The last three are new
infrastructure that has no `ArcSwap` counterpart, because `ArcSwap` makes them
unnecessary; call it 500–800 lines and a focused week, plus permanent exposure to
a crate that has broken its API eleven times in 17.5 months.

The thing that would make migration *expensive* is not salsa; it is letting
analysis functions read ambient state. Keep them pure against an explicit
snapshot and the door stays open at essentially zero carrying cost.
