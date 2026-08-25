#!/bin/sh
set -eu

ROOT=$(mktemp -d)
PORT=$((24000 + ($$ % 16000)))
BASE="http://127.0.0.1:${PORT}"
SERVER=${SERVER:-target/debug/symbol}
CLIENT=${CLIENT:-$(pwd)/static/symbol.sh}
CLIENT=$(CDPATH='' cd "$(dirname "${CLIENT}")" && pwd)/$(basename "${CLIENT}")
SERVER_PID=
tests=0

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
  if [ -f "${ROOT}/server.log" ]; then
    printf 'server log:\n' >&2
    awk '{print}' "${ROOT}/server.log" >&2
  fi
  exit 1
}

ok() {
  tests=$((tests + 1))
  printf 'ok %d - %s\n' "${tests}" "$1"
}

contains() {
  printf '%s' "$1" |
    awk -v wanted="$2" 'index($0,wanted){found=1} END{exit !found}'
}

revision() {
  python3 -c 'import json,sys; print(json.load(sys.stdin)["content_revision"])'
}

[ -x "${SERVER}" ] || cargo build --quiet --locked

mkdir -p "${ROOT}/server" "${ROOT}/work" "${ROOT}/state" "${ROOT}/bin"
SYMBOL_PUBLIC_URL="${BASE}" RUST_LOG=warn \
  "${SERVER}" --bind "127.0.0.1:${PORT}" --root "${ROOT}/server" \
  >"${ROOT}/server.log" 2>&1 &
SERVER_PID=$!

ready=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  if curl -fsS "${BASE}/STATS" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
[ "${ready}" -eq 1 ] || fail 'isolated alias server starts'
ok 'isolated alias server starts'

cp "${CLIENT}" "${ROOT}/bin/symbol"
CLIENT=${ROOT}/bin/symbol
chmod +x "${CLIENT}"
curl -fsS "${BASE}/symbol.sh/HASH" >"${ROOT}/bin/.symbol.blake3"

export SYMBOL_HOST="${BASE}" XDG_STATE_HOME="${ROOT}/state"

mkdir -p "${ROOT}/work/source/assets" "${ROOT}/work/source/docs"
printf 'home\n' >"${ROOT}/work/source/index.html"
printf 'app-v1\n' >"${ROOT}/work/source/assets/app.js"
printf 'docs\n' >"${ROOT}/work/source/docs/index.html"
ln -s assets/app.js "${ROOT}/work/source/file-link"
ln -s docs "${ROOT}/work/source/dir-link"
ln -s missing "${ROOT}/work/source/dangling"
ln -s file-link "${ROOT}/work/source/chain"
"${CLIENT}" put e2e-alias "${ROOT}/work/source" >/dev/null

inventory=$(curl -fsS -H 'Accept: application/json' \
  "${BASE}/e2e-alias/FILES")
contains "${inventory}" '"path":"file-link"' &&
  [ "$(curl -fsS "${BASE}/e2e-alias/file-link")" = app-v1 ] &&
  [ "$(curl -fsS "${BASE}/e2e-alias/dir-link/index.html")" = docs ] &&
  [ "$(curl -fsS "${BASE}/e2e-alias/chain")" = app-v1 ] ||
  fail 'tar upload preserves file directory and chained aliases'
dangling_status=$(curl -sS -o /dev/null -w '%{http_code}' \
  "${BASE}/e2e-alias/dangling")
[ "${dangling_status}" = 404 ] ||
  fail 'tar upload preserves dangling alias'
ok 'tar upload preserves file directory dangling and chained aliases'

"${CLIENT}" alias e2e-alias file-link assets/app.js \
  dir-link docs dangling missing chain file-link >/dev/null
inventory=$(curl -fsS -H 'Accept: application/json' \
  "${BASE}/e2e-alias/FILES")
contains "${inventory}" '"path":"file-link"' &&
  contains "${inventory}" '"target":"assets/app.js"' &&
  contains "${inventory}" '"path":"dangling"' ||
  fail 'FILES JSON exposes aliases'
[ "$(curl -fsS "${BASE}/e2e-alias/file-link")" = app-v1 ] ||
  fail 'ALIAS file fixture resolves'
ok 'ALIAS file fixture resolves'
dir_status=$(curl -sS -o "${ROOT}/dir-alias.out" -w '%{http_code}' \
  "${BASE}/e2e-alias/dir-link/index.html")
[ "${dir_status}" = 200 ] &&
  [ "$(cat "${ROOT}/dir-alias.out")" = docs ] ||
  fail 'ALIAS directory fixture resolves'
ok 'ALIAS directory fixture resolves'
chain_status=$(curl -sS -o "${ROOT}/chain-alias.out" -w '%{http_code}' \
  "${BASE}/e2e-alias/chain")
[ "${chain_status}" = 200 ] &&
  [ "$(cat "${ROOT}/chain-alias.out")" = app-v1 ] ||
  fail 'ALIAS chain fixture resolves'
ok 'ALIAS chain fixture resolves'
dangling_status=$(curl -sS -o /dev/null -w '%{http_code}' \
  "${BASE}/e2e-alias/dangling")
[ "${dangling_status}" = 404 ] || fail 'ALIAS fixture remains dangling'

listed=$("${CLIENT}" ls -l e2e-alias)
contains "${listed}" "${BASE}/e2e-alias/file-link -> assets/app.js" &&
  contains "${listed}" "${BASE}/e2e-alias/dir-link -> docs" ||
  fail 'client list renders aliases'
ok 'FILES list renders alias arrows'

before=$(printf '%s' "${inventory}" | revision)
"${CLIENT}" alias e2e-alias command-link assets/app.js >/dev/null
after_inventory=$(curl -fsS -H 'Accept: application/json' \
  "${BASE}/e2e-alias/FILES")
after=$(printf '%s' "${after_inventory}" | revision)
[ "${after}" -eq "$((before + 1))" ] &&
  [ "$(curl -fsS "${BASE}/e2e-alias/command-link")" = app-v1 ] ||
  fail 'single alias command mutates once'
ok 'single alias command uses frozen ALIAS endpoint'

before=${after}
"${CLIENT}" alias e2e-alias batch-file assets/app.js \
  nested/batch-file ../assets/app.js >/dev/null
after_inventory=$(curl -fsS -H 'Accept: application/json' \
  "${BASE}/e2e-alias/FILES")
after=$(printf '%s' "${after_inventory}" | revision)
[ "${after}" -eq "$((before + 1))" ] &&
  [ "$(curl -fsS "${BASE}/e2e-alias/batch-file")" = app-v1 ] &&
  [ "$(curl -fsS "${BASE}/e2e-alias/nested/batch-file")" = app-v1 ] ||
  fail 'atomic alias batch applies in one revision'
ok 'alias batch applies atomically'

if SYMBOL_TEST_RESOLVE_ONLY=1 "${CLIENT}" a \
  >"${ROOT}/resolve.out" 2>"${ROOT}/resolve.err"; then
  fail 'alias command abbreviation is ambiguous'
fi
contains "$(cat "${ROOT}/resolve.err")" \
  "ambiguous command 'a': add (put), alias" ||
  fail 'alias ambiguity reports canonical identities'
ok 'alias command participates in prefix and substring resolution'

REAL_CURL=$(command -v curl)
export REAL_CURL
mkdir "${ROOT}/drop-bin" "${ROOT}/drop-state"
cat >"${ROOT}/drop-bin/curl" <<'DROP_CURL'
#!/bin/sh
set -eu
method=GET
dump=
output=
next=
for argument do
  if [ -n "${next}" ]; then
    case "${next}" in
      method) method=${argument} ;;
      dump) dump=${argument} ;;
      output) output=${argument} ;;
    esac
    next=
  else
    case "${argument}" in
      -X) next=method ;;
      -D) next=dump ;;
      -o) next=output ;;
    esac
  fi
done
if [ "${DROP_ALIAS_ONCE:-0}" = 1 ] && [ "${method}" = ALIAS ] &&
  [ ! -f "${DROP_STATE}/alias" ]; then
  "${REAL_CURL}" "$@" >/dev/null
  [ -z "${dump}" ] || rm -f "${dump}"
  [ -z "${output}" ] || rm -f "${output}"
  : >"${DROP_STATE}/alias"
  exit 52
fi
exec "${REAL_CURL}" "$@"
DROP_CURL
chmod +x "${ROOT}/drop-bin/curl"
PATH="${ROOT}/drop-bin:${PATH}"
export PATH DROP_STATE="${ROOT}/drop-state"
before=${after}
DROP_ALIAS_ONCE=1 "${CLIENT}" alias e2e-alias retry-link assets/app.js \
  >/dev/null
after_inventory=$(curl -fsS -H 'Accept: application/json' \
  "${BASE}/e2e-alias/FILES")
after=$(printf '%s' "${after_inventory}" | revision)
[ "${after}" -eq "$((before + 1))" ] &&
  [ "$(curl -fsS "${BASE}/e2e-alias/retry-link")" = app-v1 ] ||
  fail 'dropped ALIAS response replays one mutation'
ok 'dropped ALIAS response reuses idempotency key'

"${CLIENT}" get e2e-alias "${ROOT}/work/roundtrip.tar" >/dev/null
"${CLIENT}" get e2e-alias "${ROOT}/work/roundtrip.zip" >/dev/null
python3 - "${ROOT}/work/roundtrip.tar" "${ROOT}/work/roundtrip.zip" <<'PY'
import stat
import sys
import tarfile
import zipfile

tar_path, zip_path = sys.argv[1:]
wanted = {
    "file-link": "assets/app.js",
    "dir-link": "docs",
    "dangling": "missing",
    "chain": "file-link",
}
with tarfile.open(tar_path) as archive:
    observed = {
        member.name: member.linkname
        for member in archive.getmembers()
        if member.issym()
    }
assert wanted.items() <= observed.items(), (wanted, observed)
with zipfile.ZipFile(zip_path) as archive:
    observed = {}
    for info in archive.infolist():
        mode = info.external_attr >> 16
        if stat.S_ISLNK(mode):
            observed[info.filename] = archive.read(info).decode()
assert wanted.items() <= observed.items(), (wanted, observed)
PY
ok 'tar and ZIP downloads preserve alias metadata'

python3 - "${ROOT}/work/upload.zip" <<'PY'
import stat
import sys
import zipfile

path = sys.argv[1]
with zipfile.ZipFile(path, "w") as archive:
    archive.writestr("assets/app.js", b"zip-app\n")
    archive.writestr("docs/index.html", b"zip-docs\n")
    for name, target in {
        "file-link": "assets/app.js",
        "dir-link": "docs",
        "dangling": "missing",
        "chain": "file-link",
    }.items():
        info = zipfile.ZipInfo(name)
        info.create_system = 3
        info.external_attr = (stat.S_IFLNK | 0o777) << 16
        archive.writestr(info, target)
PY
"${CLIENT}" put -u e2e-zip "${ROOT}/work/upload.zip" >/dev/null
[ "$(curl -fsS "${BASE}/e2e-zip/file-link")" = zip-app ] &&
  [ "$(curl -fsS "${BASE}/e2e-zip/dir-link/index.html")" = zip-docs ] &&
  [ "$(curl -fsS "${BASE}/e2e-zip/chain")" = zip-app ] ||
  fail 'ZIP upload preserves aliases'
zip_dangling=$(curl -sS -o /dev/null -w '%{http_code}' \
  "${BASE}/e2e-zip/dangling")
[ "${zip_dangling}" = 404 ] || fail 'ZIP upload preserves dangling alias'
"${CLIENT}" get e2e-zip "${ROOT}/work/upload-roundtrip.zip" >/dev/null
python3 - "${ROOT}/work/upload-roundtrip.zip" <<'PY'
import stat
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1]) as archive:
    links = {
        info.filename: archive.read(info).decode()
        for info in archive.infolist()
        if stat.S_ISLNK(info.external_attr >> 16)
    }
assert links["file-link"] == "assets/app.js"
assert links["dir-link"] == "docs"
assert links["dangling"] == "missing"
assert links["chain"] == "file-link"
PY
ok 'ZIP upload and download roundtrip preserves aliases'

(
  cd "${ROOT}/work"
  "${CLIENT}" clone e2e-alias checkout >/dev/null
)
[ -L "${ROOT}/work/checkout/file-link" ] &&
  [ "$(readlink "${ROOT}/work/checkout/file-link")" = assets/app.js ] &&
  [ -L "${ROOT}/work/checkout/dir-link" ] &&
  [ -L "${ROOT}/work/checkout/dangling" ] &&
  [ -L "${ROOT}/work/checkout/chain" ] ||
  fail 'clone creates safe relative aliases'
ok 'clone creates aliases in a safe second pass'

(
  cd "${ROOT}/work"
  SYMBOL_FORCE_NO_SYMLINKS=1 \
    "${CLIENT}" clone e2e-alias materialized >/dev/null
)
[ ! -L "${ROOT}/work/materialized/file-link" ] &&
  [ "$(cat "${ROOT}/work/materialized/file-link")" = app-v1 ] &&
  [ -d "${ROOT}/work/materialized/dir-link" ] &&
  [ "$(cat "${ROOT}/work/materialized/dir-link/index.html")" = docs ] &&
  [ "$(cat "${ROOT}/work/materialized/chain")" = app-v1 ] &&
  [ ! -e "${ROOT}/work/materialized/dangling" ] &&
  contains "$(cat "${ROOT}/work/materialized/symbol.toml")" '[aliases]' ||
  fail 'forced no-symlink clone materializes aliases'
printf 'app-v2\n' >"${ROOT}/work/materialized/assets/app.js"
(cd "${ROOT}/work/materialized" && "${CLIENT}" sync >/dev/null)
synced_alias=$(curl -fsS "${BASE}/e2e-alias/file-link")
synced_manifest=$(curl -fsS "${BASE}/e2e-alias/symbol.toml")
if [ "${synced_alias}" != app-v2 ] ||
  ! contains "${synced_manifest}" '"file-link" = "assets/app.js"'; then
  printf 'synced alias body: %s\nmanifest:\n%s\n' \
    "${synced_alias}" "${synced_manifest}" >&2
  fail 'sync after materialization retains remote alias'
fi
ok 'no-symlink fallback survives the next sync'

mkdir "${ROOT}/work/absolute" "${ROOT}/work/escape" "${ROOT}/work/cycle"
ln -s /etc/passwd "${ROOT}/work/absolute/link"
ln -s ../outside "${ROOT}/work/escape/link"
ln -s second "${ROOT}/work/cycle/first"
ln -s first "${ROOT}/work/cycle/second"
for fixture in absolute escape cycle; do
  if "${CLIENT}" put "e2e-${fixture}" "${ROOT}/work/${fixture}" \
    >/dev/null 2>&1; then
    fail "client accepts malicious ${fixture} symlink"
  fi
done
ok 'client rejects absolute root-escape and cyclic symlinks'

python3 - "${ROOT}/work/malicious.tar" "${ROOT}/work/malicious.zip" \
  "${ROOT}/work/cyclic.zip" <<'PY'
import io
import stat
import sys
import tarfile
import zipfile

tar_path, zip_path, cycle_path = sys.argv[1:]
with tarfile.open(tar_path, "w") as archive:
    data = b"safe\n"
    regular = tarfile.TarInfo("index.html")
    regular.size = len(data)
    archive.addfile(regular, io.BytesIO(data))
    link = tarfile.TarInfo("escape")
    link.type = tarfile.SYMTYPE
    link.linkname = "../outside"
    archive.addfile(link)
with zipfile.ZipFile(zip_path, "w") as archive:
    archive.writestr("index.html", b"safe\n")
    link = zipfile.ZipInfo("escape")
    link.create_system = 3
    link.external_attr = (stat.S_IFLNK | 0o777) << 16
    archive.writestr(link, "/etc/passwd")
with zipfile.ZipFile(cycle_path, "w") as archive:
    for name, target in {"first": "second", "second": "first"}.items():
        link = zipfile.ZipInfo(name)
        link.create_system = 3
        link.external_attr = (stat.S_IFLNK | 0o777) << 16
        archive.writestr(link, target)
PY
stable_revision=$(curl -fsS -H 'Accept: application/json' \
  "${BASE}/e2e-alias/FILES" | revision)
if "${CLIENT}" put -u e2e-malicious-tar "${ROOT}/work/malicious.tar" \
  >/dev/null 2>&1; then
  fail 'server accepts root-escaping tar alias'
fi
if "${CLIENT}" put -u e2e-malicious-zip "${ROOT}/work/malicious.zip" \
  >/dev/null 2>&1; then
  fail 'server accepts absolute ZIP alias'
fi
if "${CLIENT}" put -u e2e-malicious-cycle "${ROOT}/work/cyclic.zip" \
  >/dev/null 2>&1; then
  fail 'server accepts cyclic ZIP aliases'
fi
if "${CLIENT}" put -u e2e-alias "${ROOT}/work/malicious.tar" \
  >/dev/null 2>&1; then
  fail 'malicious archive update succeeds'
fi
[ "$(curl -fsS -H 'Accept: application/json' \
  "${BASE}/e2e-alias/FILES" | revision)" = "${stable_revision}" ] &&
  [ "$(curl -fsS "${BASE}/e2e-alias/file-link")" = app-v2 ] &&
  [ "$(curl -sS -o /dev/null -w '%{http_code}' \
    "${BASE}/e2e-malicious-tar/")" = 404 ] &&
  [ "$(curl -sS -o /dev/null -w '%{http_code}' \
    "${BASE}/e2e-malicious-zip/")" = 404 ] &&
  [ "$(curl -sS -o /dev/null -w '%{http_code}' \
    "${BASE}/e2e-malicious-cycle/")" = 404 ] ||
  fail 'rejected archives leave no partial mutations'
ok 'archive uploads reject malicious alias escapes'

mkdir "${ROOT}/work/managed"
cat >"${ROOT}/work/managed/symbol.toml" <<EOF
version = 1
host = "${BASE}"
name = "e2e-managed-alias"
content_revision = 0
tree_hash = ""

[files]
EOF
printf 'managed\n' >"${ROOT}/work/managed/index.html"
(cd "${ROOT}/work/managed" && "${CLIENT}" put --managed >/dev/null)
managed_token=$(cat "${ROOT}/work/managed/.symbol-token")
if SYMBOL_TOKEN='' "${CLIENT}" alias e2e-managed-alias link index.html \
  >/dev/null 2>&1; then
  fail 'managed alias mutation works without authorization'
fi
"${CLIENT}" -t "${managed_token}" alias e2e-managed-alias link index.html \
  >/dev/null
[ "$(curl -fsS "${BASE}/e2e-managed-alias/link")" = managed ] ||
  fail 'managed alias mutation accepts explicit token'
mkdir -p "${ROOT}/work/managed-archive/assets"
printf 'managed archive\n' >"${ROOT}/work/managed-archive/assets/file"
ln -s assets/file "${ROOT}/work/managed-archive/archive-link"
managed_before=$(curl -fsS -H 'Accept: application/json' \
  "${BASE}/e2e-managed-alias/FILES")
if SYMBOL_TOKEN='' "${CLIENT}" put e2e-managed-alias \
  "${ROOT}/work/managed-archive" >/dev/null 2>&1; then
  fail 'managed archive mutation works without authorization'
fi
managed_after_rejection=$(curl -fsS -H 'Accept: application/json' \
  "${BASE}/e2e-managed-alias/FILES")
[ "$(printf '%s' "${managed_after_rejection}" | revision)" = \
  "$(printf '%s' "${managed_before}" | revision)" ] &&
  ! contains "${managed_after_rejection}" '"path":"archive-link"' ||
  fail 'unauthorized archive mutation writes partial state'
"${CLIENT}" -t "${managed_token}" put e2e-managed-alias \
  "${ROOT}/work/managed-archive" >/dev/null
managed_inventory=$(curl -fsS -H 'Accept: application/json' \
  "${BASE}/e2e-managed-alias/FILES")
contains "${managed_inventory}" '"path":"archive-link"' &&
  [ "$(curl -fsS "${BASE}/e2e-managed-alias/archive-link")" = \
  "managed archive" ] ||
  fail 'managed archive aliases accept explicit token'
ok 'alias command follows managed authorization'

"${CLIENT}" rm e2e-alias >/dev/null
"${CLIENT}" rm e2e-zip >/dev/null
"${CLIENT}" -t "${managed_token}" rm e2e-managed-alias >/dev/null
ok 'alias transfer fixtures clean up'

printf '1..%d\n' "${tests}"
