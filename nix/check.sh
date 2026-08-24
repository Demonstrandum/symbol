#!/bin/sh
set -eu

root=$(CDPATH= cd "$(dirname "$0")/.." && pwd)
exec nix flake check --keep-going -L "path:${root}" "$@"
