#!/usr/bin/env bash
# Every check of this repository: the contracts, the Rust core and app, and the OpenCode
# plugin.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$PATH:${HOME}/go/bin" # buf, when `go install` put it there
cd "$ROOT"

(cd schemas && buf lint)

# A glob, since Node 24 (CI's) runs a directory given to --test as a module.
node --test 'crates/dispatch-core/src/setup/opencode/*.test.mjs'

cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace

pnpm --dir plugins/opencode-gui typecheck
pnpm --dir plugins/opencode-gui check
bash tooling/check-boundaries.sh

echo "verify OK"
