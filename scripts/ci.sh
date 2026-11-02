#!/usr/bin/env bash
# CI locale (Linux / macOS / Git Bash) : les mêmes vérifications que GitHub.
#
#   scripts/ci.sh          vérifications rapides : format, clippy, tests
#   scripts/ci.sh --full   + documentation, dépendances (cargo-deny), couverture
#
# GitHub reste la vérification finale (Windows, Linux et macOS, MSRV).
set -euo pipefail
cd "$(dirname "$0")/.."
start=$SECONDS

step() {
    local name=$1; shift
    printf '\033[36m==> %s\033[0m\n' "$name"
    local t=$SECONDS
    "$@" || { printf '\033[31mÉCHEC : %s\033[0m\n' "$name"; exit 1; }
    printf '\033[32m    ok (%d s)\033[0m\n' $((SECONDS - t))
}

has() { cargo "$1" --version >/dev/null 2>&1; }

step "Format (rustfmt)" cargo fmt --all --check
step "Analyse statique (clippy)" cargo clippy --all-targets --locked -- -D warnings
step "Tests" cargo test --locked --all-targets

if [[ "${1:-}" == "--full" ]]; then
    step "Documentation" env RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
    if has deny; then
        step "Dépendances (cargo-deny)" cargo deny check
    else
        echo "    cargo-deny absent, étape ignorée (cargo install --locked cargo-deny)"
    fi
    if has llvm-cov; then
        step "Couverture (seuil 85 %)" cargo llvm-cov --locked --all-targets --summary-only \
            --ignore-filename-regex 'src/main\.rs' --fail-under-lines 85
    else
        echo "    cargo-llvm-cov absent, étape ignorée (cargo install --locked cargo-llvm-cov)"
    fi
fi

printf '\033[32mTout est vert en %d s.\033[0m\n' $((SECONDS - start))
