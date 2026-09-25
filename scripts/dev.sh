#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"
if [[ -f .env ]]; then
    set -a
    source .env
    set +a
fi

action="${1:-up}"
case "$action" in
    migrate)
        exec cargo run -p llmproxy-store --bin llmproxy-db -- migrate
        ;;
    up)
        cargo build -p llmproxy-gateway -p llmproxy-store
        binary_dir="${CARGO_TARGET_DIR:-target}/debug"
        exec "$binary_dir/llmproxy"
        ;;
    *)
        echo 'Usage: bash scripts/dev.sh [up|migrate]' >&2
        exit 2
        ;;
esac
