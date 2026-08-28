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
invocation_id=$(
  sudo systemctl show --property=InvocationID --value symbol
)
main_pid=$(
  sudo systemctl show --property=MainPID --value symbol
)
case "${invocation_id}" in
  '' | *[!0123456789abcdefABCDEF]*)
    printf 'cannot inspect Symbol mutations: invalid service invocation ID\n' >&2
    exit 1
    ;;
esac
case "${main_pid}" in
  '' | 0 | *[!0123456789]*)
    printf 'cannot back up Symbol: invalid service main PID\n' >&2
    exit 1
    ;;
esac

journal_all=$(mktemp)
journal_recent=$(mktemp)
backup_inventory=$(mktemp)
paused=0
cleanup() {
  rm -f "${journal_all}" "${journal_recent}" "${backup_inventory}"
  if [ "${paused}" -eq 1 ]; then
    sudo systemctl kill --kill-who=main --signal=SIGCONT symbol \
      >/dev/null 2>&1 || :
  fi
}
trap cleanup EXIT HUP INT TERM

inspect_mutations() {
  if ! sudo journalctl --sync; then
    printf 'cannot inspect Symbol mutation journal: journal sync failed\n' >&2
    return 12
  fi
  if ! journal_output=$(sudo journalctl \
    --unit=symbol \
    "_SYSTEMD_INVOCATION_ID=${invocation_id}" \
    --no-pager \
    --output=cat); then
    printf 'cannot inspect Symbol mutation journal: full query failed\n' >&2
    return 12
  fi
  printf '%s\n' "${journal_output}" >"${journal_all}"
  if ! journal_output=$(sudo journalctl \
    --unit=symbol \
    "_SYSTEMD_INVOCATION_ID=${invocation_id}" \
    --since "${quiet_seconds} seconds ago" \
    --no-pager \
    --output=cat); then
    printf 'cannot inspect Symbol mutation journal: recent query failed\n' >&2
    return 12
  fi
  printf '%s\n' "${journal_output}" >"${journal_recent}"

  if awk '
    function mutation_id(    field) {
      for (field = 1; field <= NF; field += 1) {
        if ($field ~ /^mutation_id=[0-9]+$/) {
          sub(/^mutation_id=/, "", $field)
          return $field
        }
      }
      return ""
    }
    /symbol_mutation_start/ {
      id = mutation_id()
      if (id == "" || active[id]) {
        malformed = 1
      } else {
        active[id] = 1
      }
    }
    /symbol_mutation_finish/ {
      id = mutation_id()
      if (id == "" || !active[id]) {
        malformed = 1
      } else {
        delete active[id]
      }
    }
    /symbol_mutation_signals_ready/ {
      ready = 1
    }
    END {
      if (!ready) {
        exit 3
      }
      if (malformed) {
        exit 2
      }
      for (id in active) {
        exit 0
      }
      exit 1
    }
  ' "${journal_all}"; then
    active_status=0
  else
    active_status=$?
  fi
  case "${active_status}" in
    0) return 10 ;;
    1) ;;
    *)
      if [ "${active_status}" -eq 3 ]; then
        printf 'cannot inspect Symbol mutation journal: missing readiness signal\n' >&2
      else
        printf 'cannot inspect Symbol mutation journal: malformed signal sequence\n' >&2
      fi
      return 12
      ;;
  esac

  if awk '
    /symbol_mutation_(start|finish)/ {
      recent = 1
    }
    END { exit !recent }
  ' "${journal_recent}"; then
    return 11
  fi
  return 0
}

while :
do
  if inspect_mutations; then
    mutation_state=0
  else
    mutation_state=$?
  fi
  if [ "${mutation_state}" -eq 0 ]; then
    if [ "${paused}" -eq 0 ]; then
      if ! sudo systemctl kill --kill-who=main --signal=SIGSTOP symbol; then
        printf 'cannot pause Symbol while confirming the mutation window\n' >&2
        exit 1
      fi
      paused=1
      continue
    fi
    break
  fi
  if [ "${mutation_state}" -eq 12 ]; then
    exit 1
  fi
  if [ "${paused}" -eq 1 ]; then
    if ! sudo systemctl kill --kill-who=main --signal=SIGCONT symbol; then
      printf 'cannot resume Symbol after mutation-window check\n' >&2
      exit 1
    fi
    paused=0
  fi
  if [ "${waited}" -ge "${wait_seconds}" ]; then
    if [ "${mutation_state}" -eq 10 ]; then
      printf 'refusing to restart: active Symbol mutation\n' >&2
    else
      printf 'refusing to restart: recent Symbol mutation in the last %ss\n' \
        "${quiet_seconds}" >&2
    fi
    exit 1
  fi
  printf 'waiting for %ss without Symbol mutations before restart\n' \
    "${quiet_seconds}" >&2
  sleep 5
  waited=$((waited + 5))
done

data_root=${SYMBOL_DATA_ROOT:-/var/lib/symbol}
backup_root=${SYMBOL_BACKUP_ROOT:-/var/backups/symbol}
running_exe=${SYMBOL_RUNNING_EXE:-/proc/${main_pid}/exe}
deploy_binary=${SYMBOL_DEPLOY_BINARY:-target/release/symbol}
installed_unit=${SYMBOL_INSTALLED_UNIT:-/etc/systemd/system/symbol.service}
backup_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
backup_dir=${backup_root}/${backup_id}
sudo mkdir -m 0700 -p "${backup_dir}"
sudo cp --dereference --preserve=mode,timestamps,ownership --reflink=auto \
  "${running_exe}" "${backup_dir}/symbol"
if [ -e "${data_root}/symbol.db" ]; then
  sudo python3 - "${data_root}/symbol.db" "${backup_dir}/symbol.db" <<'PY'
import os
import sqlite3
import sys

source_path, destination_path = sys.argv[1:]
metadata = os.stat(source_path)
source = sqlite3.connect(f"file:{source_path}?mode=ro", uri=True)
destination = sqlite3.connect(destination_path)
with destination:
    source.backup(destination)
integrity = destination.execute("PRAGMA integrity_check").fetchone()
if integrity != ("ok",):
    raise SystemExit(f"backup integrity check failed: {integrity!r}")
destination.close()
source.close()
os.chown(destination_path, metadata.st_uid, metadata.st_gid)
os.chmod(destination_path, metadata.st_mode & 0o7777)
os.utime(destination_path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
PY
fi
if [ -d "${data_root}/blobs" ]; then
  sudo cp --archive --reflink=auto "${data_root}/blobs" "${backup_dir}/blobs"
  find "${data_root}/blobs" -type f -printf '%P\t%s\n' |
    LC_ALL=C sort >"${backup_inventory}"
else
  sudo touch "${backup_dir}/blobs.absent"
fi
sudo cp "${backup_inventory}" "${backup_dir}/blob-files.tsv"
if sudo test -e "${installed_unit}"; then
  sudo cp --archive "${installed_unit}" "${backup_dir}/symbol.service"
else
  sudo touch "${backup_dir}/symbol.service.absent"
fi
printf 'symbol backup: %s\n' "${backup_dir}"

wait_for_readiness() {
  attempts=${SYMBOL_STARTUP_ATTEMPTS:-180}
  attempt=1
  while ! stats=$(curl -fsS http://127.0.0.1:4340/STATS 2>/dev/null)
  do
    if [ "${attempt}" -ge "${attempts}" ]; then
      return 1
    fi
    attempt=$((attempt + 1))
    sleep 1
  done
  printf '%s' "${stats}" |
    python3 -c 'import json,sys; assert isinstance(json.load(sys.stdin), dict)' &&
    curl -fsS -o /dev/null http://127.0.0.1:4340/ &&
    curl -fsS -o /dev/null http://127.0.0.1:4340/API/
}

rollback() {
  printf 'new Symbol failed readiness; restoring backup\n' >&2
  sudo systemctl stop symbol >/dev/null 2>&1 || :
  sudo cp --archive "${backup_dir}/symbol" "${deploy_binary}"
  sudo rm -f \
    "${data_root}/symbol.db" \
    "${data_root}/symbol.db-wal" \
    "${data_root}/symbol.db-shm"
  if sudo test -e "${backup_dir}/symbol.db"; then
    sudo cp --archive "${backup_dir}/symbol.db" "${data_root}/symbol.db"
  fi
  if sudo test -d "${backup_dir}/blobs"; then
    sudo rm -rf "${data_root}/blobs"
    sudo cp --archive "${backup_dir}/blobs" "${data_root}/blobs"
  elif sudo test -e "${backup_dir}/blobs.absent"; then
    sudo rm -rf "${data_root}/blobs"
  fi
  if sudo test -e "${backup_dir}/symbol.service"; then
    sudo cp --archive "${backup_dir}/symbol.service" "${installed_unit}"
    sudo systemctl daemon-reload
  elif sudo test -e "${backup_dir}/symbol.service.absent"; then
    sudo rm -f "${installed_unit}"
    sudo systemctl daemon-reload
    paused=0
    exit 1
  fi
  if ! sudo systemctl start symbol ||
    ! sudo systemctl is-active --quiet symbol ||
    ! wait_for_readiness; then
    paused=0
    printf 'restored Symbol service failed readiness checks\n' >&2
    exit 2
  fi
  paused=0
  exit 1
}

sudo install -m 0644 ops/symbol.service "${installed_unit}"
sudo systemctl daemon-reload
if sudo systemctl restart symbol &&
  sudo systemctl is-active --quiet symbol; then
  paused=0
else
  rollback
fi
if ! wait_for_readiness; then
  printf 'new Symbol failed readiness checks\n' >&2
  rollback
fi
echo "symbol rebuilt and restarted"
