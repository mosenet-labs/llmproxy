#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"

# Accept --check for CI; TOPCOAT_BIN can select a separately installed CLI.
"${TOPCOAT_BIN:-topcoat}" fmt --rustfmt "$@" crates
