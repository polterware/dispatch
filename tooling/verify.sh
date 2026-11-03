#!/usr/bin/env bash
# Every check of this repository: the contracts, the Go core, the Rust core and app, and
# the OpenCode plugin.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$PATH:$(go env GOPATH 2>/dev/null)/bin:${HOME}/go/bin"
cd "$ROOT"

(cd schemas && buf lint)

go vet ./go-core/...
go test ./go-core/... -count=1
node --test go-core/providers/setup/opencode/

cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace

pnpm --dir plugins/opencode-gui typecheck
pnpm --dir plugins/opencode-gui check
bash tooling/sync-opencode-plugin.sh --check
bash tooling/check-boundaries.sh

echo "verify OK"
