#!/bin/sh
set -eu

cp -R "$src/." .

cat > client-under-test <<EOF
#!/bin/sh
exec dash "$PWD/crates/symbol/static/symbol.sh" "\$@"
EOF
chmod +x client-under-test

CLIENT="$PWD/client-under-test" dash tests/symbol_client.sh
touch "$out"
