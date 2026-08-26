default:
    @just --list

format: rust-format
    @:

lint: rust-lint shell-lint flake-check
    @:

test:
    cargo test --all

build:
    cargo build --release

rust-format:
    cargo fmt --all -- --check

rust-lint:
    cargo clippy --all-targets -- -D warnings

shell-lint:
    #!/usr/bin/env bash
    set -euo pipefail
    if compgen -G 'scripts/*.sh' >/dev/null; then
      shfmt -i 2 -d scripts
      shellcheck scripts/*.sh
    fi

# Skipped where nix is absent, so `just ci` is runnable in a plain container.
flake-check:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v nix >/dev/null 2>&1; then
      nix flake check --no-build
    else
      echo "flake-check: nix not on PATH, skipping"
    fi

# Index a workspace without an editor.
index path=".":
    cargo run --release --bin bazel-language-server -- index {{path}}

# Report whether the Bazel subsystem can run.
doctor path=".":
    cargo run --release --bin bazel-language-server -- doctor {{path}}

ci: format lint test
    @:
