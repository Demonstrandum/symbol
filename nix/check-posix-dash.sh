#!/bin/sh
set -eu

cp -R "$src/." .

# A copy of the client with a dash shebang, not a wrapper that execs it: the
# client resolves its own hash file from $0, and the test suite copies the
# client and writes .symbol.blake3 beside that copy. A wrapper would leave $0
# pointing at the original script, where no hash file exists.
sed "1s|.*|#!$(command -v dash)|" static/symbol.sh > client-under-test
chmod +x client-under-test

CLIENT="$PWD/client-under-test" dash tests/symbol_client.sh
touch "$out"
