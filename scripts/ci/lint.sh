#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

if ! command -v cargo >/dev/null 2>&1; then
    printf 'CI lint requires Cargo.\n' >&2
    exit 1
fi

cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
python3 scripts/ci/check-licenses.py
