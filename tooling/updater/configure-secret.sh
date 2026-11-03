#!/usr/bin/env bash
# Run only after explicitly authorizing export of the Polterware key to this repository.
set -euo pipefail
UPDATER="$(cd "$(dirname "$0")" && pwd)"
REPO="$(node -p 'require(process.argv[1]).repository' "$UPDATER/config.cjs")"
EXPECTED="$(node -p 'require(process.argv[1]).publicKey' "$UPDATER/config.cjs")"
PUBLIC="$("$UPDATER/vendor/bin/generate_keys" --account polterware -p)"
[[ "$PUBLIC" == "$EXPECTED" ]] || { echo 'Keychain public key does not match app configuration.' >&2; exit 1; }
umask 077
# generate_keys -x will not overwrite a file, so export to a new path inside a private directory.
WORK="$(mktemp -d "${TMPDIR:-/private/tmp}/polterware-sparkle-key.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
PRIVATE="$WORK/key"
"$UPDATER/vendor/bin/generate_keys" --account polterware -x "$PRIVATE" >/dev/null
gh secret set POLTERWARE_SPARKLE_PRIVATE_KEY --repo "$REPO" < "$PRIVATE"
echo 'Configured POLTERWARE_SPARKLE_PRIVATE_KEY in GitHub Actions.'
