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

printf 'production phase guard passed\n'
