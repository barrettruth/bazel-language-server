default:
    @just --list

format:
    cargo fmt --all -- --check
    cd site && pnpm install --frozen-lockfile && pnpm format:check

lint:
    cargo clippy --all-targets -- -D warnings
    cd site && pnpm install --frozen-lockfile && pnpm check

test:
    cargo test --all

build:
    cargo build --release

ci: format lint test
    @:

release version *args:
    nix develop .#ci --command ./scripts/release.sh {{version}} {{args}}
