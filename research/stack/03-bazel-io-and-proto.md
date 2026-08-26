# Driving Bazel from Rust: subprocess, cancellation, gRPC, protobuf

Measured 2026-08-25/26 on Apple M5 Max (18 cores, 128 GiB), macOS/Darwin 25.6.0,
Bazel 8.7.0 (`/nix/store/w24fpafgc7ddaqbwy939v1g33k3lf6fg-bazel-8.7.0`), rustc/cargo
1.98.0, protoc 35.1. Workspace `/tmp/hugerepo`: 20,000 packages / 60,000 targets
(3 rules per package: `filegroup`, `filegroup`, `genrule`). Warm output bases
`/tmp/ob-lsp` (LSP-private, 1.3 GB RSS) and `/tmp/ob-user` (906 MB RSS).

Prototypes written and run for this report:

| dir | what |
| --- | --- |
| `/tmp/bzproto` | prost 0.14.4 incremental pipe decoder + `shared_child` SIGINT cancellation |
| `/tmp/bzprotox` | same, but `protox` instead of `protoc` |
| `/tmp/bzqpb` | quick-protobuf 0.8.1 + pb-rs codegen |
| `/tmp/bzrpb` | rust-protobuf 3.7.2, pure codegen |
| `/tmp/bzgrpc` | tonic 0.14.6 client for `CommandServer/Run` + real `Cancel` |
| `/tmp/slowrepo` | one genrule that `sleep 60`s, for cancellation experiments |

---

## 1. Where the `.proto` files actually live, and how big the problem is

Not in the nix store output. The Bazel nix derivation ships only the launcher
(`bin/bazel`, `bin/bazel-8.7.0-darwin-arm64`); the embedded tools are unpacked at
first run into the **install base**, which is *not* the output base:

```
$ readlink /tmp/ob-lsp/install
/Users/bruth/.cache/bazel/_bazel_bruth/install/b8505634b4001c12e1a1c6250d772351
$ ls .../install/<md5>/embedded_tools/src/main/protobuf/*.proto | wc -l
31
```

The md5 is the install hash and changes with every Bazel version, so reading protos
from disk at runtime is not a stable strategy. **Vendor them.**

| file | bytes | lines | syntax | package | imports |
| --- | --- | --- | --- | --- | --- |
| `build.proto` | 22,263 | 601 | proto2 | `blaze_query` | `src/main/protobuf/stardoc_output.proto` |
| `stardoc_output.proto` | 14,574 | 418 | proto3 | `stardoc_output` | **none** |
| `command_server.proto` | 7,845 | 224 | proto3 | `command_server` | `google/protobuf/any.proto`, `failure_details.proto` |
| `failure_details.proto` | 57,157 | 1,384 | proto3 | `failure_details` | `google/protobuf/descriptor.proto` |

So the **query path costs 36.8 KB / 1,019 lines of vendored proto with zero external
dependencies** — `build.proto` + `stardoc_output.proto` is a closed set, no well-known
types, nothing else. They generate 1,378 lines of Rust via prost.

The **gRPC path costs 65 KB / 1,608 lines more**, plus two well-known types
(`Any`, `descriptor.proto`) which prost-build/protoc supply. `failure_details.proto`
contains a proto2 `extend google.protobuf.EnumValueOptions` block; prost ignored it
without complaint.

### Version compatibility of the wire format

`command_server.proto`, fetched from release tags:

| tag | lines | RPCs |
| --- | --- | --- |
| 6.5.0 | 195 | Run, Cancel, Ping |
| 7.6.1 | 196 | Run, Cancel, Ping |
| 8.0.0 | 224 | Run, Cancel, Ping |
| 8.7.0 | 224 | Run, Cancel, Ping |
| HEAD (9.x, `3a9b19c` 2026-08-24) | 280 | Run, Cancel, Ping, **UpdateTerminalSize** |

Every diff across those five versions is additive: 7.x added `ExecRequest.should_exec = 5`;
8.0 added `RunRequest.quiet = 9`, `ScriptPath`, `PathToReplace`; HEAD added
`TerminalSizeRequest/Response`, `InfoItem`, `InfoResponse`, two `PathToReplace.Type`
enumerators and a `reserved 6`. No field was renumbered or removed.

`build.proto` grows the same way: 541 → 552 → 575 → 601 lines across 6.5.0 → 8.7.0.
The 7.6.1 → 8.7.0 delta is `Rule.rule_class_key = 16`, `Rule.rule_class_info = 17`
(the stardoc `RuleInfo` — this is what `--proto:rule_classes` fills in), and a new
`Repository` message. Nothing removed.

**Conclusion:** pin one vendored copy, compiled against the *oldest* Bazel you support,
and it will decode newer Bazel output correctly (unknown fields are skipped). This is a
much lower risk than it first appears.

---

## 2. Protobuf decoding

### 2.1 The three candidates as of 2026-08-25

| | **prost** | **protobuf** (stepancheg) | **quick-protobuf** |
| --- | --- | --- | --- |
| runtime version / date | **0.14.4**, 2026-06-07 | **3.7.2**, 2025-03-10 | **0.8.1**, 2022-11-22 |
| codegen crate | `prost-build` 0.14.4 | `protobuf-codegen` 3.7.2 | `pb-rs` 0.10.0 (2022-11-22) |
| downloads (all / last 90 d) | 549.8 M / 124.5 M | 171.6 M / 28.9 M | 34.3 M / 2.7 M |
| repo | tokio-rs/prost 4,749★, 240 open, pushed 2026-08-03 | stepancheg/rust-protobuf 2,973★, 91 open, pushed 2025-09-21 | tafia/quick-protobuf 473★, 65 open, **pushed 2024-02-14** |
| MSRV | 1.85 | — | — |
| needs `protoc` at build? | **yes** (or `protox`) | **no** (`.pure()`) | **no** |
| streaming API | manual framing; `decode_length_delimiter` provided | `CodedInputStream::from_buf_read` + `push_limit` | manual framing; `BytesReader` over `&[u8]` only |
| strings | owned `String` | owned `String` | **`Cow<'a, str>`** (zero-copy) |

Three things you need to know that the table doesn't say:

1. **`protobuf` v3 is declared end-of-life by its own author.** The rust-protobuf README:
   *"This implementation is approaching end of life. Official protobuf implementation
   (not based on this implementation) is on the way, that will be released as protobuf=4
   soon."* Last release 2025-03-10; last repo push 2025-09-21.

2. **`protobuf` v4 is a different crate.** stepancheg donated the crate name to Google.
   `protobuf` 4.35.1-release (2026-06-11) / 4.36.0-rc.2 (2026-08-03) is the official
   Google runtime — and it has `cc ^1.1.6` as a **build dependency**, because it wraps
   the upb C kernel. crates.io still reports `max_stable = 3.7.2`, i.e. every 4.x is a
   prerelease-tagged version. For a language server people `cargo install`, a C toolchain
   requirement is a worse tax than `protoc`. Disqualified.

3. **quick-protobuf's codegen is the stale part.** `pb-rs` 0.10.0 pulls `clap` 2.34,
   `atty` 0.2.14 (RUSTSEC-2021-0145, unmaintained) and `env_logger` 0.7 — build-time only,
   but it's a signal. It also failed on Bazel's protos with *relative* include paths:

   ```
   InvalidImport("file src/main/protobuf/stardoc_output.proto not found on import path")
   ```

   It works if you pass absolute include paths built from `current_dir()`. The generated
   Rust is otherwise fine, including `Cow<'a, str>` borrowed strings and `Target<'a>`.

### 2.2 The `protoc` problem, and `protox`

Verified, with `protoc` removed from `PATH` and `$PROTOC` unset:

```
thread 'main' panicked at build.rs:4:10:
Could not find `protoc`. If `protoc` is installed, try setting the `PROTOC`
environment variable to the path of the `protoc` binary. ...
```

prost-build 0.14.4 hard-requires it. In the nix devshell it's free (`pkgs.protobuf`
is already in the flake's `buildInputs`), but it breaks `cargo install`.

`protox` 0.9.1 (2025-12-02; andrewhickman/protox 121★, 8 open, pushed 2026-07-27;
13.9 M downloads, 3.1 M in 90 d) is a pure-Rust `.proto` compiler that produces a
`FileDescriptorSet` prost-build can consume directly. Verified working with **no protoc
on `PATH`**, 14.4 s cold build of the codegen chain:

```rust
// build.rs — no protoc anywhere
fn main() {
    let fds = protox::compile(
        ["src/main/protobuf/build.proto", "src/main/protobuf/stardoc_output.proto"],
        ["proto"],
    ).unwrap();
    prost_build::Config::new().compile_fds(fds).unwrap();
}
```

The resulting binary produces byte-identical results to the protoc-built one.
`protox` adds `miette`, `logos`, `prost-reflect` to the *build* graph only.

The alternative — check the generated `.rs` into the repo — is what `starpls` does
*not* do (`crates/starpls_bazel/build.rs` calls `prost_build::compile_protos` with
prost-build **0.12.3**, so starpls also needs protoc at build time). Checking in
generated code is viable given the protos change ~25 lines per Bazel major, but it
means `just ci` has to verify the checked-in file is current.

### 2.3 The framing problem: `streamed_proto` is not a protobuf message

`--output=streamed_proto` emits a bare sequence of `varint(len) || Target`. There is no
outer message, so `Message::decode` on the whole stream is meaningless. Verified on the
wire (`//pkg/s0/p0:all`):

```
streamed_proto:  a0 05 | 08 01 12 9b 05 0a 0d 2f 2f 70 6b 67 ...   ← len=672, then Target
proto:        0a a0 05 | 08 01 12 9b 05 0a 0d 2f 2f 70 6b 67 ...   ← tag 0x0a, len=672, then Target
```

`--output=proto` (`QueryResult { repeated Target target = 1 }`) differs by exactly one
tag byte per record, so **the same incremental loop decodes both**. Time-to-first-byte
is identical for the two (0.39 s each on `//...`), so `streamed_proto`'s advantage is
server-side heap and the 2 GB single-message limit, not latency.

`prost::Message::decode_length_delimited` exists and is a trap on a pipe: on a short read
it returns the same `DecodeError` as on corruption, so you cannot tell "need more bytes"
from "the stream is broken". Use the two-step form and bound the varint at 10 bytes:

```rust
use bytes::{Buf, BytesMut};
use prost::Message;

/// Pull every complete length-delimited message out of `pending`.
fn drain(pending: &mut BytesMut, eof: bool, out: &mut Vec<Entry>) -> Result<(), Error> {
    loop {
        if pending.is_empty() { return Ok(()); }
        // decode_length_delimiter(mut buf: impl Buf) -> Result<usize, DecodeError>
        // consumes the varint, so hand it a *copy* of the cursor: a short read must
        // leave `pending` untouched.
        let mut peek: &[u8] = &pending[..];
        let len = match prost::decode_length_delimiter(&mut peek) {
            Ok(len) => len,
            // A truncated varint is indistinguishable from a bad one, so use the
            // fact that a varint is at most 10 bytes.
            Err(_) if pending.len() < 10 && !eof => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let hdr = pending.len() - peek.len();     // == prost::length_delimiter_len(len)
        if pending.len() - hdr < len { return Ok(()); }   // body not here yet
        pending.advance(hdr);
        let frame = pending.split_to(len);
        let t = Target::decode(&frame[..])?;
        out.push(Entry::from(&t));
        // `t` and `frame` drop here. This is the whole point.
    }
}
```

Driven by a plain read loop that never holds more than one chunk plus one partial
message:

```rust
let mut pending = BytesMut::with_capacity(1 << 20);
let mut chunk = vec![0u8; 256 * 1024];
loop {
    let n = stdout.read(&mut chunk)?;
    if n == 0 { drain(&mut pending, true, &mut out)?; break; }
    pending.extend_from_slice(&chunk[..n]);
    drain(&mut pending, false, &mut out)?;
}
```

prost's generated types for proto2, for reference — `required` collapses to a plain
field, which is convenient here:

```rust
pub struct Target {
    #[prost(enumeration = "target::Discriminator", required, tag = "1")] pub r#type: i32,
    #[prost(message, optional, tag = "2")] pub rule: Option<Rule>,
    ...
}
pub struct Rule {
    #[prost(string, required, tag = "1")] pub name: String,
    #[prost(string, required, tag = "2")] pub rule_class: String,
    #[prost(string, optional, tag = "3")] pub location: Option<String>,
    #[prost(message, repeated, tag = "4")] pub attribute: Vec<Attribute>,
    ...
}
```

### 2.4 The measurement

Corpus: `bazel query //... --output=streamed_proto --proto:rule_classes` on
`/tmp/hugerepo` → **53,696,596 bytes / 60,000 `Target` messages** (51.2 MiB).
The 205 MiB case is that file concatenated 4× (240,000 messages), which brackets the
165 MB / 189,000-target real-monorepo figure from CONTEXT.

`index` = decode the full `Target`, extract `(name, kind, file, line)` into a
`Vec<(Box<str>, u32, Box<str>, u32)>`, drop the message. That is what the two-tier
index actually needs. `drop` = decode and discard. `skim` = hand-rolled wire scan.
Three runs each, medians, release build.

**Decoding 51.2 MiB from a warm page-cached file:**

| decoder | mode | time | throughput | msg/s | peak RSS |
| --- | --- | --- | --- | --- | --- |
| prost 0.14.4 | index | 0.158 s | **324 MB/s** | 380 k | 12 MB |
| prost 0.14.4 | drop | 0.159 s | 323 MB/s | 378 k | 3 MB |
| rust-protobuf 3.7.2 | index | 0.166 s | 308 MB/s | 361 k | 7 MB |
| quick-protobuf 0.8.1 | index | 0.125 s | **409 MB/s** | 479 k | 9 MB |
| hand-rolled skim | — | 0.009 s | 5,906 MB/s | 6,920 k | 3 MB |

**Decoding 204.8 MiB (240,000 targets) — incremental vs. slurped:**

| approach | time | throughput | **peak buffer** | **peak RSS** |
| --- | --- | --- | --- | --- |
| prost, incremental (256 KB reads) | 0.622 s | 329 MB/s | **1,281 KB** | **34 MB** |
| prost, whole file in memory first | 0.641 s | 320 MB/s | 419,504 KB | **442 MB** |
| rust-protobuf, incremental | 0.675 s | 304 MB/s | (BufRead 256 KB) | 23 MB |
| quick-protobuf, incremental | 0.494 s | 415 MB/s | 2,049 KB | 24 MB |

Incremental costs **nothing** in throughput and **13×** less memory. Both retain all
240,000 index entries; the RSS difference is purely the un-streamed input buffer.

**The deliverable — decoding live off the `bazel` pipe** (`bzproto stream index`,
4 interleaved runs against `/tmp/ob-lsp`):

```
stream/index  msgs=60000  bytes=51.2MB  0.908s  56 MB/s  66k msg/s
              peak_pending=1040KB  index=60000  t_first=0.469s  maxRSS=12MB
stream/index  msgs=60000  bytes=51.2MB  0.980s  52 MB/s  61k msg/s  t_first=0.552s
stream/index  msgs=60000  bytes=51.2MB  0.965s  53 MB/s  62k msg/s  t_first=0.522s
stream/index  msgs=60000  bytes=51.2MB  0.965s  53 MB/s  62k msg/s  t_first=0.535s
```

`/usr/bin/time -l` on the whole process tree (decoder + forked bazel client):
**16,105,472 bytes = 15.4 MiB peak RSS** to turn 51.2 MiB of proto into a 60,000-entry
index.

The headline number is the one that isn't there. **Effective throughput is 52–56 MB/s
and that is Bazel's write rate, not the decoder's.** Bazel takes ~0.5 s to produce the
first byte and ~0.45 s to stream the rest; prost consumes it in 0.16 s of CPU fully
overlapped with the producer. Decode is **6× faster than the source can supply it**, and
the hand-rolled skim shows another **18×** of headroom above that if it ever mattered.

Extrapolating to the real monorepo (189,000 targets, ~165 MB): ~0.5 s of decode,
~30 MB of peak RSS for the streaming buffer plus whatever the index itself weighs,
entirely hidden behind Bazel's 3–5 s.

**Choice of decoder is therefore not a performance decision.** quick-protobuf is 1.3×
faster and irrelevant; it is also 3¾ years stale with a stale codegen tree.
rust-protobuf has the nicest native streaming API (`CodedInputStream::from_buf_read`
reads straight off a `BufRead`, no manual framing) and needs no protoc, but it is EOL.
prost wins on maintenance, ecosystem (it's what `tonic` speaks, so one codegen serves
both paths), and 4× the download volume.

### 2.5 The 10× that *is* available: ask Bazel for less

Same query, same warm server, varying the proto sub-options:

| flags | output | wall |
| --- | --- | --- |
| `--proto:rule_classes` | 53.7 MB | 0.89 s |
| (default) | 53.0 MB | 0.85 s |
| `--proto:output_rule_attrs=` | 7.6 MB | 0.48 s |
| `--proto:output_rule_attrs= --noproto:rule_inputs_and_outputs` | **5.4 MB** | **0.42 s** |

The lean output still contains everything the light index tier needs:

```
$ head -c 80 /tmp/q-lean.streamed_proto | xxd
00000000: 5008 0112 4c0a 0d2f 2f70 6b67 2f73 302f  P...L..//pkg/s0/
00000010: 7030 3a61 1209 6669 6c65 6772 6f75 701a  p0:a..filegroup.
00000020: 302f 7072 6976 6174 652f 746d 702f 6875  0/private/tmp/hu
...  //pkg/s0/p0:a | filegroup | /private/tmp/hugerepo/pkg/s0/p0/BUILD.bazel:1:1
```

60,000 targets decoded in **8 ms**. That turns the 165 MB refresh into ~17 MB and
halves Bazel's own time. Attributes should be a second, narrower query for the packages
that actually need them.

---

## 3. Subprocess supervision, and the cancellation that doesn't work

### 3.1 Killing the client does not stop the server

The load-bearing experiment. `/tmp/slowrepo` contains one genrule that runs `sleep 60`.
Start `bazel build //:slow`, wait for the action to be executing, `SIGKILL` the client:

```
### SIGKILL client=68255 action=68280
client rc=137 after 0s
  action STILL alive at +40s
-- lock test:
Another command (pid=68255) is running. Exiting immediately.
```

The `sleep 60` ran to completion. The Bazel server held the command lock for the full
duration and refused new commands, **naming a client PID that no longer existed**. A
shorter run confirms the server process stays in state `Ss` burning no extra CPU while
the action continues, and `java.log` records no interrupt.

`SIGINT` and `SIGTERM` both work:

```
### SIGINT  client=69568  action=69595   → client rc=8, action died at +1s, lock free
### SIGTERM client=69808  action=69835   → client rc=8, action died at +1s, lock free
          stderr: ERROR: build interrupted
```

The mechanism, from `src/main/cpp/blaze_util_posix.cc:144-193`:

```cpp
static void handler(int signum) {
  switch (signum) {
    case SIGINT:
      if (++sigint_count >= 3) { KillServerProcess(...); _exit(1); }   // third ^C kills the JVM
      SigPrintf("\n%s caught interrupt signal; cancelling pending invocation.\n\n", ...);
      SignalHandler::Get().CancelServer();
      break;
    case SIGTERM:
      SignalHandler::Get().CancelServer();
      break;
    case SIGQUIT:
      kill(server_pid, SIGQUIT);   // JVM thread dump into java.log
      break;
```

`CancelServer()` → `BlazeServer::Cancel()` → a dedicated cancel thread issues
`CommandServer/Cancel { cookie, command_id }` with a 10 s deadline
(`blaze.cc:1932-1947`, `blaze.cc:2097`). **The signal is only a courier for a gRPC
call.** `SIGKILL` cannot run a handler, so no `Cancel` is sent, and Bazel's server has
no client-liveness watchdog for the `Run` stream — `GrpcCommandServerImpl`'s
`BlockingStreamObserver` sets the command thread's interrupt bit only inside `onNext`
(`GrpcCommandServerImpl.java:102-129`), i.e. only when the command next tries to emit
output. A silent phase means no cancellation at all.

Note the third-`^C` behaviour: three SIGINTs in one invocation and the client
`KillServerProcess`es the JVM. An LSP that retries a signal in a loop will destroy the
user's warm server.

There is a **second, accidental cancellation path**: closing the read end of the
client's stdout. `blaze.cc:2144` calls `Cancel()` on a broken pipe. Verified — read
64 KB of a 53 MB `streamed_proto` stream, then close:

```
read 65536 bytes at 0.565s; closing read end
client exited rc=-13 at 1.011s
stderr: Cannot write to standard output; exiting...
$ bazel --noblock_for_lock info server_pid   →  42685      (lock already free)
```

It works, but only once the server tries to write again, and it kills the client with
SIGPIPE. Fine as a backstop for a dropped `Child`; not a design.

### 3.2 What this means for the Rust API choice

| | verdict |
| --- | --- |
| `std::process::Command` + threads | **`Child::kill()` sends SIGKILL** — exactly the signal that leaves Bazel's server wedged. There is no std API to send SIGINT. And `wait()` and `kill()` both take `&mut Child`, so you cannot signal from one thread while another blocks on `wait`/reads stdout. `Child`'s `Drop` does not kill or reap. Unusable alone. |
| **`shared_child` 1.1.1** (2025-07-04, 47.9 M dl, 10.4 M/90 d; oconnor663, 51★, 5 open, pushed 2026-01-22) | `Arc<SharedChild>` is `Send + Sync`; `wait()`/`try_wait()`/`kill()` take `&self`; `take_stdout()` hands the pipe to a reader thread; and `unix::SharedChildExt::send_signal(c_int)` is a **safe** wrapper over `libc::kill`, which matters under `unsafe_code = "forbid"`. Three tiny deps (`libc`, `windows-sys`, optional `sigchld`). This is the one. |
| `duct` 1.1.1 (2025-11-09, 36.4 M dl; 1,040★, 35 open) | Built on `shared_child` by the same author. Adds pipelines, redirection and a `ReaderHandle`. `Handle::kill()` is SIGKILL of the whole group; no signal API. You'd be adding an expression DSL to get less control. No. |
| `command-group` 5.0.1 | **Stale, 2023-11-18.** Superseded by `process-wrap` 10.0.0 (2026-08-24, watchexec, 45★, 5 open). `process-wrap` does have signal support and process-group/job-object semantics — but Bazel's server is *deliberately* daemonised out of your process group, so group-kill buys nothing and risks nothing useful. Overkill. |
| `tokio::process` | Also SIGKILL-only (`Child::kill()`), no signal API, and `kill_on_drop(true)` is the wrong semantic for Bazel. Requires the tokio runtime with the signal driver (a process-global SIGCHLD handler). CONTEXT says "there is no high-concurrency network IO anywhere in this program" — a handful of long subprocesses is the exact case where threads win. Only justified if you also adopt tonic (§4), which drags tokio in anyway. |

### 3.3 The pattern that works, measured

`std::process::Command` → `SharedChild::spawn`, reader thread owns stdout, main thread
keeps the handle and signals, then **waits** — the client needs to stay alive long
enough for its cancel thread to land the `Cancel` RPC:

```rust
use shared_child::SharedChild;
use shared_child::unix::SharedChildExt;

let child = Arc::new(SharedChild::spawn(&mut cmd)?);
let out = child.take_stdout().unwrap();
let reader = std::thread::spawn(move || decode_stream(out));   // §2.3 loop

// ... user typed again, index refresh is stale ...
child.send_signal(libc::SIGINT)?;   // NOT kill(): that's SIGKILL
let status = child.wait()?;         // let the client deliver Cancel and drain
let stats = reader.join().unwrap();
```

Measured (`bzproto cancelcli`):

```
[3.003s] sent SIGINT to client pid 85558
[3.003s] stderr: Bazel caught interrupt signal; cancelling pending invocation.
[3.024s] stderr: ERROR: build interrupted
cancelcli: signalled at 3.003s, client exited at 3.045s (0.042s later), status=exit(8)

# against //... on hugerepo, SIGINT at 250 ms:
[0.251s] sent SIGINT to client pid 85617
[0.260s] stderr: ERROR: query interrupted
cancelcli: signalled at 0.251s, client exited at 0.264s (0.013s later), status=exit(8)
```

**13–42 ms from signal to a fully released command lock.** Exit code 8 is Bazel's
`INTERRUPTED`; treat it as "cancelled", never as an error to surface.

---

## 4. The gRPC command server from Rust

### 4.1 It works, and it took one afternoon

`/tmp/bzgrpc`: tonic 0.14.6 + tonic-prost 0.14.6 + tonic-prost-build 0.14.6 + prost
0.14.4 + tokio. Vendored `command_server.proto`, `failure_details.proto`,
`build.proto`, `stardoc_output.proto`. **Compiled first try**, 15.3 s cold.

```rust
let port = read(format!("{ob}/server/command_port"))?;          // "[::1]:63458"
let mut client = CommandServerClient::connect(format!("http://{port}")).await?;
let mut stream = client.run(RunRequest {
    cookie: read(format!("{ob}/server/request_cookie"))?,        // 32 hex chars
    arg: ["query", "//...", "--output=streamed_proto", "--proto:rule_classes"]
        .iter().map(|a| a.as_bytes().to_vec().into()).collect(),
    block_for_lock: false,
    client_description: "bazel-language-server".into(),
    ..Default::default()
}).await?.into_inner();

while let Some(m) = stream.next().await {
    let m = m?;
    assert_eq!(m.cookie, response_cookie);
    pending.extend_from_slice(&m.standard_output);   // same §2.3 drain() loop
    drain(&mut pending, false, &mut out)?;
    if m.finished { exit = m.exit_code; }
}
```

Measured against `/tmp/ob-lsp`:

```
grpc query: connect=0.2ms  msgs=60000  bytes=53.7MB  ttfb=0.430s  total=0.856s
            max_chunk=8KB  max_pending=1032KB  exit=0
Ping: connect=0.2-0.6ms, 0.6-1.1ms round trip
```

The server chunks `standard_output` into 8 KB gRPC frames (~6,700 `RunResponse`
messages for 53.7 MB); the incremental decoder handles them with the same 1 MB
bounded buffer. No `--output=proto` reassembly needed.

### 4.2 What it actually buys

Interleaved A/B, 15 alternating reps, trivial query `//pkg/s0/p0:a`:

```
CLI  (fork + connect + run):    min=396  med=463  max=594 ms
gRPC (connect + run, in-proc):  min=324  med=357  max=508 ms
median delta = 106 ms
```

Interleaved A/B on the full `//...` streamed_proto query:

```
CLI   0.908  0.980  0.965  0.965 s
gRPC  0.994  0.856  0.912  0.853 s
```

So: **~105 ms saved per call**, consistent with the prior grpcurl finding (234 vs
296 ms) and the "~130 ms client tax" estimate. On the 0.9 s query it's ~9% and inside
the noise; on a trivial query it's 23%.

Note what it does *not* buy: the trivial one-target query still costs **~357 ms
server-side**. That is Bazel's per-command floor — option parsing, Skyframe entry, UI
setup — and no transport removes it. It is independent confirmation of CONTEXT's rule 1.

### 4.3 Cancellation, which is the real reason

`Cancel` from Rust, with the timer on an **independent task** (see the trap below):

```rust
let mut c2 = CommandServerClient::connect(endpoint).await?;   // second channel
tokio::time::sleep(delay).await;
c2.cancel(CancelRequest { cookie, command_id }).await?;
```

```
[0.036s] stderr: INFO: Analyzed target //:slow (5 packages loaded, 8 targets configured).
[2.040s] stderr: [1 / 2] Executing genrule //:slow; 1s local
[3.006s] Cancel RPC sent, returned in 2.3ms
[3.012s] stderr: ERROR: build interrupted
finished exit=8 at 3.015s; cancel sent at 3.006s => stopped 0.010s later
failure_detail: FailureDetail { message: "build interrupted",
                                category: Some(Interrupted(Interrupted { code: Interrupted })) }
```

**Cancel RPC returns in 1.0–2.3 ms; the command stops 6–27 ms later.** And you get a
structured `failure_detail` instead of parsing `ERROR: build interrupted` out of stderr.

Cancelling a `query` at increasing offsets into a 0.85 s `//...` run:

| cancel at | outcome |
| --- | --- |
| 54 ms | exit 8, stopped 27 ms later |
| 154 ms | exit 8, stopped 6 ms later |
| 254 ms | exit 8, stopped 7 ms later |
| 352 ms | **exit 0, ran to completion 440 ms later** |

**Bazel's query output-serialisation phase is not interruptible.** Once the result set
exists and it has started writing the 53 MB, `Cancel` is ignored and you pay the full
cost. Cancellation saves the *compute*, not the *transfer*. This is also true of the
SIGINT path (same server-side mechanism), and it is another argument for §2.5: a 5 MB
lean query has a 10× smaller uninterruptible tail.

Two traps found while building this:

1. **You cannot decide to cancel from inside the response-stream loop.** A build emits
   progress roughly every 10 s; my first attempt checked the clock on each stream item
   and consequently cancelled at 12.1 s instead of 3 s. The canceller must be a separate
   task/thread with its own channel (`Cancel` on the same channel as a blocked `Run` is
   asking for trouble).
2. **`RunRequest` has no working directory.** The workspace is fixed when the server
   starts — visible in `java.log`:
   `args [--max_idle_secs=15, ..., --output_base=/private/tmp/ob-idle, --workspace_directory=/private/tmp/slowrepo, ...]`.
   One server per workspace, and you must still fork the CLI once to create it.

### 4.4 Verdict

**No for v1. Design the invocation layer as a trait with two impls, ship the CLI one.**

For:
- 105 ms/call, real (measured, not estimated).
- `Cancel` with a 2 ms ack and a structured `FailureDetail`, versus a signal whose
  semantics you have to reverse-engineer from `blaze_util_posix.cc`.
- `block_for_lock` decided per request, not per process.
- `Ping` gives you liveness *and* keeps the server warm (§5) in one 1 ms call.
- Version risk is low: `Run`/`Cancel`/`Ping` unchanged 6.5.0 → 8.7.0 → HEAD, additive
  fields only (§1).

Against:
- **89 crates instead of 34** (67 runtime vs 12): tokio, hyper, h2, tower, axum,
  hyper-util, tracing… for one localhost stream. The whole point of `lsp-server` over
  `tower-lsp` was avoiding this.
- You still fork the CLI to boot the server, apply `--output_base`/`--max_idle_secs`/
  `--host_jvm_args`, and create `server/`. `RunRequest.startup_options` is documented
  "for logging only".
- Cookies, port and PID rotate on every server restart; you must re-read `server/` and
  detect death. The CLI does this for you, including the "server needs to be killed
  because startup options differ" logic (§5).
- `command_server.proto` is not a public API in the way `build.proto` is.
- **tonic itself is mid-transition.** `hyperium/tonic` now redirects to
  **`grpc/grpc-rust`** — the project was donated to the CNCF gRPC project
  (12,447★, 396 open, pushed 2026-08-25). The README says master "is currently
  preparing breaking changes" and a new `grpc` crate is coming, with the old tonic
  transport expected to be deprecated. Adopting tonic today means adopting a migration.
- The 105 ms only matters if you make several Bazel calls per user action. CONTEXT rule 1
  says you make zero per request.

The cheap 90% of the benefit is available without tonic: `--noblock_for_lock` costs
nothing, and SIGINT-then-wait gives cancellation in 13–42 ms (§3.3) versus gRPC's
6–27 ms. That gap is not worth 55 crates.

---

## 5. Keeping the server warm

### 5.1 The flag

```
$ bazel help startup_options --long
  --max_idle_secs (an integer; default: "10800")
    The number of seconds the build server will wait idling before shutting
    down. Zero means that the server will never shutdown. This is only read on
    server-startup, changing this option will not cause the server to restart.
      Tags: eagerness_to_exit, loses_incremental_state
  --[no]shutdown_on_low_sys_mem (a boolean; default: "false")
    If max_idle_secs is set and the build server has been idle for a while,
    shut down the server when the system is low on free RAM. Linux only.
  --[no]idle_server_tasks (a boolean; default: "true")
    Run System.gc() when the server is idle
  --[no]block_for_lock (a boolean; default: "true")
  --[no]preemptible (a boolean; default: "false")
```

Default 10800 s = **3 hours**. (`BlazeServerStartupOptions.java:216-227`,
`defaultValue = "" + (3 * 3600)`. The installed 8.7.0 help says `shutdown_on_low_sys_mem`
is "Linux only"; HEAD's source string says "Linux and MacOS only" — the help text is
behind the code.)

The two sentences that matter: **"This is only read on server-startup, changing this
option will not cause the server to restart."** Verified — passing `--max_idle_secs=900`
to a running server neither restarts it nor changes anything. Whoever starts the server
sets the policy for everyone using that output base.

### 5.2 What evicts it

1. **Idleness.** `ServerWatcherRunnable.run()` arms `shutdownTimeMillis = now +
   max_idle_secs` on the transition into idle and re-checks every 5 s. Only started if
   `maxIdleSeconds > 0` (`CommandServer.java:333-341`), so `--max_idle_secs=0` really is
   immortal.
2. **Different startup options.** Measured:
   ```
   $ bazel --output_base=/tmp/ob-slow --host_jvm_args=-Xmx2g info server_pid
   WARNING: Running Bazel server needs to be killed, because the following startup options are different:
     - Only in old server:
     - Only in new server: -Xmx2g --host_jvm_args=-Xmx2g
   Starting local Bazel server (8.7.0- (@non-git)) and connecting to it...
   ```
   `--max_idle_secs`, `--client_debug` and `--quiet` are exempt (their help text says so;
   confirmed for `--max_idle_secs`). **This is the eviction that will actually bite an
   LSP** if it ever shares an output base with the user.
3. **`bazel shutdown`, `bazel clean --expunge`, `--batch` against a non-batch base.**
4. **Three `SIGINT`s** in one invocation (`blaze_util_posix.cc:151-163`).
5. **`PidFileWatcher`**: every 3 s the server verifies `server/server.pid.txt` still
   contains its own PID and calls `Runtime.halt(BLAZE_INTERNAL_ERROR)` if not
   (`PidFileWatcher.java:45-70`). Never write to `<output_base>/server/`.
6. **`--shutdown_on_low_sys_mem`** — off by default, and only after 5 minutes idle
   (`TIME_IDLE_BEFORE_MEMORY_CHECK`), so not a factor unless a user enables it.

Eviction cost, measured on the *warm* `/tmp/hugerepo` output base (disk caches hot,
only the JVM and Skyframe are lost):

```
$ bazel --output_base=/tmp/ob-kill shutdown
$ time bazel ... query //... --output=streamed_proto --proto:rule_classes   → 3.09 s
$ time (again, warm server)                                                 → 0.99 s
```

**+2.1 s** here; on a real monorepo with Starlark rules and real loading it is the
16.76 s figure from CONTEXT, or worse.

### 5.3 Should the LSP ping?

**A `Ping` resets the idle timer.** `CommandServer.ping()` opens a
`RunningCommand` (`CommandServer.java:586`), which makes `commandManager` non-empty,
which makes `ServerWatcherRunnable` re-arm on the next idle transition. Verified
end-to-end with `--max_idle_secs=15`:

```
server=79590, --max_idle_secs=15
--- phase 1: Ping every 5s for 35s
  t=5s  alive=yes  ping 0.91ms   t=20s alive=yes  ping 0.70ms
  t=10s alive=yes  ping 0.85ms   t=25s alive=yes  ping 0.59ms
  t=15s alive=yes  ping 0.72ms   t=30s alive=yes  ping 0.83ms
                                 t=35s alive=yes  ping 0.84ms
--- phase 2: stop pinging
  server exited at ~18s after last ping
java.log: [ServerWatcherRunnable.run] About to shutdown due to idleness
```

So the LSP *can* keep a server alive forever with a ~1 ms call. **It should not.**

- Pinging only makes sense over gRPC; over the CLI the cheapest keepalive is
  `bazel info server_pid`, which costs ~360 ms and takes the command lock.
- Every `RunningCommand` open/close also calls `IdleTaskManager.busy()`, which
  **cancels the pending idle GC**. `GcAndInternerShrinkingIdleTask.delay()` is 10 s when
  state was kept after the build. A ping interval under ~10 s would permanently suppress
  `System.gc()` and interner shrinking on a 1.3 GB server. Fighting Bazel's own memory
  hygiene to save a 2 s restart is a bad trade on a developer laptop.
- 3 hours already outlives any plausible editing session.

**Recommendation:** don't ping. Start the LSP's private server with an *explicit*
`--max_idle_secs` — 900 (15 min) is right: long enough that the server survives lunch,
short enough that a 1.3 GB JVM doesn't outlive the editor by hours. Add
`--tool_tag=bazel-language-server` so users can identify your invocations in BEP and
profiles. Detect death by `<output_base>/server/server.pid.txt` + the file's PID being
live (or a `Ping` immediately before a batch of work, never on a timer), and just accept
the restart.

The remaining question CONTEXT already raised — private output base (+1.2 GB, never
blocked by the user's builds) versus shared (free, but 4.31 s queries behind a running
build and a restart every time startup options differ) — is decided by §5.2 item 2 as
much as by memory: on a shared base you cannot control `--max_idle_secs` and any
`--host_jvm_args` difference nukes the user's warm state.

---

## 6. Recommended crates

```toml
# decode
prost       = "0.14.4"      # + bytes = "1"
[build-dependencies]
prost-build = "0.14.4"
protox      = "0.9.1"       # so protoc is not required to build

# drive
shared_child = "1.1.1"      # Arc-shareable Child + unix::SharedChildExt::send_signal
libc         = "0.2"        # SIGINT constant only; no unsafe in our code
```

Nine crates in the runtime graph, all first-tier. No tokio, no tonic, no protoc, no
`unsafe`. Vendor `build.proto` + `stardoc_output.proto` (36.8 KB, self-contained) under
`proto/src/main/protobuf/` so the import path resolves as-written.

Deliberately rejected: `protobuf` v3 (EOL per its author), `protobuf` v4 (needs a C
compiler), `quick-protobuf`/`pb-rs` (stale since 2022; the 1.3× speed is 100× more than
needed), `duct`/`command-group`/`process-wrap` (no signal API worth the dependency, or
stale), `tokio::process` (SIGKILL-only, wrong runtime shape for ~5 long subprocesses),
`tonic` (§4.4 — behind a trait, revisit if a profile ever shows the 105 ms).

Keep the protoc-based `build.rs` path working too (one `cfg!` on an env var): protox is
a young single-maintainer crate, and the nix devshell already has protoc.

## The riskiest design decision

**Cancelling by signalling the `bazel` client, rather than speaking `CommandServer/Cancel`.**

It is correct today — 13–42 ms to a released lock, verified against Bazel 8.7.0 — but it
rests on undocumented client behaviour that could change without a release note: that
SIGINT and SIGTERM both mean "send Cancel", that the client stays alive long enough to
send it, that the third SIGINT kills the JVM, that a broken stdout pipe also cancels.
None of that is in `bazel help`; all of it is in `blaze_util_posix.cc` and `blaze.cc`.
And the failure mode is not a broken feature, it is a **1.3 GB server holding a command
lock referencing a PID that no longer exists**, which the user experiences as "Bazel is
hung" and blames on their build, not on the language server.

Three consequences that must be designed in from the start, because none can be
retrofitted:

- **Never `Child::kill()`, never `kill_on_drop`, never SIGKILL a Bazel client** — encode
  this in the type: expose only `cancel()` on the wrapper and never hand out the raw
  `Child`. `SharedChild::kill()` must not be reachable.
- **Always `wait()` after signalling**, with a timeout, and treat exit code 8 as
  "cancelled". Never abandon a client process.
- **Put the whole thing behind a trait with a gRPC implementation in mind.** If the
  signal contract breaks in Bazel 9 or 10, `CommandServer/Cancel` is the documented
  escape hatch, it is already proven to work from Rust (§4.3), and the porting cost is
  the transport, not the decoder — the `drain()` loop is identical for a pipe and for
  `RunResponse.standard_output`.

The runner-up risk is the uninterruptible tail (§4.3): past ~40% of a query's wall time
nothing can stop it, so debounce and `--proto:output_rule_attrs=` (§2.5) do more for
perceived responsiveness than any cancellation mechanism.
