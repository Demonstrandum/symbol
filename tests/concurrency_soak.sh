#!/bin/sh
set -eu

ROOT=$(mktemp -d)
PORT=$((40000 + ($$ % 20000)))
BASE="http://127.0.0.1:${PORT}"
DURATION=${SYMBOL_SOAK_SECONDS:-30}
WORKERS=${SYMBOL_SOAK_WORKERS:-8}
SERVER=${SERVER:-target/release/symbol}
SERVER_PID=

cleanup() {
  if [ -n "${SERVER_PID:-}" ]; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  rm -rf "${ROOT}"
}
trap cleanup EXIT HUP INT TERM

SYMBOL_PUBLIC_URL="${BASE}" RUST_LOG=warn \
  "${SERVER}" --bind "127.0.0.1:${PORT}" --root "${ROOT}/server" \
  >"${ROOT}/server.log" 2>&1 &
SERVER_PID=$!

for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  curl -fsS "${BASE}/STATS" >/dev/null 2>&1 && break
  sleep 0.1
done
curl -fsS "${BASE}/STATS" >/dev/null

deadline=$(( $(date +%s) + DURATION ))
worker=1
while [ "${worker}" -le "${WORKERS}" ]; do
  (
    iteration=0
    while [ "$(date +%s)" -lt "${deadline}" ]; do
      path="worker-${worker}/value-${iteration}.txt"
      printf '%s:%s\n' "${worker}" "${iteration}" |
        curl -fsS -T - "${BASE}/soak/${path}" >/dev/null
      curl -fsS "${BASE}/soak/${path}" >/dev/null
      curl -fsS -H 'Accept: application/json' "${BASE}/soak/FILES" >/dev/null
      iteration=$((iteration + 1))
    done
  ) &
  worker=$((worker + 1))
done
wait

curl -fsS "${BASE}/STATS" |
  python3 -c 'import json,sys; data=json.load(sys.stdin); assert data["sites"] == 1; assert data["files"] > 0'
curl -fsS -X DELETE "${BASE}/soak" -o "${ROOT}/soak.tar.gz"
tar -tzf "${ROOT}/soak.tar.gz" >/dev/null

printf 'concurrency soak passed: %ss, %s workers\n' "${DURATION}" "${WORKERS}"
