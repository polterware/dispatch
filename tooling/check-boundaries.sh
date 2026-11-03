#!/usr/bin/env bash
# Dispatch stands alone: nothing here imports Polterware's code. It reaches Polterware only
# through DispatchHostService, the contract in schemas/ that Polterware serves, and works on
# folders alone without it.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
fail=0

bad=$(grep -rn --include='*.toml' 'polterware/polterware' "$ROOT/app" "$ROOT/crates" || true)
if [[ -n "$bad" ]]; then
  echo "FAIL: a Rust crate depends on Polterware:"
  echo "$bad"
  fail=1
fi

if [[ $fail -ne 0 ]]; then
  exit 1
fi
echo "OK: Dispatch boundaries"
