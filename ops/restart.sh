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
backup_root=${SYMBOL_BACKUP_ROOT:-${data_root}/backups}
running_exe=${SYMBOL_RUNNING_EXE:-/proc/${main_pid}/exe}
deploy_binary=${SYMBOL_DEPLOY_BINARY:-target/release/symbol}
backup_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
backup_dir=${backup_root}/${backup_id}
sudo mkdir -m 0700 -p "${backup_dir}"
sudo cp --reflink=auto --preserve=mode,timestamps \
  "${running_exe}" "${backup_dir}/symbol"
for database_file in \
  "${data_root}/symbol.db" \
  "${data_root}/symbol.db-wal" \
  "${data_root}/symbol.db-shm"
do
  [ ! -e "${database_file}" ] ||
    sudo cp --reflink=auto --preserve=mode,timestamps \
      "${database_file}" "${backup_dir}/"
done
if [ -d "${data_root}/blobs" ]; then
  find "${data_root}/blobs" -type f -printf '%P\t%s\n' |
    LC_ALL=C sort >"${backup_inventory}"
fi
sudo cp "${backup_inventory}" "${backup_dir}/blob-files.tsv"
printf 'symbol backup: %s\n' "${backup_dir}"

sudo install -m 0644 ops/symbol.service /etc/systemd/system/symbol.service
sudo systemctl daemon-reload
if sudo systemctl restart symbol &&
  sudo systemctl is-active --quiet symbol; then
  paused=0
else
  printf 'new Symbol failed to start; restoring backup\n' >&2
  sudo systemctl stop symbol >/dev/null 2>&1 || :
  sudo cp "${backup_dir}/symbol" "${deploy_binary}"
  sudo rm -f \
    "${data_root}/symbol.db" \
    "${data_root}/symbol.db-wal" \
    "${data_root}/symbol.db-shm"
  for database_file in \
    "${backup_dir}/symbol.db" \
    "${backup_dir}/symbol.db-wal" \
    "${backup_dir}/symbol.db-shm"
  do
    [ ! -e "${database_file}" ] ||
      sudo cp "${database_file}" "${data_root}/"
  done
  sudo systemctl start symbol
  paused=0
  exit 1
fi
echo "symbol rebuilt and restarted"
