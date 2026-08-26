#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
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
JOURNAL_ALL="${TMP}/journal-all"
JOURNAL_RECENT="${TMP}/journal-recent"
SLEEP_MARKER="${TMP}/sleep-reached"
PAUSE_MARKER="${TMP}/pause-reached"
RESUME_MARKER="${TMP}/resume-reached"
export \
  DEPLOY_MARKER \
  JOURNAL_ALL \
  JOURNAL_RECENT \
  SLEEP_MARKER \
  PAUSE_MARKER \
  RESUME_MARKER

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
    printf '0123456789abcdef0123456789abcdef\n'
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
: >"${DEPLOY_MARKER}"
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
[ ! -s "${source}" ] || /bin/cat "${source}"
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
  "${TMP}/journalctl" \
  "${TMP}/sleep"

run_restart_guard() {
  rm -f \
    "${DEPLOY_MARKER}" \
    "${SLEEP_MARKER}" \
    "${SLEEP_MARKER}.finished" \
    "${PAUSE_MARKER}" \
    "${RESUME_MARKER}"
  set +e
  GUARD_OUTPUT=$(
    PATH="${TMP}:${PATH}" \
      SYMBOL_PRODUCTION_PHASE=10 \
      SYMBOL_RESTART_QUIET_SECONDS=30 \
      SYMBOL_RESTART_WAIT_SECONDS="${1}" \
      JOURNAL_FAILURE="${JOURNAL_FAILURE:-0}" \
      PAUSE_ACTION="${PAUSE_ACTION:-}" \
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
  [ -e "${DEPLOY_MARKER}" ] ||
  {
    printf 'quiet journal did not permit restart:\n%s\n' "${GUARD_OUTPUT}" >&2
    exit 1
  }

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

case "$(/bin/cat "${ROOT}/ops/symbol.service")" in
  *"Environment=RUST_LOG=info,symbol::mutation=info"*"LogRateLimitIntervalSec=0"*) ;;
  *)
    printf 'service does not reliably retain mutation signals at info\n' >&2
    exit 1
    ;;
esac

printf 'production phase guard passed\n'
