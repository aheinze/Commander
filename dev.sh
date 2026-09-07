#!/usr/bin/env bash
set -euo pipefail

readonly project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
profile="debug"

usage() {
    printf '%s\n' \
        "Usage: ./dev.sh [--debug|--release] [-- COMMANDER_ARGS...]" \
        "" \
        "Build and launch Commander from the current working tree." \
        "" \
        "Options:" \
        "  --debug      Use the fast development profile (default)" \
        "  --release    Use the optimized release profile" \
        "  -h, --help   Show this help" \
        "" \
        "Examples:" \
        "  ./dev.sh" \
        "  ./dev.sh --release" \
        "  ./dev.sh -- --left /tmp --right \"$HOME/Downloads\""
}

case "${1:-}" in
    --debug)
        shift
        ;;
    --release)
        profile="release"
        shift
        ;;
    -h|--help)
        usage
        exit 0
        ;;
esac

if [[ "${1:-}" == "--" ]]; then
    shift
fi

cd -- "$project_dir"

cargo_args=(run --package dualpane-app)
if [[ "$profile" == "release" ]]; then
    cargo_args+=(--release)
fi

exec cargo "${cargo_args[@]}" -- "$@"
