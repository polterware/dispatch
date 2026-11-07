#!/usr/bin/env bash
set -euo pipefail

BIN_PATH="$HOME/.local/bin/voice-engine"
case "$(uname -s)" in
  Darwin)
    MODELS_DIR="$HOME/Library/Application Support/voice-engine/models"
    ;;
  Linux)
    DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
    MODELS_DIR="$DATA_HOME/voice-engine/models"
    ;;
  *)
    MODELS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/voice-engine/models"
    ;;
esac

if [[ -e "$BIN_PATH" ]]; then
  rm -f "$BIN_PATH"
  printf 'Removed %s\n' "$BIN_PATH"
else
  printf 'No engine binary found at %s\n' "$BIN_PATH"
fi

printf '\nRemove the plugin path from the "plugins" array in ~/.config/opencode/opencode.json or opencode.jsonc:\n'
printf '  packages/opencode-plugin (the absolute path printed by scripts/install.sh)\n'

if [[ -d "$MODELS_DIR" ]]; then
  printf '\nDelete downloaded models at %s? This cannot be undone. [y/N] ' "$MODELS_DIR"
  answer=""
  read -r answer || true
  if [[ "$answer" == "y" || "$answer" == "Y" ]]; then
    rm -rf "$MODELS_DIR"
    printf 'Removed downloaded models.\n'
  else
    printf 'Kept downloaded models.\n'
  fi
else
  printf '\nNo model directory found at %s\n' "$MODELS_DIR"
fi
