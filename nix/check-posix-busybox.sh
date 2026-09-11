#!/bin/sh
set -eu

cp -R "$src/." .
PATH=$busyboxPath
export PATH

# A copy with a busybox-sh shebang rather than a wrapper, for the same reason
# as the dash check: the client finds its hash file relative to $0.
sed "1s|.*|#!$(command -v busybox) sh|" static/symbol.sh > client-under-test
chmod +x client-under-test

CLIENT="$PWD/client-under-test" busybox sh tests/symbol_client.sh
touch "$out"
