#!/bin/sh
set -eu

ROOT=$(mktemp -d)
PORT=$((20000 + ($$ % 20000)))
BASE="http://127.0.0.1:${PORT}"
SERVER=${SERVER:-target/debug/symbol}
CLIENT_SOURCE=${CLIENT:-$(pwd)/static/symbol.sh}
CLIENT_SOURCE=$(CDPATH='' cd "$(dirname "${CLIENT_SOURCE}")" && pwd)/$(basename "${CLIENT_SOURCE}")
SERVER_PID=

cleanup() {
  if [ -n "${SERVER_PID:-}" ]; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  rm -rf "${ROOT}"
}
trap cleanup EXIT HUP INT TERM

fail() {
  printf 'not ok %d - %s\n' "$((tests + 1))" "$1"
  printf 'server log:\n' >&2
  awk '{print}' "${ROOT}/server.log" >&2
  exit 1
}

ok() {
  tests=$((tests + 1))
  printf 'ok %d - %s\n' "${tests}" "$1"
}

contains() {
  printf '%s' "$1" | awk -v wanted="$2" 'index($0,wanted){found=1} END{exit !found}'
}

with_tty() {
  python3 -c '
import os
import pty
import subprocess
import sys

cmd = sys.argv[1:]
master, slave = pty.openpty()
try:
    proc = subprocess.Popen(
        cmd,
        stdin=sys.stdin,
        stdout=subprocess.PIPE,
        stderr=slave,
    )
finally:
    os.close(slave)
out, _ = proc.communicate()
os.close(master)
sys.stdout.buffer.write(out)
raise SystemExit(proc.returncode)
' "$@"
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

revision() {
  awk '
    match($0, /"content_revision":[0-9]+/) {
      value = substr($0, RSTART, RLENGTH)
      sub(/^.*:/, "", value)
      print value
      exit
    }
  '
}

tests=0
mkdir -p "${ROOT}/server" "${ROOT}/bin" "${ROOT}/work"

[ -x "${SERVER}" ] || cargo build --quiet --locked
SYMBOL_PUBLIC_URL="${BASE}" RUST_LOG=warn \
  "${SERVER}" --bind "127.0.0.1:${PORT}" --root "${ROOT}/server" \
  >"${ROOT}/server.log" 2>&1 &
SERVER_PID=$!

ready=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  if curl -fsSL "${BASE}/STATS" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
[ "${ready}" -eq 1 ] || fail "isolated server starts"
ok "isolated server starts"

curl -fsSL "${BASE}/install.sh" |
  PREFIX="${ROOT}/bin" SYMBOL_HOST="${BASE}" sh >/dev/null
cp "${CLIENT_SOURCE}" "${ROOT}/bin/symbol"
CLIENT=${ROOT}/bin/symbol
[ -x "${CLIENT}" ] || fail "client installs"
ok "client installs"
export SYMBOL_HOST="${BASE}" XDG_STATE_HOME="${ROOT}/state"

REAL_CURL=$(command -v curl)
DROP_LOG=${ROOT}/drop.log
export REAL_CURL DROP_LOG
mkdir "${ROOT}/drop-bin" "${ROOT}/drop-state"
cat >"${ROOT}/drop-bin/curl" <<'DROP_CURL'
#!/bin/sh
set -eu
method=GET
next_is_method=0
next_is_dump=0
next_is_output=0
dump=
output=
for argument do
  if [ "$next_is_method" -eq 1 ]; then
    method=$argument
    next_is_method=0
  elif [ "$next_is_dump" -eq 1 ]; then
    dump=$argument
    next_is_dump=0
  elif [ "$next_is_output" -eq 1 ]; then
    output=$argument
    next_is_output=0
  elif [ "$argument" = -X ]; then
    next_is_method=1
  elif [ "$argument" = -D ]; then
    next_is_dump=1
  elif [ "$argument" = -o ]; then
    next_is_output=1
  fi
done
{
  printf 'METHOD=%s\n' "$method"
  for argument do
    printf 'ARG=%s\n' "$argument"
  done
} >>"${DROP_LOG}"
if [ "${DROP_ALWAYS_METHOD:-}" = "$method" ]; then
  if ! ls "$XDG_STATE_HOME/symbol/claims"/pending-* >/dev/null 2>&1; then
    : >"${DROP_STATE}/missing-pending-$method"
  fi
  if [ ! -f "${DROP_STATE}/always-$method" ]; then
    "$REAL_CURL" "$@" >/dev/null
    : >"${DROP_STATE}/always-$method"
  fi
  [ -z "$dump" ] || rm -f "$dump"
  [ -z "$output" ] || rm -f "$output"
  exit 52
fi

if [ "${DROP_METHOD:-}" = "$method" ] &&
  [ ! -f "${DROP_STATE}/$method" ]; then
  if ! ls "$XDG_STATE_HOME/symbol/claims"/pending-* >/dev/null 2>&1; then
    : >"${DROP_STATE}/missing-pending-$method"
  fi
  "$REAL_CURL" "$@" >/dev/null
  [ -z "$dump" ] || rm -f "$dump"
  [ -z "$output" ] || rm -f "$output"
  : >"${DROP_STATE}/$method"
  exit 52
fi
exec "$REAL_CURL" "$@"
DROP_CURL
chmod +x "${ROOT}/drop-bin/curl"
PATH="${ROOT}/drop-bin:${PATH}"
export PATH DROP_STATE="${ROOT}/drop-state"

explicit=$(printf '<h1>explicit</h1>\n' | "${CLIENT}" put -)
explicit_name=$(printf '%s\n' "${explicit}" | site_from_put)
[ -n "${explicit_name}" ] &&
  [ "$(curl -fsSL "${BASE}/${explicit_name}/index.html")" = '<h1>explicit</h1>' ] ||
  fail "put dash publishes stdin as random index"
ok "put dash publishes stdin as random index"

implicit=$(printf '<h1>implicit</h1>\n' | with_tty "${CLIENT}" put)
implicit_name=$(printf '%s\n' "${implicit}" | site_from_put)
[ -n "${implicit_name}" ] &&
  [ "$(curl -fsSL "${BASE}/${implicit_name}/index.html")" = '<h1>implicit</h1>' ] ||
  fail "bare piped put publishes random index in a terminal"
[ -s "${XDG_STATE_HOME}/symbol/claims/${explicit_name}" ] &&
  [ -s "${XDG_STATE_HOME}/symbol/claims/${implicit_name}" ] ||
  fail "ordinary creation pre-persists creator claims"
ok "bare piped put publishes random index in a terminal"

rm -f "${DROP_STATE}/PUT" "${DROP_STATE}/missing-pending-PUT"
dropped_put=$(printf '<h1>dropped response</h1>\n' |
  DROP_METHOD=PUT "${CLIENT}" put -)
dropped_put_name=$(printf '%s\n' "${dropped_put}" | site_from_put)
[ -n "${dropped_put_name}" ] &&
  [ -s "${XDG_STATE_HOME}/symbol/claims/${dropped_put_name}" ] &&
  [ ! -f "${DROP_STATE}/missing-pending-PUT" ] &&
  [ "$(curl -fsSL "${BASE}/${dropped_put_name}/index.html")" = '<h1>dropped response</h1>' ] ||
  fail "dropped PUT response recovers committed site and claim"
ok "dropped PUT response recovers committed site and claim"

rm -f "${DROP_STATE}/always-PUT" "${DROP_STATE}/missing-pending-PUT"
if printf '<h1>process restart</h1>\n' |
  DROP_ALWAYS_METHOD=PUT "${CLIENT}" put - >/dev/null 2>&1; then
  fail "repeated response loss should leave PUT pending"
fi
pending=$(find "${XDG_STATE_HOME}/symbol/claims" -type d -name 'pending-*' | awk 'NR==1{print}')
[ -n "${pending}" ] && [ -s "${pending}/record" ] && [ -s "${pending}/body" ] &&
  grep -q '^method=PUT$' "${pending}/record" ||
  fail "PUT pending record survives process loss with replay body"
recovery=$("${CLIENT}" recover)
restarted_put_name=$(printf '%s\n' "${recovery}" |
  awk '$1 == "recovered" { url=$2; sub(/\/$/, "", url); sub(/^.*\//, "", url); print url; exit }')
[ -n "${restarted_put_name}" ] &&
  [ -s "${XDG_STATE_HOME}/symbol/claims/${restarted_put_name}" ] &&
  [ "$(curl -fsSL "${BASE}/${restarted_put_name}/index.html")" = '<h1>process restart</h1>' ] ||
  fail "new client process recovers committed PUT after repeated loss"
ok "new client process recovers committed PUT after repeated loss"

printf '<h1>named drop</h1>\n' >"${ROOT}/work/named-drop.html"
rm -f "${DROP_STATE}/PUT" "${DROP_STATE}/missing-pending-PUT"
: >"${DROP_LOG}"
DROP_METHOD=PUT "${CLIENT}" put e2e-named-drop \
  "${ROOT}/work/named-drop.html" >"${ROOT}/named-drop.out" \
  2>"${ROOT}/named-drop.err"
named_drop_puts=$(awk '$0=="METHOD=PUT"{n++} END{print n+0}' "${DROP_LOG}")
[ "${named_drop_puts}" -eq 1 ] &&
  ! awk 'index($0, "Idempotency-Key:"){found=1} END{exit !found}' \
    "${DROP_LOG}" &&
  [ ! -e "${XDG_STATE_HOME}/symbol/claims/e2e-named-drop" ] &&
  [ "$(curl -fsSL "${BASE}/e2e-named-drop/index.html")" = '<h1>named drop</h1>' ] &&
  contains "$(cat "${ROOT}/named-drop.err")" 'one-time creator claim were lost' ||
  fail "named file creation reports irrecoverable dropped claim"
ok "named file creation reports irrecoverable dropped claim"

printf '<h1>main</h1>\n' >"${ROOT}/work/index.html"
printf 'body{}\n' >"${ROOT}/work/style.css"
"${CLIENT}" put e2e-main "${ROOT}/work/index.html" >/dev/null
"${CLIENT}" put e2e-main "${ROOT}/work/style.css" >/dev/null
inventory=$(curl -fsSL -H 'Accept: application/json' "${BASE}/e2e-main/FILES")
contains "${inventory}" '"path":"index.html"' &&
  contains "${inventory}" '"path":"style.css"' &&
  curl -fsSL "${BASE}/e2e-main/symbol.toml" |
    awk '$1 == "content_revision" { found=1 } END { exit !found }' ||
  fail "named puts merge and generate manifest"
ok "named puts merge and generate manifest"

printf 'committed once\n' >"${ROOT}/work/drop-file.txt"
before_file_drop=$(curl -fsSL -H 'Accept: application/json' \
  "${BASE}/e2e-main/FILES" | revision)
rm -f "${DROP_STATE}/PUT"
: >"${DROP_LOG}"
file_drop_result=$(DROP_METHOD=PUT "${CLIENT}" put e2e-main \
  "${ROOT}/work/drop-file.txt" drop-file.txt)
after_file_drop=$(curl -fsSL -H 'Accept: application/json' \
  "${BASE}/e2e-main/FILES" | revision)
file_drop_puts=$(awk '$0=="METHOD=PUT"{n++} END{print n+0}' "${DROP_LOG}")
[ "${file_drop_puts}" -eq 1 ] &&
  ! awk 'index($0, "Idempotency-Key:"){found=1} END{exit !found}' \
    "${DROP_LOG}" &&
  [ "${after_file_drop}" -eq "$((before_file_drop + 1))" ] &&
  [ "$(curl -fsSL "${BASE}/e2e-main/drop-file.txt")" = 'committed once' ] &&
  contains "${file_drop_result}" \
    "verified committed update ${BASE}/e2e-main/drop-file.txt after response loss" ||
  fail "dropped file PUT verifies one commit without unsupported replay"
ok "dropped file PUT verifies one commit without unsupported replay"

printf '<h1>api example</h1>\n' >"${ROOT}/work/api.html"
"${CLIENT}" put hello "${ROOT}/work/api.html" >/dev/null
python3 tests/api_examples.py "${BASE}" ||
  fail "documented raw HTTP and curl examples execute"
ok "documented raw HTTP and curl examples execute"

"${CLIENT}" get e2e-main "${ROOT}/work/main.zip" >/dev/null
unzip -t "${ROOT}/work/main.zip" >/dev/null
unzip -p "${ROOT}/work/main.zip" symbol.toml |
  awk '$1 == "tree_hash" { found=1 } END { exit !found }' ||
  fail "get zip contains canonical manifest"
ok "get zip contains canonical manifest"

(
  cd "${ROOT}/work"
  "${CLIENT}" clone e2e-main checkout >/dev/null
)
[ -f "${ROOT}/work/checkout/symbol.toml" ] || fail "clone creates checkout"
baseline_before=$(awk -F '"' '$1 ~ /^tree_hash/ { print $2 }' "${ROOT}/work/checkout/symbol.toml")
printf 'extensionless\n' |
  (cd "${ROOT}/work/checkout" && "${CLIENT}" put -f data - >/dev/null)
[ "$(curl -fsSL "${BASE}/e2e-main/data")" = extensionless ] ||
  fail "forced extensionless stdin file publishes to manifest target"
baseline_after=$(awk -F '"' '$1 ~ /^tree_hash/ { print $2 }' "${ROOT}/work/checkout/symbol.toml")
[ "${baseline_before}" != "${baseline_after}" ] ||
  fail "explicit successful put refreshes checkout baseline"
(cd "${ROOT}/work/checkout" && "${CLIENT}" put </dev/null >/dev/null)
ok "forced file and empty-stdin manifest fallback work"

printf 'new\n' >"${ROOT}/work/checkout/about.txt"
sync_check=$(cd "${ROOT}/work/checkout" && "${CLIENT}" sync --check)
contains "${sync_check}" '+ about.txt' || fail "sync check reports local addition"
(cd "${ROOT}/work/checkout" && "${CLIENT}" sync >/dev/null)
[ "$(curl -fsSL "${BASE}/e2e-main/about.txt")" = new ] ||
  fail "sync conditionally publishes additions"
ok "clone and strict sync publish additions"

printf 'sync once\n' >"${ROOT}/work/checkout/drop-sync.txt"
before_sync_drop=$(curl -fsSL -H 'Accept: application/json' \
  "${BASE}/e2e-main/FILES" | revision)
rm -f "${DROP_STATE}/PUT"
: >"${DROP_LOG}"
sync_drop_result=$(cd "${ROOT}/work/checkout" &&
  DROP_METHOD=PUT "${CLIENT}" sync 2>"${ROOT}/sync-drop.err")
after_sync_drop=$(curl -fsSL -H 'Accept: application/json' \
  "${BASE}/e2e-main/FILES" | revision)
sync_drop_puts=$(awk '$0=="METHOD=PUT"{n++} END{print n+0}' "${DROP_LOG}")
sync_drop_keys=$(awk -F 'Idempotency-Key: ' \
  'index($0, "Idempotency-Key:"){print $2}' "${DROP_LOG}" |
  LC_ALL=C sort -u | awk 'END{print NR+0}')
sync_drop_matches=$(awk 'index($0, "ARG=If-Match:"){n++} END{print n+0}' \
  "${DROP_LOG}")
[ "${sync_drop_puts}" -eq 2 ] &&
  [ "${sync_drop_keys}" -eq 1 ] &&
  [ "${sync_drop_matches}" -eq 2 ] &&
  [ "${after_sync_drop}" -eq "$((before_sync_drop + 1))" ] &&
  [ "$(curl -fsSL "${BASE}/e2e-main/drop-sync.txt")" = 'sync once' ] &&
  contains "$(cat "${ROOT}/sync-drop.err")" \
    'sync response was lost; remote tree verified' &&
  ! contains "$(cat "${ROOT}/sync-drop.err")" \
    'upstream changed; nothing was written' &&
  contains "${sync_drop_result}" "synced ${BASE}/e2e-main/" ||
  fail "dropped sync replays one conditional site PUT"
ok "dropped sync replays one conditional site PUT"

mkdir -p "${ROOT}/work/rooted-source/assets"
printf 'before\n' >"${ROOT}/work/rooted-source/assets/file.txt"
"${CLIENT}" put e2e-rooted "${ROOT}/work/rooted-source/assets/file.txt" \
  assets/file.txt >/dev/null
(
  cd "${ROOT}/work"
  "${CLIENT}" clone e2e-rooted rooted-checkout >/dev/null
)
printf 'after\n' >"${ROOT}/work/rooted-checkout/assets/file.txt"
(cd "${ROOT}/work/rooted-checkout" && "${CLIENT}" sync >/dev/null)
root_file_status=$(curl -sS -o /dev/null -w '%{http_code}' \
  "${BASE}/e2e-rooted/file.txt")
rooted_sync_manifest=$(curl -fsSL "${BASE}/e2e-rooted/symbol.toml")
[ "$(curl -fsSL "${BASE}/e2e-rooted/assets/file.txt")" = after ] &&
  [ "${root_file_status}" = 404 ] &&
  contains "${rooted_sync_manifest}" 'version = 1' &&
  ! contains "${rooted_sync_manifest}" 'partial sync root anchor' ||
  fail "single-root partial sync keeps assets/file.txt in place"
ok "single-root partial sync keeps assets/file.txt in place"

mkdir "${ROOT}/work/rooted-checkout/links"
ln -s ../assets/file.txt "${ROOT}/work/rooted-checkout/links/current"
ln -s ../assets/file.txt "${ROOT}/work/rooted-checkout/links/next"
(cd "${ROOT}/work/rooted-checkout" && "${CLIENT}" sync >/dev/null)
root_alias_status=$(curl -sS -o /dev/null -w '%{http_code}' \
  "${BASE}/e2e-rooted/current")
rooted_manifest=$(curl -fsSL "${BASE}/e2e-rooted/symbol.toml")
[ "$(curl -fsSL "${BASE}/e2e-rooted/links/current")" = after ] &&
  [ "$(curl -fsSL "${BASE}/e2e-rooted/links/next")" = after ] &&
  [ "${root_alias_status}" = 404 ] &&
  contains "${rooted_manifest}" '"links/current" = "assets/file.txt"' &&
  contains "${rooted_manifest}" '"links/next" = "assets/file.txt"' ||
  fail "alias-only common-root sync keeps full alias paths"
ok "alias-only common-root sync keeps full alias paths"

"${CLIENT}" copy e2e-main e2e-copy >/dev/null
"${CLIENT}" move e2e-copy e2e-moved >/dev/null
curl -fsSL "${BASE}/e2e-moved/index.html" >/dev/null ||
  fail "copy and move preserve content"
ok "copy and move preserve content"

rm -f "${DROP_STATE}/COPY" "${DROP_STATE}/missing-pending-COPY"
dropped_copy=$(DROP_METHOD=COPY "${CLIENT}" copy e2e-main)
dropped_copy_name=$(printf '%s\n' "${dropped_copy}" |
  awk '$1 == "copied" { url=$4; sub(/\/$/, "", url); sub(/^.*\//, "", url); print url; exit }')
[ -n "${dropped_copy_name}" ] &&
  [ -s "${XDG_STATE_HOME}/symbol/claims/${dropped_copy_name}" ] &&
  [ ! -f "${DROP_STATE}/missing-pending-COPY" ] &&
  curl -fsSL "${BASE}/${dropped_copy_name}/index.html" >/dev/null ||
  fail "dropped COPY response recovers committed destination and claim"
ok "dropped COPY response recovers committed destination and claim"

rm -f "${DROP_STATE}/always-COPY" "${DROP_STATE}/missing-pending-COPY"
if DROP_ALWAYS_METHOD=COPY "${CLIENT}" copy e2e-main >/dev/null 2>&1; then
  fail "repeated response loss should leave COPY pending"
fi
pending=$(find "${XDG_STATE_HOME}/symbol/claims" -type d -name 'pending-*' | awk 'NR==1{print}')
[ -n "${pending}" ] && grep -q '^method=COPY$' "${pending}/record" ||
  fail "COPY pending record survives process loss"
recovery=$("${CLIENT}" recover)
restarted_copy_name=$(printf '%s\n' "${recovery}" |
  awk '$1 == "recovered" { url=$2; sub(/\/$/, "", url); sub(/^.*\//, "", url); print url; exit }')
[ -n "${restarted_copy_name}" ] &&
  [ -s "${XDG_STATE_HOME}/symbol/claims/${restarted_copy_name}" ] &&
  curl -fsSL "${BASE}/${restarted_copy_name}/index.html" >/dev/null ||
  fail "new client process recovers committed COPY after repeated loss"
ok "new client process recovers committed COPY after repeated loss"

rm -f "${DROP_STATE}/COPY" "${DROP_STATE}/missing-pending-COPY"
DROP_METHOD=COPY "${CLIENT}" copy e2e-main e2e-drop-explicit >/dev/null
[ -s "${XDG_STATE_HOME}/symbol/claims/e2e-drop-explicit" ] &&
  [ ! -f "${DROP_STATE}/missing-pending-COPY" ] &&
  curl -fsSL "${BASE}/e2e-drop-explicit/index.html" >/dev/null ||
  fail "explicit dropped COPY persists claim before request"
ok "explicit dropped COPY recovers known destination and claim"

(
  cd "${ROOT}/work"
  "${CLIENT}" remix e2e-main e2e-remix >/dev/null
)
[ -f "${ROOT}/work/e2e-remix/symbol.toml" ] &&
  curl -fsSL "${BASE}/e2e-remix/index.html" >/dev/null ||
  fail "remix copies server site and clones locally"
ok "remix copies server site and clones locally"

mkdir "${ROOT}/work/existing-remix"
printf occupied >"${ROOT}/work/existing-remix/file"
if (cd "${ROOT}/work" && "${CLIENT}" remix e2e-main existing-remix >/dev/null 2>&1); then
  fail "remix refuses occupied local destination before server copy"
fi
code=$(curl -sS -o /dev/null -w '%{http_code}' "${BASE}/existing-remix/")
[ "${code}" = 404 ] || fail "failed remix does not leave server copy"
ok "remix validates local destination before copy"

(
  cd "${ROOT}/work"
  "${CLIENT}" remix --managed e2e-main e2e-managed-remix >/dev/null
)
[ -s "${ROOT}/work/e2e-managed-remix/.symbol-token" ] &&
  [ -s "${ROOT}/work/e2e-managed-remix/.symbol-claim" ] ||
  fail "managed remix attaches recovery credentials to clone"
printf 'managed remix update\n' >"${ROOT}/work/e2e-managed-remix/remix.txt"
(cd "${ROOT}/work/e2e-managed-remix" && "${CLIENT}" put >/dev/null)
curl -fsSL "${BASE}/e2e-managed-remix/remix.txt" >/dev/null ||
  fail "managed remix checkout can publish with attached token"
ok "managed remix attaches token and claim to checkout"

printf 'changed\n' >"${ROOT}/work/change.txt"
"${CLIENT}" put e2e-main "${ROOT}/work/change.txt" >/dev/null
stack=$("${CLIENT}" undo --stack e2e-main)
contains "${stack}" 'restore previous state of e2e-main' ||
  fail "undo stack reports mutation"
"${CLIENT}" undo e2e-main >/dev/null
code=$(curl -sS -o /dev/null -w '%{http_code}' "${BASE}/e2e-main/change.txt")
[ "${code}" = 404 ] || fail "undo restores previous state"
"${CLIENT}" rm e2e-main style.css >/dev/null
code=$(curl -sS -o /dev/null -w '%{http_code}' "${BASE}/e2e-main/style.css")
[ "${code}" = 404 ] || fail "file deletion applies"
"${CLIENT}" undo e2e-main >/dev/null
curl -fsSL "${BASE}/e2e-main/style.css" >/dev/null ||
  fail "file deletion undo restores path"
ok "undo stack, mutation restore, and file-delete restore work"

"${CLIENT}" expire e2e-main >/dev/null
expiry=$("${CLIENT}" expire e2e-main --show)
normalized=$(printf '%s\n' "${expiry}" |
  sed -e 's/20[0-9][0-9]-[0-9T:.-]*Z/<TIME>/g' \
    -e 's/(in [^)]*)/(in <DURATION>)/g')
expected=$(cat <<'EOF'
expiry policies for e2e-main
TARGET                       MODE      EXPIRES                LIMITED BY
e2e-main/                    decay     <TIME> (in <DURATION>) -
EOF
)
[ "${normalized}" = "${expected}" ] ||
  fail "site-wide expiry inventory differs from golden output"
"${CLIENT}" expire e2e-main index.html >/dev/null
expiry=$("${CLIENT}" expire e2e-main index.html --show)
normalized=$(printf '%s\n' "${expiry}" |
  sed -e 's/20[0-9][0-9]-[0-9T:.-]*Z/<TIME>/g' \
    -e 's/(in [^)]*)/(in <DURATION>)/g' \
    -e 's/^policy retention:.*/policy retention:  <DURATION>/' \
    -e 's/^          .* elapsed; .* remaining$/          <DURATION> elapsed; <DURATION> remaining/')
expected=$(printf '%s\n' \
  'effective lifetime: e2e-main/index.html' \
  'size:              14 B' \
  'policy:            decay (30d..365d @ 512 MiB ^3.0)' \
  'policy retention:  <DURATION>' \
  'refreshed:         <TIME>' \
  'own expiry:        <TIME>' \
  'effective expiry:  <TIME> (in <DURATION>)' \
  'inherited cap:      site (site) at <TIME>' \
  'limited by:         site (site)' \
  '' \
  'retention by size' \
  '    365d |\' \
  '         | *....................................  you are here: 14 B' \
  '     30d |.....................................' \
  '      +-------------------------------------' \
  '       0                           512 MiB' \
  '' \
  'effective lifetime' \
  'refreshed |*------------------------------------| expires' \
  '          <DURATION> elapsed; <DURATION> remaining')
[ "${normalized}" = "${expected}" ] ||
  fail "target expiry report differs from golden output"
never=$("${CLIENT}" expire e2e-main --never)
normalized=$(printf '%s\n' "${never}" |
  sed -e 's/^undo within 4h: symbol undo e2e-main .*/undo within 4h: symbol undo e2e-main <TOKEN>/' \
    -e 's|^expiration disabled for .*/e2e-main$|expiration disabled for <BASE>/e2e-main|')
expected=$(cat <<'EOF'
undo within 4h: symbol undo e2e-main <TOKEN>
expiration disabled for <BASE>/e2e-main
EOF
)
[ "${normalized}" = "${expected}" ] ||
  fail "--never differs from golden output"
ok "site and target expiry reports work"

printf '<h1>claim later</h1>\n' >"${ROOT}/work/claim.html"
"${CLIENT}" put e2e-claim "${ROOT}/work/claim.html" >/dev/null
"${CLIENT}" manage e2e-claim --claim >/dev/null
[ -s "${XDG_STATE_HOME}/symbol/tokens/e2e-claim" ] ||
  fail "manage claim stores token when no checkout exists"
claim_token=$(cat "${XDG_STATE_HOME}/symbol/tokens/e2e-claim")
printf 'claimed\n' |
  SYMBOL_TOKEN="${claim_token}" "${CLIENT}" put - e2e-claim claimed.txt >/dev/null
[ "$(curl -fsSL "${BASE}/e2e-claim/claimed.txt")" = claimed ] ||
  fail "manage claim token authorizes mutation"
SYMBOL_TOKEN="${claim_token}" "${CLIENT}" manage e2e-claim --release >/dev/null
ok "manage claim workflow uses persisted creator receipt"

mkdir "${ROOT}/work/managed-loss"
cat >"${ROOT}/work/managed-loss/symbol.toml" <<EOF
version = 1
host = "${BASE}"
name = "e2e-managed-loss"
content_revision = 0
tree_hash = ""

[files]
EOF
printf '<h1>managed loss</h1>\n' >"${ROOT}/work/managed-loss/index.html"
rm -f "${DROP_STATE}/always-PUT" "${DROP_STATE}/missing-pending-PUT"
if (cd "${ROOT}/work/managed-loss" &&
  DROP_ALWAYS_METHOD=PUT "${CLIENT}" put --managed >/dev/null 2>&1); then
  fail "managed repeated response loss should leave pending recovery"
fi
(cd "${ROOT}/work/managed-loss" && "${CLIENT}" recover >/dev/null)
[ -s "${ROOT}/work/managed-loss/.symbol-claim" ] &&
  [ -s "${XDG_STATE_HOME}/symbol/tokens/e2e-managed-loss" ] ||
  fail "managed first creation recovers token from persisted claim"
managed_loss_token=$(cat "${XDG_STATE_HOME}/symbol/tokens/e2e-managed-loss")
SYMBOL_TOKEN="${managed_loss_token}" "${CLIENT}" put e2e-managed-loss \
  "${ROOT}/work/managed-loss/index.html" >/dev/null ||
  fail "recovered management token authorizes later write"
ok "managed first creation survives repeated response loss"

mkdir "${ROOT}/work/managed"
cat >"${ROOT}/work/managed/symbol.toml" <<EOF
version = 1
host = "${BASE}"
name = "e2e-secure"
EOF
printf '<h1>secure</h1>\n' >"${ROOT}/work/managed/index.html"
(cd "${ROOT}/work/managed" && "${CLIENT}" put --managed >/dev/null)
[ -s "${ROOT}/work/managed/.symbol-token" ] ||
  fail "managed creation saves token sidecar"
token=$(awk '{print; exit}' "${ROOT}/work/managed/.symbol-token")
claim=$(awk '{print; exit}' "${ROOT}/work/managed/.symbol-claim")

unauthorized=$(printf nope |
  SYMBOL_TOKEN='' "${CLIENT}" put - e2e-secure blocked.txt 2>&1 || true)
contains "${unauthorized}" 'management token required' ||
  fail "managed site rejects unauthorized mutation"

printf 'token=%s\n' "${token}" |
  SYMBOL_TOKEN="${token}" "${CLIENT}" put - e2e-secure secret.txt >/dev/null
stored=$(curl -fsSL "${BASE}/e2e-secure/secret.txt")
contains "${stored}" 'sym_mgmt_' &&
  ! contains "${stored}" "${token#sym_mgmt_}" ||
  fail "managed upload sanitizes token payload"
ok "management authorization and sanitization work"

if grep -F "${token}" "${ROOT}/server.log" >/dev/null ||
  grep -F "${claim}" "${ROOT}/server.log" >/dev/null ||
  grep -F 'sym_mgmt_' "${ROOT}/server.log" >/dev/null ||
  grep -F 'sym_claim_' "${ROOT}/server.log" >/dev/null; then
  fail "application tracing leaked request or one-time response secrets"
fi
ok "real request and response tracing excludes all management secrets"

"${CLIENT}" expire e2e-secure --show >/dev/null
(cd "${ROOT}/work/managed" && "${CLIENT}" manage e2e-secure --rotate >/dev/null)
rotated=$(awk '{print; exit}' "${ROOT}/work/managed/.symbol-token")
[ "${rotated}" != "${token}" ] || fail "management token rotates"
(cd "${ROOT}/work/managed" && "${CLIENT}" manage e2e-secure --release >/dev/null)
ok "management rotate and release work"

"${CLIENT}" pop e2e-moved "${ROOT}/work/moved.tar" >/dev/null
tar -tf "${ROOT}/work/moved.tar" | awk '$0 == "symbol.toml" { found=1 } END { exit !found }' ||
  fail "pop archive includes manifest"
code=$(curl -sS -o /dev/null -w '%{http_code}' "${BASE}/e2e-moved/")
[ "${code}" = 404 ] || fail "pop removes site"
"${CLIENT}" undo e2e-moved >/dev/null
curl -fsSL "${BASE}/e2e-moved/index.html" >/dev/null ||
  fail "site pop undo restores site"
ok "pop archives, removes, and undo restores site"

"${CLIENT}" undo "${explicit_name}" >/dev/null
code=$(curl -sS -o /dev/null -w '%{http_code}' "${BASE}/${explicit_name}/")
[ "${code}" = 404 ] || fail "create undo removes newly created site"
ok "create undo removes newly created site"

"${CLIENT}" copy e2e-main e2e-stream >/dev/null
"${CLIENT}" pop e2e-stream - >"${ROOT}/work/stream.tar.gz" 2>"${ROOT}/work/stream.err"
tar -tzf "${ROOT}/work/stream.tar.gz" |
  awk '$0 == "symbol.toml" { found=1 } END { exit !found }' ||
  fail "pop dash keeps stdout archive binary-only"
contains "$(cat "${ROOT}/work/stream.err")" 'undo within 4h:' ||
  fail "pop dash routes undo hint to stderr"
ok "pop dash keeps binary stdout clean"

mkdir -p "${ROOT}/work/replace-a" "${ROOT}/work/replace-b"
printf 'A\n' >"${ROOT}/work/replace-a/a.txt"
printf 'B\n' >"${ROOT}/work/replace-a/b.txt"
"${CLIENT}" put e2e-replace "${ROOT}/work/replace-a" >/dev/null
[ "$(curl -fsSL "${BASE}/e2e-replace/a.txt")" = 'A' ] &&
  [ "$(curl -fsSL "${BASE}/e2e-replace/b.txt")" = 'B' ] ||
  fail "replace fixture publishes A and B"
printf 'A2\n' >"${ROOT}/work/replace-b/a.txt"
"${CLIENT}" put --replace e2e-replace "${ROOT}/work/replace-b" >/dev/null
[ "$(curl -fsSL "${BASE}/e2e-replace/a.txt")" = 'A2' ] ||
  fail "replace keeps uploaded A"
code=$(curl -sS -o /dev/null -w '%{http_code}' "${BASE}/e2e-replace/b.txt")
[ "${code}" = 404 ] || fail "replace deletes remote B"
curl -fsSL "${BASE}/e2e-replace/symbol.toml" >/dev/null ||
  fail "replace keeps generated symbol.toml"
"${CLIENT}" undo e2e-replace >/dev/null
[ "$(curl -fsSL "${BASE}/e2e-replace/b.txt")" = 'B' ] ||
  fail "one undo restores B after replace"
ok "put --replace prunes missing paths and one undo restores them"

"${CLIENT}" rm e2e-remix >/dev/null
"${ROOT}/bin/symbol" -t "$(cat "${ROOT}/work/e2e-managed-remix/.symbol-token")" \
  rm e2e-managed-remix >/dev/null
"${CLIENT}" -t "${managed_loss_token}" rm e2e-managed-loss >/dev/null
"${CLIENT}" rm e2e-main >/dev/null
"${CLIENT}" rm e2e-rooted >/dev/null
"${CLIENT}" rm e2e-claim >/dev/null
"${CLIENT}" rm e2e-moved >/dev/null
"${CLIENT}" rm e2e-secure >/dev/null
"${CLIENT}" rm "${implicit_name}" >/dev/null
"${CLIENT}" rm "${dropped_put_name}" >/dev/null
"${CLIENT}" rm "${dropped_copy_name}" >/dev/null
"${CLIENT}" rm "${restarted_put_name}" >/dev/null
"${CLIENT}" rm "${restarted_copy_name}" >/dev/null
"${CLIENT}" rm e2e-drop-explicit >/dev/null
"${CLIENT}" rm e2e-named-drop >/dev/null
"${CLIENT}" rm e2e-replace >/dev/null

left=$(curl -fsSL -H 'Accept: application/json' "${BASE}/FILES")
for name in e2e-main e2e-moved e2e-remix e2e-secure "${explicit_name}" "${implicit_name}" \
  "${dropped_put_name}" "${dropped_copy_name}" "${restarted_put_name}" \
  "${restarted_copy_name}" e2e-drop-explicit e2e-named-drop e2e-managed-loss \
  e2e-claim e2e-rooted e2e-replace; do
  contains "${left}" "\"name\":\"${name}\"" && fail "cleanup removes all test sites"
done
ok "cleanup removes all test sites"

printf '1..%d\n' "${tests}"
