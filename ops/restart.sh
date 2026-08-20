#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
cargo build --release
sudo systemctl restart symbol
sudo systemctl is-active --quiet symbol
echo "symbol rebuilt and restarted"
