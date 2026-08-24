#!/bin/sh
set -eu

CLIENT=${CLIENT:-$(dirname "$0")/../static/symbol.sh}
CLIENT=$(cd "$(dirname "$CLIENT")" && pwd)/$(basename "$CLIENT")
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT HUP INT TERM
mkdir "$ROOT/bin" "$ROOT/work"
cp "$CLIENT" "$ROOT/bin/symbol-client"
CLIENT=$ROOT/bin/symbol-client
printf 'ok\n' > "$ROOT/bin/.symbol.blake3"
LOG=$ROOT/curl.log
export MOCK_CURL_LOG=$LOG
export MOCK_CURL_STATE=$ROOT/curl-state
mkdir "$MOCK_CURL_STATE"

cat > "$ROOT/bin/curl" <<'MOCK'
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
  if [ -n "$upload" ] &&
    printf '%s\n' "$headers" | awk '$0=="Unpack: 1"{found=1} END{exit !found}'; then
    tar -tzf "$upload" | sed 's/^/ARCHIVE=/'
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
  PUT:http://mock|PUT:*/) status=201; location=http://mock/abcd/; body='created http://mock/abcd/' ;;
  PUT:*) body=updated ;;
  GET:*video.mp4/EXPIRES)
    body='{"target":{"site":"hello","path":"video.mp4","kind":"file"},"size":5,"refreshed_at":"2026-01-01T00:00:00Z","own_policy":{"mode":"relative","retention_seconds":100,"expires_at":"2027-01-01T00:00:00Z"},"inherited_caps":[{"kind":"site","path":null,"expires_at":"2026-12-01T00:00:00Z"}],"effective_expires_at":"2026-12-01T00:00:00Z","remaining_seconds":50,"limited_by":{"kind":"site","path":null}}'
    ;;
  EXPIRE:*|GET:*/EXPIRES)
    body='{"site":"hello","entries":[{"target":{"site":"hello","path":null,"kind":"site"},"size":10,"own_policy":{"mode":"decay"},"inherited_caps":[],"effective_expires_at":"2027-01-01T00:00:00Z","remaining_seconds":100,"limited_by":null},{"target":{"site":"hello","path":"video.mp4","kind":"file"},"size":5,"own_policy":{"mode":"relative"},"inherited_caps":[{"kind":"site","path":null,"expires_at":"2027-01-01T00:00:00Z"}],"effective_expires_at":"2027-01-01T00:00:00Z","remaining_seconds":50,"limited_by":{"kind":"site","path":null}}]}'
    ;;
  GET:*/UNDO)
    body='{"site":"hello","entries":[{"token":"tok1","description":"restore file","expires_at":"2027-01-01T00:00:00Z","remaining_seconds":100}]}'
    ;;
  GET:*/symbol.toml)
    body='version = 1
host = "http://mock"
name = "hello"
content_revision = 2
tree_hash = "blake3:new"

[files]
"index.html" = "blake3:file"'
    ;;
  GET:*/FILES)
    body='{"site":"hello","content_revision":1,"tree_hash":"blake3:base","files":[{"path":"index.html","hash":"blake3:local","size":10}]}'
    ;;
  GET:*.tar.gz|DELETE:*.tar.gz) body=ARCHIVE-BYTES ;;
  DELETE:*/whole) body=ARCHIVE-BYTES ;;
  DELETE:*) body=deleted ;;
  MANAGE:*) body=managed ;;
esac

if [ "${MOCK_HTTP_ERROR_METHOD:-}" = "$method" ]; then
  status=400
  body='error: forced failure'
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
      PUT|DELETE|COPY|MOVE|EXPIRE)
        printf 'Undo-Token: undo1\r\nUndo-Expires: 2027-01-01T00:00:00Z\r\n'
        ;;
    esac
    printf '\r\n'
  } > "$dump"
fi
if [ -n "$output" ]; then printf '%s' "$body" > "$output"; else printf '%s' "$body"; fi
[ -z "$write" ] || printf '%s' "$status"
MOCK
chmod +x "$ROOT/bin/curl"
cat > "$ROOT/bin/b3sum" <<'MOCK'
#!/bin/sh
printf 'local  %s\n' "$1"
MOCK
chmod +x "$ROOT/bin/b3sum"

PATH="$ROOT/bin:$PATH"
export PATH SYMBOL_HOST=http://mock XDG_STATE_HOME="$ROOT/state"
failures=0 tests=0
ok() { tests=$((tests + 1)); printf 'ok %d - %s\n' "$tests" "$1"; }
not_ok() { tests=$((tests + 1)); failures=$((failures + 1)); printf 'not ok %d - %s\n' "$tests" "$1"; }
contains() { printf '%s' "$1" | awk -v wanted="$2" 'index($0,wanted){found=1} END{exit !found}'; }

out=$("$CLIENT" help)
contains "$out" 'symbol sync [--check]' && ok 'canonical help' || not_ok 'canonical help'

if "$CLIENT" p >"$ROOT/out" 2>"$ROOT/err"; then
  not_ok 'ambiguous prefix exits nonzero'
elif contains "$(cat "$ROOT/err")" "ambiguous command 'p': put, pop, pull (clone)"; then
  ok 'identity-collapsed ambiguity'
else
  not_ok 'identity-collapsed ambiguity'
fi

registry=$(SYMBOL_TEST_COMMAND_REGISTRY=1 "$CLIENT")
registry_ok=1
printf '%s\n' "$registry" | while read -r canonical spellings; do
  [ -n "$canonical" ] || continue
  for spelling in $spellings; do
    resolved=$(SYMBOL_TEST_RESOLVE_ONLY=1 "$CLIENT" "$spelling") || exit 1
    [ "$resolved" = "$canonical" ] || exit 1
  done
done || registry_ok=0
[ "$registry_ok" -eq 1 ] &&
  ok 'all canonical commands and aliases resolve exactly' ||
  not_ok 'all canonical commands and aliases resolve exactly'

abbreviation_ok=1
for pair in 'l:ls' 'del:rm' 'cl:clone' 'co:copy' 'ren:move' 'sy:sync' 'x:remix' 'own:get'; do
  spelling=${pair%%:*}
  canonical=${pair#*:}
  resolved=$(SYMBOL_TEST_RESOLVE_ONLY=1 "$CLIENT" "$spelling") || abbreviation_ok=0
  [ "$resolved" = "$canonical" ] || abbreviation_ok=0
done
[ "$abbreviation_ok" -eq 1 ] &&
  ok 'prefix and substring abbreviations resolve to expected identities' ||
  not_ok 'prefix and substring abbreviations resolve to expected identities'

: > "$LOG"
out=$("$CLIENT" co hello target)
contains "$(cat "$LOG")" 'METHOD=COPY URL=http://mock/hello' &&
  contains "$(cat "$LOG")" 'Destination: /target' &&
  contains "$out" 'copied http://mock/hello/ -> http://mock/target/' &&
  contains "$out" 'undo within 4h: symbol undo target undo1' &&
  ok 'copy uses COPY and Destination' || not_ok 'copy uses COPY and Destination'

: > "$LOG"
"$CLIENT" ren old new >/dev/null
contains "$(cat "$LOG")" 'METHOD=MOVE URL=http://mock/old' &&
  contains "$(cat "$LOG")" 'Destination: /new' &&
  ok 'rename alias resolves to MOVE' || not_ok 'rename alias resolves to MOVE'

: > "$LOG"
out=$("$CLIENT" get hello -)
[ "$out" = ARCHIVE-BYTES ] &&
  contains "$(cat "$LOG")" 'METHOD=GET URL=http://mock/hello.tar.gz' &&
  ok 'get dash keeps stdout binary-only' || not_ok 'get dash keeps stdout binary-only'

: > "$LOG"
printf '<h1>x</h1>' | "$CLIENT" -t explicit put >/dev/null
log=$(cat "$LOG")
contains "$log" 'METHOD=PUT URL=http://mock/' &&
  contains "$log" 'UPLOAD=-' &&
  contains "$log" 'Authorization: Bearer explicit' &&
  contains "$log" 'Unpack: 1' &&
  contains "$log" 'ARCHIVE=./index.html' &&
  ok 'stdin put and explicit token' || not_ok 'stdin put and explicit token'

: > "$LOG"
printf '<h1>explicit</h1>\n' | "$CLIENT" put - >/dev/null
contains "$(cat "$LOG")" 'METHOD=PUT URL=http://mock/' &&
  contains "$(cat "$LOG")" 'UPLOAD=-' &&
  contains "$(cat "$LOG")" 'ARCHIVE=./index.html' &&
  ok 'explicit stdin dash publishes random index' ||
  not_ok 'explicit stdin dash publishes random index'

: > "$LOG"
printf '<h1>implicit</h1>\n' | "$CLIENT" put >/dev/null
contains "$(cat "$LOG")" 'METHOD=PUT URL=http://mock/' &&
  contains "$(cat "$LOG")" 'UPLOAD=-' &&
  contains "$(cat "$LOG")" 'ARCHIVE=./index.html' &&
  ok 'implicit piped stdin publishes random index' ||
  not_ok 'implicit piped stdin publishes random index'

: > "$LOG"
printf '<h1>slash</h1>\n' |
  SYMBOL_HOST=http://mock/ "$CLIENT" put - >/dev/null
contains "$(cat "$LOG")" 'METHOD=PUT URL=http://mock/' &&
  ! contains "$(cat "$LOG")" 'URL=http://mock//' &&
  ok 'trailing host slash is normalized for stdin put' ||
  not_ok 'trailing host slash is normalized for stdin put'

rm -f "$MOCK_CURL_STATE/dropped-PUT" "$MOCK_CURL_STATE/missing-pending-PUT"
out=$(printf '<h1>dropped</h1>\n' | MOCK_DROP_ONCE_METHOD=PUT "$CLIENT" put)
contains "$out" 'created http://mock/abcd/' &&
  [ -s "$XDG_STATE_HOME/symbol/claims/abcd" ] &&
  [ ! -f "$MOCK_CURL_STATE/missing-pending-PUT" ] &&
  ! ls "$XDG_STATE_HOME/symbol/claims"/pending-* >/dev/null 2>&1 &&
  ok 'dropped PUT response retries with pre-persisted claim' ||
  not_ok 'dropped PUT response retries with pre-persisted claim'

rm -f "$MOCK_CURL_STATE/dropped-COPY" "$MOCK_CURL_STATE/missing-pending-COPY"
out=$(MOCK_DROP_ONCE_METHOD=COPY "$CLIENT" copy hello)
contains "$out" 'copied http://mock/hello/ -> http://mock/wxyz/' &&
  [ -s "$XDG_STATE_HOME/symbol/claims/wxyz" ] &&
  [ ! -f "$MOCK_CURL_STATE/missing-pending-COPY" ] &&
  ! ls "$XDG_STATE_HOME/symbol/claims"/pending-* >/dev/null 2>&1 &&
  ok 'dropped COPY response retries with pre-persisted claim' ||
  not_ok 'dropped COPY response retries with pre-persisted claim'

rm -f "$MOCK_CURL_STATE/dropped-COPY" "$MOCK_CURL_STATE/missing-pending-COPY"
out=$(MOCK_DROP_ONCE_METHOD=COPY "$CLIENT" copy hello target)
contains "$out" 'copied http://mock/hello/ -> http://mock/target/' &&
  [ -s "$XDG_STATE_HOME/symbol/claims/target" ] &&
  [ ! -f "$MOCK_CURL_STATE/missing-pending-COPY" ] &&
  ok 'explicit COPY persists destination claim before dropped response' ||
  not_ok 'explicit COPY persists destination claim before dropped response'

out=$("$CLIENT" expire)
contains "$out" 'expiration is disabled until explicitly enabled.' &&
  contains "$out" 'default retention' &&
  ok 'bare expire is local help' || not_ok 'bare expire is local help'

: > "$LOG"
out=$("$CLIENT" expire hello --show)
contains "$(cat "$LOG")" 'METHOD=GET URL=http://mock/hello/EXPIRES' &&
  contains "$out" 'hello/' &&
  contains "$out" 'hello/video.mp4' &&
  contains "$out" '2027-01-01T00:00:00Z (in 1m 40s)' &&
  contains "$out" 'site' &&
  ok 'expire show report' || not_ok 'expire show report'

: > "$LOG"
out=$("$CLIENT" expire hello video.mp4 --show)
contains "$out" 'effective lifetime: hello/video.mp4' &&
  contains "$out" 'inherited cap:' &&
  contains "$out" 'limited by:         site (site)' &&
  ok 'target expiry report renders inherited limiting policy' ||
  not_ok 'target expiry report renders inherited limiting policy'

: > "$LOG"
out=$("$CLIENT" undo --stack hello)
contains "$out" 'TOKEN      WOULD UNDO' &&
  contains "$out" '2027-01-01 00:00Z (in 1m 40s)' &&
  ok 'undo stack uses canonical compact UTC and duration output' ||
  not_ok 'undo stack uses canonical compact UTC and duration output'

: > "$LOG"
rm_status=0
"$CLIENT" rm hello old.css >"$ROOT/rm.out" 2>"$ROOT/rm.err" || rm_status=$?
[ "$rm_status" -eq 0 ] &&
  contains "$(cat "$LOG")" 'METHOD=DELETE URL=http://mock/hello/old.css' &&
  ok 'remote file deletion path' || not_ok 'remote file deletion path'

: > "$LOG"
out=$("$CLIENT" rm whole)
contains "$out" 'deleted whole' &&
  ! contains "$out" 'ARCHIVE-BYTES' &&
  ok 'whole-site rm discards archive bytes' || not_ok 'whole-site rm discards archive bytes'

mkdir "$ROOT/work/project"
cat > "$ROOT/work/project/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "hello"
token = "./.symbol-token"
content_revision = 1
tree_hash = "blake3:old"

[files]
"index.html" = "blake3:old"
MANIFEST
printf 'manifest-token\n' > "$ROOT/work/project/.symbol-token"
printf '<h1>project</h1>\n' > "$ROOT/work/project/index.html"
: > "$LOG"
(cd "$ROOT/work/project" && "$CLIENT" put >/dev/null)
log=$(cat "$LOG")
contains "$log" 'Authorization: Bearer manifest-token' &&
  contains "$log" 'ARCHIVE=./index.html' &&
  ! contains "$log" 'ARCHIVE=./symbol.toml' &&
  ! contains "$log" 'ARCHIVE=./.symbol-token' &&
  contains "$(cat "$ROOT/work/project/symbol.toml")" 'token = "./.symbol-token"' &&
  ! contains "$(cat "$ROOT/work/project/symbol.toml")" 'claim =' &&
  [ ! -e "$ROOT/work/project/.symbol-claim" ] &&
  ok 'manifest put token and upload exclusions' || not_ok 'manifest put token and upload exclusions'

mkdir "$ROOT/work/failing-project"
cat >"$ROOT/work/failing-project/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "failing"
content_revision = 1
tree_hash = "blake3:old"

[files]
MANIFEST
printf 'failure\n' >"$ROOT/work/failing-project/file.txt"
if (cd "$ROOT/work/failing-project" &&
  MOCK_HTTP_ERROR_METHOD=PUT "$CLIENT" put >/dev/null 2>&1); then
  not_ok 'failed put should fail'
elif [ ! -e "$ROOT/work/failing-project/.symbol-claim" ] &&
  ! contains "$(cat "$ROOT/work/failing-project/symbol.toml")" 'claim ='; then
  ok 'failed put removes prewritten claim sidecar'
else
  not_ok 'failed put removes prewritten claim sidecar'
fi

mkdir "$ROOT/work/sync"
cat > "$ROOT/work/sync/symbol.toml" <<'MANIFEST'
version = 1
host = "http://mock"
name = "hello"
content_revision = 1
tree_hash = "blake3:base"

[files]
"index.html" = "blake3:local"
MANIFEST
printf 'index\n' > "$ROOT/work/sync/index.html"
printf 'about\n' > "$ROOT/work/sync/about.html"
: > "$LOG"
out=$(cd "$ROOT/work/sync" && "$CLIENT" sync --check)
contains "$out" 'would sync:' &&
  contains "$out" '+ about.html' &&
  ! contains "$(cat "$LOG")" 'METHOD=PUT' &&
  ok 'sync check previews without writing' || not_ok 'sync check previews without writing'

awk '{if ($0 ~ /^tree_hash[[:space:]]*=/) print "tree_hash = \"blake3:stale\""; else print}' \
  "$ROOT/work/sync/symbol.toml" > "$ROOT/work/sync/manifest.tmp"
mv "$ROOT/work/sync/manifest.tmp" "$ROOT/work/sync/symbol.toml"
: > "$LOG"
sync_status=0
(cd "$ROOT/work/sync" && "$CLIENT" sync >"$ROOT/sync.out" 2>"$ROOT/sync.err") || sync_status=$?
[ "$sync_status" -ne 0 ] &&
  contains "$(cat "$ROOT/sync.err")" 'upstream changed since this checkout' &&
  contains "$(cat "$ROOT/sync.err")" 'local changes:' &&
  contains "$(cat "$ROOT/sync.err")" 'upstream changes:' &&
  contains "$(cat "$ROOT/sync.err")" 'symbol clone hello ../hello-upstream' &&
  ! contains "$(cat "$LOG")" 'METHOD=PUT' &&
  ok 'sync drift aborts without writing' || not_ok 'sync drift aborts without writing'

printf '1..%d\n' "$tests"
[ "$failures" -eq 0 ]
