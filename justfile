default:
    @just --list

format: rust-format site-format
    @:

lint: rust-lint site-check
    @:

test:
    cargo test --all

build:
    cargo build --release

rust-format:
    cargo fmt --all -- --check

rust-lint:
    cargo clippy --all-targets -- -D warnings

site-install:
    cd site && pnpm install --frozen-lockfile

site-check: site-install
    cd site && pnpm check

site-format: site-install
    cd site && pnpm format:check

site-build: site-install
    cd site && pnpm build

ci: format lint test
    @:
