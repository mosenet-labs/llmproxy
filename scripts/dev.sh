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
    console)
        exec cargo run -p llmproxy-console
        ;;
    gateway)
        exec cargo run -p llmproxy-gateway
        ;;
    up)
        cargo build --workspace
        binary_dir="${CARGO_TARGET_DIR:-target}/debug"
        "$binary_dir/llmproxy-db" migrate
        gateway_pid=''
        console_pid=''
        cleanup() {
            if [[ -n "$gateway_pid" ]]; then
                kill "$gateway_pid" 2>/dev/null || true
                wait "$gateway_pid" 2>/dev/null || true
            fi
            if [[ -n "$console_pid" ]]; then
                kill "$console_pid" 2>/dev/null || true
                wait "$console_pid" 2>/dev/null || true
            fi
        }
        trap cleanup EXIT
        trap 'exit 130' INT
        trap 'exit 143' TERM
        "$binary_dir/llmproxy-gateway" &
        gateway_pid=$!
        "$binary_dir/llmproxy-console" &
        console_pid=$!
        while kill -0 "$gateway_pid" 2>/dev/null && kill -0 "$console_pid" 2>/dev/null; do
            sleep 1
        done
        echo 'A development service exited; stopping the other service.' >&2
        exit 1
        ;;
    *)
        echo 'Usage: bash scripts/dev.sh [up|migrate|console|gateway]' >&2
        exit 2
        ;;
esac
