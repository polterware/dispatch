#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
BIN_DIR="$HOME/.local/bin"
BIN_PATH="$BIN_DIR/voice-engine"

missing=()
for tool in cargo cmake espeak-ng bun; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing+=("$tool")
  fi
done

if ((${#missing[@]})); then
  printf 'Missing prerequisites: %s\n' "${missing[*]}" >&2
  case "$(uname -s)" in
    Darwin)
      brew_missing=()
      for tool in "${missing[@]}"; do
        case "$tool" in
          cmake|espeak-ng|bun) brew_missing+=("$tool") ;;
        esac
      done
      if ((${#brew_missing[@]})); then
        printf 'Install system tools with: brew install %s\n' "${brew_missing[*]}" >&2
      fi
      if [[ " ${missing[*]} " == *" cargo "* ]]; then
        printf 'Install Rust and Cargo with: https://rustup.rs/\n' >&2
      fi
      if [[ " ${missing[*]} " == *" bun "* ]]; then
        printf 'Install Bun with: https://bun.sh/docs/installation\n' >&2
      fi
      ;;
    Linux)
      apt_missing=()
      for tool in "${missing[@]}"; do
        case "$tool" in
          cmake) apt_missing+=(cmake) ;;
          espeak-ng) apt_missing+=(espeak-ng) ;;
        esac
      done
      if ((${#apt_missing[@]})); then
        printf 'Install system tools with: apt install %s\n' "${apt_missing[*]}" >&2
      fi
      if [[ " ${missing[*]} " == *" cargo "* ]]; then
        printf 'Install Rust and Cargo with: https://rustup.rs/\n' >&2
      fi
      if [[ " ${missing[*]} " == *" bun "* ]]; then
        printf 'Install Bun with: https://bun.sh/docs/installation\n' >&2
      fi
      ;;
    *)
      printf 'Install the missing tools using their official installation instructions.\n' >&2
      ;;
  esac
  exit 1
fi

cd "$REPO_ROOT"
if [[ "$(uname -s)" == "Darwin" ]]; then
  export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.4}"
  os_runtime="$(clang --print-resource-dir)/lib/darwin/libclang_rt.osx.a"
  export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C link-arg=$os_runtime"
fi

cargo build --offline --release -p voice-server --bin voice-engine
mkdir -p "$BIN_DIR"
install_tmp="$BIN_DIR/.voice-engine.$$"
trap 'rm -f "$install_tmp"' EXIT
cp "$REPO_ROOT/target/release/voice-engine" "$install_tmp"
chmod 755 "$install_tmp"
mv -f "$install_tmp" "$BIN_PATH"

if [[ ":${PATH}:" != *":$BIN_DIR:"* ]]; then
  printf 'Warning: %s is not on PATH. Add it to your shell PATH to run voice-engine.\n' "$BIN_DIR" >&2
fi

bun install
bun run --cwd packages/opencode-plugin build --configLoader runner

printf '\nAdd this entry to the "plugins" array in ~/.config/opencode/opencode.json or opencode.jsonc:\n'
printf '  "plugins": ["%s/packages/opencode-plugin"]\n' "$REPO_ROOT"
printf '\nThen download models when ready; this command asks for consent before downloading:\n'
printf '  voice-engine models download\n'
