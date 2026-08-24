#!/bin/sh
set -eu

ROOT=$(mktemp -d)
PORT=$((20000 + ($$ % 20000)))
BASE="http://127.0.0.1:${PORT}"
SERVER=${SERVER:-target/debug/symbol}
SERVER_PID=

cleanup() {
  if [ -n "${SERVER_PID:-}" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$ROOT"
}
trap cleanup EXIT HUP INT TERM

fail() {
  printf 'not ok %d - %s\n' "$((tests + 1))" "$1"
  printf 'server log:\n' >&2
  awk '{print}' "$ROOT/server.log" >&2
  exit 1
}

ok() {
  tests=$((tests + 1))
  printf 'ok %d - %s\n' "$tests" "$1"
}

contains() {
  printf '%s' "$1" | awk -v wanted="$2" 'index($0,wanted){found=1} END{exit !found}'
}

site_from_put() {
  awk '
    $1 == "ok" { print $2; exit }
    $1 == "created" {
      url = $2
      sub(/\/$/, "", url)
      sub(/^.*\//, "", url)
      print url
      exit
    }
  '
}

tests=0
mkdir -p "$ROOT/server" "$ROOT/bin" "$ROOT/work"

[ -x "$SERVER" ] || cargo build --quiet --locked
SYMBOL_PUBLIC_URL="$BASE" RUST_LOG=warn \
  "$SERVER" --bind "127.0.0.1:${PORT}" --root "$ROOT/server" \
  >"$ROOT/server.log" 2>&1 &
SERVER_PID=$!

ready=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  if curl -fsS "$BASE/STATS" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
[ "$ready" -eq 1 ] || fail "isolated server starts"
ok "isolated server starts"

curl -fsS "$BASE/install.sh" |
  PREFIX="$ROOT/bin" SYMBOL_HOST="$BASE" sh >/dev/null
CLIENT=$ROOT/bin/symbol
[ -x "$CLIENT" ] || fail "client installs"
ok "client installs"
export XDG_STATE_HOME="$ROOT/state"

explicit=$(printf '<h1>explicit</h1>\n' | "$CLIENT" put -)
explicit_name=$(printf '%s\n' "$explicit" | site_from_put)
[ -n "$explicit_name" ] &&
  [ "$(curl -fsS "$BASE/$explicit_name/index.html")" = '<h1>explicit</h1>' ] ||
  fail "put dash publishes stdin as random index"
ok "put dash publishes stdin as random index"

implicit=$(printf '<h1>implicit</h1>\n' | "$CLIENT" put)
implicit_name=$(printf '%s\n' "$implicit" | site_from_put)
[ -n "$implicit_name" ] &&
  [ "$(curl -fsS "$BASE/$implicit_name/index.html")" = '<h1>implicit</h1>' ] ||
  fail "bare piped put publishes random index"
[ -s "$XDG_STATE_HOME/symbol/claims/$explicit_name" ] &&
  [ -s "$XDG_STATE_HOME/symbol/claims/$implicit_name" ] ||
  fail "ordinary creation pre-persists creator claims"
ok "bare piped put publishes random index"

printf '<h1>main</h1>\n' >"$ROOT/work/index.html"
printf 'body{}\n' >"$ROOT/work/style.css"
"$CLIENT" put e2e-main "$ROOT/work/index.html" >/dev/null
"$CLIENT" put e2e-main "$ROOT/work/style.css" >/dev/null
inventory=$(curl -fsS -H 'Accept: application/json' "$BASE/e2e-main/FILES")
contains "$inventory" '"path":"index.html"' &&
  contains "$inventory" '"path":"style.css"' &&
  curl -fsS "$BASE/e2e-main/symbol.toml" |
    awk '$1 == "content_revision" { found=1 } END { exit !found }' ||
  fail "named puts merge and generate manifest"
ok "named puts merge and generate manifest"

"$CLIENT" get e2e-main "$ROOT/work/main.zip" >/dev/null
unzip -t "$ROOT/work/main.zip" >/dev/null
unzip -p "$ROOT/work/main.zip" symbol.toml |
  awk '$1 == "tree_hash" { found=1 } END { exit !found }' ||
  fail "get zip contains canonical manifest"
ok "get zip contains canonical manifest"

(
  cd "$ROOT/work"
  "$CLIENT" clone e2e-main checkout >/dev/null
)
[ -f "$ROOT/work/checkout/symbol.toml" ] || fail "clone creates checkout"
baseline_before=$(awk -F '"' '$1 ~ /^tree_hash/ { print $2 }' "$ROOT/work/checkout/symbol.toml")
printf 'extensionless\n' |
  (cd "$ROOT/work/checkout" && "$CLIENT" put -f data >/dev/null)
[ "$(curl -fsS "$BASE/e2e-main/data")" = extensionless ] ||
  fail "forced extensionless stdin file publishes to manifest target"
baseline_after=$(awk -F '"' '$1 ~ /^tree_hash/ { print $2 }' "$ROOT/work/checkout/symbol.toml")
[ "$baseline_before" != "$baseline_after" ] ||
  fail "explicit successful put refreshes checkout baseline"
(cd "$ROOT/work/checkout" && "$CLIENT" put </dev/null >/dev/null)
ok "forced file and empty-stdin manifest fallback work"

printf 'new\n' >"$ROOT/work/checkout/about.txt"
sync_check=$(cd "$ROOT/work/checkout" && "$CLIENT" sync --check)
contains "$sync_check" '+ about.txt' || fail "sync check reports local addition"
(cd "$ROOT/work/checkout" && "$CLIENT" sync >/dev/null)
[ "$(curl -fsS "$BASE/e2e-main/about.txt")" = new ] ||
  fail "sync conditionally publishes additions"
ok "clone and strict sync publish additions"

"$CLIENT" copy e2e-main e2e-copy >/dev/null
"$CLIENT" move e2e-copy e2e-moved >/dev/null
curl -fsS "$BASE/e2e-moved/index.html" >/dev/null ||
  fail "copy and move preserve content"
ok "copy and move preserve content"

(
  cd "$ROOT/work"
  "$CLIENT" remix e2e-main e2e-remix >/dev/null
)
[ -f "$ROOT/work/e2e-remix/symbol.toml" ] &&
  curl -fsS "$BASE/e2e-remix/index.html" >/dev/null ||
  fail "remix copies server site and clones locally"
ok "remix copies server site and clones locally"

mkdir "$ROOT/work/existing-remix"
printf occupied >"$ROOT/work/existing-remix/file"
if (cd "$ROOT/work" && "$CLIENT" remix e2e-main existing-remix >/dev/null 2>&1); then
  fail "remix refuses occupied local destination before server copy"
fi
code=$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/existing-remix/")
[ "$code" = 404 ] || fail "failed remix does not leave server copy"
ok "remix validates local destination before copy"

(
  cd "$ROOT/work"
  "$CLIENT" remix --managed e2e-main e2e-managed-remix >/dev/null
)
[ -s "$ROOT/work/e2e-managed-remix/.symbol-token" ] &&
  [ -s "$ROOT/work/e2e-managed-remix/.symbol-claim" ] ||
  fail "managed remix attaches recovery credentials to clone"
printf 'managed remix update\n' >"$ROOT/work/e2e-managed-remix/remix.txt"
(cd "$ROOT/work/e2e-managed-remix" && "$CLIENT" put >/dev/null)
curl -fsS "$BASE/e2e-managed-remix/remix.txt" >/dev/null ||
  fail "managed remix checkout can publish with attached token"
ok "managed remix attaches token and claim to checkout"

printf 'changed\n' >"$ROOT/work/change.txt"
"$CLIENT" put e2e-main "$ROOT/work/change.txt" >/dev/null
stack=$("$CLIENT" undo --stack e2e-main)
contains "$stack" 'restore previous state of e2e-main' ||
  fail "undo stack reports mutation"
"$CLIENT" undo e2e-main >/dev/null
code=$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/e2e-main/change.txt")
[ "$code" = 404 ] || fail "undo restores previous state"
"$CLIENT" rm e2e-main style.css >/dev/null
code=$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/e2e-main/style.css")
[ "$code" = 404 ] || fail "file deletion applies"
"$CLIENT" undo e2e-main >/dev/null
curl -fsS "$BASE/e2e-main/style.css" >/dev/null ||
  fail "file deletion undo restores path"
ok "undo stack, mutation restore, and file-delete restore work"

"$CLIENT" expire e2e-main >/dev/null
expiry=$("$CLIENT" expire e2e-main --show)
contains "$expiry" 'effective expiry:' &&
  contains "$expiry" 'retention by size' ||
  fail "expiry policy and report work"
"$CLIENT" expire e2e-main --never >/dev/null
ok "expiry policy and report work"

mkdir "$ROOT/work/managed"
cat >"$ROOT/work/managed/symbol.toml" <<EOF
version = 1
host = "$BASE"
name = "e2e-secure"
EOF
printf '<h1>secure</h1>\n' >"$ROOT/work/managed/index.html"
(cd "$ROOT/work/managed" && "$CLIENT" put --managed >/dev/null)
[ -s "$ROOT/work/managed/.symbol-token" ] ||
  fail "managed creation saves token sidecar"
token=$(awk '{print; exit}' "$ROOT/work/managed/.symbol-token")

unauthorized=$(printf nope |
  SYMBOL_TOKEN= "$CLIENT" put e2e-secure blocked.txt 2>&1 || true)
contains "$unauthorized" 'management token required' ||
  fail "managed site rejects unauthorized mutation"

printf 'token=%s\n' "$token" |
  SYMBOL_TOKEN="$token" "$CLIENT" put e2e-secure secret.txt >/dev/null
stored=$(curl -fsS "$BASE/e2e-secure/secret.txt")
contains "$stored" 'sym_mgmt_' &&
  ! contains "$stored" "${token#sym_mgmt_}" ||
  fail "managed upload sanitizes token payload"
ok "management authorization and sanitization work"

"$CLIENT" expire e2e-secure --show >/dev/null
(cd "$ROOT/work/managed" && "$CLIENT" manage e2e-secure --rotate >/dev/null)
rotated=$(awk '{print; exit}' "$ROOT/work/managed/.symbol-token")
[ "$rotated" != "$token" ] || fail "management token rotates"
(cd "$ROOT/work/managed" && "$CLIENT" manage e2e-secure --release >/dev/null)
ok "management rotate and release work"

"$CLIENT" pop e2e-moved "$ROOT/work/moved.tar" >/dev/null
tar -tf "$ROOT/work/moved.tar" | awk '$0 == "symbol.toml" { found=1 } END { exit !found }' ||
  fail "pop archive includes manifest"
code=$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/e2e-moved/")
[ "$code" = 404 ] || fail "pop removes site"
"$CLIENT" undo e2e-moved >/dev/null
curl -fsS "$BASE/e2e-moved/index.html" >/dev/null ||
  fail "site pop undo restores site"
ok "pop archives, removes, and undo restores site"

"$CLIENT" undo "$explicit_name" >/dev/null
code=$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/$explicit_name/")
[ "$code" = 404 ] || fail "create undo removes newly created site"
ok "create undo removes newly created site"

"$CLIENT" copy e2e-main e2e-stream >/dev/null
"$CLIENT" pop e2e-stream - >"$ROOT/work/stream.tar.gz" 2>"$ROOT/work/stream.err"
tar -tzf "$ROOT/work/stream.tar.gz" |
  awk '$0 == "symbol.toml" { found=1 } END { exit !found }' ||
  fail "pop dash keeps stdout archive binary-only"
contains "$(cat "$ROOT/work/stream.err")" 'undo:' ||
  fail "pop dash routes undo hint to stderr"
ok "pop dash keeps binary stdout clean"

"$CLIENT" rm e2e-remix >/dev/null
"$ROOT/bin/symbol" -t "$(cat "$ROOT/work/e2e-managed-remix/.symbol-token")" \
  rm e2e-managed-remix >/dev/null
"$CLIENT" rm e2e-main >/dev/null
"$CLIENT" rm e2e-moved >/dev/null
"$CLIENT" rm e2e-secure >/dev/null
"$CLIENT" rm "$implicit_name" >/dev/null

left=$(curl -fsS -H 'Accept: application/json' "$BASE/FILES")
for name in e2e-main e2e-moved e2e-remix e2e-secure "$explicit_name" "$implicit_name"; do
  contains "$left" "\"name\":\"$name\"" && fail "cleanup removes all test sites"
done
ok "cleanup removes all test sites"

printf '1..%d\n' "$tests"
