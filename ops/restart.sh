#!/bin/sh
set -eu

if [ "${SYMBOL_PRODUCTION_PHASE:-}" != 10 ]; then
  printf 'production access is restricted to Phase 10\n' >&2
  exit 64
fi

cd "$(dirname "$0")/.."
cargo build --release --locked -p symbol

quiet_seconds=${SYMBOL_RESTART_QUIET_SECONDS:-30}
wait_seconds=${SYMBOL_RESTART_WAIT_SECONDS:-300}
waited=0
while sudo journalctl -u symbol --since "${quiet_seconds} seconds ago" --no-pager -o cat |
  awk '
    /http_request\{method=(PUT|DELETE|COPY|MOVE|UNDO|EXPIRE|MANAGE)/ {
      active = 1
    }
    END { exit !active }
  '
do
  if [ "${waited}" -ge "${wait_seconds}" ]; then
    printf 'refusing to restart: Symbol received mutations in the last %ss\n' \
      "${quiet_seconds}" >&2
    exit 1
  fi
  printf 'waiting for %ss without Symbol mutations before restart\n' \
    "${quiet_seconds}" >&2
  sleep 5
  waited=$((waited + 5))
done

sudo install -m 0644 ops/symbol.service /etc/systemd/system/symbol.service
sudo systemctl daemon-reload
sudo systemctl restart symbol
sudo systemctl is-active --quiet symbol
echo "symbol rebuilt and restarted"
