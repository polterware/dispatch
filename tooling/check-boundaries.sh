#!/usr/bin/env bash
# Dispatch stands alone: nothing here imports Polterware's code. It reaches Polterware only
# through DispatchHostService, the contract in schemas/ that Polterware serves, and works on
# folders alone without it. Its Go domains stay free of concrete infrastructure.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
fail=0

bad=$(grep -rn --include='*.go' '"github.com/polterware/polterware' "$ROOT/go-core" || true)
if [[ -n "$bad" ]]; then
  echo "FAIL: go-core imports Polterware:"
  echo "$bad"
  fail=1
fi

bad=$(grep -rn --include='*.toml' 'polterware/polterware' "$ROOT/app" "$ROOT/crates" || true)
if [[ -n "$bad" ]]; then
  echo "FAIL: a Rust crate depends on Polterware:"
  echo "$bad"
  fail=1
fi

bad=$(grep -rnE --include='*.go' 'modernc.org/sqlite|connectrpc.com/connect|"database/sql"' "$ROOT/go-core/domains" || true)
if [[ -n "$bad" ]]; then
  echo "FAIL: go-core domains import concrete infrastructure:"
  echo "$bad"
  fail=1
fi

if [[ $fail -ne 0 ]]; then
  exit 1
fi
echo "OK: Dispatch boundaries"
