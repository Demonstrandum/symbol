#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
REAL_CAT=$(command -v cat)
REAL_CP=$(command -v cp)
export REAL_CAT REAL_CP
GUARD_MARKER="${TMP}/operation-reached"
export GUARD_MARKER
cat >"${TMP}/cargo" <<'EOF'
#!/bin/sh
: >"${GUARD_MARKER}"
printf 'cargo must not run before the production phase guard\n' >&2
exit 99
EOF
chmod +x "${TMP}/cargo"

for phase in '' 0 9 phase-10 010; do
  rm -f "${GUARD_MARKER}"
  set +e
  output=$(
    PATH="${TMP}:${PATH}" SYMBOL_PRODUCTION_PHASE="${phase}" \
      sh "${ROOT}/ops/restart.sh" 2>&1
  )
  status=$?
  set -e
  [ "${status}" -eq 64 ] ||
    {
      printf 'production guard returned %s for phase %s\n' \
        "${status}" "${phase:-<unset>}" >&2
      exit 1
    }
  [ ! -e "${GUARD_MARKER}" ] ||
    {
      printf 'production operation was reached for phase %s\n' \
        "${phase:-<unset>}" >&2
      exit 1
    }
  case "${output}" in
    *"production access is restricted to Phase 10"*) ;;
    *)
      printf 'production guard failed before Phase 10:\n%s\n' "${output}" >&2
      exit 1
      ;;
  esac
done

set +e
output=$(
  SYMBOL_DEPLOY_CHECK=1 SYMBOL_PRODUCTION_PHASE=9 \
    sh "${ROOT}/release-check" 2>&1
)
status=$?
set -e
[ "${status}" -eq 64 ] ||
  {
    printf 'release gate production guard returned %s\n' "${status}" >&2
    exit 1
  }
case "${output}" in
  *"production access is restricted to Phase 10"*) ;;
  *)
    printf 'release gate production guard failed:\n%s\n' "${output}" >&2
    exit 1
    ;;
esac

rm -f "${GUARD_MARKER}"
set +e
PATH="${TMP}:${PATH}" SYMBOL_PRODUCTION_PHASE=10 \
  sh "${ROOT}/ops/restart.sh" >/dev/null 2>&1
status=$?
set -e
[ "${status}" -eq 99 ] && [ -e "${GUARD_MARKER}" ] ||
  {
    printf 'Phase 10 did not dispatch to the guarded operation\n' >&2
    exit 1
  }

DEPLOY_MARKER="${TMP}/deploy-reached"
BACKUP_MARKER="${TMP}/backup-reached"
RESTORE_MARKER="${TMP}/restore-reached"
CURL_FAILURE_MARKER="${TMP}/curl-failure-reached"
JOURNAL_ALL="${TMP}/journal-all"
JOURNAL_RECENT="${TMP}/journal-recent"
SLEEP_MARKER="${TMP}/sleep-reached"
PAUSE_MARKER="${TMP}/pause-reached"
RESUME_MARKER="${TMP}/resume-reached"
START_MARKER="${TMP}/start-reached"
RUNNING_EXE="${TMP}/running-symbol"
RUNNING_EXE_TARGET="${TMP}/running-symbol-target"
DEPLOY_BINARY="${TMP}/deployed-symbol"
DATA_ROOT="${TMP}/data"
BACKUP_ROOT="${TMP}/backups"
INSTALLED_UNIT="${TMP}/installed-symbol.service"
export \
  BACKUP_MARKER \
  BACKUP_ROOT \
  CURL_FAILURE_MARKER \
  DATA_ROOT \
  DEPLOY_BINARY \
  DEPLOY_MARKER \
  JOURNAL_ALL \
  JOURNAL_RECENT \
  SLEEP_MARKER \
  PAUSE_MARKER \
  RESTORE_MARKER \
  RESUME_MARKER \
  START_MARKER
mkdir -p "${DATA_ROOT}/blobs"
printf 'old binary\n' >"${RUNNING_EXE_TARGET}"
ln -s "${RUNNING_EXE_TARGET}" "${RUNNING_EXE}"
printf 'new binary\n' >"${DEPLOY_BINARY}"
python3 - "${DATA_ROOT}/symbol.db" <<'PY'
import sqlite3
import sys

database = sqlite3.connect(sys.argv[1])
database.execute("CREATE TABLE fixture (value TEXT NOT NULL)")
database.execute("INSERT INTO fixture VALUES ('database')")
database.commit()
database.close()
PY
printf 'blob\n' >"${DATA_ROOT}/blobs/aa"
printf 'old unit\n' >"${INSTALLED_UNIT}"

cat >"${TMP}/cargo" <<'EOF'
#!/bin/sh
exit 0
EOF
cat >"${TMP}/sudo" <<'EOF'
#!/bin/sh
exec "$@"
EOF
cat >"${TMP}/systemctl" <<'EOF'
#!/bin/sh
case "$1" in
  show)
    case "$*" in
      *MainPID*) printf '12345\n' ;;
      *) printf '0123456789abcdef0123456789abcdef\n' ;;
    esac
    ;;
  daemon-reload)
    ;;
  kill)
    case "$*" in
      *"SIGSTOP"*)
        : >"${PAUSE_MARKER}"
        if [ "${PAUSE_ACTION:-}" = inject-active ]; then
          printf 'symbol_mutation_start mutation_id=71 method=PUT\n' \
            >>"${JOURNAL_ALL}"
          printf 'symbol_mutation_start mutation_id=71 method=PUT\n' \
            >>"${JOURNAL_RECENT}"
        fi
        ;;
      *"SIGCONT"*)
        : >"${RESUME_MARKER}"
        ;;
      *)
        printf 'unexpected systemctl kill: %s\n' "$*" >&2
        exit 91
        ;;
    esac
    ;;
  restart)
    : >"${DEPLOY_MARKER}"
    if [ "${RESTART_ACTION:-}" = create-blobs ]; then
      mkdir -p "${DATA_ROOT}/blobs"
      printf 'new blob\n' >"${DATA_ROOT}/blobs/new"
    fi
    [ "${RESTART_FAILURE:-0}" != 1 ] || exit 1
    ;;
  stop)
    ;;
  start)
    : >"${START_MARKER}"
    [ -e "${SYMBOL_INSTALLED_UNIT}" ] || exit 5
    ;;
  is-active)
    exit 0
    ;;
  *)
    printf 'unexpected systemctl command: %s\n' "$*" >&2
    exit 90
    ;;
esac
EOF
cat >"${TMP}/install" <<'EOF'
#!/bin/sh
[ -e "${BACKUP_MARKER}" ] || {
  printf 'deployment reached before backup\n' >&2
  exit 92
}
for destination do :; done
printf 'new unit\n' >"${destination}"
: >"${DEPLOY_MARKER}"
EOF
cat >"${TMP}/cp" <<'EOF'
#!/bin/sh
"${REAL_CP}" "$@"
case "$*" in
  *"${DEPLOY_BINARY}") : >"${RESTORE_MARKER}" ;;
  *) : >"${BACKUP_MARKER}" ;;
esac
EOF
cat >"${TMP}/curl" <<'EOF'
#!/bin/sh
case "${CURL_FAILURE:-0}" in
  once)
    if [ ! -e "${CURL_FAILURE_MARKER}" ]; then
      : >"${CURL_FAILURE_MARKER}"
      exit 22
    fi
    ;;
  always) exit 22 ;;
esac
case "$*" in
  *"/STATS"*) printf '{}\n' ;;
esac
EOF
cat >"${TMP}/journalctl" <<'EOF'
#!/bin/sh
if [ "${JOURNAL_FAILURE:-0}" = 1 ] ||
  {
    [ "${JOURNAL_FAILURE:-0}" = after-pause ] &&
      [ -e "${PAUSE_MARKER}" ]
  }; then
  printf 'injected journal failure\n' >&2
  exit 23
fi
[ "$1" != "--sync" ] || exit 0
case " $* " in
  *" --since "*) source=${JOURNAL_RECENT} ;;
  *) source=${JOURNAL_ALL} ;;
esac
[ ! -s "${source}" ] || "${REAL_CAT}" "${source}"
EOF
cat >"${TMP}/sleep" <<'EOF'
#!/bin/sh
: >"${SLEEP_MARKER}"
if [ "${SLEEP_ACTION:-}" = finish-active ] &&
  [ ! -e "${SLEEP_MARKER}.finished" ]; then
  printf 'symbol_mutation_finish mutation_id=41 method=PUT status=200\n' \
    >>"${JOURNAL_ALL}"
  : >"${JOURNAL_RECENT}"
  : >"${SLEEP_MARKER}.finished"
fi
EOF
chmod +x \
  "${TMP}/cargo" \
  "${TMP}/sudo" \
  "${TMP}/systemctl" \
  "${TMP}/install" \
  "${TMP}/cp" \
  "${TMP}/curl" \
  "${TMP}/journalctl" \
  "${TMP}/sleep"

run_restart_guard() {
  rm -f \
    "${BACKUP_MARKER}" \
    "${CURL_FAILURE_MARKER}" \
    "${DEPLOY_MARKER}" \
    "${SLEEP_MARKER}" \
    "${SLEEP_MARKER}.finished" \
    "${PAUSE_MARKER}" \
    "${RESTORE_MARKER}" \
    "${RESUME_MARKER}" \
    "${START_MARKER}"
  set +e
  GUARD_OUTPUT=$(
    PATH="${TMP}:${PATH}" \
      SYMBOL_BACKUP_ROOT="${BACKUP_ROOT}" \
      SYMBOL_DATA_ROOT="${DATA_ROOT}" \
      SYMBOL_DEPLOY_BINARY="${DEPLOY_BINARY}" \
      SYMBOL_INSTALLED_UNIT="${INSTALLED_UNIT}" \
      SYMBOL_PRODUCTION_PHASE=10 \
      SYMBOL_RESTART_QUIET_SECONDS=30 \
      SYMBOL_RESTART_WAIT_SECONDS="${1}" \
      SYMBOL_RUNNING_EXE="${RUNNING_EXE}" \
      JOURNAL_FAILURE="${JOURNAL_FAILURE:-0}" \
      CURL_FAILURE="${CURL_FAILURE:-0}" \
      PAUSE_ACTION="${PAUSE_ACTION:-}" \
      RESTART_ACTION="${RESTART_ACTION:-}" \
      RESTART_FAILURE="${RESTART_FAILURE:-0}" \
      SLEEP_ACTION="${SLEEP_ACTION:-}" \
      sh "${ROOT}/ops/restart.sh" 2>&1
  )
  GUARD_STATUS=$?
  set -e
}

assert_guard_refused() {
  [ "${GUARD_STATUS}" -eq 1 ] ||
    {
      printf 'quiet-window guard returned %s instead of 1:\n%s\n' \
        "${GUARD_STATUS}" "${GUARD_OUTPUT}" >&2
      exit 1
    }
  [ ! -e "${DEPLOY_MARKER}" ] ||
    {
      printf 'quiet-window guard reached deployment while blocked\n' >&2
      exit 1
    }
}

write_journal_all() {
  printf '%s\n' 'symbol_mutation_signals_ready' "$@" >"${JOURNAL_ALL}"
}

write_journal_all \
  'symbol_mutation_start mutation_id=7 method=PUT' \
  'symbol_mutation_finish mutation_id=7 method=PUT status=200'
: >"${JOURNAL_RECENT}"
run_restart_guard 0
[ "${GUARD_STATUS}" -eq 0 ] &&
  [ -e "${PAUSE_MARKER}" ] &&
  [ -e "${BACKUP_MARKER}" ] &&
  [ -e "${DEPLOY_MARKER}" ] ||
  {
    printf 'quiet journal did not permit restart:\n%s\n' "${GUARD_OUTPUT}" >&2
    exit 1
  }
set -- "${BACKUP_ROOT}"/*
[ -f "$1/blobs/aa" ] &&
  [ -f "$1/symbol.service" ] &&
  [ -f "$1/symbol" ] &&
  [ ! -L "$1/symbol" ] &&
  [ "$(cat "$1/symbol")" = 'old binary' ] ||
  {
    printf 'successful restart backup omitted blobs or service unit\n' >&2
    exit 1
  }

write_journal_all \
  'symbol_mutation_start mutation_id=8 method=PUT' \
  'symbol_mutation_finish mutation_id=8 method=PUT status=200'
: >"${JOURNAL_RECENT}"
printf 'new binary\n' >"${DEPLOY_BINARY}"
printf 'old unit\n' >"${INSTALLED_UNIT}"
RESTART_FAILURE=1
export RESTART_FAILURE
run_restart_guard 0
unset RESTART_FAILURE
[ "${GUARD_STATUS}" -eq 1 ] &&
  [ -e "${BACKUP_MARKER}" ] &&
  [ -e "${RESTORE_MARKER}" ] ||
  {
    printf 'failed restart did not restore its backup:\n%s\n' "${GUARD_OUTPUT}" >&2
    exit 1
  }

write_journal_all \
  'symbol_mutation_start mutation_id=10 method=PUT' \
  'symbol_mutation_finish mutation_id=10 method=PUT status=200'
: >"${JOURNAL_RECENT}"
rm -f "${INSTALLED_UNIT}"
printf 'new binary\n' >"${DEPLOY_BINARY}"
RESTART_FAILURE=1
export RESTART_FAILURE
run_restart_guard 0
unset RESTART_FAILURE
[ "${GUARD_STATUS}" -eq 1 ] &&
  [ ! -e "${INSTALLED_UNIT}" ] &&
  [ ! -e "${START_MARKER}" ] ||
  {
    printf 'first-install rollback left or started the new service unit:\n%s\n' \
      "${GUARD_OUTPUT}" >&2
    exit 1
  }
printf 'old unit\n' >"${INSTALLED_UNIT}"
[ "$(cat "${DEPLOY_BINARY}")" = 'old binary' ] &&
  [ "$(cat "${DATA_ROOT}/blobs/aa")" = 'blob' ] &&
  [ "$(cat "${INSTALLED_UNIT}")" = 'old unit' ] ||
  {
    printf 'rollback did not restore binary, blobs, and service unit\n' >&2
    exit 1
  }

write_journal_all \
  'symbol_mutation_start mutation_id=12 method=PUT' \
  'symbol_mutation_finish mutation_id=12 method=PUT status=200'
: >"${JOURNAL_RECENT}"
rm -rf "${DATA_ROOT}/blobs"
printf 'new binary\n' >"${DEPLOY_BINARY}"
RESTART_ACTION=create-blobs
RESTART_FAILURE=1
export RESTART_ACTION RESTART_FAILURE
run_restart_guard 0
unset RESTART_ACTION RESTART_FAILURE
[ "${GUARD_STATUS}" -eq 1 ] && [ ! -e "${DATA_ROOT}/blobs" ] ||
  {
    printf 'rollback did not restore an absent blob directory:\n%s\n' \
      "${GUARD_OUTPUT}" >&2
    exit 1
  }
mkdir -p "${DATA_ROOT}/blobs"
printf 'blob\n' >"${DATA_ROOT}/blobs/aa"

write_journal_all \
  'symbol_mutation_start mutation_id=9 method=PUT' \
  'symbol_mutation_finish mutation_id=9 method=PUT status=200'
: >"${JOURNAL_RECENT}"
printf 'new binary\n' >"${DEPLOY_BINARY}"
printf 'old unit\n' >"${INSTALLED_UNIT}"
CURL_FAILURE=once
export CURL_FAILURE
SYMBOL_STARTUP_ATTEMPTS=1
export SYMBOL_STARTUP_ATTEMPTS
run_restart_guard 0
unset CURL_FAILURE SYMBOL_STARTUP_ATTEMPTS
[ "${GUARD_STATUS}" -eq 1 ] &&
  [ -e "${BACKUP_MARKER}" ] &&
  [ -e "${RESTORE_MARKER}" ] ||
  {
    printf 'readiness failure did not restore its backup:\n%s\n' "${GUARD_OUTPUT}" >&2
    exit 1
  }

write_journal_all \
  'symbol_mutation_start mutation_id=11 method=PUT' \
  'symbol_mutation_finish mutation_id=11 method=PUT status=200'
: >"${JOURNAL_RECENT}"
printf 'new binary\n' >"${DEPLOY_BINARY}"
CURL_FAILURE=always
export CURL_FAILURE
SYMBOL_STARTUP_ATTEMPTS=1
export SYMBOL_STARTUP_ATTEMPTS
run_restart_guard 0
unset CURL_FAILURE SYMBOL_STARTUP_ATTEMPTS
[ "${GUARD_STATUS}" -eq 2 ] &&
  [ -e "${RESTORE_MARKER}" ] ||
  {
    printf 'failed restored-service readiness was not diagnosed:\n%s\n' \
      "${GUARD_OUTPUT}" >&2
    exit 1
  }
case "${GUARD_OUTPUT}" in
  *"restored Symbol service failed readiness checks"*) ;;
  *)
    printf 'restored-service readiness failure lacked diagnosis:\n%s\n' \
      "${GUARD_OUTPUT}" >&2
    exit 1
    ;;
esac

printf '%s\n' \
  'symbol_mutation_start mutation_id=6 method=PUT' \
  'symbol_mutation_finish mutation_id=6 method=PUT status=200' \
  >"${JOURNAL_ALL}"
: >"${JOURNAL_RECENT}"
run_restart_guard 0
assert_guard_refused
case "${GUARD_OUTPUT}" in
  *"missing readiness signal"*) ;;
  *)
    printf 'missing mutation-signal capability was not diagnosed:\n%s\n' \
      "${GUARD_OUTPUT}" >&2
    exit 1
    ;;
esac

write_journal_all 'symbol_mutation_start mutation_id=41 method=PUT'
: >"${JOURNAL_RECENT}"
run_restart_guard 0
assert_guard_refused
case "${GUARD_OUTPUT}" in
  *"active Symbol mutation"*) ;;
  *)
    printf 'active mutation was not diagnosed:\n%s\n' "${GUARD_OUTPUT}" >&2
    exit 1
    ;;
esac

write_journal_all \
  'symbol_mutation_start mutation_id=51 method=PATCH' \
  'symbol_mutation_finish mutation_id=51 method=PATCH status=200'
printf '%s\n' \
  'symbol_mutation_finish mutation_id=51 method=PATCH status=200' \
  >"${JOURNAL_RECENT}"
run_restart_guard 0
assert_guard_refused
case "${GUARD_OUTPUT}" in
  *"recent Symbol mutation"*) ;;
  *)
    printf 'recent mutation was not diagnosed:\n%s\n' "${GUARD_OUTPUT}" >&2
    exit 1
    ;;
esac

write_journal_all \
  'symbol_mutation_start mutation_id=70 method=PUT' \
  'symbol_mutation_finish mutation_id=70 method=PUT status=200'
: >"${JOURNAL_RECENT}"
PAUSE_ACTION=inject-active
export PAUSE_ACTION
run_restart_guard 0
unset PAUSE_ACTION
assert_guard_refused
[ -e "${PAUSE_MARKER}" ] && [ -e "${RESUME_MARKER}" ] ||
  {
    printf 'guard did not resume after a mutation raced with pause\n' >&2
    exit 1
  }

write_journal_all 'symbol_mutation_start mutation_id=41 method=PUT'
: >"${JOURNAL_RECENT}"
SLEEP_ACTION=finish-active
export SLEEP_ACTION
run_restart_guard 5
unset SLEEP_ACTION
[ "${GUARD_STATUS}" -eq 0 ] &&
  [ -e "${SLEEP_MARKER}" ] &&
  [ -e "${DEPLOY_MARKER}" ] ||
  {
    printf 'guard did not pause for an active mutation:\n%s\n' \
      "${GUARD_OUTPUT}" >&2
    exit 1
  }

write_journal_all \
  'symbol_mutation_start mutation_id=61 method=DELETE' \
  'symbol_mutation_finish mutation_id=61 method=DELETE status=200'
: >"${JOURNAL_RECENT}"
JOURNAL_FAILURE=1
export JOURNAL_FAILURE
run_restart_guard 0
unset JOURNAL_FAILURE
assert_guard_refused
case "${GUARD_OUTPUT}" in
  *"cannot inspect Symbol mutation journal"*) ;;
  *)
    printf 'journal failure was not diagnosed:\n%s\n' "${GUARD_OUTPUT}" >&2
    exit 1
    ;;
esac

write_journal_all \
  'symbol_mutation_start mutation_id=62 method=DELETE' \
  'symbol_mutation_finish mutation_id=62 method=DELETE status=200'
: >"${JOURNAL_RECENT}"
JOURNAL_FAILURE=after-pause
export JOURNAL_FAILURE
run_restart_guard 0
unset JOURNAL_FAILURE
assert_guard_refused
[ -e "${PAUSE_MARKER}" ] && [ -e "${RESUME_MARKER}" ] ||
  {
    printf 'journal failure while paused did not resume Symbol\n' >&2
    exit 1
  }

case "$(cat "${ROOT}/ops/symbol.service")" in
  *"Environment=RUST_LOG=info,symbol::mutation=info"*"LogRateLimitIntervalSec=0"*) ;;
  *)
    printf 'service does not reliably retain mutation signals at info\n' >&2
    exit 1
    ;;
esac

printf 'production phase guard passed\n'
