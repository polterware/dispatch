#!/usr/bin/env bash
# Every check of this repository: the contracts and the Rust core and app.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$PATH:${HOME}/go/bin" # buf, when `go install` put it there
cd "$ROOT"

(cd schemas && buf lint)

cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace

bash tooling/check-boundaries.sh

echo "verify OK"
