#!/bin/sh
# install the symbol client
set -eu
HOST="${SYMBOL_HOST:-__HOST__}"
BIN_DIR="${PREFIX:-${HOME}/.local/bin}"
mkdir -p "$BIN_DIR"
curl -fsSL "${HOST}/symbol.sh" -o "${BIN_DIR}/symbol"
chmod +x "${BIN_DIR}/symbol"
curl -fsSL "${HOST}/symbol.sh/HASH" -o "${BIN_DIR}/.symbol.blake3"
case ":$PATH:" in
  *":${BIN_DIR}:"*) echo "installed ${BIN_DIR}/symbol" ;;
  *) echo "installed ${BIN_DIR}/symbol  (add ${BIN_DIR} to PATH)" ;;
esac
