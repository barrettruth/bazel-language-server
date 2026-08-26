# VFS, watching, traversal, interning, persistence

Research date 2026-08-25. All measurements on this machine unless stated:
macOS 26.6.1 (25G76), arm64, 18 cores, 128 GB, APFS, rustc 1.98.0.
Reference implementation read at `/tmp/ra-probe`, rust-analyzer
`014d54b685ccc00f0774d755d1a4c889f7e8e512` (2026-08-25).
Trees: `/tmp/hugerepo` = 20,000 packages (20,202 dirs / 20,002 files / 40,204
entries), `/tmp/hugerepo74k` = 74,000 packages (74,742 dirs / 74,001 files /
148,743 entries), built to match the CONTEXT.md target sizes.

Every number below marked **measured** came out of a program in `/tmp`
(`traverse-bench`, `watch-bench`, `watch-rc4`, `mem-bench`, `persist-bench`).
Where I could not measure — anything Linux or Windows — I say so.

---

## 1. Watching crates

### 1.1 Current state of `notify`

| crate | max stable | published | newest | downloads | recent 90d |
| --- | --- | --- | --- | --- | --- |
| `notify` | **8.2.0** | 2025-08-03 | 9.0.0-rc.4 (2026-05-02) | 143.4 M | 35.7 M |
| `notify-types` | 2.1.0 | 2026-01-25 | — | 58.7 M | 25.3 M |
| `notify-debouncer-full` | **0.7.0** | 2026-01-23 | 0.8.0-rc.2 (2026-05-02) | 14.7 M | 3.9 M |
| `notify-debouncer-mini` | 0.7.0 | 2025-08-03 | — | 13.6 M | 3.2 M |

**8.x is the stable line, unambiguously.** 8.2.0 has 40.9 M downloads;
9.0.0-rc.4 has 218 K. The 9.0 cycle opened 2026-01-25 (rc.1) and has been
sitting at rc.4 since 2026-05-02 — **nearly four months with no rc.5**, while
`main` has accumulated a further 18 changelog entries in the `## unreleased`
section. The repo is alive (last push 2026-08-24, 3,440 stars, 82 open issues,
7 open PRs), the release just isn't happening. rust-analyzer HEAD pins
`notify = "8.2.0"` (`crates/vfs-notify/Cargo.toml:19`).

Take 8.2.0 and read the 9.0 changelog as a list of bugs you currently have.

### 1.2 The macOS trap, measured

`RecommendedWatcher` on macOS is `FsEventWatcher`. FSEvents is *natively
recursive*: one stream root covers a whole subtree. But notify's API lets you
call `watch()` once per directory, and **that is exactly what rust-analyzer's
server-side watcher does** (`crates/vfs-notify/src/lib.rs:329-331`):

```rust
if is_dir && do_watch {
    watch(abs_path.as_ref());   // -> watcher.watch(path, RecursiveMode::Recursive)
}
```

In notify 8.2.0 each such call appends to one `CFMutableArray` and tears down
and recreates the whole `FSEventStream` (`fsevent.rs:388 append_path`,
`fsevent.rs:411 run`). N calls means N stream restarts over an array growing
to N paths. **Measured** (notify 8.2.0, per-directory recursive watches):

| directories watched | setup time | `watch()` errors | events actually delivered |
| ---: | ---: | ---: | --- |
| 10 | 0.03 s | 0 | yes |
| 500 | 0.62 s | 0 | yes |
| 800 | 0.96 s | 0 | — |
| 1,600 | 5.34 s | 0 | — |
| 2,000 | 13.20 s | 0 | yes |
| 3,200 | 35.37 s | 0 | — |
| 4,096 | 66.8 s | 0 | **yes** |
| 4,100 | 76.2 s | 0 | **NO** |
| 4,200 | 77.0 s | 0 | **NO** |
| 4,300 | 72.4 s | 0 | **NO** |
| 4,500 | 73.1 s | 0 | **NO** |
| 6,000 | 78.1 s | 0 | **NO** |

Two findings, both bad:

1. **Quadratic setup.** 72 s to register 4,096 watches. Extrapolating the
   O(n²) fit to 74,000 directories gives hours. This is the design
   rust-analyzer ships behind `files.watcher = "server"`.
2. **Silent total failure at exactly 4,096 stream paths.** Past the FSEvents
   path limit, every `watch()` still returns `Ok(())` and the watcher delivers
   **zero events for any path, including the first one registered**. The
   probe writes a file in the *first* watched directory and gets nothing. The
   answer to "does it fail loudly or silently" on macOS is: silently, totally,
   and with no way for the caller to notice. notify 8.2.0 ignores the
   `FSEventStreamStart` return value; 9.0.0-rc.1 fixed that (#733).

notify 9.0.0-rc.4, same probe — **loud, but worse**:

| directories | setup | `watch()` errors | events |
| ---: | ---: | ---: | --- |
| 300 | 9.26 s | 0 | yes |
| 500 | 21.95 s | 182 (`unable to start FSEvent stream`) | no |
| 600 | 29.02 s | 282 | no |
| 600, `ulimit -n 10240` | 37.88 s | **0** | **yes** |
| 4,100 | 256.0 s | 3,782 | no |

rc.4 reports the failure (good) but hits it at a few hundred paths under this
machine's default soft `RLIMIT_NOFILE` of **256** (`ulimit -n` = 256,
`launchctl limit maxfiles` = 256), and is ~30× slower per registration than
8.2.0. Raising the fd limit to 10,240 makes n=600 work — **FSEvents consumes
a file descriptor per stream path**, and it consumes them out of *your*
process's table.

notify `main` (post-rc.4, unreleased) has the fix and documents the mechanism
(`notify/src/fsevent.rs:552`, `:711`):

```rust
// Over roughly RLIMIT_NOFILE/10 paths across all live streams, FSEvents
// closes fd 0, which this process owns. The corruption then surfaces as
// EBADF on unrelated files.
fn fsevents_path_budget() -> Option<usize> { /* soft RLIMIT_NOFILE / 12 */ }
```

Exceeding it now returns `ErrorKind::MaxFilesWatch`, the budget is shared
across all live streams in the process via a `static AtomicUsize`, and
`main` also collapses nested recursive watches onto one stream root. **None
of that is in a release yet.**

**The fix is not a newer notify — it is one watch.** **Measured**, single
recursive watch on the 74,000-package root with notify 8.2.0:

```
single recursive watch on root: setup 0.002 s
  event for pkg/s739/p99 delivered in 15.8 ms
  burst: 2000 writes in 0.275 s -> 4048 path-events, 0 rescan flags
```

Two milliseconds instead of hours, 15.8 ms latency to the deepest package,
and no event loss under a 7,300 writes/s burst. FSEvents coalescing is not
something you fight; it is the thing that makes this work. Note the burst
produced *more* events than writes (4,048 for 2,000 files: create + modify),
so coalescing does not mean "you lose edits" — it means you get a
directory-granular, latency-batched stream. Latency is configurable in notify
`main` only (`Config::with_fsevent_latency`, default `Duration::ZERO`); 8.2.0
hardcodes `latency: 0.0` with `kFSEventStreamCreateFlagNoDefer`, i.e. fire
immediately on the first event of a burst.

### 1.3 The Linux trap

I have no Linux box in this session; the following is from source and kernel
code, not measured here. Flag it as such.

`RecommendedWatcher` on Linux is `INotifyWatcher`, and inotify is **not**
recursive. notify emulates recursion by walking the tree and adding one watch
per directory (`notify-8.2.0/src/inotify.rs:407`):

```rust
for entry in WalkDir::new(path).follow_links(self.follow_links).into_iter().filter_map(filter_dir) {
    self.add_single_watch(entry.into_path(), is_recursive, watch_self)?;
```

So on Linux, 74,000 packages is **74,000+ inotify watches**, unavoidably.

`fs.inotify.max_user_watches` was a flat 8192 from 2005 until kernel 5.11
(commit `92890123749b`), which made it memory-proportional
(`fs/notify/inotify/inotify_user.c:815`):

```c
watches_max = (((si.totalram - si.totalhigh) / 100) << PAGE_SHIFT) / INOTIFY_WATCH_COST;
watches_max = clamp(watches_max, 8192UL, 1048576UL);
```

with `INOTIFY_WATCH_COST = sizeof(struct inotify_inode_mark) + 2 * sizeof(struct inode)`
≈ 1,376 B on x86_64. That gives roughly:

| RAM | default `max_user_watches` |
| ---: | ---: |
| 4 GB | ~31,000 |
| 8 GB | ~62,000 (matches the figure reported in vscode issue threads) |
| 16 GB | ~125,000 |
| 32 GB | ~250,000 |
| ≥ 128 GB | 1,048,576 (clamp) |

74,000 directories therefore **does not fit on an 8 GB machine and barely
fits on 16 GB** — and the limit is **per user, not per process**, shared with
VS Code's own watcher, `cargo watch`, Dropbox, and every other LSP server the
editor started.

Does it fail loudly? On Linux, yes — but only since 8.2.0. `ENOSPC` from
`inotify_add_watch` is mapped to `ErrorKind::MaxFilesWatch` rather than a
generic io error (issue #266), and notify 8.2.0's headline feature was
"notify user if inotify's `max_user_watches` has been reached" (#698):

```rust
if let ErrorKind::MaxFilesWatch = add_watch_error.kind {
    self.event_handler.handle_event(Err(add_watch_error));
    break;   // every subsequent add would fail identically
}
```

Before 8.2.0 the recursive-add path swallowed it. **Your server must handle
`ErrorKind::MaxFilesWatch` and surface it as a diagnostic** — that is
architectural commitment #4 ("degrade loudly") applied to the watcher.

Two further Linux hazards:

- **`Config::default().follow_symlinks == true`** in notify 8.x. The recursive
  `WalkDir` therefore follows `bazel-out` into the output base and registers
  inotify watches on the entire execroot symlink forest. Set
  `Config::default().with_follow_symlinks(false)` (added in 8.0.0, #635).
- **Every file `open()` generates an event.** notify 8.2.0's watchmask is
  fixed at `ATTRIB | CREATE | OPEN | DELETE | CLOSE_WRITE | MODIFY |
  MOVED_FROM | MOVED_TO`. A `bazel build` reads every source file in the
  repo, so a build produces one inotify event per source file read. With
  `inotify_max_queued_events = 16384` (also a compile-time default), that
  overflows and the kernel sets `IN_Q_OVERFLOW`. notify's own test suite has
  a comment about this: *"the parallel threads opening files would otherwise
  flood the queue with OPEN events causing Rescan"*. There is no way to turn
  `OPEN` off in 8.x. notify 9.0.0-rc.1 added `EventKindMask`
  (`Config::with_event_kinds(EventKindMask::CORE)`, #736) which drops `OPEN`
  from the kernel mask. **This is the single strongest argument for 9.0** and
  worth pinning the RC over if you go server-side on Linux.

### 1.4 Windows

Also not measured here. `RecommendedWatcher` is `ReadDirectoryChangesWatcher`,
which is natively recursive: **one `CreateFileW` handle + one
`ReadDirectoryChangesW` per watched root**, no per-directory cost. The
handle is opened with `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`
and `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED`, so it does not lock
the directory.

The trap is the buffer: `const BUF_SIZE: u32 = 16384` (`windows.rs:42`), a
fixed 16 KB per completion, not configurable. `FILE_NOTIFY_INFORMATION` is
12 bytes plus a UTF-16 filename, so ~130-200 events per completion before the
kernel discards details and returns `ERROR_NOTIFY_ENUM_DIR` or a zero-byte
completion. notify turns that into `EventKind::Other` with `Flag::Rescan`
(`windows.rs:528`). A Bazel build blows through 16 KB instantly.

### 1.5 `Flag::Rescan` — the bug you inherit if you copy rust-analyzer

All three backends emit `EventKind::Other` + `Flag::Rescan` when the OS
dropped events (inotify `Q_OVERFLOW`, Windows `ERROR_NOTIFY_ENUM_DIR`,
FSEvents `kFSEventStreamEventFlagUserDropped`/`KernelDropped`).
rust-analyzer's handler (`vfs-notify/src/lib.rs:200-205`) matches only:

```rust
if let Some(event) = log_notify_error(event)
    && let EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
         | EventKind::Access(AccessKind::Open(_)) = event.kind
```

`EventKind::Other` falls straight through and is **dropped**. After any
overflow, rust-analyzer's server-side VFS is permanently stale with no
indication. Handle `event.need_rescan()` explicitly and re-walk the affected
roots.

### 1.6 Debouncing

`notify-debouncer-full` 0.7.0 (2026-01-23) is the right layer for a Bazel LS:
it merges the create/modify pairs FSEvents produces, tracks file IDs so
rename pairs resolve, and — critically — **propagates rescan requests** rather
than swallowing them. 0.8.0-rc.2 tracks notify 9.0.0-rc.4 and includes
"speed up debouncer root tracking for large numbers of watched paths" (#913),
which matters if you ever watch more than one root. 0.7.0 pairs with notify
8.2.0; do not mix majors.

`notify-debouncer-mini` is a strictly weaker version of the same thing (no
file-ID cache, no rename pairing). No reason to prefer it.

---

## 2. `workspace/didChangeWatchedFiles` vs watching ourselves

### 2.1 What rust-analyzer actually does

Default: **client**. `crates/rust-analyzer/src/config.rs:1083`

```rust
client: struct ClientDefaultConfigData <- ClientConfigInput -> {
    /// Controls file watching implementation.
    files_watcher: FilesWatcherDef = FilesWatcherDef::Client,
}
```

and it falls back to server-side only if the client cannot do dynamic
registration (`config.rs:2380`):

```rust
watcher: match self.files_watcher() {
    FilesWatcherDef::Client if self.did_change_watched_files_dynamic_registration() =>
        FilesWatcher::Client,
    _ => FilesWatcher::Server,
},
```

When client-side, `reload.rs:561-648` registers globs — `**/*.rs`,
`**/Cargo.{lock,toml}`, `**/rust-analyzer.toml`, `**/*.md` — preferring
`RelativePattern` when `relativePatternSupport` is set, and `watch` is set to
the empty vec so the notify actor registers nothing (`reload.rs:733`):

```rust
let watch = match files_config.watcher {
    FilesWatcher::Client => vec![],
    FilesWatcher::Server => project_folders.watch,
};
```

The reasoning is in the crate doc comment, and it is blunt
(`crates/vfs-notify/src/lib.rs:1-8`):

> The file watching bits here are untested and quite probably buggy. For this
> reason, by default we don't watch files and rely on editor's file watching
> capabilities.
>
> Hopefully, one day a reliable file watching/walking crate appears on
> crates.io, and we can reduce this to trivial glue code.

That comment has survived to HEAD in 2026. The maintainer position on the
issue tracker matches: *"The LSP spec recommends client file watching hence
the default, though for vscode we should probably default to server as
vscode's watcher is broken"* and *"server-side file watching had its own share
of issues in the past, causing us to disable it (#1541, linking to
notify-rs/notify#208)"* (rust-analyzer#17423). It is a defensive default, not
a considered one.

The spec's own argument (LSP 3.17, `DidChangeWatchedFiles Notification`) is
worth having in front of you:

> Servers are allowed to run their own file system watching mechanism and not
> rely on clients to provide file system events. However this is not
> recommended [because] getting file system watching on disk right is
> challenging, especially [...] across multiple OSes; [...] a client usually
> starts more than one server. If every server runs its own file system
> watching it can become a CPU or memory problem; [...] there are more server
> than client implementations.

### 2.2 Why the client is worse for BUILD files specifically

The registration payload has watchers and **nothing else**:

```ts
interface DidChangeWatchedFilesRegistrationOptions { watchers: FileSystemWatcher[]; }
```

There is no exclude field. You can ask for `**/BUILD.bazel`; you cannot say
"but not under `bazel-out`". For a Bazel repo that is the whole problem:
`bazel-out` contains generated `BUILD` files, and the `bazel-<workspace>`
execroot symlink re-enters the source tree (see §3.3). The only exclusion
lever is client-side and user-configured. VS Code's default
`files.watcherExclude` is `**/.git/objects/**`, `**/.git/subtree-cache/**`,
`**/.hg/store/**`, `**/node_modules/**` — **no Bazel entries**. Real Bazel
users are hand-maintaining this today (microsoft/vscode#232665):

```json
"files.watcherExclude": {
  "**/bazel-bando/**": true, "**/bazel-bin/**": true,
  "**/bazel-out/**": true,   "**/bazel-testlogs/**": true
}
```

Note `bazel-bando` — the workspace-named symlink, which nobody but that user
can guess. rust-analyzer has the identical problem in miniature and documents
it: *"You may also need to add the folders to Code's `files.watcherExclude`."*

Client-side also loses:

- **Non-VS-Code clients.** Neovim, Helix and others have shipped
  `didChangeWatchedFiles` with holes for years (rust-analyzer#14669,
  helix#2479). Capability negotiation tells you *whether* the client claims
  support, not whether it works.
- **Overflow signalling.** There is no `Flag::Rescan` equivalent in the
  protocol. If the client's watcher drops events, you get silence.
- **`bazel build` storms.** Every file a build writes is delivered to you
  through JSON-RPC. A 52 MB `bazel-out` churn becomes tens of thousands of
  `FileEvent` objects to parse.

The one thing client-side wins on is real: **it is a single OS-level watcher
shared by every server the editor hosts**, and VS Code's is `@parcel/watcher`,
which applies excludes natively in the watcher rather than filtering after
the fact, and explicitly does not follow symlinks ("symbolic links are not
followed automatically but you can explicitly add symbolic links to be watched
via the `files.watcherInclude` setting" — microsoft/vscode wiki, File Watcher
Issues). On VS Code specifically, the client watcher will not walk into
`bazel-<workspace>`.

### 2.3 Verdict

**Own the watcher. Default to server-side, on one recursive root per
workspace, with `workspace/didChangeWatchedFiles` accepted as a redundant
secondary source and `files.watcher = "client"` available as an escape
hatch.**

The reasoning:

1. The cost that made rust-analyzer retreat — per-directory watch
   registration — is self-inflicted. **Measured**: one recursive root over
   74,000 packages costs 0.002 s and 15.8 ms of latency. rust-analyzer never
   tried this shape; its loader walks and calls `watch()` per directory
   because the same code path also does the initial load.
2. Bazel exclusion is expressible server-side (`.bazelignore`, `REPO.bazel
   ignore_directories()`, `--symlink_prefix`) and is **not** expressible in an
   LSP watcher registration. You know where `bazel-out` is; the client does
   not and cannot be told.
3. You get `Flag::Rescan` and `ErrorKind::MaxFilesWatch`, so degradation is
   loud (commitment #4). Through the client you get silence.
4. Correctness does not depend on which editor the user picked.

Concessions the verdict requires:

- On **Linux**, one recursive root still means 74,000 inotify watches.
  Register them, and on `ErrorKind::MaxFilesWatch` emit a diagnostic naming
  `fs.inotify.max_user_watches` and fall back to client-side watching for
  the remainder. Do not silently half-watch.
- Watch `MODULE.bazel`, `WORKSPACE*`, `.bazelrc`, `.bazelignore`, `REPO.bazel`
  and `*.bzl` as *files* (cheap, exact) and the package tree as one recursive
  root.
- Never watch the output base. Ever. Poll `bazel info output_base` once at
  startup and hard-exclude it; that also covers a non-default
  `--symlink_prefix`.

---

## 3. Traversal

### 3.1 Crate status

| crate | version | published | downloads | recent 90d | note |
| --- | --- | --- | --- | --- | --- |
| `walkdir` | 2.5.0 | 2024-03-01 | 581.9 M | 136.7 M | finished, not abandoned |
| `ignore` | 0.4.33 | **2026-08-04** | 162.8 M | 35.6 M | ripgrep; MSRV 1.88 |
| `jwalk` | 0.9.0 | 2026-08-05 | 11.2 M | 2.2 M | **deprecated** |

`jwalk` 0.9.0's crates.io description is literally `"Use dua-core instead"` and
its README opens `# Unmaintained / This crate is no longer maintained or
supported.` It still compiles and runs (I benchmarked it), but it is out. Its
last functional release before the tombstone was 0.8.1 in **December 2022**.

### 3.2 Measured throughput

Best of 5 (20k) / 3 (74k), warm dentry cache, `follow_links(false)`,
`bazel-*` pruned. Entries = files + dirs + symlinks visited.

**20,000 packages — 40,204 entries**

| walker | wall | entries/s |
| --- | ---: | ---: |
| `walkdir` 2.5.0, serial | 1.104 s | 36,433 |
| `ignore` 0.4.33 serial, `standard_filters(false)` | 1.116 s | 36,012 |
| `ignore` serial, gitignore **on** | 2.664 s | 15,094 |
| `ignore` parallel ×18, filters off | **0.585 s** | **68,774** |
| `ignore` parallel ×18, gitignore on | 0.588 s | 68,418 |
| `jwalk` 0.9.0, serial | 1.100 s | 36,541 |
| `jwalk` parallel ×18 | 0.633 s | 63,511 |

**74,000 packages — 148,743 entries**

| walker | wall | entries/s |
| --- | ---: | ---: |
| `walkdir` serial | 4.010 s | 37,095 |
| `ignore` serial, filters off | 4.028 s | 36,925 |
| `ignore` serial, gitignore **on** | 9.141 s | 16,272 |
| `ignore` parallel ×18, filters off | **2.151 s** | **69,139** |
| `jwalk` serial | 4.122 s | 36,089 |
| `jwalk` parallel ×18 | 2.301 s | 64,655 |
| BSD `find -name BUILD.bazel` (baseline) | 8.9-9.9 s | ~15,000 |

Readings:

- **The three crates are the same walker when single-threaded** (4.01 / 4.03 /
  4.12 s — inside noise). The choice is entirely about the parallel mode and
  the filtering API.
- **The filesystem is the ceiling, not the crate.** 37 K entries/s serial;
  BSD `find` manages 15 K on the same tree. 18 cores buy only 1.87×, so the
  walk is `getdirentries` and VFS-lock bound, not CPU bound. Do not expect
  a faster crate to help.
- **`ignore`'s gitignore machinery costs 2.27× and is *wrong* for Bazel.**
  Bazel does not consult `.gitignore` — a git-ignored directory is still a
  package, which is precisely why `.bazelignore` exists. Turning the standard
  filters on would make the LS miss packages Bazel can see. Use `ignore`
  with `standard_filters(false)`, i.e. purely as a parallel walker.
- `ignore` parallel beats `jwalk` parallel by 7% and is maintained. That
  settles it.

Second half of a cold index build, **measured** (74,000 `BUILD.bazel`):

| pass | wall | rate |
| --- | ---: | ---: |
| read all, serial, cold | 9.007 s | 8,216 files/s |
| read all, serial, warm | 3.351 s | 22,083 files/s |
| read all, parallel ×18 | **2.213 s** | **33,439 files/s** |

So a full cold light-index build is roughly **2.2 s of walking + 2.2 s of
reading** (overlappable) plus parse. At the CONTEXT-measured 26,000 files/s
for `starlark-cst`, parsing 74,000 BUILD files is 2.85 s single-threaded and
under 0.5 s across 18 cores. **Call it 3-5 s cold, no Bazel involved, versus
16.76 s for a cold `bazel query //...`.** Keep that ratio in mind for §5.

### 3.3 The `bazel-*` symlink trap, measured

`/tmp/hugerepo` has the four convenience symlinks, and
`bazel-hugerepo -> …/execroot/_main` contains `pkg -> /private/tmp/hugerepo/pkg`.
Following it re-enters the source tree.

| walkdir configuration | wall | entries | BUILD files found |
| --- | ---: | ---: | ---: |
| `follow_links(false)`, no exclusion | 1.473 s | 40,208 | 20,001 |
| `follow_links(true)`, **no exclusion** | 3.334 s | **81,865** | **40,118** |
| `follow_links(true)`, `bazel-*` pruned | 1.139 s | 40,204 | 20,001 |

At 74k: follow + no exclusion gives 190,200 entries and **94,118 BUILD files
instead of 74,001**. Every package is discovered twice under two different
paths, so every label resolves ambiguously and every symbol appears twice in
completion. It does not hang — it silently doubles and corrupts the index.

`walkdir`'s loop detection does not save you: it compares against *ancestors*
only, and `hugerepo/bazel-hugerepo/pkg` is not an ancestor of itself.
rust-analyzer's own heuristic does not save you either
(`vfs-notify/src/lib.rs:378`):

```rust
fn path_might_be_cyclic(path: &Path) -> bool {
    let Ok(destination) = std::fs::read_link(path) else { return false };
    let is_relative_parent = destination.components().all(|c| matches!(c, Component::CurDir | Component::ParentDir));
    is_relative_parent || path.starts_with(destination)
}
```

The execroot link is absolute and points *outside* the tree, so
`is_relative_parent` is false and `path.starts_with(destination)` is false.
rust-analyzer calls `WalkDir::new(root).follow_links(true)` unconditionally
(`vfs-notify/src/lib.rs:303`), so **rust-analyzer's loader walks a Bazel
workspace twice.**

#### Telling each crate not to follow them

The base rule: `follow_links(false)`. A symlink is not a directory, so the
walkers stop at `bazel-out` with no explicit rule — **measured**, nofollow
without exclusion gives 40,208 entries vs 40,204 with, a difference of exactly
the four symlink entries. Exclusion still matters because (a) users configure
`--symlink_prefix`, so the names are not always `bazel-*`, and (b) a *real*
directory named `bazel-out` (checked-in generated output, common) must also be
pruned, and (c) the same predicate feeds the watcher.

The robust, config-free predicate is **"prune any directory symlink whose
target escapes the workspace root"** — the execroot always does, whatever it
is called. `bazel-<workspace>/pkg -> <root>/pkg` never gets evaluated because
you stopped one level up.

```rust
// ignore 0.4.33
let mut wb = ignore::WalkBuilder::new(root);
wb.follow_links(false)
  .standard_filters(false)      // Bazel does not honour .gitignore
  .hidden(false)                // .foo/BUILD is a real package
  .same_file_system(true)       // output base is often a separate mount
  .threads(nthreads);
wb.filter_entry(move |e| {
    let is_link = e.path_is_symlink();          // free, from the DirEntry
    if !(is_link || e.file_type().is_some_and(|f| f.is_dir())) { return true; }
    !(rules.ignored(e.path()) || (is_link && rules.escapes(e.path())))
});
for r in wb.build_parallel() { /* … */ }
```

```rust
// walkdir 2.5.0
walkdir::WalkDir::new(root)
    .follow_links(false)
    .same_file_system(true)
    .into_iter()
    .filter_entry(|e| !excludes.contains(e.path()));
```

```rust
// jwalk 0.9.0 (deprecated; shown for completeness)
jwalk::WalkDir::new(root)
    .follow_links(false)
    .skip_hidden(false)
    .parallelism(jwalk::Parallelism::RayonNewPool(n))
    .process_read_dir(move |_depth, _path, _state, children| {
        children.retain(|c| c.as_ref().map_or(true, |e| !excludes.contains(&e.path())));
    });
```

**Do not call `symlink_metadata`/`canonicalize` on every directory.**
`ignore` and `walkdir` already know whether an entry is a symlink from the
`readdir` `d_type`. **Measured** on the 74k tree, identical results:

| bazel-aware parallel walk | wall |
| --- | ---: |
| `lstat` on every directory to test for symlink-ness | 3.886 s |
| use the `DirEntry`'s symlink bit, `canonicalize` only links | **1.894 s** |

That is a 2.05× penalty for one redundant syscall per directory. Note the
correct version (1.894 s) beats even the plain unfiltered parallel walk
(2.151 s), because the ignore rules pruned 11,300 packages.

### 3.4 `.bazelignore` and friends

`.bazelignore` is **not** gitignore syntax. Bazel's own docs: *"Entries are
relative to the workspace root. [...] The `.bazelignore` file does not permit
glob semantics."* One literal, root-relative directory path per line. So
`ignore`'s `WalkBuilder::add_custom_ignore_filename(".bazelignore")` is
**wrong** — gitignore treats a slashless pattern like `node_modules` as
"match at any depth", `.bazelignore` treats it as "the directory
`<root>/node_modules` and nothing else" (bazelbuild/bazel#8106, still open,
opened 2019). Parse it yourself into a `HashSet<String>` of root-relative
paths; it is ten lines.

Since **Bazel 8**, `REPO.bazel`'s `ignore_directories(dirs)` is the
glob-capable replacement: *"a directory is ignored if any of the given strings
matches its repository-relative path according to the semantics of the
`glob()` function"* — Bazel glob, where `*` does not cross `/` and `**`
matches whole segments. Not gitignore, not `globset` defaults either. A Bazel
LS in 2026 must support both files; `ignore_directories` is where new
configuration is going.

Also: repo boundary markers. A subdirectory containing its own `MODULE.bazel`,
`REPO.bazel`, `WORKSPACE` or `WORKSPACE.bazel` is a separate repository, and
its packages are not packages of the main repo. I did not verify the exact
Bazel 8 semantics here — treat as a to-confirm, but it is cheap to detect
during the same walk since you are already looking for those filenames.

Working end-to-end walk, **measured** on 74k with a 2-line `.bazelignore`
plus `ignore_directories(["pkg/s2*", "**/node_modules"])`:

```
bazel-aware parallel walk: 1.894 s  build_files=62702  pruned_dirs=115
```

115 directory prunes removed 11,301 packages.

---

## 4. Interning and memory

### 4.1 Crate status and shape

| crate | version | published | downloads | recent 90d | key size | thread-safe? |
| --- | --- | --- | --- | --- | ---: | --- |
| `lasso` | 0.7.3 | **2024-08-19** | 11.3 M | 2.4 M | `Spur` = 4 B | `Rodeo` no, `ThreadedRodeo` yes, `RodeoReader`/`RodeoResolver` `Send+Sync` |
| `string-interner` | 0.20.0 | 2026-04-30 | 35.8 M | 5.6 M | `SymbolU32` = 4 B | no (`&mut self` to intern) |
| `ustr` | 1.1.0 | 2024-10-26 | 3.1 M | 0.9 M | `Ustr` = 8 B | yes, global static, never freed |
| `smol_str` | 0.3.6 | 2026-03-04 | 107.8 M | 29.7 M | 24 B, inline ≤ 23 | `Send+Sync`, `O(1)` clone |
| `compact_str` | 0.10.0 | 2026-07-13 | 140.7 M | 43.5 M | 24 B, inline ≤ 24 | `Send+Sync` |

`size_of` verified on this machine: `String` 24, `SmolStr` 24, `CompactString`
24, `Spur` 4, `Option<Spur>` **4** (niche), `DefaultSymbol` 4, `Ustr` 8.
Inline capacity probed: `SmolStr` heap-allocates at length 24,
`CompactString` at length 25.

`lasso` last released 2024-08-19 — two years quiet. Not deprecated, no
tombstone, 2.4 M downloads in the last 90 days, and the API surface is done.
It is the memory winner by a clear margin, but note the staleness.
`string-interner` 0.20.0 (2026-04-30) is the actively-maintained alternative
and costs 15% more.

`smol_str` is maintained *inside the rust-analyzer repo*
(`lib/smol_str`), which is a real signal about fitness for this workload.

### 4.2 The arithmetic

Entry layout for `(name, kind, file_id, offset)`:

```
#[repr(C)] struct Entry { name: Spur /*4*/, file_id: u32 /*4*/, offset: u32 /*4*/, kind: u16 /*2*/, _pad: u16 /*2*/ }
```

= **16 B**, no padding waste. 189,000 × 16 = 3,024,000 B = **2.88 MiB** for
the entry table, *if* you `Vec::with_capacity(189_000)`. Let it grow by
doubling and you land on 262,144 slots = 4.00 MiB, 39% wasted — worth the one
line.

Name interner, 94,507 distinct names averaging 24 B (2.16 MiB of distinct
bytes). `lasso::Rodeo` is three parts (`lasso-0.7.3/src/rodeo.rs:38`):

- `map: HashMap<K, (), ()>` — stores **only the 4-byte key**, hashed by the
  string's hash through hashbrown's raw API, deliberately *"so that we only
  store one hasher"* and *"only store references to the internally allocated
  strings once, which drastically decreases memory usage"*. 94,507 entries at
  87.5% max load → 131,072 slots × (4 B + 1 control byte) = **0.63 MiB**.
- `strings: Vec<&'static str>` — 94,507 × 16 B = **1.44 MiB**.
- bump arena holding the bytes — **2.16 MiB** + chunk slack.

Predicted total: 2.88 + 0.63 + 1.44 + 2.16 = **7.11 MiB**.
Measured in isolation: **6.02 MiB**. The arithmetic is 18% high. (Do the same
sum for `(String, tail)` — 189,000 × (40 B tuple + 24-byte-average heap
allocation rounded to a 32 B malloc bucket) = 12.9 MiB — and it comes out 11%
*low* against the measured 11.58 MiB. Allocator behaviour dominates at this
scale; measure.)

### 4.3 Measured RSS, 189,000 entries

One process per variant, `ps -o rss=`, delta over a pre-built name corpus so
only the index is counted.

**50% distinct names** (94,523 distinct, mean length 15.1, 2.73 MiB total /
2.16 MiB distinct) — the realistic monorepo shape:

| representation | elem | index RSS | B/entry |
| --- | ---: | ---: | ---: |
| **packed 16 B `Entry` + `lasso::Rodeo`** | 16 | **6.02 MiB** | **33.4** |
| `(Spur, tail)` + `Rodeo` | 16 | 6.08 MiB | 33.7 |
| `(SymbolU32, tail)` + `string-interner` | 16 | 6.95 MiB | 38.6 |
| `(CompactString, tail)` | 40 | 9.05 MiB | 50.2 |
| `(SmolStr, tail)` | 40 | 9.88 MiB | 54.8 |
| `(String, tail)` | 40 | 11.58 MiB | 64.2 |
| `(Ustr, tail)` | 24 | 14.83 MiB | 82.3 |
| `(Spur, tail)` + `ThreadedRodeo` | 16 | **17.20 MiB** | 95.4 |

**100% distinct** (worst case, every target name unique):

| lasso 8.31 | string-interner 7.55 | CompactString 10.81 | SmolStr 12.48 | String 12.95 | ustr 18.92 |

**10% distinct** (heavy reuse):

| lasso 3.08 | string-interner 3.23 | CompactString 7.64 | SmolStr 7.80 | String 10.52 | ustr 12.61 |

Findings:

- **`ThreadedRodeo` costs 2.8× `Rodeo`** (17.20 vs 6.08 MiB) — it is
  `dashmap`-backed, 16+ shards each with its own table. Intern on one thread
  behind the index build, then `Rodeo::into_reader()` for a `Send + Sync`
  read-only `RodeoReader`. Do not reach for `ThreadedRodeo`.
- **`ustr` is the worst option here** — 82-105 B/entry, worse than plain
  `String`. It is a global static that never frees, tuned for FFI-heavy DCC
  workloads with millions of repeated lookups, not for a compact index. Its
  8-byte handle does not compensate for its per-entry allocation overhead.
- `SmolStr`/`CompactString` only win when names are long enough to matter and
  you want to skip an interner. At mean length 15 they inline everything and
  still cost 50-55 B/entry because the tuple is 40 B. They are the right type
  for *transient* names in the parser, not for the index.
- The gap between `lasso` and `string-interner` is 15% at 50% distinct and
  reverses at 100% distinct (8.31 vs 7.55). Either is fine.

### 4.4 Package paths — the part everyone forgets

74,000 package paths, 2.89 MiB of raw bytes, mean 41 chars. **Measured**,
generated one at a time so no transient corpus inflates RSS:

| storage | RSS | B/path |
| --- | ---: | ---: |
| `IndexSet<Box<str>, FxBuildHasher>` (rust-analyzer's `PathInterner`) | 6.47 MiB | 91.7 |
| byte arena + `Vec<(u32,u32)>` spans + `FxHashMap<u64,u32>` | 5.77 MiB | 81.7 |
| **`lasso::Rodeo`** | **4.78 MiB** | **67.8** |

rust-analyzer's `PathInterner` is `IndexSet<VfsPath>`
(`crates/vfs/src/path_interner.rs`), which is the most expensive of the three:
one heap allocation per path plus a 16-byte `Box<str>` in the index vec plus
the hash table. `lasso` for paths too, and use `FileId(u32)` = the `Spur`.

### 4.5 The full budget, measured

One process, `lasso::Rodeo` for both paths and names, packed 16 B entries:

```
entry size            16 B
package paths         74000 paths, 2.89 MiB of path bytes
  path interner          4.80 MiB
  names + entries        9.02 MiB   (94507 interned names)
  process baseline       1.77 MiB
TOTAL RSS               15.58 MiB
index only              13.81 MiB   (76.6 B per target)
```

Adding a `FxHashMap<Spur, Vec<u32>>` name → entry index, which is what
completion and `workspace/symbol` actually query:

```
  name -> entry map     10.44 MiB   (94507 keys)
TOTAL RSS               26.00 MiB
index only              24.23 MiB   (134.5 B per target)
```

The 10.44 MiB for the lookup map is the single largest line item and is
mostly `Vec<u32>` headers — 94,507 `Vec`s at 24 B each is 2.2 MiB of headers
before any data, and each `Vec` mallocs separately. Flatten it: sort the
entry table by `(name, file_id)` once and binary-search, or build a CSR
(`Vec<u32> offsets` + `Vec<u32> entry_ids`) which costs 94,507×4 + 189,000×4 =
**1.08 MiB** instead of 10.44 MiB. That takes the whole index to **~15 MiB**
with a fast name lookup.

For reference, rust-analyzer solves the same problem with `fst` 0.4.7 (a
finite-state transducer per file, unioned at query time —
`crates/ide-db/src/symbol_index.rs`), which buys fuzzy matching. `fst` was
last released **2021-06-06**; it is finished rather than abandoned, but note
`fst` sets are immutable, hence rust-analyzer's per-file-FST-plus-union
design. For Bazel labels, prefix matching on a sorted table is probably
enough and much simpler.

---

## 5. Persistence

### 5.1 What rust-analyzer does: nothing

There is no on-disk cache in rust-analyzer at HEAD. The feature request
(rust-lang/rust-analyzer#4712) has been open since 2020, labelled `E-hard`,
`S-unactionable`. It blocks on salsa-rs/salsa#10 (open since 2018, *"I
definitely want to punt on this"*). The stated objections are worth reading
before you build one:

> Persistent caches will require changes to salsa to be able to serialize its
> cache. In addition it has the disadvantage that it makes optimizing the
> initial analysis less important, which may over time result in not just
> regressions of the initial analysis time, but also when performing a change.

and, quoted by matklad from the original RLS RFC:

> Don't store anything to disk. It's likely the oracle can be fast enough
> without doing this; and unnecessary complexity creates bugs. "Have you tried
> deleting the .ncb file?"

As of 2026, the new-salsa port lists *"Persistent/saved state of nameres"* as
newly *possible*, and the maintainer answer on discussion #21682 is still
*"Serialization for faster startup is planned but not done yet."*

### 5.2 Crate status

| crate | version | published | downloads | recent 90d | verdict |
| --- | --- | --- | --- | --- | --- |
| `redb` | **4.2.0** | 2026-08-17 | 9.8 M | 4.3 M | active, MSRV 1.90 |
| `rkyv` | **0.8.18** | 2026-08-05 | 146.6 M | 35.8 M | active, MSRV 1.81 |
| `postcard` | 1.1.3 | 2025-07-24 | 55.5 M | 20.9 M | active |
| `bincode` | 2.0.1 | 2025-03-10 | 301.9 M | 57.6 M | **dead — see below** |
| `sled` | 0.34.7 stable / 1.0.0-alpha.124 | 2024-10-11 | 14.6 M | 2.6 M | **stalled 22 months** |

**`bincode` is unmaintained as of 2025-12-16.** Version 3.0.0 is a tombstone;
its entire `src/lib.rs` is:

```rust
compile_error!("https://xkcd.com/2347/");
```

and its README opens *"Bincode is now unmaintained. Due to a doxxing and
harassment incident, development on bincode has ceased. No further releases
will be published on crates.io."* It recommends `wincode`, `postcard`, and
*"[rkyv] honestly the best option for many of the usecases that bincode was
intended for"*. **2.0.1 still works and is what `cargo add bincode@2` gets
you, but it will never be fixed.** Do not start here in 2026.

**`sled` is out.** Stable is 0.34.7; the 1.0 line has been in alpha since
2020 and its last publish was 2024-10-11 — 22 months stale. Its own crates.io
page says *"sled is beta. if reliability is your primary constraint, use
SQLite."* For a cache holding an index you can rebuild in 4 seconds, taking
on a beta storage engine is indefensible.

### 5.3 Measured

189,000 entries + 94,504 distinct names (2.17 MiB arena), 5.77 MiB in memory,
74,000 packages:

| candidate | encode | on disk | load to usable |
| --- | ---: | ---: | --- |
| **`rkyv` 0.8.18** | <1 ms | 5.77 MiB | **mmap + validate 0.1 ms**; full scan of all 189 K entries 1 ms |
| `rkyv`, read-to-`Vec` + validate | — | — | 1 ms |
| `postcard` 1.1.3 | 3 ms | 4.40 MiB | decode 3 ms |
| `bincode` 2.0.1 | 2 ms | 4.95 MiB | decode 2 ms |
| `redb` 4.2.0, 74,000 rows (one per package) | 37 ms write+commit | 4.02 MiB | open 3.9 ms; read all rows 6 ms; **point get 0.2 µs**; single-package update + commit **4 ms** |

Every one of these is fast enough that the format is not the decision. The
whole cache is 4-6 MB and loads in single-digit milliseconds.

### 5.4 So is it worth it?

The tempting number is the 16.76 s cold `bazel query //...`. But look at what
each layer actually costs:

| layer | cold cost | cacheable? |
| --- | ---: | --- |
| walk 74 K dirs + read 74 K BUILD files | **~4.4 s** (measured, parallel) | yes, but cheap |
| parse with `starlark-cst` | ~2.9 s serial, <0.5 s ×18 (from CONTEXT rates) | yes, but cheap |
| **cold `bazel query //...`** | **16.76 s** (CONTEXT) | yes — and dangerous |
| `query --output=streamed_proto` | 0.95 s warm, 52 MB → ~165 MB at 74 K pkgs | derived from the above |

**Do not persist the light index.** It rebuilds in ~4 s from disk you have to
touch anyway (you must stat every BUILD file to know the cache is valid, and
once you have stat'd 74,000 files you have paid 2.2 s of the 4.4 s). Loading
a cache you cannot trust is worse than a rebuild you can.

**Persisting the Bazel query result is the only thing with real value, and it
is the one with the worst invalidation story.** A `bazel query` answer depends
on every BUILD file, every `.bzl` transitively loaded, `MODULE.bazel` and its
lockfile, every fetched external repo, the `.bazelrc` (including
`--define`s and `--config` selection), the Bazel version, and the state of
`--enable_bzlmod` and friends. Hazards specifically:

1. **You cannot cheaply enumerate the inputs.** `.bzl` load graphs cross
   repository boundaries into `@rules_foo`, which lives in the output base.
2. **`select()` and configuration.** The same query under a different
   `--config` yields different `deps`. Key the cache on the resolved config
   or accept wrong answers.
3. **External repos are refetched out from under you.** A `bazel sync` or a
   registry update changes `@maven//` labels with no source-tree mtime change.
4. **Editors run concurrently with builds.** CONTEXT already measures a query
   against a base held by a running build at 4.31 s, bounded only by the
   build. A cache written mid-build captures a half-configured graph.
5. **mtime is not enough.** rust-analyzer#17423 is exactly this: *"mtime isn't
   sufficient (I believe this is a long-standing bug in Cargo…)"*. Content
   hashing 74,000 files costs the 2.2 s you were trying to save.

If you cache the query result anyway — and there is a real argument for it,
because 16.76 s in the background still means 16.76 s of degraded answers on
first open — then:

- **Use `redb` 4.2.0**, keyed **per package**, not one blob. Measured: 0.2 µs
  point get, 4 ms to update a single package durably. Per-package rows make
  invalidation granular, which is the only thing that makes the cache
  survivable, and they match commitment #3 (never eagerly index `//...`).
- Store the value bytes as **`rkyv`** archives so a row is usable without
  deserialising (0.1 ms mmap+validate for the whole index; per-row it is free).
- **Key every row on a content hash of the package's BUILD file plus a global
  epoch** derived from `MODULE.bazel.lock` + `.bazelrc` + `bazel info release`
  + `bazel info output_base`. Bump the epoch and the whole cache is dead,
  which is correct and costs 4 s.
- Version the schema and **delete the cache on any mismatch, silently and
  always**. The failure mode you must never ship is "have you tried deleting
  the cache".
- Never let a cache hit answer a request that a fresh answer would answer
  differently without saying so. Cached-and-possibly-stale is a diagnostic
  state (commitment #4), not a silent one.

`rkyv` 0.8's zero-copy story is the genuinely differentiating property here:
`rkyv::access::<ArchivedIndex, _>(&mmap)` gave a usable 189,000-entry index in
**0.1 ms** with no allocation, because the archived layout *is* the in-memory
layout. With `postcard` or `bincode` you pay 2-3 ms and 5.8 MiB of fresh
allocation. At this size neither matters; at 10× this size only `rkyv` stays
flat. Its one cost is that `access` validates untrusted bytes (that is the
1 ms scan above) — keep validation on, it is cheap and the file is on a disk
you do not control.

---

## 6. Recommendations

### Crates

| role | crate | version | why |
| --- | --- | --- | --- |
| file watching | `notify` | **8.2.0** | 9.0 has been in RC for 7 months; 8.2.0 is what rust-analyzer ships. Move to 9.0 the day it releases — you want `EventKindMask::CORE` (no `OPEN` storms) and the FSEvents fd budget. |
| debouncing | `notify-debouncer-full` | **0.7.0** | matches notify 8.x; merges FSEvents create/modify pairs, tracks file IDs, propagates rescan |
| traversal | `ignore` | **0.4.33** | only maintained crate with `build_parallel()`; 69 K entries/s vs 37 K serial. **`standard_filters(false)`** — Bazel does not honour `.gitignore` |
| — reject | `jwalk` | 0.9.0 | deprecated 2026-08-05, "use dua-core instead", 7% slower than `ignore` |
| — reject | `walkdir` alone | 2.5.0 | fine, but serial-only; it is already a transitive dep of `ignore` |
| interning | `lasso` | **0.7.3** | 33.4 B/entry, lowest measured. `Rodeo` to build, `into_reader()` for a `Send+Sync` snapshot. Never `ThreadedRodeo` (2.8× the RSS). Note: last release 2024-08-19 — if that bothers you, `string-interner` 0.20.0 costs +15% and is current. |
| transient strings | `smol_str` | 0.3.6 | inline ≤ 23 B, `O(1)` clone, maintained in the rust-analyzer tree. For parser/AST strings, not for the index. |
| — reject | `ustr` | 1.1.0 | 82-105 B/entry, worse than `String`; global leak-forever arena |
| — reject | `compact_str` for the index | 0.10.0 | excellent crate, wrong layer: 50 B/entry vs 33 |
| serialization | `rkyv` | **0.8.18** | zero-copy: 0.1 ms to a usable 189 K-entry index off an mmap |
| — reject | `bincode` | any | **unmaintained since 2025-12-16**; 3.0.0 is `compile_error!` |
| — if you need serde | `postcard` | 1.1.3 | the maintained bincode replacement; 4.40 MiB, 3 ms decode |
| kv store (only if caching `bazel query`) | `redb` | **4.2.0** | 0.2 µs point get, 4 ms durable single-package update, active |
| — reject | `sled` | any | 1.0 alpha since 2020, last publish 2024-10-11, "if reliability is your primary constraint, use SQLite" |

### Architecture

1. **One recursive watch per workspace root.** 0.002 s setup, 15.8 ms latency
   at 74 K packages, measured. Never one watch per directory — that is
   rust-analyzer's shape and it dies silently at 4,096 on macOS after 72 s of
   setup.
2. **Server-side watching by default**, client-side as an escape hatch. The
   LSP registration cannot express "not under `bazel-out`", and that is
   non-negotiable for Bazel.
3. **`follow_links(false)` everywhere**, plus prune any directory symlink
   resolving outside the root. Following them found 94,118 BUILD files where
   there are 74,001.
4. **Handle `event.need_rescan()` and `ErrorKind::MaxFilesWatch`.** Both mean
   the index is now a lie; say so (commitment #4).
5. **Test the symlink prune and the 4,096-path boundary in CI.** Both failure
   modes are silent, both are one line away.

### Measured memory budget, 74,000 packages / 189,000 targets

| component | measured RSS |
| --- | ---: |
| package-path interner (`lasso::Rodeo`, 74 K paths) | 4.80 MiB |
| target-name interner + 189 K × 16 B entry table | 9.02 MiB |
| name → entry lookup, `FxHashMap<Spur, Vec<u32>>` | 10.44 MiB |
| name → entry lookup, CSR (`Vec<u32>` offsets + ids) — computed, not measured | 1.08 MiB |
| process baseline | 1.77 MiB |
| **light index, hash-map lookup** | **26.00 MiB** (134.5 B/target) |
| **light index, CSR lookup** | **~16.6 MiB** (~79 B/target) |

Add the second tier: open files plus an LRU of full rowan CSTs. CONTEXT puts
all 74 K files at 1.6-3.3 GB, i.e. **22-45 KB per file**; a 64-file LRU is
**1.4-2.9 MB**, a 256-file LRU **5.6-11.5 MB**.

**Total steady-state target: 25-40 MB.** That is the whole point of the
two-tier design — for comparison, CONTEXT measures the Bazel server alone at
~991 MB RSS, and a second Bazel server at +1.2 GB. The language server should
be a rounding error next to the build tool it drives, and at 79-135 bytes per
target it is.

---

## Uncertainties

- **All watching measurements are macOS.** The inotify limits, the
  `follow_symlinks` default consequence, the `OPEN`-event storm and the
  16 KB `ReadDirectoryChangesW` buffer are read from kernel and crate source,
  not observed. The Linux 74 K-watch question in particular deserves a real
  measurement on a 16 GB box before shipping server-side watching as the
  default there.
- **The 4,096 FSEvents boundary is one machine.** `RLIMIT_NOFILE` soft here is
  256, which is macOS default but not universal; notify `main` computes its
  budget as `RLIMIT_NOFILE / 12`, implying the true limit scales with the fd
  limit while 4,096 is a separate hard ceiling. I bracketed 4,096 (works) /
  4,100 (dead) but did not vary `ulimit` on 8.2.0.
- **Traversal rates are APFS on one Mac.** ext4 and NTFS will differ, possibly
  a lot. The *relative* ranking of the three crates should hold; the absolute
  37 K/69 K entries per second should not be quoted as a cross-platform figure.
- **Names in the memory benchmark are synthetic**, with a deliberately swept
  distinct-name fraction (10% / 50% / 100%) because I had no real 74 K-package
  BUILD corpus. The 50% row is my guess at realistic; the 100% row is a safe
  upper bound and only moves `lasso` from 33.4 to 46.2 B/entry.
- **Repo boundary markers** (a subdirectory with its own `MODULE.bazel` /
  `REPO.bazel` / `WORKSPACE` being excluded from the parent repo's package
  space) — I did not verify the exact Bazel 8 semantics.
- **`lasso` has not shipped since 2024-08-19.** It is not deprecated and the
  API is complete, but if a two-year gap is disqualifying, `string-interner`
  0.20.0 (2026-04-30) is the drop-in at +15% memory.
