#!/bin/sh
set -eu

cp -R "$src/." .
PATH=$busyboxPath
export PATH

cat > client-under-test <<EOF
#!/bin/sh
exec busybox sh "$PWD/crates/symbol/static/symbol.sh" "\$@"
EOF
chmod +x client-under-test

CLIENT="$PWD/client-under-test" busybox sh tests/symbol_client.sh
touch "$out"
