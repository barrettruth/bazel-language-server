# Repository Agent Guide

## Working tree and tools

- This is a colocated Jujutsu/Git checkout. Establish state with `jj status`
  and `jj log`; create small commits with `jj commit`.
- Run Rust and site tooling through Nix. The complete local gate is
  `nix develop .#ci --command just ci`, followed by `nix flake check`.
- Install the release binary with
  `nix develop -c cargo install --path . --force --locked`. Confirm the
  installed executable and `target/release/bazel-language-server` have the
  same digest.

## Architecture invariants

- The stdio protocol loop owns mutable session state and must not block on
  filesystem walks, Bazel, buildifier, or request handlers.
- A request captures `Documents` and `Index` together. Handlers operate on
  those immutable snapshots; do not load a newer index from a queued worker.
- Request admission, completed results, diagnostics, and filesystem wakes are
  bounded or coalesced without blocking the protocol loop.
- Syntax diagnostics publish immediately. Buildifier lint runs on the
  latest-only worker keyed by URI, and results publish only while their
  `Arc<Document>` is still current.
- Index tiers have one writer each: the protocol loop publishes open buffers,
  the watch thread publishes disk state, and the Bazel actor publishes graph
  and repository state.
- The Bazel actor owns every blocking Bazel operation. Register subprocesses
  before reading them, check supersession between multi-command stages, keep
  the operation generation live through publication, and retain one refresh
  after supersession unless a refresh or stop is already queued.
- Interrupt Bazel with `SIGINT`, not `SIGKILL`; killing the client can leave its
  server-side command holding the shared command lock.
- Watcher bursts accumulate classified invalidations up to a fixed bound and
  settle against one deadline. Dropped events, package-tree changes, and an
  edited `.bazelignore` require a full scan; ignored and output-tree paths do
  not invalidate either index tier.

## Workspace probes

- Run the large static/LSP probe and the single-file replacement measurement
  directly. Their defaults target `/Users/bruth/dev/imc/roadrunner`; use the
  documented `BLS_*` variables for other workspaces.

  ```sh
  env BLS_WORKSPACE=/Users/bruth/dev/imc/roadrunner \
    nix develop -c cargo test --release --test workspace_probe -- \
    --ignored --nocapture --test-threads=1
  env BLS_WORKSPACE=/Users/bruth/dev/imc/roadrunner \
    nix develop -c cargo test --release \
    index::tests::probe_workspace_file_update -- \
    --ignored --nocapture --test-threads=1
  ```

- Exercise the evaluated graph separately with the checked-in fixture:

  ```sh
  env BLS_WORKSPACE="$PWD/tests/workspace" \
    BLS_PROBE_FILE=lib/BUILD.bazel BLS_PROBE_LABEL=//lib:srcs \
    BLS_LARGE_FILE=lib/BUILD.bazel BLS_LARGE_LABEL=//lib/sub:sub_srcs \
    BLS_BAZEL_PATH=bazel BLS_GRAPH_LABEL=//lib:from_legacy_0 \
    nix develop -c cargo test --release --test workspace_probe -- \
    --ignored --nocapture --test-threads=1
  ```

- Keep comments for invariants a reader cannot derive from the code. Remove
  transition notes and restatements.

## Site

- Order documentation around a Bazel user's questions: supported versions and
  files, installation, configuration, then capabilities. Keep shipped behavior
  separate from the roadmap.
- Keep user outcomes and ordering on the site. Put dependency and architecture
  detail in `ROADMAP.md`.
- Preserve the static, minimalist presentation. Add client JavaScript only for
  an interaction that ordinary links and HTML cannot provide.
- Use `~/dev/vimdoc-language-server/site` as a hierarchy and visual reference,
  not as a reason to copy its documentation depth.
