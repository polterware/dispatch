#!/usr/bin/env bash
# The OpenCode plugin's files live in crates/dispatch-core/src/setup/opencode, which
# dispatch-core embeds. go-core embeds its own copy until dispatch-core replaces it; this
# copies them over, and with --check fails when the copy differs.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/crates/dispatch-core/src/setup/opencode"
dst="$root/go-core/providers/setup/opencode"
files=(gui-experimental.cjs package.json rpc.js server.js)
if [[ "${1:-}" == "--check" ]]; then
  for f in "${files[@]}"; do
    cmp -s "$src/$f" "$dst/$f" || { echo "go-core/providers/setup/opencode/$f is stale: run tooling/sync-opencode-plugin.sh" >&2; exit 1; }
  done
  echo "opencode plugin copies OK"
  exit 0
fi
for f in "${files[@]}"; do cp "$src/$f" "$dst/$f"; done
echo "copied the opencode plugin into go-core"
