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

validate-workspace workspace="/Users/bruth/dev/imc/roadrunner":
    BLS_WORKSPACE={{workspace}} cargo test --release --test workspace_probe -- --ignored --nocapture --test-threads=1
    BLS_WORKSPACE={{workspace}} cargo test --release index::tests::probe_workspace_file_update -- --ignored --nocapture --test-threads=1

build:
    cargo build --release

ci: format lint test
    @:

release version *args:
    nix develop .#ci --command ./scripts/release.sh {{version}} {{args}}
