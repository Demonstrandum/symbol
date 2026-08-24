#!/bin/sh
set -eu

cp -R "$src/." .

failed=0

for script in \
  ops/restart.sh \
  static/install.sh \
  static/symbol.sh \
  tests/lifecycle_e2e.sh \
  tests/symbol_client.sh
do
  shellcheck -s sh -o require-variable-braces -e SC2015,SC2016 "$script" || failed=1
  checkbashisms -fpx "$script" || failed=1
  dash -n "$script" || failed=1
done

[ "$failed" -eq 0 ] || exit 1
touch "$out"
