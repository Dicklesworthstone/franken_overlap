#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

MODE="${1:-full}"

case "$MODE" in
    quick|full)
        ;;
    *)
        printf 'Usage: %s [quick|full]\n' "$0" >&2
        exit 2
        ;;
esac

if [[ -t 1 ]]; then
    BOLD=$'\033[1m'
    GREEN=$'\033[32m'
    BLUE=$'\033[34m'
    RESET=$'\033[0m'
else
    BOLD=""
    GREEN=""
    BLUE=""
    RESET=""
fi

run() {
    printf '\n%s%s==>%s' "$BOLD" "$BLUE" "$RESET"
    printf ' %q' "$@"
    printf '\n'
    "$@"
}

run rustup toolchain install stable \
    --profile minimal \
    --component rustfmt,clippy

if [[ "$MODE" == "full" ]]; then
    run rustup toolchain install 1.85.0 \
        --profile minimal \
        --component rustfmt,clippy
fi

if [[ ! -f Cargo.lock ]]; then
    run cargo +stable generate-lockfile
fi

run git diff --check
run cargo +stable fmt --all -- --check
run cargo +stable check --workspace --all-targets --locked
run cargo +stable test --workspace --locked

if [[ "$MODE" == "full" ]]; then
    run cargo +stable clippy \
        --workspace \
        --all-targets \
        --locked \
        -- \
        -D warnings

    run cargo +stable check \
        -p fo-cli \
        --features frankenscipy \
        --all-targets \
        --locked

    run cargo +stable test \
        -p fo-core \
        --features frankenscipy \
        --locked

    run cargo +stable clippy \
        -p fo-cli \
        --features frankenscipy \
        --all-targets \
        --locked \
        -- \
        -D warnings

    run cargo +1.85.0 check \
        --workspace \
        --all-targets \
        --locked

    run cargo +1.85.0 test \
        --workspace \
        --locked
fi

printf '\n%s%sLocal CI passed in %s mode.%s\n' \
    "$BOLD" "$GREEN" "$MODE" "$RESET"
