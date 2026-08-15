#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

MODE="${1:-full}"
TOOLCHAIN="${FO_RUST_TOOLCHAIN:-nightly}"
UPDATE_NIGHTLY="${FO_UPDATE_NIGHTLY:-0}"
USE_RCH="${FO_USE_RCH:-0}"

case "$MODE" in
    quick|full) ;;
    *)
        printf 'Usage: %s [quick|full]\n' "$0" >&2
        exit 2
        ;;
esac

if [[ -t 1 ]]; then
    BOLD=$'\033[1m'
    BLUE=$'\033[34m'
    GREEN=$'\033[32m'
    RED=$'\033[31m'
    RESET=$'\033[0m'
else
    BOLD=""
    BLUE=""
    GREEN=""
    RED=""
    RESET=""
fi

run() {
    printf '\n%s%s==>%s' "$BOLD" "$BLUE" "$RESET"
    printf ' %q' "$@"
    printf '\n'
    "$@"
}

fail() {
    printf '\n%s%sERROR:%s %s\n' "$BOLD" "$RED" "$RESET" "$*" >&2
    exit 1
}

command -v git >/dev/null 2>&1 || fail "git is not installed"
command -v rustup >/dev/null 2>&1 || fail "rustup is not installed"

if [[ "$UPDATE_NIGHTLY" == "1" ]]; then
    run rustup update "$TOOLCHAIN" --no-self-update
    run rustup component add rustfmt clippy --toolchain "$TOOLCHAIN"
fi

if ! rustup run "$TOOLCHAIN" rustc -V >/dev/null 2>&1; then
    fail "Rust toolchain '$TOOLCHAIN' is unavailable. Install it with: rustup toolchain install $TOOLCHAIN --profile minimal --component rustfmt,clippy"
fi
if ! rustup run "$TOOLCHAIN" cargo fmt --version >/dev/null 2>&1; then
    fail "rustfmt is unavailable for '$TOOLCHAIN'. Run: rustup component add rustfmt --toolchain $TOOLCHAIN"
fi
if [[ "$MODE" == "full" ]] && ! rustup run "$TOOLCHAIN" cargo clippy --version >/dev/null 2>&1; then
    fail "Clippy is unavailable for '$TOOLCHAIN'. Run: rustup component add clippy --toolchain $TOOLCHAIN"
fi

CARGO=(rustup run "$TOOLCHAIN" cargo)
if [[ "$USE_RCH" == "1" ]]; then
    command -v rch >/dev/null 2>&1 || fail "FO_USE_RCH=1 but rch is not installed"
    CARGO=(rch exec -- rustup run "$TOOLCHAIN" cargo)
fi

HAD_LOCKFILE=0
LOCK_ARGS=()
if [[ -f Cargo.lock ]]; then
    HAD_LOCKFILE=1
    LOCK_ARGS=(--locked)
fi

export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"

run rustup run "$TOOLCHAIN" rustc -Vv
run rustup run "$TOOLCHAIN" cargo -V
run git diff --check
run git diff --cached --check
run rustup run "$TOOLCHAIN" cargo fmt --all -- --check
run "${CARGO[@]}" check --workspace --all-targets "${LOCK_ARGS[@]}"
run "${CARGO[@]}" test --workspace "${LOCK_ARGS[@]}"

if [[ "$MODE" == "full" ]]; then
    run "${CARGO[@]}" clippy --workspace --all-targets "${LOCK_ARGS[@]}" -- -D warnings
    run "${CARGO[@]}" check -p fo-cli --features frankenscipy --all-targets "${LOCK_ARGS[@]}"
    run "${CARGO[@]}" test -p fo-core --features frankenscipy "${LOCK_ARGS[@]}"
    run "${CARGO[@]}" clippy -p fo-core --features frankenscipy --all-targets "${LOCK_ARGS[@]}" -- -D warnings
    run "${CARGO[@]}" clippy -p fo-cli --features frankenscipy --all-targets "${LOCK_ARGS[@]}" -- -D warnings
fi

if [[ "$HAD_LOCKFILE" == "0" && -f Cargo.lock ]]; then
    printf '\n%s%sNOTE:%s Cargo created Cargo.lock during validation; commit it for reproducible application builds.\n' "$BOLD" "$BLUE" "$RESET"
fi

printf '\n%s%sLocal CI passed in %s mode using %s.%s\n' "$BOLD" "$GREEN" "$MODE" "$TOOLCHAIN" "$RESET"
