#!/usr/bin/env bash
set -euo pipefail

SOURCE="${1:-/Applications/ParsecPlus.app}"
DEST="${2:-/Applications/ParsecPlus-LocalTest.app}"

if [[ ! -d "$SOURCE" ]]; then
  echo "ParsecPlus.app not found at $SOURCE" >&2
  echo "Install the DMG first, or pass another .app path." >&2
  exit 1
fi

if [[ -d "$DEST" ]]; then
  echo "Removing old test copy at $DEST"
  rm -rf "$DEST"
fi

echo "Copying $SOURCE -> $DEST"
ditto "$SOURCE" "$DEST"

echo "Opening a second ParsecPlus instance. In the original app:"
echo "  1. Host -> Remote access ON, copy the direct connection."
echo "  2. In this copy: Computers -> Direct connect -> Invite code -> paste -> Send request."
echo "  3. Approve the request in the original app."
echo
open -n "$DEST"
