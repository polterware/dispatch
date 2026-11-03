#!/usr/bin/env bash
set -euo pipefail
UPDATER="$(cd "$(dirname "$0")" && pwd)"
VERSION="$(node -p 'require(process.argv[1]).version' "$UPDATER/config.cjs")"
DIGEST="$(node -p 'require(process.argv[1]).archiveSha256' "$UPDATER/config.cjs")"
mkdir -p "$UPDATER/vendor"
ARCHIVE="$UPDATER/vendor/Sparkle-$VERSION.tar.xz"
if [[ ! -f "$ARCHIVE" ]]; then
  curl --fail --location --retry 3 "https://github.com/sparkle-project/Sparkle/releases/download/$VERSION/Sparkle-$VERSION.tar.xz" -o "$ARCHIVE.tmp"
  mv "$ARCHIVE.tmp" "$ARCHIVE"
fi
printf '%s  %s\n' "$DIGEST" "$ARCHIVE" | shasum -a 256 -c -
# Always extract from the verified archive rather than trusting a previously unpacked copy.
tar -xJf "$ARCHIVE" -C "$UPDATER/vendor"
