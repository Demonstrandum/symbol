#!/bin/sh
set -eu

CLIENT=${CLIENT:-$(dirname "$0")/../static/symbol.sh}
CLIENT=$(cd "$(dirname "${CLIENT}")" && pwd)/$(basename "${CLIENT}")
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT HUP INT TERM
mkdir "${ROOT}/bin" "${ROOT}/work"
cp "${CLIENT}" "${ROOT}/bin/symbol-client"
CLIENT=${ROOT}/bin/symbol-client
printf 'ok\n' > "${ROOT}/bin/.symbol.blake3"
LOG=${ROOT}/curl.log
export MOCK_CURL_LOG="${LOG}"
export MOCK_CURL_STATE="${ROOT}"/curl-state
mkdir "${MOCK_CURL_STATE}"

cat > "${ROOT}/bin/curl" <<'MOCK'
#!/bin/sh
set -eu
method=GET headers= output= dump= write= upload= url=
while [ "$#" -gt 0 ]; do
  case "$1" in
    -X) method=$2; shift 2 ;;
    -H) headers="${headers}${headers:+
}$2"; shift 2 ;;
    -D) dump=$2; shift 2 ;;
    -o) output=$2; shift 2 ;;
    -w) write=$2; shift 2 ;;
    -T) upload=$2; shift 2 ;;
    --data-binary) upload=${2#@}; shift 2 ;;
    --max-time) shift 2 ;;
    -s|-S|-f|-L|-fsS|-fsSL|-sS) shift ;;
    -*) shift ;;
    *) url=$1; shift ;;
  esac
done
{
  printf 'METHOD=%s URL=%s\n' "$method" "$url"
  [ -z "$headers" ] || printf '%s\n' "$headers"
  [ -z "$upload" ] || printf 'UPLOAD=%s\n' "$upload"
  if [ -n "$upload" ] && [ "$method" = ALIAS ]; then
    printf 'BODY='
    awk '{printf "%s",$0}' "$upload"
    printf '\n'
  elif [ -n "$upload" ] &&
    printf '%s\n' "$headers" | awk '$0=="Unpack: 1"{found=1} END{exit !found}'; then
    if printf '%s\n' "$headers" |
      awk '$0=="Content-Type: application/gzip"{found=1} END{exit !found}'; then
      tar -tzf "$upload" | sed 's/^/ARCHIVE=/'
      [ "$upload" = - ] ||
        tar -tvzf "$upload" | sed 's/^/ARCHIVE-LONG=/'
    fi
  fi
} >> "$MOCK_CURL_LOG"

status=200 body=ok location=
case "$method:$url" in
  COPY:*)
    status=201
    destination=$(printf '%s\n' "$headers" | awk -F ': ' '$1=="Destination"{print $2}')
    [ -n "$destination" ] || destination=/wxyz
    location="http://mock${destination}/"
    body="copied $location"
    ;;
  MOVE:*)
    destination=$(printf '%s\n' "$headers" | awk -F ': ' '$1=="Destination"{print $2}')
    location="http://mock${destination}/"
    body="moved $location"
    ;;
  ALIAS:*)
    status=201
    body='{"changed":true}'
    ;;
  PUT:http://mock|PUT:*/) status=201; location=http://mock/abcd/; body='created http://mock/abcd/' ;;
  PUT:*) body=updated ;;
  GET:*/drop-file.txt) body=${MOCK_FILE_BODY:-verified-file-body} ;;
  GET:*video.mp4/EXPIRES|EXPIRE:*video.mp4)
    body='{"target":{"site":"hello","path":"video.mp4","kind":"file"},"size":5,"refreshed_at":"2026-01-01T00:00:00Z","own_policy":{"mode":"relative","retention_seconds":100,"expires_at":"2027-01-01T00:00:00Z"},"inherited_caps":[{"kind":"site","path":null,"expires_at":"2026-12-01T00:00:00Z"}],"effective_expires_at":"2026-12-01T00:00:00Z","remaining_seconds":50,"limited_by":{"kind":"site","path":null}}'
    ;;
  EXPIRE:*|GET:*/EXPIRES)
    body='{"site":"hello","entries":[{"target":{"site":"hello","path":null,"kind":"site"},"size":10,"own_policy":{"mode":"decay"},"inherited_caps":[],"effective_expires_at":"2027-01-01T00:00:00Z","remaining_seconds":100,"limited_by":null},{"target":{"site":"hello","path":"video.mp4","kind":"file"},"size":5,"own_policy":{"mode":"relative"},"inherited_caps":[{"kind":"site","path":null,"expires_at":"2027-01-01T00:00:00Z"}],"effective_expires_at":"2027-01-01T00:00:00Z","remaining_seconds":50,"limited_by":{"kind":"site","path":null}}]}'
    ;;
  GET:*/UNDO)
    body='{"site":"hello","entries":[{"token":"tok1","description":"restore file","expires_at":"2027-01-01T00:00:00Z","remaining_seconds":100}]}'
    ;;
  GET:*/symbol.toml)
    if [ "${MOCK_COMMON_ROOT_INVENTORY:-0}" = 1 ]; then
      body='version = 1
host = "http://mock"
name = "rooted"
content_revision = 2
tree_hash = "blake3:new"

[files]
"assets/file.txt" = "blake3:local"'
    elif [ "${MOCK_ALIAS_ROOT_INVENTORY:-0}" = 1 ]; then
      body='version = 1
host = "http://mock"
name = "alias-root"
content_revision = 2
tree_hash = "blake3:new"

[files]
"assets/file.txt" = "blake3:local"'
    else
      body='version = 1
host = "http://mock"
name = "hello"
content_revision = 2
tree_hash = "blake3:new"

[files]
"index.html" = "blake3:file"'
    fi
    ;;
  GET:*/FILES)
    if [ "${MOCK_LIST_ALIASES:-0}" = 1 ]; then
      body='hello/       2 files   10 B
file-link -> index.html
dir-link -> assets'
    elif [ "${MOCK_ALIAS_INVENTORY:-0}" = 1 ]; then
      body='{"site":"hello","content_revision":1,"tree_hash":"blake3:base","files":[{"path":"-leading","hash":"blake3:local","size":8},{"path":"assets/app.js","hash":"blake3:local","size":4},{"path":"docs/index.html","hash":"blake3:local","size":5}],"aliases":[{"path":"chain","target":"file-link","target_kind":"file"},{"path":"dangling","target":"missing","target_kind":null},{"path":"dir-link","target":"docs","target_kind":"directory"},{"path":"file-link","target":"assets/app.js","target_kind":"file"}]}'
    elif [ "${MOCK_COMMON_ROOT_INVENTORY:-0}" = 1 ]; then
      body='{"site":"rooted","content_revision":1,"tree_hash":"blake3:base","files":[{"path":"assets/file.txt","hash":"blake3:old","size":4}],"aliases":[]}'
    elif [ "${MOCK_ALIAS_ROOT_INVENTORY:-0}" = 1 ]; then
      body='{"site":"alias-root","content_revision":1,"tree_hash":"blake3:base","files":[{"path":"assets/file.txt","hash":"blake3:local","size":7}],"aliases":[]}'
    else
      body='{"site":"hello","content_revision":1,"tree_hash":"blake3:base","files":[{"path":"index.html","hash":"blake3:local","size":10}],"aliases":[]}'
    fi
    ;;
  GET:*.tar.gz|DELETE:*.tar.gz) body=ARCHIVE-BYTES ;;
  DELETE:*/whole) body=ARCHIVE-BYTES ;;
  DELETE:*) body=deleted ;;
  MANAGE:*) body=managed ;;
esac

if [ "${MOCK_BAD_ARCHIVE:-0}" = 1 ] && [ "$method" = GET ]; then
  body='not-an-archive'
fi

if [ "${MOCK_HTTP_ERROR_METHOD:-}" = "$method" ]; then
  status=400
  body='error: forced failure'
fi

if [ "${MOCK_DROP_ALWAYS_METHOD:-}" = "$method" ]; then
  if ! ls "$XDG_STATE_HOME/symbol/claims"/pending-* >/dev/null 2>&1; then
    : > "$MOCK_CURL_STATE/missing-pending-$method"
  fi
  : > "$MOCK_CURL_STATE/committed-$method"
  exit 52
fi

if [ "${MOCK_DROP_ONCE_METHOD:-}" = "$method" ] &&
  [ ! -f "$MOCK_CURL_STATE/dropped-$method" ]; then
  if ! ls "$XDG_STATE_HOME/symbol/claims"/pending-* >/dev/null 2>&1; then
    : > "$MOCK_CURL_STATE/missing-pending-$method"
  fi
  : > "$MOCK_CURL_STATE/dropped-$method"
  exit 52
fi

if [ -n "$dump" ]; then
  {
    printf 'HTTP/1.1 %s OK\r\n' "$status"
    [ -z "$location" ] || printf 'Location: %s\r\n' "$location"
    case "$method" in
      PUT|DELETE|COPY|MOVE|ALIAS|EXPIRE)
        printf 'Undo-Token: undo1\r\nUndo-Expires: 2027-01-01T00:00:00Z\r\n'
        ;;
      MANAGE)
        printf 'Management-Token: sym_mgmt_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\r\n'
        ;;
    esac
    printf '\r\n'
  } > "$dump"
fi
if [ -n "${MOCK_ARCHIVE:-}" ] && [ "$method" = GET ]; then
  case "$url" in
    *.tar.gz)
      if [ -n "$output" ]; then
        cp "$MOCK_ARCHIVE" "$output"
      else
        cat "$MOCK_ARCHIVE"
      fi
      ;;
    *) if [ -n "$output" ]; then printf '%s' "$body" > "$output"; else printf '%s' "$body"; fi ;;
  esac
elif [ -n "$output" ]; then
  printf '%s' "$body" > "$output"
else
  printf '%s' "$body"
fi
[ -z "$write" ] || printf '%s' "$status"
MOCK
chmod +x "${ROOT}/bin/curl"
cat > "${ROOT}/bin/b3sum" <<'MOCK'
#!/bin/sh
printf 'local  %s\n' "$1"
MOCK
chmod +x "${ROOT}/bin/b3sum"

PATH="${ROOT}/bin:${PATH}"
export PATH SYMBOL_HOST=http://mock XDG_STATE_HOME="${ROOT}/state"
failures=0 tests=0
ok() { tests=$((tests + 1)); printf 'ok %d - %s\n' "${tests}" "$1"; }
not_ok() { tests=$((tests + 1)); failures=$((failures + 1)); printf 'not ok %d - %s\n' "${tests}" "$1"; }
contains() { printf '%s' "$1" | awk -v wanted="$2" 'index($0,wanted){found=1} END{exit !found}'; }

out=$("${CLIENT}" help)
contains "${out}" 'symbol sync [--check]' &&
  contains "${out}" 'symbol alias SITE PATH TARGET [PATH TARGET ...]' &&
  ! contains "${out}" 'alias ->' &&
  ok 'canonical help treats alias as a command' ||
  not_ok 'canonical help treats alias as a command'

if "${CLIENT}" p >"${ROOT}/out" 2>"${ROOT}/err"; then
  not_ok 'ambiguous prefix exits nonzero'
elif contains "$(cat "${ROOT}/err")" "ambiguous command 'p': put, pop, pull (clone)"; then
  ok 'identity-collapsed ambiguity'
else
  not_ok 'identity-collapsed ambiguity'
fi

registry=$(SYMBOL_TEST_COMMAND_REGISTRY=1 "${CLIENT}")
registry_ok=1
printf '%s\n' "${registry}" | while read -r canonical spellings; do
  [ -n "${canonical}" ] || continue
  for spelling in ${spellings}; do
    resolved=$(SYMBOL_TEST_RESOLVE_ONLY=1 "${CLIENT}" "${spelling}") || exit 1
    [ "${resolved}" = "${canonical}" ] || exit 1
  done
done || registry_ok=0
[ "${registry_ok}" -eq 1 ] &&
  ok 'all canonical commands and aliases resolve exactly' ||
  not_ok 'all canonical commands and aliases resolve exactly'

abbreviation_ok=1
for pair in 'l:ls' 'del:rm' 'cl:clone' 'co:copy' 'ren:move' 'sy:sync' 'x:remix' 'own:get'; do
  spelling=${pair%%:*}
  canonical=${pair#*:}
  resolved=$(SYMBOL_TEST_RESOLVE_ONLY=1 "${CLIENT}" "${spelling}") || abbreviation_ok=0
  [ "${resolved}" = "${canonical}" ] || abbreviation_ok=0
done
[ "${abbreviation_ok}" -eq 1 ] &&
  ok 'prefix and substring abbreviations resolve to expected identities' ||
  not_ok 'prefix and substring abbreviations resolve to expected identities'

if SYMBOL_TEST_RESOLVE_ONLY=1 "${CLIENT}" a >"${ROOT}/out" 2>"${ROOT}/err"; then
  not_ok 'alias command participates in ambiguity resolution'
elif contains "$(cat "${ROOT}/err")" \
  "ambiguous command 'a': add (put), alias"; then
  ok 'alias command participates in ambiguity resolution'
else
  not_ok 'alias command participates in ambiguity resolution'
fi

: > "${LOG}"
out=$("${CLIENT}" -t alias-token alias hello current/app.js ../assets/app.js)
log=$(cat "${LOG}")
contains "${log}" 'METHOD=ALIAS URL=http://mock/hello/current/app.js' &&
  contains "${log}" 'Alias-Target: ../assets/app.js' &&
  contains "${log}" 'Authorization: Bearer alias-token' &&
  contains "${log}" 'Idempotency-Key:' &&
  contains "${out}" 'aliased http://mock/hello/current/app.js -> ../assets/app.js' &&
  contains "${out}" 'undo within 4h: symbol undo hello undo1' &&
  ok 'alias is a conditional-capable proper command' ||
  not_ok 'alias is a conditional-capable proper command'

: > "${LOG}"
out=$("${CLIENT}" alias hello one target.txt nested/two ../shared/two.txt)
log=$(cat "${LOG}")
contains "${log}" 'METHOD=ALIAS URL=http://mock/hello/' &&
  contains "${log}" 'Content-Type: application/json' &&
  contains "${log}" \
    'BODY={"aliases":[{"path":"one","target":"target.txt"},{"path":"nested/two","target":"../shared/two.txt"}]}' &&
  contains "${out}" 'aliased 2 paths atomically in http://mock/hello/' &&
  ok 'alias batch uses one atomic request' ||
  not_ok 'alias batch uses one atomic request'

rm -f "${MOCK_CURL_STATE}/dropped-ALIAS"
: > "${LOG}"
out=$(MOCK_DROP_ONCE_METHOD=ALIAS \
  "${CLIENT}" alias hello retry-link target.txt)
alias_keys=$(awk -F ': ' '$1=="Idempotency-Key"{print $2}' "${LOG}" |
  LC_ALL=C sort -u | awk 'END{print NR+0}')
contains "${out}" 'aliased http://mock/hello/retry-link -> target.txt' &&
  [ "${alias_keys}" -eq 1 ] &&
  ok 'dropped ALIAS response retries one idempotency key' ||
  not_ok 'dropped ALIAS response retries one idempotency key'

: > "${LOG}"
out=$(MOCK_LIST_ALIASES=1 "${CLIENT}" ls -l hello)
contains "${out}" 'http://mock/hello/file-link -> index.html' &&
  contains "${out}" 'http://mock/hello/dir-link -> assets' &&
  ok 'linked list output renders alias arrows' ||
  not_ok 'linked list output renders alias arrows'

: > "${LOG}"
out=$("${CLIENT}" co hello target)
contains "$(cat "${LOG}")" 'METHOD=COPY URL=http://mock/hello' &&
  contains "$(cat "${LOG}")" 'Destination: /target' &&
  contains "${out}" 'copied http://mock/hello/ -> http://mock/target/' &&
  contains "${out}" 'undo within 4h: symbol undo target undo1' &&
  ok 'copy uses COPY and Destination' || not_ok 'copy uses COPY and Destination'

: > "${LOG}"
"${CLIENT}" ren old new >/dev/null
contains "$(cat "${LOG}")" 'METHOD=MOVE URL=http://mock/old' &&
  contains "$(cat "${LOG}")" 'Destination: /new' &&
  ok 'rename alias resolves to MOVE' || not_ok 'rename alias resolves to MOVE'

if MOCK_BAD_ARCHIVE=1 "${CLIENT}" remix hello failed-remix \
  >"${ROOT}/remix.out" 2>"${ROOT}/remix.err"; then
  not_ok 'remix clone failure should fail'
elif contains "$(cat "${ROOT}/remix.err")" \
  'server copy remains at http://mock/failed-remix/' &&
  contains "$(cat "${ROOT}/remix.err")" 'cleanup with: symbol rm failed-remix'; then
  ok 'remix clone failure retains server copy with exact cleanup guidance'
else
  not_ok 'remix clone failure retains server copy with exact cleanup guidance'
fi

: > "${LOG}"
out=$("${CLIENT}" get hello -)
[ "${out}" = ARCHIVE-BYTES ] &&
  contains "$(cat "${LOG}")" 'METHOD=GET URL=http://mock/hello.tar.gz' &&
  ok 'get dash keeps stdout binary-only' || not_ok 'get dash keeps stdout binary-only'

: > "${LOG}"
printf '<h1>x</h1>' | "${CLIENT}" -t explicit put >/dev/null
log=$(cat "${LOG}")
contains "${log}" 'METHOD=PUT URL=http://mock/' &&
  contains "${log}" 'UPLOAD=-' &&
  contains "${log}" 'Authorization: Bearer explicit' &&
  contains "${log}" 'Unpack: 1' &&
  contains "${log}" 'ARCHIVE=./index.html' &&
  ok 'stdin put and explicit token' || not_ok 'stdin put and explicit token'

: > "${LOG}"
printf '<h1>explicit</h1>\n' | "${CLIENT}" put - >/dev/null
contains "$(cat "${LOG}")" 'METHOD=PUT URL=http://mock/' &&
  contains "$(cat "${LOG}")" 'UPLOAD=-' &&
  contains "$(cat "${LOG}")" 'ARCHIVE=./index.html' &&
  ok 'explicit stdin dash publishes random index' ||
  not_ok 'explicit stdin dash publishes random index'

: > "${LOG}"
printf '<h1>implicit</h1>\n' | "${CLIENT}" put >/dev/null
contains "$(cat "${LOG}")" 'METHOD=PUT URL=http://mock/' &&
  contains "$(cat "${LOG}")" 'UPLOAD=-' &&
  contains "$(cat "${LOG}")" 'ARCHIVE=./index.html' &&
  ok 'implicit piped stdin publishes random index' ||
  not_ok 'implicit piped stdin publishes random index'

: > "${LOG}"
printf '<h1>slash</h1>\n' |
  SYMBOL_HOST=http://mock/ "${CLIENT}" put - >/dev/null
contains "$(cat "${LOG}")" 'METHOD=PUT URL=http://mock/' &&
  ! contains "$(cat "${LOG}")" 'URL=http://mock//' &&
  ok 'trailing host slash is normalized for stdin put' ||
  not_ok 'trailing host slash is normalized for stdin put'

rm -f "${MOCK_CURL_STATE}/dropped-PUT" "${MOCK_CURL_STATE}/missing-pending-PUT"
out=$(printf '<h1>dropped</h1>\n' | MOCK_DROP_ONCE_METHOD=PUT "${CLIENT}" put)
contains "${out}" 'created http://mock/abcd/' &&
  [ -s "${XDG_STATE_HOME}/symbol/claims/abcd" ] &&
  [ ! -f "${MOCK_CURL_STATE}/missing-pending-PUT" ] &&
  ! ls "${XDG_STATE_HOME}/symbol/claims"/pending-* >/dev/null 2>&1 &&
  ok 'dropped PUT response retries with pre-persisted claim' ||
  not_ok 'dropped PUT response retries with pre-persisted claim'

rm -f "${MOCK_CURL_STATE}/dropped-COPY" "${MOCK_CURL_STATE}/missing-pending-COPY"
out=$(MOCK_DROP_ONCE_METHOD=COPY "${CLIENT}" copy hello)
contains "${out}" 'copied http://mock/hello/ -> http://mock/wxyz/' &&
  [ -s "${XDG_STATE_HOME}/symbol/claims/wxyz" ] &&
  [ ! -f "${MOCK_CURL_STATE}/missing-pending-COPY" ] &&
  ! ls "${XDG_STATE_HOME}/symbol/claims"/pending-* >/dev/null 2>&1 &&
  ok 'dropped COPY response retries with pre-persisted claim' ||
  not_ok 'dropped COPY response retries with pre-persisted claim'

rm -f "${MOCK_CURL_STATE}/dropped-COPY" "${MOCK_CURL_STATE}/missing-pending-COPY"
out=$(MOCK_DROP_ONCE_METHOD=COPY "${CLIENT}" copy hello target)
contains "${out}" 'copied http://mock/hello/ -> http://mock/target/' &&
  [ -s "${XDG_STATE_HOME}/symbol/claims/target" ] &&
  [ ! -f "${MOCK_CURL_STATE}/missing-pending-COPY" ] &&
  ok 'explicit COPY persists destination claim before dropped response' ||
  not_ok 'explicit COPY persists destination claim before dropped response'

rm -f "${MOCK_CURL_STATE}/committed-PUT" "${MOCK_CURL_STATE}/missing-pending-PUT"
if printf '<h1>process loss</h1>\n' |
  MOCK_DROP_ALWAYS_METHOD=PUT "${CLIENT}" put >/dev/null 2>&1; then
  not_ok 'repeatedly dropped PUT should leave pending recovery'
elif ls "${XDG_STATE_HOME}/symbol/claims"/pending-* >/dev/null 2>&1; then
  pending=$(find "${XDG_STATE_HOME}/symbol/claims" -type d -name 'pending-*' | awk 'NR==1{print}')
  contains "$(cat "${pending}/record")" 'method=PUT' &&
    contains "$(cat "${pending}/record")" 'idempotency=' &&
    [ -s "${pending}/body" ] ||
    not_ok 'pending PUT record contains replay identity and body'
  "${CLIENT}" recover >/dev/null
  [ -s "${XDG_STATE_HOME}/symbol/claims/abcd" ] &&
    ! ls "${XDG_STATE_HOME}/symbol/claims"/pending-* >/dev/null 2>&1 &&
    ok 'new process recovers pending generated PUT' ||
    not_ok 'new process recovers pending generated PUT'
else
  not_ok 'repeatedly dropped PUT persists typed pending state'
fi

mkdir -p "${XDG_STATE_HOME}/symbol/claims/pending-stale"
printf stale > "${XDG_STATE_HOME}/symbol/claims/pending-stale/claim"
touch -t 202001010000 "${XDG_STATE_HOME}/symbol/claims/pending-stale"
"${CLIENT}" recover >/dev/null
[ ! -e "${XDG_STATE_HOME}/symbol/claims/pending-stale" ] &&
  ok 'recover prunes abandoned pending state after seven days' ||
  not_ok 'recover prunes abandoned pending state after seven days'

rm -f "${MOCK_CURL_STATE}/committed-COPY" "${MOCK_CURL_STATE}/missing-pending-COPY"
if MOCK_DROP_ALWAYS_METHOD=COPY "${CLIENT}" copy hello >/dev/null 2>&1; then
  not_ok 'repeatedly dropped COPY should leave pending recovery'
elif ls "${XDG_STATE_HOME}/symbol/claims"/pending-* >/dev/null 2>&1; then
  "${CLIENT}" recover >/dev/null
  [ -s "${XDG_STATE_HOME}/symbol/claims/wxyz" ] &&
    ! ls "${XDG_STATE_HOME}/symbol/claims"/pending-* >/dev/null 2>&1 &&
    ok 'new process recovers pending generated COPY' ||
    not_ok 'new process recovers pending generated COPY'
else
  not_ok 'repeatedly dropped COPY persists typed pending state'
fi

printf 'verified-file-body' > "${ROOT}/work/drop-file.txt"
rm -f "${MOCK_CURL_STATE}/dropped-PUT"
: > "${LOG}"
out=$(MOCK_FILE_BODY=verified-file-body MOCK_DROP_ONCE_METHOD=PUT \
  "${CLIENT}" put hello "${ROOT}/work/drop-file.txt" drop-file.txt)
file_puts=$(awk '$0=="METHOD=PUT URL=http://mock/hello/drop-file.txt"{n++} END{print n+0}' \
  "${LOG}")
[ "${file_puts}" -eq 1 ] &&
  ! contains "$(cat "${LOG}")" 'Idempotency-Key:' &&
  contains "${out}" \
    'verified committed update http://mock/hello/drop-file.txt after response loss' &&
  ! ls "${XDG_STATE_HOME}/symbol/claims"/pending-* >/dev/null 2>&1 &&
  ok 'dropped file PUT verifies without unsupported idempotent retry' ||
  not_ok 'dropped file PUT verifies without unsupported idempotent retry'

mkdir "${ROOT}/work/managed-loss"
cat > "${ROOT}/work/managed-loss/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "managed-loss"
content_revision = 0
tree_hash = ""

[files]
MANIFEST
printf 'managed\n' > "${ROOT}/work/managed-loss/index.html"
rm -f "${MOCK_CURL_STATE}/committed-PUT" "${MOCK_CURL_STATE}/missing-pending-PUT"
if (cd "${ROOT}/work/managed-loss" &&
  MOCK_DROP_ALWAYS_METHOD=PUT "${CLIENT}" put --managed >/dev/null 2>&1); then
  not_ok 'managed dropped response should require recovery'
else
  "${CLIENT}" recover >/dev/null
  [ -s "${XDG_STATE_HOME}/symbol/tokens/managed-loss" ] &&
    [ -s "${ROOT}/work/managed-loss/.symbol-claim" ] &&
    ok 'new process recovers management token with persisted claim' ||
    not_ok 'new process recovers management token with persisted claim'
fi

out=$("${CLIENT}" expire)
contains "${out}" 'expiration is disabled until explicitly enabled.' &&
  contains "${out}" 'default retention' &&
  ok 'bare expire is local help' || not_ok 'bare expire is local help'

: > "${LOG}"
out=$("${CLIENT}" expire hello --show)
contains "$(cat "${LOG}")" 'METHOD=GET URL=http://mock/hello/EXPIRES' &&
  contains "${out}" 'hello/' &&
  contains "${out}" 'hello/video.mp4' &&
  contains "${out}" '2027-01-01T00:00:00Z (in 1m 40s)' &&
  contains "${out}" 'site' &&
  ok 'expire show report' || not_ok 'expire show report'

: > "${LOG}"
out=$("${CLIENT}" expire hello video.mp4 --show)
contains "${out}" 'effective lifetime: hello/video.mp4' &&
  contains "${out}" 'inherited cap:' &&
  contains "${out}" 'limited by:         site (site)' &&
  ok 'target expiry report renders inherited limiting policy' ||
  not_ok 'target expiry report renders inherited limiting policy'

: > "${LOG}"
out=$("${CLIENT}" expire hello video.mp4 --never)
contains "${out}" \
  'effective expiry remains 2026-12-01T00:00:00Z (in 50s, limited by site)' &&
  ok 'never output names inherited limiting policy' ||
  not_ok 'never output names inherited limiting policy'

: > "${LOG}"
out=$("${CLIENT}" undo --stack hello)
expected=$(printf 'TOKEN      WOULD UNDO                         EXPIRES\n%-10s %-34s %s (in %s)' \
  tok1 'restore file' '2027-01-01 00:00Z' '1m 40s')
[ "${out}" = "${expected}" ] &&
  ok 'undo stack uses canonical compact UTC and duration output' ||
  not_ok 'undo stack uses canonical compact UTC and duration output'

: > "${LOG}"
rm_status=0
"${CLIENT}" rm hello old.css >"${ROOT}/rm.out" 2>"${ROOT}/rm.err" || rm_status=$?
[ "${rm_status}" -eq 0 ] &&
  contains "$(cat "${LOG}")" 'METHOD=DELETE URL=http://mock/hello/old.css' &&
  ok 'remote file deletion path' || not_ok 'remote file deletion path'

: > "${LOG}"
out=$("${CLIENT}" rm whole)
contains "${out}" 'deleted whole' &&
  ! contains "${out}" 'ARCHIVE-BYTES' &&
  ok 'whole-site rm discards archive bytes' || not_ok 'whole-site rm discards archive bytes'

mkdir -p "${ROOT}/work/symlinks/assets" "${ROOT}/work/symlinks/docs"
printf 'app\n' > "${ROOT}/work/symlinks/assets/app.js"
printf 'docs\n' > "${ROOT}/work/symlinks/docs/index.html"
ln -s assets/app.js "${ROOT}/work/symlinks/file-link"
ln -s docs "${ROOT}/work/symlinks/dir-link"
ln -s missing "${ROOT}/work/symlinks/dangling"
ln -s file-link "${ROOT}/work/symlinks/chain"
: > "${LOG}"
"${CLIENT}" put alias-upload "${ROOT}/work/symlinks" >/dev/null
log=$(cat "${LOG}")
contains "${log}" 'ARCHIVE=./file-link' &&
  contains "${log}" 'ARCHIVE=./dir-link' &&
  contains "${log}" 'ARCHIVE=./dangling' &&
  contains "${log}" 'ARCHIVE=./chain' &&
  contains "${log}" 'file-link -> assets/app.js' &&
  contains "${log}" 'dir-link -> docs' &&
  contains "${log}" 'dangling -> missing' &&
  contains "${log}" 'chain -> file-link' &&
  ok 'directory put archives file directory dangling and chained symlinks' ||
  not_ok 'directory put archives file directory dangling and chained symlinks'

mkdir "${ROOT}/work/escape-links"
ln -s ../outside "${ROOT}/work/escape-links/root-escape"
: > "${LOG}"
if "${CLIENT}" put unsafe "${ROOT}/work/escape-links" \
  >"${ROOT}/unsafe.out" 2>"${ROOT}/unsafe.err"; then
  not_ok 'put rejects root-escaping symlinks'
elif contains "$(cat "${ROOT}/unsafe.err")" 'unsafe symlink target' &&
  ! contains "$(cat "${LOG}")" 'METHOD=PUT'; then
  ok 'put rejects root-escaping symlinks'
else
  not_ok 'put rejects root-escaping symlinks'
fi

mkdir "${ROOT}/work/cycle-links"
ln -s second "${ROOT}/work/cycle-links/first"
ln -s first "${ROOT}/work/cycle-links/second"
: > "${LOG}"
if "${CLIENT}" put cyclic "${ROOT}/work/cycle-links" \
  >"${ROOT}/cyclic.out" 2>"${ROOT}/cyclic.err"; then
  not_ok 'put rejects symlink cycles'
elif contains "$(cat "${ROOT}/cyclic.err")" 'unsafe symlink graph' &&
  ! contains "$(cat "${LOG}")" 'METHOD=PUT'; then
  ok 'put rejects symlink cycles'
else
  not_ok 'put rejects symlink cycles'
fi

mkdir -p "${ROOT}/work/archive-root/assets" "${ROOT}/work/archive-root/docs"
printf 'leading\n' > "${ROOT}/work/archive-root/-leading"
printf 'app\n' > "${ROOT}/work/archive-root/assets/app.js"
printf 'docs\n' > "${ROOT}/work/archive-root/docs/index.html"
ln -s assets/app.js "${ROOT}/work/archive-root/file-link"
ln -s docs "${ROOT}/work/archive-root/dir-link"
ln -s missing "${ROOT}/work/archive-root/dangling"
ln -s file-link "${ROOT}/work/archive-root/chain"
cat > "${ROOT}/work/archive-root/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "hello"
content_revision = 1
tree_hash = "blake3:base"

[files]
"-leading" = "blake3:local"
"assets/app.js" = "blake3:local"
"docs/index.html" = "blake3:local"

[aliases]
"chain" = "file-link"
"dangling" = "missing"
"dir-link" = "docs"
"file-link" = "assets/app.js"
MANIFEST
(
  cd "${ROOT}/work/archive-root"
  tar -czf "${ROOT}/work/aliases.tar.gz" -- symbol.toml -leading assets/app.js \
    docs/index.html file-link dir-link dangling chain
)
(
  cd "${ROOT}/work"
  MOCK_ARCHIVE="${ROOT}/work/aliases.tar.gz" \
    "${CLIENT}" clone hello clone-links >/dev/null
)
[ -L "${ROOT}/work/clone-links/file-link" ] &&
  [ "$(cat "${ROOT}/work/clone-links/-leading")" = leading ] &&
  [ "$(readlink "${ROOT}/work/clone-links/file-link")" = assets/app.js ] &&
  [ -L "${ROOT}/work/clone-links/dir-link" ] &&
  [ "$(readlink "${ROOT}/work/clone-links/dir-link")" = docs ] &&
  [ -L "${ROOT}/work/clone-links/dangling" ] &&
  [ -L "${ROOT}/work/clone-links/chain" ] &&
  ok 'clone creates validated relative symlinks in a second pass' ||
  not_ok 'clone creates validated relative symlinks in a second pass'

(
  cd "${ROOT}/work"
  SYMBOL_FORCE_NO_SYMLINKS=1 MOCK_ARCHIVE="${ROOT}/work/aliases.tar.gz" \
    "${CLIENT}" clone hello clone-materialized >/dev/null
)
[ ! -L "${ROOT}/work/clone-materialized/file-link" ] &&
  [ "$(cat "${ROOT}/work/clone-materialized/file-link")" = app ] &&
  [ -d "${ROOT}/work/clone-materialized/dir-link" ] &&
  [ "$(cat "${ROOT}/work/clone-materialized/dir-link/index.html")" = docs ] &&
  [ "$(cat "${ROOT}/work/clone-materialized/chain")" = app ] &&
  [ ! -e "${ROOT}/work/clone-materialized/dangling" ] &&
  contains "$(cat "${ROOT}/work/clone-materialized/symbol.toml")" '[aliases]' &&
  ok 'forced no-symlink clone materializes resolvable targets' ||
  not_ok 'forced no-symlink clone materializes resolvable targets'

out=$(cd "${ROOT}/work/clone-materialized" &&
  MOCK_ALIAS_INVENTORY=1 "${CLIENT}" sync --check)
contains "${out}" 'no changes made' &&
  ! contains "${out}" '+ file-link' &&
  ! contains "${out}" '+ dir-link' &&
  ok 'sync preserves aliases after no-symlink materialization' ||
  not_ok 'sync preserves aliases after no-symlink materialization'

: > "${LOG}"
(cd "${ROOT}/work/clone-materialized" && "${CLIENT}" put >/dev/null)
log=$(cat "${LOG}")
contains "${log}" 'file-link -> assets/app.js' &&
  contains "${log}" 'dir-link -> docs' &&
  contains "${log}" 'dangling -> missing' &&
  contains "${log}" 'chain -> file-link' &&
  ok 'put re-emits aliases instead of materialized fallback content' ||
  not_ok 'put re-emits aliases instead of materialized fallback content'

mkdir "${ROOT}/work/malicious-root"
cat > "${ROOT}/work/malicious-root/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "hello"
content_revision = 1
tree_hash = "blake3:base"

[files]

[aliases]
"escape" = "../outside"
MANIFEST
ln -s ../outside "${ROOT}/work/malicious-root/escape"
(
  cd "${ROOT}/work/malicious-root"
  tar -czf "${ROOT}/work/malicious.tar.gz" -- symbol.toml escape
)
if (cd "${ROOT}/work" &&
  MOCK_ARCHIVE="${ROOT}/work/malicious.tar.gz" \
    "${CLIENT}" clone hello malicious-clone >/dev/null 2>&1); then
  not_ok 'clone rejects malicious alias escape metadata'
elif [ ! -e "${ROOT}/work/outside" ]; then
  ok 'clone rejects malicious alias escape metadata'
else
  not_ok 'clone rejects malicious alias escape metadata'
fi

mkdir "${ROOT}/work/project"
cat > "${ROOT}/work/project/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "hello"
token = "./.symbol-token"
content_revision = 1
tree_hash = "blake3:old"

[files]
"index.html" = "blake3:old"
MANIFEST
printf 'manifest-token\n' > "${ROOT}/work/project/.symbol-token"
printf '<h1>project</h1>\n' > "${ROOT}/work/project/index.html"
: > "${LOG}"
(cd "${ROOT}/work/project" && "${CLIENT}" put >/dev/null)
log=$(cat "${LOG}")
contains "${log}" 'Authorization: Bearer manifest-token' &&
  contains "${log}" 'ARCHIVE=./index.html' &&
  ! contains "${log}" 'ARCHIVE=./symbol.toml' &&
  ! contains "${log}" 'ARCHIVE=./.symbol-token' &&
  contains "$(cat "${ROOT}/work/project/symbol.toml")" 'token = "./.symbol-token"' &&
  ! contains "$(cat "${ROOT}/work/project/symbol.toml")" 'claim =' &&
  [ ! -e "${ROOT}/work/project/.symbol-claim" ] &&
  ok 'manifest put token and upload exclusions' || not_ok 'manifest put token and upload exclusions'

: > "${LOG}"
(cd "${ROOT}/work/project" &&
  "${CLIENT}" alias hello linked index.html >/dev/null)
log=$(cat "${LOG}")
contains "${log}" 'METHOD=ALIAS URL=http://mock/hello/linked' &&
  contains "${log}" 'Authorization: Bearer manifest-token' &&
  contains "${log}" 'If-Match: blake3:new' &&
  contains "$(cat "${ROOT}/work/project/symbol.toml")" \
    'token = "./.symbol-token"' &&
  ok 'alias uses checkout token and If-Match baseline' ||
  not_ok 'alias uses checkout token and If-Match baseline'

mkdir "${ROOT}/work/failing-project"
cat >"${ROOT}/work/failing-project/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "failing"
content_revision = 1
tree_hash = "blake3:old"

[files]
MANIFEST
printf 'failure\n' >"${ROOT}/work/failing-project/file.txt"
if (cd "${ROOT}/work/failing-project" &&
  MOCK_HTTP_ERROR_METHOD=PUT "${CLIENT}" put >/dev/null 2>&1); then
  not_ok 'failed put should fail'
elif [ ! -e "${ROOT}/work/failing-project/.symbol-claim" ] &&
  ! contains "$(cat "${ROOT}/work/failing-project/symbol.toml")" 'claim ='; then
  ok 'failed put removes prewritten claim sidecar'
else
  not_ok 'failed put removes prewritten claim sidecar'
fi

mkdir -p "${ROOT}/work/sync-rooted/assets"
cat > "${ROOT}/work/sync-rooted/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "rooted"
content_revision = 1
tree_hash = "blake3:base"

[files]
"assets/file.txt" = "blake3:old"
MANIFEST
printf 'updated\n' > "${ROOT}/work/sync-rooted/assets/file.txt"
: > "${LOG}"
(cd "${ROOT}/work/sync-rooted" &&
  MOCK_COMMON_ROOT_INVENTORY=1 "${CLIENT}" sync >/dev/null)
rooted_log=$(cat "${LOG}")
contains "${rooted_log}" 'ARCHIVE=./assets/file.txt' &&
  contains "${rooted_log}" 'ARCHIVE=./symbol.toml' &&
  ! contains "${rooted_log}" 'ARCHIVE=./file.txt' &&
  ok 'sync anchors a partial archive with one shared file root' ||
  not_ok 'sync anchors a partial archive with one shared file root'

mkdir -p "${ROOT}/work/sync-alias-root/assets" \
  "${ROOT}/work/sync-alias-root/links"
cat > "${ROOT}/work/sync-alias-root/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "alias-root"
content_revision = 1
tree_hash = "blake3:base"

[files]
"assets/file.txt" = "blake3:local"
MANIFEST
printf 'target\n' > "${ROOT}/work/sync-alias-root/assets/file.txt"
ln -s ../assets/file.txt "${ROOT}/work/sync-alias-root/links/current"
ln -s ../assets/file.txt "${ROOT}/work/sync-alias-root/links/next"
: > "${LOG}"
(cd "${ROOT}/work/sync-alias-root" &&
  MOCK_ALIAS_ROOT_INVENTORY=1 "${CLIENT}" sync >/dev/null)
alias_root_log=$(cat "${LOG}")
contains "${alias_root_log}" 'ARCHIVE=./links/current' &&
  contains "${alias_root_log}" 'ARCHIVE=./links/next' &&
  ! contains "${alias_root_log}" 'ARCHIVE=./current' &&
  ! contains "${alias_root_log}" 'ARCHIVE=./next' &&
  ok 'alias-only common root keeps full alias paths' ||
  not_ok 'alias-only common root keeps full alias paths'

mkdir "${ROOT}/work/sync-drop"
cat > "${ROOT}/work/sync-drop/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "hello"
content_revision = 1
tree_hash = "blake3:base"

[files]
"index.html" = "blake3:local"
MANIFEST
printf 'index\n' > "${ROOT}/work/sync-drop/index.html"
printf 'drop\n' > "${ROOT}/work/sync-drop/drop.txt"
rm -f "${MOCK_CURL_STATE}/dropped-PUT"
: > "${LOG}"
out=$(cd "${ROOT}/work/sync-drop" &&
  MOCK_DROP_ONCE_METHOD=PUT "${CLIENT}" sync)
sync_puts=$(awk '$0=="METHOD=PUT URL=http://mock/hello"{n++} END{print n+0}' \
  "${LOG}")
sync_keys=$(awk -F ': ' '$1=="Idempotency-Key"{print $2}' "${LOG}" |
  LC_ALL=C sort -u | awk 'END{print NR+0}')
sync_matches=$(awk '$0=="If-Match: blake3:base"{n++} END{print n+0}' \
  "${LOG}")
[ "${sync_puts}" -eq 2 ] &&
  [ "${sync_keys}" -eq 1 ] &&
  [ "${sync_matches}" -eq 2 ] &&
  contains "${out}" 'synced http://mock/hello/' &&
  ok 'dropped sync replays supported site PUT key with If-Match' ||
  not_ok 'dropped sync replays supported site PUT key with If-Match'

mkdir "${ROOT}/work/sync"
cat > "${ROOT}/work/sync/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "hello"
content_revision = 1
tree_hash = "blake3:base"

[files]
"index.html" = "blake3:local"
MANIFEST
printf 'index\n' > "${ROOT}/work/sync/index.html"
printf 'about\n' > "${ROOT}/work/sync/about.html"
: > "${LOG}"
out=$(cd "${ROOT}/work/sync" && "${CLIENT}" sync --check)
contains "${out}" 'would sync:' &&
  contains "${out}" '+ about.html' &&
  ! contains "$(cat "${LOG}")" 'METHOD=PUT' &&
  ok 'sync check previews without writing' || not_ok 'sync check previews without writing'

awk '{if ($0 ~ /^tree_hash[[:space:]]*=/) print "tree_hash = \"blake3:stale\""; else print}' \
  "${ROOT}/work/sync/symbol.toml" > "${ROOT}/work/sync/manifest.tmp"
mv "${ROOT}/work/sync/manifest.tmp" "${ROOT}/work/sync/symbol.toml"
: > "${LOG}"
sync_status=0
(cd "${ROOT}/work/sync" && "${CLIENT}" sync >"${ROOT}/sync.out" 2>"${ROOT}/sync.err") || sync_status=$?
[ "${sync_status}" -ne 0 ] &&
  contains "$(cat "${ROOT}/sync.err")" 'upstream changed since this checkout' &&
  contains "$(cat "${ROOT}/sync.err")" 'local changes:' &&
  contains "$(cat "${ROOT}/sync.err")" 'upstream changes:' &&
  contains "$(cat "${ROOT}/sync.err")" 'symbol clone hello ../hello-upstream' &&
  ! contains "$(cat "${LOG}")" 'METHOD=PUT' &&
  ok 'sync drift aborts without writing' || not_ok 'sync drift aborts without writing'

printf '1..%d\n' "${tests}"
[ "${failures}" -eq 0 ]
