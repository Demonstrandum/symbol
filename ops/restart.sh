#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
cargo build --release
sudo install -m 0644 ops/symbol.service /etc/systemd/system/symbol.service
sudo systemctl daemon-reload
sudo systemctl restart symbol
sudo systemctl is-active --quiet symbol
echo "symbol rebuilt and restarted"
