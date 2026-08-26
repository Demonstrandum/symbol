#!/bin/sh
set -eu

root=$(CDPATH= cd "$(dirname "$0")/.." && pwd)
SYMBOL_NIX_BUILD_COMMIT=$(git -C "$root" rev-parse HEAD)
SYMBOL_NIX_BUILD_DIRTY=false
if ! git -C "$root" diff --quiet --ignore-submodules -- ||
  ! git -C "$root" diff --cached --quiet --ignore-submodules -- ||
  [ -n "$(git -C "$root" ls-files --others --exclude-standard)" ]; then
  SYMBOL_NIX_BUILD_DIRTY=true
fi
export SYMBOL_NIX_BUILD_COMMIT SYMBOL_NIX_BUILD_DIRTY

exec nix flake check --impure --keep-going -L "path:${root}" "$@"
