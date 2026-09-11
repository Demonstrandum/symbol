#!/bin/sh
# symbol: put a static site on the tailnet
# install: curl -fsSL __HOST__/install.sh | sh
set -eu

HOST="${SYMBOL_HOST:-__HOST__}"
HOST=${HOST%/}

UPDATE_PID=
UPDATE_NOTE=

client_path() {
  cmd=$0
  case "${cmd}" in
    /*)
      printf '%s\n' "${cmd}"
      return
      ;;
    */*)
      printf '%s\n' "$(pwd)/${cmd}"
      return
      ;;
  esac
  oldifs=${IFS}
  IFS=:
  for dir in ${PATH}; do
    if [ -n "${dir}" ] && [ -f "${dir}/${cmd}" ] && [ -x "${dir}/${cmd}" ]; then
      IFS=${oldifs}
      printf '%s\n' "${dir}/${cmd}"
      return
    fi
  done
  IFS=${oldifs}
  printf '%s\n' "${cmd}"
}

start_update_check() {
  case "${1:-}" in
    update|upgrade|-h|--help|help|"") return 0 ;;
  esac
  [ -z "${SYMBOL_NO_UPDATE_CHECK:-}" ] || return 0
  case "${HOST}" in
    http://*|https://*) ;;
    *) return 0 ;;
  esac
  nag_state=${XDG_STATE_HOME:-${HOME}/.local/state}/symbol/update-nag
  now=$(date +%s)
  if [ -f "${nag_state}" ]; then
    last=$(tr -d ' \t\r\n' < "${nag_state}")
    case "${last}" in
      ''|*[!0-9]*) ;;
      *) [ "$((now - last))" -lt 86400 ] && return 0 ;;
    esac
  fi
  hashfile=$(dirname "$(client_path)")/.symbol.blake3
  UPDATE_NOTE=$(mktemp) || return 0
  {
    mkdir -p "$(dirname "${nag_state}")" || exit 0
    printf '%s\n' "${now}" > "${nag_state}"
    if [ ! -f "${hashfile}" ]; then
      printf '\nsymbol client is missing its hash file. run:\n  symbol update\n' > "${UPDATE_NOTE}"
      exit 0
    fi
    remote=$(curl -fsS --max-time 2 "${HOST}/symbol.sh/HASH" 2>/dev/null || true)
    remote=$(printf '%s' "${remote}" | tr -d ' \t\r\n')
    [ -n "${remote}" ] || exit 0
    have=$(tr -d ' \t\r\n' < "${hashfile}")
    if [ "${remote}" != "${have}" ]; then
      printf '\nsymbol client is out of date. run:\n  symbol update\n' > "${UPDATE_NOTE}"
    fi
  } &
  UPDATE_PID=$!
}

join_update_check() {
  if [ -n "${UPDATE_PID:-}" ]; then
    wait "${UPDATE_PID}" 2>/dev/null || true
    UPDATE_PID=
  fi
  if [ -n "${UPDATE_NOTE:-}" ]; then
    if [ -s "${UPDATE_NOTE}" ]; then
      if update_check_color; then
        printf '\033[33m' >&2
        cat "${UPDATE_NOTE}" >&2
        printf '\033[0m' >&2
      else
        cat "${UPDATE_NOTE}" >&2
      fi
    fi
    rm -f "${UPDATE_NOTE}"
    UPDATE_NOTE=
  fi
}

update_check_color() {
  [ -z "${NO_COLOR:-}" ] || return 1
  [ "${CLICOLOR:-1}" != 0 ] || return 1
  [ -t 2 ] || return 1
  case "${TERM:-}" in
    ''|dumb) return 1 ;;
  esac
  return 0
}

usage() {
  cat <<EOF
symbol: static hosting on ${HOST}

usage:
  symbol put [-u] [--replace] [--managed] [NAME [FILE [DEST]]]
  symbol clone NAME [DIR]
  symbol get NAME [ARCHIVE]
  symbol pop NAME [ARCHIVE]
  symbol copy [--managed] SRC [DST]
  symbol remix [--managed] SRC [DST]
  symbol move SRC DST
  symbol alias SITE PATH TARGET [PATH TARGET ...]
  symbol sync [--check]
  symbol undo [--stack] [NAME [TOKEN]]
  symbol expire [NAME [PATH]] [POLICY]
  symbol manage [NAME ACTION]
  symbol recover
  symbol ls [-l] [--json] [NAME]
  symbol rm NAME [PATH]
  symbol url NAME
  symbol api [--json]
  symbol stats [--json]
  symbol update
  symbol help

global:
  -t, --token TOKEN   management token (also after the command, before --)
  --json              machine-readable output where the command supports it

put:
  -u, --unpack        unpack archives; directories are always unpacked
  -f PATH             force piped input's remote file path
  --replace           replace the site tree; remote paths not in the upload are removed
  --managed           create a managed site
  with no arguments, publish the nearest symbol.toml project

streams:
  - in a source position reads stdin
  - put reads a pipe without - only when a terminal is attached
    and no file source is given; SYMBOL_STDIN=always|never overrides
  - as get/pop output writes archive bytes to stdout

aliases:
  push -> put; pull -> clone; rename -> move; add -> put; x -> remix
  list -> ls; download -> get; delete -> rm; upgrade -> update

env: SYMBOL_HOST (default ${HOST}); SYMBOL_TOKEN
     SYMBOL_STDIN=tty|always|never (default tty)
     SYMBOL_NO_UPDATE_CHECK=1 skips the client update nag
EOF
}

command_help() {
  case "$1" in
    put) cat <<'EOF'
symbol put: publish or merge files into a site
usage: symbol put [-u|--unpack] [--replace] [--managed] [--json]
                  [NAME [FILE [DEST]]]
       command | symbol put [NAME] -
       command | symbol put            (terminal only)

puts merge into an existing site and keep remote paths you did not upload.
--replace replaces the whole site tree and deletes remote paths missing
from the upload. generated symbol.toml is kept either way.
there is no prompt and no confirmation.
stdin is read only when the source is - , or when no file source is given
and a terminal is attached (SYMBOL_STDIN=always|never overrides).
--json prints a mutation envelope instead of status lines.
EOF
      ;;
    clone) cat <<'EOF'
symbol clone: create a local checkout
usage: symbol clone NAME [DIR]

downloads the site, writes symbol.toml, and recreates aliases as symlinks
when the filesystem allows it.
EOF
      ;;
    get) cat <<'EOF'
symbol get: download a site without deleting it
usage: symbol get NAME [ARCHIVE|-]

writes a tar.gz, tar, or zip. - writes archive bytes to stdout.
EOF
      ;;
    pop) cat <<'EOF'
symbol pop: download and remove a site
usage: symbol pop NAME [ARCHIVE|-]

the site is removed only after the archive is produced. undo is retained
for 4 hours. - writes archive bytes to stdout.
EOF
      ;;
    copy) cat <<'EOF'
symbol copy: duplicate a site on the server
usage: symbol copy [--managed] [--json] SRC [DST]

reuses blob content. omit DST for a generated name.
--json prints a mutation envelope.
EOF
      ;;
    remix) cat <<'EOF'
symbol remix: duplicate a site and clone the copy locally
usage: symbol remix [--managed] SRC [DST]

if the clone fails, the server copy is kept and the cleanup command is
printed.
EOF
      ;;
    move) cat <<'EOF'
symbol move: rename a site without transferring files
usage: symbol move [--json] SRC DST

--json prints a mutation envelope.
EOF
      ;;
    alias) cat <<'EOF'
symbol alias: create live path aliases atomically
usage: symbol alias [--json] SITE PATH TARGET [PATH TARGET ...]

one pair is one alias. several pairs are one batch. --json prints the
server receipt.
EOF
      ;;
    api) cat <<'EOF'
symbol api: show the live server API version and build
usage: symbol api [--json]

--json writes the /API/VERSION document.
EOF
      ;;
    stats) cat <<'EOF'
symbol stats: show storage, deduplication, cache, and reader totals
usage: symbol stats [--json]

--json writes the /STATS document.
EOF
      ;;
    sync) cat <<'EOF'
symbol sync: publish only when the remote baseline has not changed
usage: symbol sync [--check]

requires a local symbol.toml checkout. compares local files to the
recorded baseline and current upstream tree. it does not delete remote
paths that are missing locally. --check prints the proposed diff.
EOF
      ;;
    undo) cat <<'EOF'
symbol undo: reverse a retained mutation or inspect the undo stack
usage: symbol undo [--stack] [--json] [NAME [TOKEN]]

without a token, reverses the newest applicable mutation. --stack lists
retained undos. --json writes the stack or a mutation envelope.
EOF
      ;;
    expire) expire_help ;;
    manage) cat <<'EOF'
symbol manage: enable or inspect write protection
usage: symbol manage [--json] NAME --status|--claim|--rotate|--release
       symbol put --managed NAME SOURCE

reads stay public. claim and rotate print a token once. --json writes
the {"managed":...} body only.
EOF
      ;;
    recover) cat <<'EOF'
symbol recover: resume interrupted idempotent creations and copies
usage: symbol recover

replays pending generated PUT/COPY operations with their original
idempotency identity.
EOF
      ;;
    ls) cat <<'EOF'
symbol ls: list sites or one site tree
usage: symbol ls [-l|--links] [--json] [NAME]

without NAME, lists sites. with NAME, lists every file and alias path.
-l adds URLs. names are taken from JSON fields and are not parsed from
the table. --json writes the server listing or inventory document.
EOF
      ;;
    rm) cat <<'EOF'
symbol rm: delete a path or site without saving an archive
usage: symbol rm [--json] NAME [PATH]

there is no prompt. NAME deletes the whole site. NAME PATH deletes that
file or subtree. undo metadata is retained for 4 hours.
--json prints a mutation envelope.
EOF
      ;;
    url) cat <<'EOF'
symbol url: print the public URL for a site
usage: symbol url NAME
EOF
      ;;
    update) cat <<'EOF'
symbol update: reinstall the client from the configured server
usage: symbol update
EOF
      ;;
    help) cat <<'EOF'
symbol help: show general or command-specific help
usage: symbol help [COMMAND]
EOF
      ;;
    *) usage_error "unknown command for help: $1" ;;
  esac
}

need() {
  if [ "$#" -lt "$1" ]; then
    usage >&2
    exit 2
  fi
}

diff_stats() {
  out=$(mktemp) || return 0
  diff -u "$1" "$2" > "${out}" || true
  awk '
    substr($0,1,3) == "+++" { next }
    substr($0,1,3) == "---" { next }
    substr($0,1,2) == "@@" { next }
    substr($0,1,1) == "+" { a++ }
    substr($0,1,1) == "-" { d++ }
    END {
      if (a+0 == 0 && d+0 == 0) exit
      printf "%d insertion%s(+), %d deletion%s(-)\n", a+0, (a==1)?"":"s", d+0, (d==1)?"":"s"
    }
  ' "${out}"
  rm -f "${out}"
}

request() {
  method=$1
  path=$2
  shift 2
  curl -sS -X "${method}" "$@" "${HOST}${path}"
}

print_api() {
  json=$(cat)
  version=$(printf '%s\n' "${json}" | json_string api_version) ||
    die "invalid API version document"
  revision=$(printf '%s\n' "${json}" | json_string absolute_revision) ||
    die "invalid API version document"
  source=$(printf '%s\n' "${json}" | json_string source_hash) ||
    die "invalid API version document"
  commit=$(printf '%s\n' "${json}" | json_string commit) ||
    die "invalid API version document"
  dirty=$(printf '%s\n' "${json}" | json_string dirty) ||
    die "invalid API version document"
  case "${dirty}" in
    true) dirty=yes ;;
    false) dirty=no ;;
    *) die "invalid API version dirty flag" ;;
  esac
  printf 'version   %s\n' "${version}"
  printf 'revision  %s\n' "${revision}"
  printf 'source    %s\n' "${source}"
  printf 'commit    %s\n' "${commit}"
  printf 'dirty     %s\n' "${dirty}"
}

print_stats() {
  awk '
    function spaces(n, out) {
      out = ""
      while (n-- > 0) out = out " "
      return out
    }
    function max(a, b) {
      return a > b ? a : b
    }
    function json_value(json, key, token, start, rest, values) {
      token = "\"" key "\":"
      start = index(json, token)
      if (start == 0) return ""
      rest = substr(json, start + length(token))
      split(rest, values, /[,}]/)
      return values[1]
    }
    function json_object(json, key, token, start, rest, finish) {
      token = "\"" key "\":{"
      start = index(json, token)
      if (start == 0) return ""
      rest = substr(json, start + length(token))
      finish = index(rest, "}")
      return substr(rest, 1, finish - 1)
    }
    function set_human(n, base, binary, fixed, labels, units, i, precision, format, text, parts) {
      if (n == "" || n == "null") {
        human_integer = "-"
        human_fraction = ""
        human_unit = ""
        return
      }
      labels = binary ? "B KiB MiB GiB TiB PiB" : "B kB MB GB TB PB"
      split(labels, units, " ")
      i = 1
      while (n >= base && i < 6) {
        n /= base
        i++
      }
      if (i == 1) precision = 0
      else if (fixed) precision = 2
      else if (n >= 100) precision = 0
      else if (n >= 10) precision = 1
      else precision = 2
      format = "%." precision "f"
      text = sprintf(format, n)
      split(text, parts, ".")
      human_integer = parts[1]
      human_fraction = precision == 0 ? "" : parts[2]
      human_unit = units[i]
    }
    function aligned(integer, fraction, unit, integer_width, fraction_width, unit_width, out) {
      out = spaces(integer_width - length(integer)) integer
      if (fraction_width > 0) {
        if (fraction == "") out = out spaces(fraction_width + 1)
        else out = out "." fraction spaces(fraction_width - length(fraction))
      }
      return out " " unit spaces(unit_width - length(unit))
    }
    function centered(text, width, left) {
      left = int((width - length(text)) / 2)
      return spaces(left) text spaces(width - length(text) - left)
    }
    {
      json = $0
    }
    END {
      labels[1] = "sites"
      labels[2] = "files"
      labels[3] = "blobs"
      labels[4] = "bytes"
      labels[5] = "saved"
      values[1] = json_value(json, "sites")
      values[2] = json_value(json, "files")
      values[3] = json_value(json, "blobs")
      values[4] = json_value(json, "bytes")
      values[5] = json_value(json, "saved_bytes")
      logical = json_value(json, "logical_bytes")
      saved_fraction = json_value(json, "saved_fraction")

      primary_width = 1
      for (i = 1; i <= 5; i++) primary_width = max(primary_width, length(values[i]))

      summary_bytes[2] = logical
      summary_bytes[3] = values[4]
      summary_bytes[4] = values[4]
      summary_bytes[5] = values[5]
      suffix[2] = " logical"
      suffix[3] = " unique"
      suffix[4] = ""
      suffix[5] = sprintf(", %.1f%%", saved_fraction * 100)

      for (i = 2; i <= 5; i++) {
        set_human(summary_bytes[i], 1000, 0, 1)
        decimal_integer[i] = human_integer
        decimal_fraction[i] = human_fraction
        decimal_unit[i] = human_unit
        decimal_integer_width = max(decimal_integer_width, length(human_integer))
        decimal_fraction_width = max(decimal_fraction_width, length(human_fraction))
        decimal_unit_width = max(decimal_unit_width, length(human_unit))

        set_human(summary_bytes[i], 1024, 1, 1)
        binary_integer[i] = human_integer
        binary_fraction[i] = human_fraction
        binary_unit[i] = human_unit
        binary_integer_width = max(binary_integer_width, length(human_integer))
        binary_fraction_width = max(binary_fraction_width, length(human_fraction))
        binary_unit_width = max(binary_unit_width, length(human_unit))
      }

      printf "%-5s %s%s\n", labels[1], spaces(primary_width - length(values[1])), values[1]
      for (i = 2; i <= 5; i++) {
        printf "%-5s %s%s    %s /  %s%s\n",
          labels[i],
          spaces(primary_width - length(values[i])),
          values[i],
          aligned(decimal_integer[i], decimal_fraction[i], decimal_unit[i],
            decimal_integer_width, decimal_fraction_width, decimal_unit_width),
          aligned(binary_integer[i], binary_fraction[i], binary_unit[i],
            binary_integer_width, binary_fraction_width, binary_unit_width),
          suffix[i]
      }

      file_object = json_object(json, "file_sizes")
      blob_object = json_object(json, "blob_sizes")
      split("min p25 median mean p75 max iqr stddev", metric_keys, " ")
      metric_labels[1] = "min"
      metric_labels[2] = "p25"
      metric_labels[3] = "median"
      metric_labels[4] = "mean"
      metric_labels[5] = "p75"
      metric_labels[6] = "max"
      metric_labels[7] = "IQR"
      metric_labels[8] = "stddev"

      for (i = 1; i <= 8; i++) {
        set_human(json_value(file_object, metric_keys[i]), 1024, 1, 0)
        file_integer[i] = human_integer
        file_fraction[i] = human_fraction
        file_unit[i] = human_unit
        file_integer_width = max(file_integer_width, length(human_integer))
        file_fraction_width = max(file_fraction_width, length(human_fraction))
        file_unit_width = max(file_unit_width, length(human_unit))

        set_human(json_value(blob_object, metric_keys[i]), 1024, 1, 0)
        blob_integer[i] = human_integer
        blob_fraction[i] = human_fraction
        blob_unit[i] = human_unit
        blob_integer_width = max(blob_integer_width, length(human_integer))
        blob_fraction_width = max(blob_fraction_width, length(human_fraction))
        blob_unit_width = max(blob_unit_width, length(human_unit))
      }

      file_width = file_integer_width + (file_fraction_width > 0 ? file_fraction_width + 1 : 0) + 1 + file_unit_width
      blob_width = blob_integer_width + (blob_fraction_width > 0 ? blob_fraction_width + 1 : 0) + 1 + blob_unit_width
      printf "\n%-8s %s   %s\n", "size", centered("files", file_width), centered("blobs", blob_width)
      for (i = 1; i <= 8; i++) {
        printf "%-8s %s   %s\n",
          metric_labels[i],
          aligned(file_integer[i], file_fraction[i], file_unit[i],
            file_integer_width, file_fraction_width, file_unit_width),
          aligned(blob_integer[i], blob_fraction[i], blob_unit[i],
            blob_integer_width, blob_fraction_width, blob_unit_width)
      }
    }
  '
}

json_ls_lib() {
  cat <<'AWK'
function spaces(n, out) {
  out = ""
  while (n-- > 0) out = out " "
  return out
}
function skip_string(s, i,    c) {
  i++
  while (i <= length(s)) {
    c = substr(s, i, 1)
    if (c == "\\") { i += 2; continue }
    if (c == "\"") return i
    i++
  }
  return i
}
function collect_objects(text, dest,    i, c, depth, start, n) {
  n = 0
  depth = 0
  for (i = 1; i <= length(text); i++) {
    c = substr(text, i, 1)
    if (c == "\"") { i = skip_string(text, i); continue }
    if (c == "{") {
      if (depth == 0) start = i
      depth++
    } else if (c == "}") {
      depth--
      if (depth == 0) {
        n++
        dest[n] = substr(text, start, i - start + 1)
      }
    }
  }
  return n
}
function array_after(text, key,    token, start, i, c, depth) {
  token = "\"" key "\""
  start = index(text, token)
  if (!start) return ""
  text = substr(text, start + length(token))
  sub(/^[[:space:]]*:[[:space:]]*/, "", text)
  if (substr(text, 1, 1) != "[") return ""
  depth = 0
  for (i = 1; i <= length(text); i++) {
    c = substr(text, i, 1)
    if (c == "\"") { i = skip_string(text, i); continue }
    if (c == "[") depth++
    else if (c == "]") {
      depth--
      if (depth == 0) return substr(text, 1, i)
    }
  }
  return text
}
function unescape(s,    out, i, c, n) {
  out = ""
  n = length(s)
  for (i = 1; i <= n; i++) {
    c = substr(s, i, 1)
    if (c == "\\" && i < n) {
      i++
      c = substr(s, i, 1)
      if (c == "n") out = out "\n"
      else if (c == "t") out = out "\t"
      else if (c == "r") out = out "\r"
      else out = out c
    } else out = out c
  }
  return out
}
function json_str(obj, key,    token, rest, i, c, start) {
  token = "\"" key "\""
  start = index(obj, token)
  if (!start) return ""
  rest = substr(obj, start + length(token))
  sub(/^[[:space:]]*:[[:space:]]*/, "", rest)
  if (substr(rest, 1, 1) != "\"") return ""
  for (i = 2; i <= length(rest); i++) {
    c = substr(rest, i, 1)
    if (c == "\\") { i++; continue }
    if (c == "\"") return unescape(substr(rest, 2, i - 2))
  }
  return ""
}
function json_num(obj, key,    token, rest, value) {
  token = "\"" key "\""
  if (!index(obj, token)) return ""
  rest = substr(obj, index(obj, token) + length(token))
  sub(/^[[:space:]]*:[[:space:]]*/, "", rest)
  if (substr(rest, 1, 4) == "null") return ""
  split(rest, value, /[,}]/)
  gsub(/[[:space:]]/, "", value[1])
  return value[1]
}
function human_size(n,    units, i, value) {
  units[1] = "B"; units[2] = "KiB"; units[3] = "MiB"
  units[4] = "GiB"; units[5] = "TiB"; units[6] = "PiB"
  value = n + 0
  i = 1
  while (value >= 1024 && i < 6) { value = value / 1024; i++ }
  if (i == 1) return sprintf("%d B", n + 0)
  if (value >= 100) return sprintf("%.0f %s", value, units[i])
  if (value >= 10) return sprintf("%.1f %s", value, units[i])
  return sprintf("%.2f %s", value, units[i])
}
AWK
}

print_listing() {
  base=${1%/}
  links=$2
  awk -v base="${base}" -v links="${links}" "$(json_ls_lib)"'
    {
      text = text $0
    }
    END {
      n = collect_objects(array_after(text, "entries"), obj)
      for (i = 1; i <= n; i++) {
        kind = json_str(obj[i], "kind")
        name = json_str(obj[i], "name")
        target = json_str(obj[i], "target")
        files = json_num(obj[i], "files")
        bytes = json_num(obj[i], "bytes")
        label = name
        if (kind == "site" || kind == "directory" || kind == "builtin") label = name "/"
        if (links) {
          if (kind == "builtin") row = base "/" name
          else row = base "/" name
        } else row = label
        if (kind == "alias" && target != "") row = row " -> " target
        rows[i] = row
        if (kind == "builtin") meta[i] = "built-in"
        else if (files != "") meta[i] = files " files   " human_size(bytes)
        else if (bytes != "") meta[i] = human_size(bytes)
        else meta[i] = ""
        if (length(row) > width) width = length(row)
      }
      for (i = 1; i <= n; i++) {
        if (meta[i] == "") print rows[i]
        else printf "%s%s  %s\n", rows[i], spaces(width - length(rows[i])), meta[i]
      }
      total_files = json_num(text, "files")
      total_bytes = json_num(text, "bytes")
      if (total_files != "") {
        printf "%s%s  %s files   %s total\n",
          spaces(width), "", total_files, human_size(total_bytes)
      }
    }
  '
}

print_inventory() {
  base=${1%/}
  links=$2
  awk -v base="${base}" -v links="${links}" "$(json_ls_lib)"'
    {
      text = text $0
    }
    END {
      n = 0
      files = collect_objects(array_after(text, "files"), fileobj)
      for (i = 1; i <= files; i++) {
        n++
        path = json_str(fileobj[i], "path")
        bytes = json_num(fileobj[i], "size")
        row = links ? base "/" path : path
        rows[n] = row
        meta[n] = human_size(bytes)
        if (length(row) > width) width = length(row)
      }
      aliases = collect_objects(array_after(text, "aliases"), aliasobj)
      for (i = 1; i <= aliases; i++) {
        n++
        path = json_str(aliasobj[i], "path")
        target = json_str(aliasobj[i], "target")
        row = links ? base "/" path : path
        if (target != "") row = row " -> " target
        rows[n] = row
        meta[n] = ""
        if (length(row) > width) width = length(row)
      }
      for (i = 1; i <= n; i++) {
        if (meta[i] == "") print rows[i]
        else printf "%s%s  %s\n", rows[i], spaces(width - length(rows[i])), meta[i]
      }
    }
  '
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage_error() {
  printf 'error: %s\n' "$*" >&2
  exit 2
}

is_site_name() {
  case "$1" in
    ''|'-'|*[!abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-]*)
      return 1
      ;;
  esac
}

urlencode_path() {
  printf '%s' "$1" | awk '
    BEGIN {
      for (i = 0; i < 256; i++) hex[sprintf("%c", i)] = sprintf("%%%02X", i)
      safe = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~/"
    }
    {
      out = ""
      for (i = 1; i <= length($0); i++) {
        c = substr($0, i, 1)
        out = out (index(safe, c) ? c : hex[c])
      }
      print out
    }
  '
}

manifest_find() {
  dir=${1:-$(pwd)}
  case "${dir}" in
    /*) ;;
    *) dir=$(cd "${dir}" 2>/dev/null && pwd) || return 1 ;;
  esac
  if stat -c %d "${dir}" >/dev/null 2>&1; then
    device=$(stat -c %d "${dir}")
    stat_device() { stat -c %d "$1"; }
  else
    device=$(stat -f %d "${dir}")
    stat_device() { stat -f %d "$1"; }
  fi
  while :; do
    if [ -f "${dir}/symbol.toml" ]; then
      printf '%s\n' "${dir}/symbol.toml"
      return 0
    fi
    parent=${dir%/*}
    [ -n "${parent}" ] || parent=/
    [ "${parent}" != "${dir}" ] || return 1
    [ "$(stat_device "${parent}")" = "${device}" ] || return 1
    dir=${parent}
  done
}

manifest_value() {
  key=$1
  file=$2
  awk -v wanted="${key}" '
    /^[[:space:]]*\[/ { exit }
    /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
    {
      line = $0
      sub(/[[:space:]]*#.*/, "", line)
      eq = index(line, "=")
      if (!eq) next
      key = substr(line, 1, eq - 1)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
      if (key != wanted) next
      value = substr(line, eq + 1)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
      if (value ~ /^".*"$/) {
        value = substr(value, 2, length(value) - 2)
        if (value ~ /\\/) exit 2
      } else if (value !~ /^[0-9]+$/ && value != "true" && value != "false") {
        exit 2
      }
      print value
      found = 1
      exit
    }
    END { if (!found) exit 1 }
  ' "${file}"
}

manifest_target() {
  MANIFEST=$(manifest_find) || die "no symbol.toml found; specify a site"
  MANIFEST_DIR=$(dirname "${MANIFEST}")
  MANIFEST_HOST=$(manifest_value host "${MANIFEST}") ||
    die "invalid symbol.toml: missing top-level host"
  MANIFEST_NAME=$(manifest_value name "${MANIFEST}") ||
    die "invalid symbol.toml: missing top-level name"
  case "${MANIFEST_HOST}" in http://*|https://*) ;; *) die "invalid symbol.toml host" ;; esac
  is_site_name "${MANIFEST_NAME}" || die "invalid symbol.toml site name"
}

token_from_manifest() {
  file=$1
  value=$(manifest_value token "${file}" 2>/dev/null || true)
  [ -n "${value}" ] || return 1
  case "${value}" in
    sym_mgmt_*) printf '%s\n' "${value}" ;;
    *)
      case "${value}" in /*) path=${value} ;; *) path=$(dirname "${file}")/${value} ;; esac
      [ -f "${path}" ] || die "management token file not found: ${path}"
      tr -d '\r\n' < "${path}"
      ;;
  esac
}

claim_from_manifest() {
  file=$1
  value=$(manifest_value claim "${file}" 2>/dev/null || true)
  [ -n "${value}" ] || return 1
  case "${value}" in
    sym_claim_*) printf '%s\n' "${value}" ;;
    *)
      case "${value}" in /*) path=${value} ;; *) path=$(dirname "${file}")/${value} ;; esac
      [ -f "${path}" ] || die "creator claim file not found: ${path}"
      tr -d '\r\n' < "${path}"
      ;;
  esac
}

TOKEN_EXPLICIT=
TOKEN_SEEN=0
JSON_OUTPUT=0
parse_global_tokens() {
  args=$(mktemp) || exit 1
  : > "${args}"
  after_dash=0
  while [ "$#" -gt 0 ]; do
    if [ "${after_dash}" -eq 0 ]; then
      case "$1" in
        --)
          after_dash=1
          printf '%s\n' "$1" >> "${args}"
          shift
          continue
          ;;
        -t|--token)
          [ "${TOKEN_SEEN}" -eq 0 ] || usage_error "management token specified more than once"
          [ "$#" -ge 2 ] || usage_error "$1 requires a token"
          TOKEN_EXPLICIT=$2
          TOKEN_SEEN=1
          shift 2
          continue
          ;;
        --token=*)
          [ "${TOKEN_SEEN}" -eq 0 ] || usage_error "management token specified more than once"
          TOKEN_EXPLICIT=${1#*=}
          [ -n "${TOKEN_EXPLICIT}" ] || usage_error "--token requires a token"
          TOKEN_SEEN=1
          shift
          continue
          ;;
        --json)
          JSON_OUTPUT=1
          shift
          continue
          ;;
      esac
    fi
    printf '%s\n' "$1" >> "${args}"
    shift
  done
  set --
  while IFS= read -r arg; do
    set -- "$@" "${arg}"
  done < "${args}"
  rm -f "${args}"
  PARSED_ARGS=$(mktemp) || exit 1
  : > "${PARSED_ARGS}"
  for arg do printf '%s\n' "${arg}" >> "${PARSED_ARGS}"; done
}

select_token() {
  TOKEN=
  if [ "${TOKEN_SEEN}" -eq 1 ]; then
    TOKEN=${TOKEN_EXPLICIT}
  elif [ -n "${SYMBOL_TOKEN:-}" ]; then
    TOKEN=${SYMBOL_TOKEN}
  else
    local_manifest=$(manifest_find 2>/dev/null || true)
    if [ -n "${local_manifest}" ]; then
      if manifest_value token "${local_manifest}" >/dev/null 2>&1; then
        TOKEN=$(token_from_manifest "${local_manifest}")
      fi
    fi
  fi
}

auth_args_file() {
  file=$1
  : > "${file}"
  if [ -n "${TOKEN:-}" ]; then
    printf '%s\n%s\n' '-H' "Authorization: Bearer ${TOKEN}" >> "${file}"
  fi
}

random_key() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 16
  else
    od -An -N16 -tx1 /dev/urandom | tr -d ' \n'
  fi
}

HTTP_DIR=
http_cleanup() {
  [ -z "${HTTP_DIR:-}" ] || rm -rf "${HTTP_DIR}"
  HTTP_DIR=
}

http_request() {
  method=$1
  url=$2
  shift 2
  http_cleanup
  HTTP_DIR=$(mktemp -d) || exit 1
  HTTP_HEADERS=${HTTP_DIR}/headers
  HTTP_BODY=${HTTP_DIR}/body
  HTTP_STATUS=$(curl -sS -X "${method}" -D "${HTTP_HEADERS}" -o "${HTTP_BODY}" \
    -w '%{http_code}' "$@" "${url}") || {
      status=$?
      [ -s "${HTTP_BODY}" ] && cat "${HTTP_BODY}" >&2
      return "${status}"
    }
  case "${HTTP_STATUS}" in
    2??) return 0 ;;
    *) [ -s "${HTTP_BODY}" ] && cat "${HTTP_BODY}" >&2; return 1 ;;
  esac
}

header_value() {
  wanted=$1
  awk -v wanted="${wanted}" '
    {
      line = $0
      sub(/\r$/, "", line)
      colon = index(line, ":")
      if (colon && tolower(substr(line, 1, colon - 1)) == tolower(wanted)) {
        value = substr(line, colon + 1)
        sub(/^[[:space:]]+/, "", value)
      }
    }
    END { if (value != "") print value }
  ' "${HTTP_HEADERS}"
}

print_mutation_json() {
  message=$1
  if [ -z "${message}" ] && [ -s "${HTTP_BODY}" ]; then
    message=$(tr -d '\r' < "${HTTP_BODY}")
    message=${message%"${message##*[![:space:]]}"}
  fi
  location=$(header_value Location)
  etag=$(header_value ETag)
  revision=$(header_value Content-Revision)
  undo=$(header_value Undo-Token)
  printf '{'
  sep=
  if [ -n "${message}" ]; then
    printf '"message":%s' "$(printf '%s\n' "${message}" | json_quote)"
    sep=,
  fi
  if [ -n "${location}" ]; then
    printf '%s"location":%s' "${sep}" "$(printf '%s\n' "${location}" | json_quote)"
    sep=,
  fi
  if [ -n "${etag}" ]; then
    printf '%s"etag":%s' "${sep}" "$(printf '%s\n' "${etag}" | json_quote)"
    sep=,
  fi
  if [ -n "${revision}" ]; then
    printf '%s"content_revision":%s' "${sep}" "$(printf '%s\n' "${revision}" | json_quote)"
    sep=,
  fi
  if [ -n "${undo}" ]; then
    printf '%s"undo_token":%s' "${sep}" "$(printf '%s\n' "${undo}" | json_quote)"
  fi
  printf '}\n'
}

print_mutation_result() {
  if [ "${JSON_OUTPUT}" -eq 1 ]; then
    print_mutation_json "${STATUS_MESSAGE:-}"
    STATUS_MESSAGE=
    management_count=$(header_value Sanitized-Management-Tokens)
    claim_count=$(header_value Sanitized-Creator-Claims)
    management_count=${management_count:-0}
    claim_count=${claim_count:-0}
    if [ "${management_count}" != 0 ] || [ "${claim_count}" != 0 ]; then
      printf 'warning: redacted %s management tokens and %s creator claims from uploaded files\n' \
        "${management_count}" "${claim_count}" >&2
    fi
    return
  fi
  suppress_body=${2:-0}
  if [ "${suppress_body}" -eq 0 ] && [ -s "${HTTP_BODY}" ]; then
    cat "${HTTP_BODY}"
    last=$(tail -c 1 "${HTTP_BODY}" 2>/dev/null || true)
    [ -z "${last}" ] || printf '\n'
  fi
  undo=$(header_value Undo-Token)
  if [ -n "${undo}" ]; then
    printf 'undo within 4h: symbol undo %s %s\n' "$1" "${undo}"
  fi
  management_count=$(header_value Sanitized-Management-Tokens)
  claim_count=$(header_value Sanitized-Creator-Claims)
  management_count=${management_count:-0}
  claim_count=${claim_count:-0}
  if [ "${management_count}" != 0 ] || [ "${claim_count}" != 0 ]; then
    printf 'warning: redacted %s management tokens and %s creator claims from uploaded files\n' \
      "${management_count}" "${claim_count}" >&2
  fi
}

emit_status() {
  if [ "${JSON_OUTPUT}" -eq 1 ]; then
    STATUS_MESSAGE=$1
    return
  fi
  printf '%s\n' "$1"
}

command_registry() {
  cat <<'EOF'
put put push add
pop pop
clone clone pull
get get download
copy copy
remix remix x
move move rename
alias alias
api api
stats stats
sync sync
undo undo
expire expire
manage manage
recover recover
ls ls list
rm rm delete
url url
update update upgrade
help help -h --help
EOF
}

if [ "${SYMBOL_TEST_COMMAND_REGISTRY:-0}" = 1 ]; then
  command_registry
  exit 0
fi

resolve_command() {
  query=$1
  command_registry | awk -v q="${query}" '
    function add(id, spelling, canonical_match, exact_match) {
      if (!seen[id]++) {
        order[++count] = id
        display[id] = spelling
      }
      if (spelling == id) canonical[id] = 1
      if (canonical_match) canonical[id] = 1
      if (exact_match) exact[id] = 1
    }
    {
      id = $1
      for (i = 2; i <= NF; i++) {
        spell[++n] = $i
        identity[n] = id
      }
    }
    END {
      for (i = 1; i <= n; i++) if (spell[i] == q) {
        print identity[i]
        exit 0
      }
      for (pass = 1; pass <= 2; pass++) {
        delete seen; delete order; delete display; delete canonical
        count = 0
        for (i = 1; i <= n; i++) {
          matches = pass == 1 ? index(spell[i], q) == 1 : index(spell[i], q) > 0
          if (matches) add(identity[i], spell[i], spell[i] == identity[i], 0)
        }
        if (count == 1) {
          print order[1]
          exit 0
        }
        if (count > 1) {
          printf "error: ambiguous command \047%s\047:", q > "/dev/stderr"
          for (i = 1; i <= count; i++) {
            id = order[i]
            if (canonical[id]) label = id
            else label = display[id] " (" id ")"
            separator = i == 1 ? " " : ", "
            printf "%s%s", separator, label > "/dev/stderr"
          }
          print "" > "/dev/stderr"
          exit 2
        }
      }
      printf "error: unknown command \047%s\047\n", q > "/dev/stderr"
      exit 2
    }
  '
}

parse_global_tokens "$@"
set --
while IFS= read -r arg; do set -- "$@" "${arg}"; done < "${PARSED_ARGS}"
rm -f "${PARSED_ARGS}"
raw_cmd=${1:-help}
[ "$#" -eq 0 ] || shift
cmd=$(resolve_command "${raw_cmd}") || exit $?
if [ "${SYMBOL_TEST_RESOLVE_ONLY:-0}" = 1 ]; then
  printf '%s\n' "${cmd}"
  exit 0
fi
archive_suffix() {
  case "$1" in
    -|'') printf '%s\n' .tar.gz ;;
    *.tar.gz|*.tgz) printf '%s\n' .tar.gz ;;
    *.tar) printf '%s\n' .tar ;;
    *.zip) printf '%s\n' .zip ;;
    *) usage_error "archive must end in .tar.gz, .tgz, .tar, or .zip" ;;
  esac
}

archive_transfer() {
  method=$1
  base=$2
  name=$3
  dest=$4
  suffix=$(archive_suffix "${dest}")
  url="${base}/${name}${suffix}"
  auth=$(mktemp) || exit 1
  archive_headers=$(mktemp) || exit 1
  auth_args_file "${auth}"
  set -- -D "${archive_headers}"
  while IFS= read -r arg; do set -- "$@" "${arg}"; done < "${auth}"
  rm -f "${auth}"
  if [ "${dest}" = "-" ]; then
    if ! curl -sS -f -X "${method}" "$@" "${url}"; then
      rm -f "${archive_headers}"
      die "archive transfer failed"
    fi
    if [ "${method}" = DELETE ]; then
      HTTP_HEADERS=${archive_headers}
      undo=$(header_value Undo-Token)
      [ -z "${undo}" ] ||
        printf 'undo within 4h: symbol undo %s %s\n' "${name}" "${undo}" >&2
    fi
    rm -f "${archive_headers}"
    return
  fi
  tmp=$(mktemp) || exit 1
  if curl -sS -f -X "${method}" "$@" "${url}" -o "${tmp}"; then
    mv "${tmp}" "${dest}"
    if [ "${method}" = DELETE ]; then
      HTTP_HEADERS=${archive_headers}
      undo=$(header_value Undo-Token)
      [ -z "${undo}" ] ||
        printf 'undo within 4h: symbol undo %s %s\n' "${name}" "${undo}" >&2
    fi
    rm -f "${archive_headers}"
  else
    rm -f "${tmp}"
    rm -f "${archive_headers}"
    return 1
  fi
}

content_type() {
  case "$1" in
    *.zip) printf '%s\n' application/zip ;;
    *.tgz|*.tar.gz|*.gz) printf '%s\n' application/gzip ;;
    *.tar) printf '%s\n' application/x-tar ;;
    *.html|*.htm) printf '%s\n' text/html ;;
    *.css) printf '%s\n' text/css ;;
    *.js) printf '%s\n' application/javascript ;;
    *.json) printf '%s\n' application/json ;;
    *.svg) printf '%s\n' image/svg+xml ;;
    *.png) printf '%s\n' image/png ;;
    *.jpg|*.jpeg) printf '%s\n' image/jpeg ;;
    *.mp3) printf '%s\n' audio/mpeg ;;
    *.m4a) printf '%s\n' audio/mp4 ;;
    *.mp4|*.m4v) printf '%s\n' video/mp4 ;;
    *.webm) printf '%s\n' video/webm ;;
    *.ogg|*.oga) printf '%s\n' audio/ogg ;;
    *.wav) printf '%s\n' audio/wav ;;
    *) printf '%s\n' application/octet-stream ;;
  esac
}

validate_remote_path() {
  remote=$1
  [ -n "${remote}" ] || return 0
  case "${remote}" in
    /*|*\\*) usage_error "invalid remote path: ${remote}" ;;
  esac
  case "/${remote}/" in
    */../*|*/./*|*'//'*) usage_error "invalid remote path: ${remote}" ;;
  esac
  terminal=$(basename "./${remote}")
  case "${terminal}" in
    symbol.toml|.symbol-token|.symbol-claim|FILES|HASH|UNDO|EXPIRES)
      usage_error "reserved remote path: ${remote}"
      ;;
  esac
}

canonical_alias_target() (
  alias_path=$1
  alias_target=$2
  LC_ALL=C
  export LC_ALL
  printf '%s\t%s\n' "${alias_path}" "${alias_target}" | awk -F '\t' '
    function reserved(path, count, parts, terminal) {
      count = split(path, parts, "/")
      terminal = parts[count]
      return terminal == "symbol.toml" || terminal == ".symbol-token" ||
        terminal == ".symbol-claim" || terminal == "FILES" ||
        terminal == "HASH" || terminal == "UNDO" || terminal == "EXPIRES"
    }
    function noise(path, count, parts, i, part, lower) {
      count = split(path, parts, "/")
      for (i = 1; i <= count; i++) {
        part = parts[i]
        lower = tolower(part)
        if (substr(part, 1, 2) == "._" || part == "Icon\r" ||
            lower == "__macosx" || lower == ".appledouble" ||
            lower == ".ds_store" || lower == ".lsoverride" ||
            lower == "thumbs.db" || lower == "ehthumbs.db" ||
            lower == "desktop.ini") return 1
      }
      return 0
    }
    function external(target, lower) {
      lower = tolower(target)
      return index(lower, "://") ||
        lower ~ /^(data|file|ftp|ftps|git|http|https|javascript|mailto|ssh|ws|wss):/
    }
    {
      path = $1
      target = $2
      if (NF != 2 || path == "" || target == "" ||
          length(target) > 4096 || target ~ /^\// ||
          target ~ /\\/ || target ~ /[[:cntrl:]]/ ||
          path ~ /\\/ || path ~ /[[:cntrl:]]/ || external(target)) exit 1
      count = split(path, path_parts, "/")
      depth = 0
      for (i = 1; i < count; i++) {
        if (path_parts[i] == "" || path_parts[i] == "." ||
            path_parts[i] == "..") exit 1
        parts[++depth] = path_parts[i]
      }
      count = split(target, target_parts, "/")
      for (i = 1; i <= count; i++) {
        part = target_parts[i]
        if (part == "" || part == ".") continue
        if (part == "..") {
          if (depth == 0) exit 1
          delete parts[depth--]
        } else {
          parts[++depth] = part
        }
      }
      if (depth == 0) exit 1
      canonical = parts[1]
      for (i = 2; i <= depth; i++) canonical = canonical "/" parts[i]
      if (reserved(canonical) || noise(canonical)) exit 1
      print canonical
    }
  '
)

relative_alias_target() (
  alias_path=$1
  canonical_target=$2
  printf '%s\t%s\n' "${alias_path}" "${canonical_target}" | awk -F '\t' '
    {
      path_count = split($1, path_parts, "/")
      from_count = path_count - 1
      to_count = split($2, target_parts, "/")
      common = 0
      while (common < from_count && common < to_count &&
             path_parts[common + 1] == target_parts[common + 1]) common++
      out = ""
      for (i = common + 1; i <= from_count; i++)
        out = out (out == "" ? "" : "/") ".."
      for (i = common + 1; i <= to_count; i++)
        out = out (out == "" ? "" : "/") target_parts[i]
      print (out == "" ? "." : out)
    }
  '
)

safe_symlink() {
  symlink_target=$1
  symlink_path=$2
  case "${symlink_target}" in
    -*) symlink_target=./${symlink_target} ;;
  esac
  ln -s "${symlink_target}" "${symlink_path}"
}

validate_alias_map() (
  alias_map=$1
  awk -F '\t' '
    function beneath(path, parent) {
      return index(path, parent "/") == 1
    }
    function substitution(path, best, candidate) {
      best = ""
      for (candidate in target) {
        if ((path == candidate || beneath(path, candidate)) &&
            length(candidate) > length(best)) best = candidate
      }
      matched = best
      return best == "" ? "" : target[best] substr(path, length(best) + 1)
    }
    {
      if (NF != 2 || $1 == "" || $2 == "") exit 1
      if (($1 in target) && target[$1] != $2) exit 1
      target[$1] = $2
    }
    END {
      if (NR == 0) exit 0
      for (path in target) {
        for (ancestor in target)
          if (path != ancestor && beneath(path, ancestor)) exit 1
        if (beneath(path, target[path])) exit 1
        current = path
        for (seen_path in seen) delete seen[seen_path]
        complete = 0
        for (hop = 0; hop <= NR; hop++) {
          if (current in seen) exit 1
          seen[current] = 1
          resolved = substitution(current)
          if (matched == "") {
            complete = 1
            break
          }
          current = resolved
        }
        if (!complete || beneath(path, current)) exit 1
      }
    }
  ' "${alias_map}"
)

path_covered_by_alias() (
  covered_path=$1
  covered_aliases=$2
  awk -F '\t' -v path="${covered_path}" '
    $1 == path || index(path, $1 "/") == 1 { found = 1; exit }
    END { exit !found }
  ' "${covered_aliases}"
)

path_below_alias() (
  covered_path=$1
  covered_aliases=$2
  awk -F '\t' -v path="${covered_path}" '
    index(path, $1 "/") == 1 { found = 1; exit }
    END { exit !found }
  ' "${covered_aliases}"
)

local_alias_map() (
  alias_root=$1
  alias_manifest=${2:-}
  alias_work=$(mktemp -d) || exit 1
  trap 'rm -rf "${alias_work}"' EXIT HUP INT TERM
  alias_baseline=${alias_work}/baseline
  alias_detected=${alias_work}/detected
  alias_combined=${alias_work}/combined
  : > "${alias_baseline}"
  : > "${alias_detected}"
  if [ -n "${alias_manifest}" ] && [ -f "${alias_manifest}" ]; then
    manifest_aliases "${alias_manifest}" > "${alias_baseline}"
  fi
  (
    cd "${alias_root}"
    find . -type l ! -path './.git/*' ! -name .git \
      ! -name '.symbol-token' ! -name '.symbol-claim' \
      ! -name 'symbol.toml' -print | LC_ALL=C sort
  ) | while IFS= read -r alias_source; do
    alias_relative=${alias_source#./}
    if path_below_alias "${alias_relative}" "${alias_baseline}"; then
      continue
    fi
    alias_link=$(readlink "${alias_root}/${alias_relative}") ||
      die "could not read symlink: ${alias_root}/${alias_relative}"
    alias_canonical=$(canonical_alias_target "${alias_relative}" "${alias_link}") ||
      die "unsafe symlink target: ${alias_relative} -> ${alias_link}"
    printf '%s\t%s\n' "${alias_relative}" "${alias_canonical}"
  done > "${alias_detected}"
  {
    cat "${alias_baseline}"
    cat "${alias_detected}"
  } | awk -F '\t' '{ targets[$1] = $2 } END {
    for (path in targets) print path "\t" targets[path]
  }' | LC_ALL=C sort > "${alias_combined}"
  validate_alias_map "${alias_combined}" ||
    die "unsafe symlink graph: alias cycle or write through alias"
  cat "${alias_combined}"
)

stage_upload_directory() {
  source=$1
  stage_manifest=
  [ ! -f "${source}/symbol.toml" ] || stage_manifest=${source}/symbol.toml
  stage_aliases=$(mktemp) || exit 1
  local_alias_map "${source}" "${stage_manifest}" > "${stage_aliases}"
  STAGE_ROOT=$(mktemp -d) || exit 1
  (
    cd "${source}"
    find . \( -type f -o -type l \) ! -path './.git/*' ! -name .git \
      ! -name '.symbol-token' ! -name '.symbol-claim' \
      ! -name 'symbol.toml' -print
  ) | while IFS= read -r path; do
    relative=${path#./}
    path_covered_by_alias "${relative}" "${stage_aliases}" && continue
    mkdir -p "${STAGE_ROOT}/$(dirname "./${relative}")"
    cp -P "${source}/${relative}" "${STAGE_ROOT}/${relative}"
  done
  while IFS='	' read -r stage_alias_path stage_alias_target; do
    [ -n "${stage_alias_path}" ] || continue
    mkdir -p "${STAGE_ROOT}/$(dirname "./${stage_alias_path}")"
    stage_relative_target=$(relative_alias_target \
      "${stage_alias_path}" "${stage_alias_target}")
    safe_symlink "${stage_relative_target}" "${STAGE_ROOT}/${stage_alias_path}"
  done < "${stage_aliases}"
  rm -f "${stage_aliases}"
  stage_aliases=
  staged_alias_check=$(mktemp) || exit 1
  local_alias_map "${STAGE_ROOT}" "" > "${staged_alias_check}"
  rm -f "${staged_alias_check}"
}

make_archive_from_symlink() {
  symlink_source=$1
  symlink_remote=$2
  symlink_directory=$(CDPATH='' cd "$(dirname "${symlink_source}")" && pwd) ||
    die "symlink parent does not exist: ${symlink_source}"
  symlink_name=$(basename "${symlink_source}")
  symlink_local_path=${symlink_name}
  symlink_checkout_manifest=$(manifest_find "${symlink_directory}" 2>/dev/null || true)
  if [ -n "${symlink_checkout_manifest}" ]; then
    symlink_checkout=$(dirname "${symlink_checkout_manifest}")
    case "${symlink_directory}/${symlink_name}" in
      "${symlink_checkout}"/*)
        symlink_local_path=${symlink_directory}/${symlink_name}
        symlink_local_path=${symlink_local_path#"${symlink_checkout}"/}
        ;;
    esac
  fi
  symlink_value=$(readlink "${symlink_source}") ||
    die "could not read symlink: ${symlink_source}"
  canonical_alias_target "${symlink_local_path}" "${symlink_value}" >/dev/null ||
    die "unsafe symlink target: ${symlink_source} -> ${symlink_value}"
  canonical_alias_target "${symlink_remote}" "${symlink_value}" >/dev/null ||
    die "symlink target escapes remote root: ${symlink_remote} -> ${symlink_value}"
  STAGE_ROOT=$(mktemp -d) || exit 1
  mkdir -p "${STAGE_ROOT}/$(dirname "./${symlink_remote}")"
  safe_symlink "${symlink_value}" "${STAGE_ROOT}/${symlink_remote}"
  ARCHIVE_FILE=$(mktemp) || exit 1
  tar -czf "${ARCHIVE_FILE}" -C "${STAGE_ROOT}" -- .
  rm -rf "${STAGE_ROOT}"
  STAGE_ROOT=
}

make_archive_from_directory() {
  source=$1
  stage_upload_directory "${source}"
  ARCHIVE_FILE=$(mktemp) || exit 1
  tar -czf "${ARCHIVE_FILE}" -C "${STAGE_ROOT}" -- .
  rm -rf "${STAGE_ROOT}"
  STAGE_ROOT=
}

write_secret_sidecar() {
  kind=$1
  secret=$2
  manifest=$3
  dir=$(dirname "${manifest}")
  case "${kind}" in
    token) sidecar=.symbol-token; key=token ;;
    claim) sidecar=.symbol-claim; key=claim ;;
  esac
  (umask 077; printf '%s\n' "${secret}" > "${dir}/${sidecar}")
  tmp=$(mktemp "${dir}/.symbol.toml.XXXXXX") || exit 1
  awk -v key="${key}" -v value="./${sidecar}" '
    BEGIN { written = 0; section = 0 }
    /^[[:space:]]*\[/ && !written {
      printf "%s = \"%s\"\n", key, value
      written = 1
      section = 1
    }
    {
      if (!section && $0 ~ "^[[:space:]]*" key "[[:space:]]*=") {
        if (!written) printf "%s = \"%s\"\n", key, value
        written = 1
        next
      }
      print
    }
    END { if (!written) printf "%s = \"%s\"\n", key, value }
  ' "${manifest}" > "${tmp}"
  mv "${tmp}" "${manifest}"
  if [ -d "${dir}/.git" ] || [ -f "${dir}/.gitignore" ]; then
    touch "${dir}/.gitignore"
    if ! awk -v line="/${sidecar}" '$0 == line { found=1 } END { exit !found }' "${dir}/.gitignore"; then
      printf '/%s\n' "${sidecar}" >> "${dir}/.gitignore"
    fi
  fi
  printf 'saved %s to %s/%s (mode 0600)\n' "${kind}" "${dir}" "${sidecar}"
}

remove_secret_sidecar() {
  kind=$1
  manifest=$2
  dir=$(dirname "${manifest}")
  case "${kind}" in
    token) sidecar=.symbol-token ;;
    claim) sidecar=.symbol-claim ;;
    *) return 1 ;;
  esac
  rm -f "${dir}/${sidecar}"
  tmp=$(mktemp "${dir}/.symbol.toml.XXXXXX") || exit 1
  awk -v key="${kind}" '$0 !~ "^[[:space:]]*" key "[[:space:]]*="' \
    "${manifest}" > "${tmp}"
  mv "${tmp}" "${manifest}"
}

save_claim_recovery() {
  name=$1
  claim=$2
  state_root=${XDG_STATE_HOME:-${HOME}/.local/state}
  directory=${state_root}/symbol/claims
  old_umask=$(umask)
  umask 077
  mkdir -p "${directory}"
  file=${directory}/${name}
  printf '%s\n' "${claim}" > "${file}"
  chmod 600 "${file}"
  umask "${old_umask}"
  printf 'saved creator claim to %s (mode 0600)\n' "${file}"
}

save_management_recovery() {
  name=$1
  token=$2
  state_root=${XDG_STATE_HOME:-${HOME}/.local/state}
  directory=${state_root}/symbol/tokens
  old_umask=$(umask)
  umask 077
  mkdir -p "${directory}"
  file=${directory}/${name}
  printf '%s\n' "${token}" > "${file}"
  chmod 600 "${file}"
  umask "${old_umask}"
  printf 'saved management token to %s (mode 0600)\n' "${file}"
}

persist_pending_claim() {
  key=$1
  claim=$2
  state_root=${XDG_STATE_HOME:-${HOME}/.local/state}
  directory=${state_root}/symbol/claims
  old_umask=$(umask)
  umask 077
  mkdir -p "${directory}"
  PENDING_CLAIM_DIR=${directory}/pending-${key}
  mkdir -p "${PENDING_CLAIM_DIR}"
  printf '%s\n' "${claim}" > "${PENDING_CLAIM_DIR}/claim"
  chmod 600 "${PENDING_CLAIM_DIR}/claim"
  umask "${old_umask}"
}

persist_pending_request() {
  pending_method=$1
  pending_host=$2
  pending_source=$3
  pending_destination=$4
  pending_managed=$5
  pending_body=${6:-}
  pending_content_type=${7:-}
  pending_unpack=${8:-0}
  pending_url=${9:-}
  [ -n "${PENDING_CLAIM_DIR:-}" ] || return 0
  {
    printf 'version=1\n'
    printf 'method=%s\n' "${pending_method}"
    printf 'host=%s\n' "${pending_host}"
    printf 'source=%s\n' "${pending_source}"
    printf 'destination=%s\n' "${pending_destination}"
    printf 'managed=%s\n' "${pending_managed}"
    printf 'content_type=%s\n' "${pending_content_type}"
    printf 'unpack=%s\n' "${pending_unpack}"
    printf 'url=%s\n' "${pending_url}"
    printf 'idempotency=%s\n' "${idempotency_key}"
  } > "${PENDING_CLAIM_DIR}/record"
  if [ -n "${pending_body}" ]; then
    cp "${pending_body}" "${PENDING_CLAIM_DIR}/body"
  fi
}

finish_pending_claim() {
  name=$1
  [ -n "${PENDING_CLAIM_DIR:-}" ] || return 0
  destination=$(dirname "${PENDING_CLAIM_DIR}")/${name}
  mv "${PENDING_CLAIM_DIR}/claim" "${destination}"
  rm -rf "${PENDING_CLAIM_DIR}"
  PENDING_CLAIM_DIR=
  printf 'saved creator claim to %s (mode 0600)\n' "${destination}"
}

remove_claim_recovery() {
  name=$1
  state_root=${XDG_STATE_HOME:-${HOME}/.local/state}
  rm -f "${state_root}/symbol/claims/${name}"
}

claim_from_recovery() {
  name=$1
  state_root=${XDG_STATE_HOME:-${HOME}/.local/state}
  file=${state_root}/symbol/claims/${name}
  [ -f "${file}" ] || return 1
  awk '{ print; exit }' "${file}"
}

pending_value() {
  key=$1
  record=$2
  awk -v key="${key}" '
    index($0, key "=") == 1 { print substr($0, length(key) + 2); exit }
  ' "${record}"
}

recover_pending_operation() {
  pending=$1
  record=${pending}/record
  [ -f "${record}" ] && [ -f "${pending}/claim" ] || return 1
  method=$(pending_value method "${record}")
  host=$(pending_value host "${record}")
  source=$(pending_value source "${record}")
  destination=$(pending_value destination "${record}")
  managed=$(pending_value managed "${record}")
  content_type=$(pending_value content_type "${record}")
  unpack=$(pending_value unpack "${record}")
  url=$(pending_value url "${record}")
  idempotency_key=$(pending_value idempotency "${record}")
  claim=$(awk '{ print; exit }' "${pending}/claim")
  PENDING_CLAIM_DIR=${pending}

  if [ -n "${destination}" ] && [ "${managed}" = 1 ] &&
    curl -fsS "${host}/${destination}/symbol.toml" >/dev/null 2>&1; then
    http_request MANAGE "${host}/${destination}" \
      -H 'Management-Action: rotate' \
      -H "Creator-Claim: ${claim}" \
      -H "Idempotency-Key: ${idempotency_key}-recover" || return 1
    management=$(header_value Management-Token)
    [ -n "${management}" ] || return 1
    save_management_recovery "${destination}" "${management}"
    finish_pending_claim "${destination}"
    printf 'recovered managed site %s/%s/\n' "${host}" "${destination}"
    return 0
  fi
  if [ "${method}" = COPY ] && [ -n "${destination}" ] &&
    curl -fsS "${host}/${destination}/symbol.toml" >/dev/null 2>&1; then
    finish_pending_claim "${destination}"
    printf 'recovered copied site %s/%s/\n' "${host}" "${destination}"
    return 0
  fi

  set -- -H "Idempotency-Key: ${idempotency_key}" -H "Creator-Claim: ${claim}"
  [ "${managed}" != 1 ] || set -- "$@" -H 'Management-Action: claim'
  if [ "${method}" = PUT ]; then
    [ -z "${content_type}" ] || set -- "$@" -H "Content-Type: ${content_type}"
    [ "${unpack}" != 1 ] || set -- "$@" -H 'Unpack: 1'
    if [ "${unpack}" != 1 ] && [ -n "${source}" ]; then
      set -- "$@" -H "Content-Disposition: attachment; filename=\"$(basename "${source}")\""
    fi
    if [ "${url}" = "${host}/" ]; then
      http_request PUT "${url}" "$@" -T - < "${pending}/body" || return 1
    else
      http_request PUT "${url}" "$@" -T "${pending}/body" || return 1
    fi
  elif [ "${method}" = COPY ]; then
    [ -z "${destination}" ] || set -- "$@" -H "Destination: /${destination}"
    http_request COPY "${url}" "$@" || return 1
  else
    return 1
  fi
  location=$(header_value Location)
  if [ -n "${destination}" ]; then
    recovered=${destination}
  else
    [ -n "${location}" ] || return 1
    recovered=${location%/}
    recovered=${recovered##*/}
  fi
  finish_pending_claim "${recovered}"
  printf 'recovered %s/%s/\n' "${host}" "${recovered}"
}

recover_pending_operations() {
  state_root=${XDG_STATE_HOME:-${HOME}/.local/state}
  directory=${state_root}/symbol/claims
  [ -d "${directory}" ] || {
    printf 'no pending operations\n'
    return
  }
  find "${directory}" -type d -name 'pending-*' -mtime +7 -exec rm -rf {} \; 2>/dev/null || true
  found=0
  for pending in "${directory}"/pending-*; do
    [ -d "${pending}" ] || continue
    found=1
    recover_pending_operation "${pending}" ||
      printf 'could not recover %s; pending state retained\n' "${pending}" >&2
  done
  [ "${found}" -eq 1 ] || printf 'no pending operations\n'
}

matching_manifest() {
  expected_host=$1
  expected_name=$2
  found=$(manifest_find 2>/dev/null || true)
  [ -n "${found}" ] || return 1
  found_host=$(manifest_value host "${found}" 2>/dev/null || true)
  found_name=$(manifest_value name "${found}" 2>/dev/null || true)
  [ "${found_host%/}" = "${expected_host%/}" ] && [ "${found_name}" = "${expected_name}" ] || return 1
  printf '%s\n' "${found}"
}

save_response_secrets() {
  base=$1
  name=$2
  management=$(header_value Management-Token)
  claim=$(header_value Creator-Claim)
  [ -z "${management}" ] || {
    [ "${JSON_OUTPUT}" -eq 1 ] ||
      printf 'management token (shown once):\n  %s\n' "${management}"
    local_manifest=$(matching_manifest "${base}" "${name}" 2>/dev/null || true)
    [ -z "${local_manifest}" ] || write_secret_sidecar token "${management}" "${local_manifest}"
  }
  [ -z "${claim}" ] || {
    [ "${JSON_OUTPUT}" -eq 1 ] ||
      printf 'creator claim (shown once):\n  %s\n' "${claim}"
    local_manifest=$(matching_manifest "${base}" "${name}" 2>/dev/null || true)
    [ -z "${local_manifest}" ] || write_secret_sidecar claim "${claim}" "${local_manifest}"
  }
}

put_file_request() {
  base=$1
  site=$2
  source=$3
  remote=$4
  unpack=$5
  managed=$6
  conditional=${7:-}
  generated=${8:-0}
  validate_remote_path "${remote}"
  is_site_name "${site}" || [ "${generated}" -eq 1 ] ||
    usage_error "invalid site name: ${site}"
  auth=$(mktemp) || exit 1
  auth_args_file "${auth}"
  set --
  while IFS= read -r arg; do set -- "$@" "${arg}"; done < "${auth}"
  rm -f "${auth}"
  [ -z "${conditional}" ] || set -- "$@" -H "If-Match: ${conditional}"
  if [ "${REPLACE_SITE:-0}" -eq 1 ]; then
    [ "${generated}" -eq 0 ] || usage_error "--replace requires a site name"
    [ -z "${remote}" ] || usage_error "--replace applies to a whole site, not a single path"
    set -- "$@" -H 'Replace: 1'
  fi
  claim_manifest=$(matching_manifest "${base}" "${site}" 2>/dev/null || true)
  if [ -z "${claim_manifest}" ] && [ -d "${source}" ] && [ -f "${source}/symbol.toml" ]; then
    candidate_host=$(manifest_value host "${source}/symbol.toml" 2>/dev/null || true)
    candidate_name=$(manifest_value name "${source}/symbol.toml" 2>/dev/null || true)
    if [ "${candidate_host%/}" = "${base%/}" ] && [ "${candidate_name}" = "${site}" ]; then
      claim_manifest=${source}/symbol.toml
    fi
  fi
  request_unpack=${unpack}
  expected_alias_target=
  if [ -L "${source}" ]; then
    [ -n "${remote}" ] || remote=$(basename "./${source}")
    expected_alias_target=$(readlink "${source}")
    make_archive_from_symlink "${source}" "${remote}"
    source=${ARCHIVE_FILE}
    ctype=application/gzip
    request_unpack=1
    set -- "$@" -H 'Content-Type: application/gzip' -H 'Unpack: 1'
  elif [ "${generated}" -eq 1 ] && [ -n "${remote}" ]; then
    upload_stage=$(mktemp -d) || exit 1
    mkdir -p "${upload_stage}/$(dirname "./${remote}")"
    cp -P "${source}" "${upload_stage}/${remote}"
    ARCHIVE_FILE=$(mktemp) || exit 1
    tar -czf "${ARCHIVE_FILE}" -C "${upload_stage}" -- .
    rm -rf "${upload_stage}"
    source=${ARCHIVE_FILE}
    ctype=application/gzip
    request_unpack=1
    set -- "$@" -H 'Content-Type: application/gzip' -H 'Unpack: 1'
  elif [ -d "${source}" ]; then
    make_archive_from_directory "${source}"
    source=${ARCHIVE_FILE}
    ctype=application/gzip
    request_unpack=1
    set -- "$@" -H 'Content-Type: application/gzip' -H 'Unpack: 1'
  else
    [ -f "${source}" ] || die "not a file or directory: ${source}"
    if [ -n "${remote}" ]; then
      ctype=$(content_type "${remote}")
      set -- "$@" -H "Content-Disposition: attachment; filename=\"$(basename "./${remote}")\""
    else
      ctype=$(content_type "${source}")
    fi
    set -- "$@" -H "Content-Type: ${ctype}"
    [ "${unpack}" -eq 0 ] || set -- "$@" -H 'Unpack: 1'
  fi
  if [ "${generated}" -eq 1 ]; then
    url="${base}/"
  elif [ -n "${remote}" ]; then
    encoded=$(urlencode_path "${remote}")
    url="${base}/${site}/${encoded}"
  else
    url="${base}/${site}"
  fi
  if [ "${generated}" -eq 0 ] && [ -n "${remote}" ]; then
    file_put_failed=0
    if ! http_request PUT "${url}" "$@" -T "${source}"; then
      if [ -z "${HTTP_STATUS:-}" ] || [ "${HTTP_STATUS}" = 000 ]; then
        verified=0
        verify_file=$(mktemp) || exit 1
        # -L because an unambiguous .html now redirects to its canonical
        # extensionless URL; without following it this reads an empty body and
        # a committed write looks lost.
        if [ "${request_unpack}" -eq 0 ] &&
          curl -fsSL "${url}" -o "${verify_file}" &&
          cmp -s "${source}" "${verify_file}"; then
          verified=1
        elif [ -n "${expected_alias_target}" ]; then
          verify_manifest=$(mktemp) || exit 1
          if curl -fsS "${base}/${site}/symbol.toml" -o "${verify_manifest}" &&
            manifest_aliases "${verify_manifest}" |
              awk -F '	' -v path="${remote}" -v target="${expected_alias_target}" '
                $1 == path && $2 == target { found=1 }
                END { exit !found }
              '; then
            verified=1
          fi
          rm -f "${verify_manifest}"
        fi
        rm -f "${verify_file}"
        if [ "${verified}" -eq 1 ]; then
          printf 'verified committed update %s after response loss\n' "${url}"
          printf 'warning: response and any one-time creator claim were lost; no undo token is available\n' >&2
          local_manifest=$(matching_manifest "${base}" "${site}" 2>/dev/null || true)
          [ -z "${local_manifest}" ] ||
            refresh_local_manifest "${local_manifest}" "${base}" "${site}"
        else
          printf 'error: response lost after file PUT; not retried because this endpoint does not support Idempotency-Key\n' >&2
          printf 'verify the remote file before retrying: %s\n' "${url}" >&2
          file_put_failed=1
        fi
      else
        file_put_failed=1
      fi
    else
      if awk 'index($0, "changed: false") { found=1 } END { exit !found }' "${HTTP_BODY}"; then
        emit_status "already up to date ${base}/${site}/"
      elif [ "${HTTP_STATUS}" = 201 ]; then
        emit_status "created ${base}/${site}/"
      elif [ "${HTTP_STATUS}" = 200 ]; then
        emit_status "updated ${base}/${site}/"
      fi
      print_mutation_result "${site}" 1
      save_response_secrets "${base}" "${site}"
      local_manifest=$(matching_manifest "${base}" "${site}" 2>/dev/null || true)
      response_claim=$(header_value Creator-Claim)
      if [ "${HTTP_STATUS}" = 201 ] && [ -n "${response_claim}" ] &&
        [ -z "${local_manifest}" ]; then
        save_claim_recovery "${site}" "${response_claim}"
      fi
      [ -z "${local_manifest}" ] ||
        refresh_local_manifest "${local_manifest}" "${base}" "${site}"
    fi
    [ -z "${ARCHIVE_FILE:-}" ] || rm -f "${ARCHIVE_FILE}"
    ARCHIVE_FILE=
    [ "${file_put_failed}" -eq 0 ]
    return
  fi
  idempotency_key=$(random_key)
  set -- "$@" -H "Idempotency-Key: ${idempotency_key}"
  claim=
  claim_generated=0
  if [ -n "${claim_manifest}" ] && manifest_value claim "${claim_manifest}" >/dev/null 2>&1; then
    claim=$(claim_from_manifest "${claim_manifest}")
  fi
  if [ -z "${claim}" ]; then
    claim="sym_claim_$(random_key)$(random_key)"
    claim_generated=1
    [ -z "${claim_manifest}" ] || write_secret_sidecar claim "${claim}" "${claim_manifest}"
  fi
  PENDING_CLAIM_DIR=
  persist_pending_claim "${idempotency_key}" "${claim}"
  set -- "$@" -H "Creator-Claim: ${claim}"
  if [ "${managed}" -eq 1 ]; then
    set -- "$@" -H 'Management-Action: claim'
  fi
  persist_pending_request PUT "${base}" "${remote}" "${site}" "${managed}" \
    "${source}" "${ctype}" "${request_unpack}" "${url}"
  put_attempt() {
    if [ "${generated}" -eq 1 ]; then
      http_request PUT "${url}" "$@" -T - < "${source}"
    else
      http_request PUT "${url}" "$@" -T "${source}"
    fi
  }
  response_was_lost=0
  managed_response_lost=0
  if ! put_attempt "$@"; then
    if [ -z "${HTTP_STATUS:-}" ] || [ "${HTTP_STATUS}" = 000 ]; then
      response_was_lost=1
      if ! put_attempt "$@"; then
        if [ "${managed}" -eq 1 ] && [ -n "${site}" ] &&
          [ "${HTTP_STATUS:-}" = 409 ] &&
          curl -fsS "${base}/${site}/symbol.toml" >/dev/null 2>&1; then
          HTTP_STATUS=200
          managed_response_lost=1
        else
          if [ -n "${HTTP_STATUS:-}" ] && [ "${HTTP_STATUS}" != 000 ]; then
            rm -rf "${PENDING_CLAIM_DIR:-}"
            PENDING_CLAIM_DIR=
            if [ "${claim_generated}" -eq 1 ] && [ -n "${claim_manifest}" ]; then
              remove_secret_sidecar claim "${claim_manifest}"
            fi
          fi
          [ -z "${ARCHIVE_FILE:-}" ] || rm -f "${ARCHIVE_FILE}"
          return 1
        fi
      fi
    else
      rm -rf "${PENDING_CLAIM_DIR:-}"
      PENDING_CLAIM_DIR=
      if [ "${claim_generated}" -eq 1 ] && [ -n "${claim_manifest}" ]; then
        remove_secret_sidecar claim "${claim_manifest}"
      fi
      [ -z "${ARCHIVE_FILE:-}" ] || rm -f "${ARCHIVE_FILE}"
      return 1
    fi
  fi
  [ -z "${ARCHIVE_FILE:-}" ] || rm -f "${ARCHIVE_FILE}"
  ARCHIVE_FILE=
  location=$(header_value Location)
  final_name=${site}
  if [ "${generated}" -eq 1 ]; then
    [ -n "${location}" ] || die "server omitted Location for generated site"
    final_name=${location%/}
    final_name=${final_name##*/}
  fi
  if [ "${managed_response_lost}" -eq 1 ]; then
    emit_status "created ${base}/${site}/; response was lost, run: symbol recover"
  elif awk 'index($0, "changed: false") { found=1 } END { exit !found }' "${HTTP_BODY}"; then
    emit_status "already up to date ${base}/${final_name}/"
  elif [ "${HTTP_STATUS}" = 201 ]; then
    emit_status "created ${base}/${final_name}/"
  elif [ "${HTTP_STATUS}" = 200 ]; then
    emit_status "updated ${base}/${final_name}/"
  fi
  if [ "${managed_response_lost}" -eq 0 ]; then
    print_mutation_result "${final_name}" 1
    save_response_secrets "${base}" "${final_name}"
  fi
  if [ "${HTTP_STATUS}" = 201 ] && [ -z "${claim_manifest}" ]; then
    finish_pending_claim "${final_name}"
  elif [ "${HTTP_STATUS}" = 201 ]; then
    rm -rf "${PENDING_CLAIM_DIR:-}"
    PENDING_CLAIM_DIR=
  elif [ "${managed_response_lost}" -eq 1 ]; then
    :
  elif [ "${response_was_lost}" -eq 1 ] && [ "${HTTP_STATUS}" = 200 ]; then
    if [ -z "${claim_manifest}" ]; then
      finish_pending_claim "${final_name}"
    else
      rm -rf "${PENDING_CLAIM_DIR:-}"
      PENDING_CLAIM_DIR=
    fi
  elif [ "${HTTP_STATUS}" != 201 ]; then
    rm -rf "${PENDING_CLAIM_DIR:-}"
    PENDING_CLAIM_DIR=
    if [ "${claim_generated}" -eq 1 ] && [ -n "${claim_manifest}" ]; then
      remove_secret_sidecar claim "${claim_manifest}"
    fi
  fi
  management=$(header_value Management-Token)
  if [ -n "${management}" ] && [ -n "${claim_manifest}" ]; then
    write_secret_sidecar token "${management}" "${claim_manifest}"
  fi
  local_manifest=$(matching_manifest "${base}" "${final_name}" 2>/dev/null || true)
  [ -z "${local_manifest}" ] ||
    refresh_local_manifest "${local_manifest}" "${base}" "${final_name}"
}

refresh_local_manifest() {
  manifest=$1
  base=$2
  name=$3
  dir=$(dirname "${manifest}")
  work=$(mktemp -d) || exit 1
  preserved=${work}/preserved
  awk '/^[[:space:]]*(token|claim)[[:space:]]*=/{print}' "${manifest}" > "${preserved}"
  fresh=${work}/symbol.toml
  curl -sS -f "${base}/${name}/symbol.toml" -o "${fresh}" || {
    rm -rf "${work}"
    die "publish succeeded, but could not refresh local symbol.toml"
  }
  if [ -s "${preserved}" ]; then
    merged=${work}/merged
    awk 'BEGIN{done=0} /^[[:space:]]*\[/ && !done {while((getline l < p)>0) print l; done=1} {print}
      END{if(!done) while((getline l < p)>0) print l}' p="${preserved}" "${fresh}" > "${merged}"
    mv "${merged}" "${fresh}"
  fi
  tmp=$(mktemp "${dir}/.symbol.toml.XXXXXX") || exit 1
  cp "${fresh}" "${tmp}"
  mv "${tmp}" "${manifest}"
  rm -rf "${work}"
}

stdin_to_temp() {
  STDIN_FILE=$(mktemp) || exit 1
  cat > "${STDIN_FILE}"
}

attached_to_terminal() {
  [ -t 1 ] || [ -t 2 ]
}

imply_stdin_tty() {
  [ ! -t 0 ] && attached_to_terminal
}

imply_stdin_always() {
  [ ! -t 0 ]
}

imply_stdin_never() {
  return 1
}

resolve_imply_stdin() {
  case "${SYMBOL_STDIN:-tty}" in
    tty) IMPLY_STDIN=imply_stdin_tty ;;
    always) IMPLY_STDIN=imply_stdin_always ;;
    never) IMPLY_STDIN=imply_stdin_never ;;
    *) usage_error "SYMBOL_STDIN must be tty, always, or never" ;;
  esac
}

copy_or_move_request() {
  method=$1
  source=$2
  destination=$3
  managed=$4
  is_site_name "${source}" || usage_error "invalid source site: ${source}"
  [ -z "${destination}" ] || is_site_name "${destination}" ||
    usage_error "invalid destination site: ${destination}"
  auth=$(mktemp) || exit 1
  auth_args_file "${auth}"
  set --
  while IFS= read -r arg; do set -- "$@" "${arg}"; done < "${auth}"
  rm -f "${auth}"
  [ -z "${destination}" ] || set -- "$@" -H "Destination: /${destination}"
  PENDING_CLAIM_DIR=
  if [ "${method}" = COPY ]; then
    idempotency_key=$(random_key)
    set -- "$@" -H "Idempotency-Key: ${idempotency_key}"
    claim="sym_claim_$(random_key)$(random_key)"
    persist_pending_claim "${idempotency_key}" "${claim}"
    set -- "$@" -H "Creator-Claim: ${claim}"
    if [ "${managed}" -eq 1 ]; then
      set -- "$@" -H 'Management-Action: claim'
    fi
    persist_pending_request COPY "${HOST}" "${source}" "${destination}" "${managed}" "" "" 0 \
      "${HOST}/${source}"
  fi
  recovered_location=
  if ! http_request "${method}" "${HOST}/${source}" "$@"; then
    if [ "${method}" = COPY ] &&
      { [ -z "${HTTP_STATUS:-}" ] || [ "${HTTP_STATUS}" = 000 ]; }; then
      if [ -n "${destination}" ]; then
        if curl -fsS "${HOST}/${destination}/symbol.toml" >/dev/null 2>&1; then
          recovered_location="${HOST}/${destination}/"
        else
          return 1
        fi
      else
        http_request "${method}" "${HOST}/${source}" "$@" || return 1
      fi
    else
      rm -rf "${PENDING_CLAIM_DIR:-}"
      PENDING_CLAIM_DIR=
      return 1
    fi
  fi
  location=${recovered_location:-$(header_value Location)}
  [ -n "${location}" ] || die "server omitted Location"
  RESULT_NAME=${location%/}
  RESULT_NAME=${RESULT_NAME##*/}
  if [ -n "${recovered_location}" ]; then
    RESULT_MANAGEMENT_TOKEN=
    RESULT_CLAIM_TOKEN=
  else
    RESULT_MANAGEMENT_TOKEN=$(header_value Management-Token)
    RESULT_CLAIM_TOKEN=$(header_value Creator-Claim)
  fi
  if [ "${method}" = COPY ] && [ -n "${claim}" ]; then
    RESULT_CLAIM_TOKEN=${claim}
    finish_pending_claim "${RESULT_NAME}"
  fi
  if [ "${method}" = COPY ]; then
    emit_status "copied ${HOST}/${source}/ -> ${location}"
  else
    emit_status "moved ${HOST}/${source}/ -> ${location}"
  fi
  if [ -z "${recovered_location}" ]; then
    print_mutation_result "${RESULT_NAME}" 1
    save_response_secrets "${HOST}" "${RESULT_NAME}"
  else
    printf 'response was lost; creator claim retained for recovery\n' >&2
  fi
}

checkout_path_safe() (
  checkout_path=$1
  [ -n "${checkout_path}" ] || exit 1
  case "${checkout_path}" in
    /*|*\\*) exit 1 ;;
  esac
  case "/${checkout_path}/" in
    */../*|*/./*|*'//'*) exit 1 ;;
  esac
  printf '%s\n' "${checkout_path}" |
    awk '/[[:cntrl:]]/ { exit 1 }'
)

validate_manifest_alias_map() (
  checkout_aliases=$1
  while IFS='	' read -r checkout_alias_path checkout_alias_target; do
    [ -n "${checkout_alias_path}" ] || continue
    checkout_path_safe "${checkout_alias_path}" || exit 1
    checkout_relative=$(relative_alias_target \
      "${checkout_alias_path}" "${checkout_alias_target}") || exit 1
    checkout_canonical=$(canonical_alias_target \
      "${checkout_alias_path}" "${checkout_relative}") || exit 1
    [ "${checkout_canonical}" = "${checkout_alias_target}" ] || exit 1
  done < "${checkout_aliases}"
  validate_alias_map "${checkout_aliases}"
)

resolved_alias_map() (
  checkout_aliases=$1
  awk -F '\t' '
    function beneath(path, parent) {
      return index(path, parent "/") == 1
    }
    function substitution(path, best, candidate) {
      best = ""
      for (candidate in target) {
        if ((path == candidate || beneath(path, candidate)) &&
            length(candidate) > length(best)) best = candidate
      }
      matched = best
      return best == "" ? "" : target[best] substr(path, length(best) + 1)
    }
    { target[$1] = $2 }
    END {
      for (path in target) {
        current = target[path]
        for (hop = 0; hop <= NR; hop++) {
          resolved = substitution(current)
          if (matched == "") break
          current = resolved
        }
        print path "\t" current
      }
    }
  ' "${checkout_aliases}" | LC_ALL=C sort
)

create_checkout_symlinks() {
  checkout_root=$1
  checkout_aliases=$2
  while IFS='	' read -r checkout_alias_path checkout_alias_target; do
    [ -n "${checkout_alias_path}" ] || continue
    mkdir -p "${checkout_root}/$(dirname "./${checkout_alias_path}")"
    checkout_relative=$(relative_alias_target \
      "${checkout_alias_path}" "${checkout_alias_target}")
    safe_symlink "${checkout_relative}" \
      "${checkout_root}/${checkout_alias_path}"
  done < "${checkout_aliases}"
}

materialize_checkout_aliases() {
  checkout_root=$1
  checkout_aliases=$2
  checkout_resolved=$(mktemp) || exit 1
  resolved_alias_map "${checkout_aliases}" > "${checkout_resolved}"
  while IFS='	' read -r checkout_alias_path checkout_alias_target; do
    [ -n "${checkout_alias_path}" ] || continue
    checkout_source=${checkout_root}/${checkout_alias_target}
    checkout_destination=${checkout_root}/${checkout_alias_path}
    if [ -f "${checkout_source}" ] && [ ! -L "${checkout_source}" ]; then
      mkdir -p "$(dirname "${checkout_destination}")"
      cp -P "${checkout_source}" "${checkout_destination}"
    fi
  done < "${checkout_resolved}"
  checkout_passes=$(awk 'END { print NR + 1 }' "${checkout_resolved}")
  while [ "${checkout_passes}" -gt 0 ]; do
    while IFS='	' read -r checkout_alias_path checkout_alias_target; do
      [ -n "${checkout_alias_path}" ] || continue
      checkout_source=${checkout_root}/${checkout_alias_target}
      checkout_destination=${checkout_root}/${checkout_alias_path}
      if [ -d "${checkout_source}" ] && [ ! -L "${checkout_source}" ]; then
        rm -rf "${checkout_destination}"
        mkdir -p "${checkout_destination}"
        cp -RP "${checkout_source}/." "${checkout_destination}/"
      fi
    done < "${checkout_resolved}"
    checkout_passes=$((checkout_passes - 1))
  done
  rm -f "${checkout_resolved}"
}

extract_clone_archive() {
  clone_archive=$1
  clone_extract=$2
  clone_work=$3
  clone_manifest=${clone_work}/symbol.toml
  clone_members=${clone_work}/members
  clone_files=${clone_work}/files
  clone_aliases=${clone_work}/aliases
  if ! tar -tzf "${clone_archive}" > "${clone_members}"; then
    return 1
  fi
  if ! awk '
    {
      path = $0
      while (substr(path, 1, 2) == "./") path = substr(path, 3)
      sub(/\/$/, "", path)
      if (path == "") next
      if (path ~ /^\// || path ~ /\\/ ||
          ("/" path "/") ~ /\/\.\.?\// ||
          path ~ /[[:cntrl:]]/) exit 1
    }
  ' "${clone_members}"; then
    printf 'error: archive contains an unsafe path\n' >&2
    return 1
  fi
  if ! tar -xzOf "${clone_archive}" -- symbol.toml > "${clone_manifest}"; then
    printf 'error: archive has no readable symbol.toml\n' >&2
    return 1
  fi
  manifest_files "${clone_manifest}" > "${clone_files}"
  manifest_aliases "${clone_manifest}" > "${clone_aliases}"
  validate_manifest_alias_map "${clone_aliases}" || {
    printf 'error: archive contains an unsafe alias\n' >&2
    return 1
  }
  mkdir "${clone_extract}"
  while IFS='	' read -r clone_path _clone_hash; do
    [ -n "${clone_path}" ] || continue
    if ! checkout_path_safe "${clone_path}"; then
      printf 'error: archive manifest contains an unsafe path\n' >&2
      return 1
    fi
    clone_output=${clone_extract}/${clone_path}
    mkdir -p "$(dirname "${clone_output}")"
    clone_temporary=${clone_output}.symbol-tmp
    if ! tar -xzOf "${clone_archive}" -- "${clone_path}" > "${clone_temporary}"; then
      rm -f "${clone_temporary}"
      printf 'error: archive member is missing: %s\n' "${clone_path}" >&2
      return 1
    fi
    mv "${clone_temporary}" "${clone_output}"
  done < "${clone_files}"
  cp "${clone_manifest}" "${clone_extract}/symbol.toml"
  case "${SYMBOL_FORCE_NO_SYMLINKS:-0}" in
    0)
      clone_probe=${clone_extract}/.symbol-symlink-probe
      if safe_symlink probe-target "${clone_probe}" 2>/dev/null &&
        [ -L "${clone_probe}" ]; then
        rm -f "${clone_probe}"
        create_checkout_symlinks "${clone_extract}" "${clone_aliases}"
      else
        rm -f "${clone_probe}"
        materialize_checkout_aliases "${clone_extract}" "${clone_aliases}"
      fi
      ;;
    1)
      materialize_checkout_aliases "${clone_extract}" "${clone_aliases}"
      ;;
    *) usage_error "SYMBOL_FORCE_NO_SYMLINKS must be 0 or 1" ;;
  esac
}

clone_site() {
  name=$1
  destination=${2:-${name}}
  is_site_name "${name}" || usage_error "invalid site name: ${name}"
  [ "${destination}" != "-" ] || usage_error "clone destination cannot be -; use get ${name} -"
  if [ -e "${destination}" ]; then
    [ -d "${destination}" ] && [ -z "$(ls -A "${destination}")" ] ||
      die "clone destination exists and is not empty: ${destination}"
  fi
  destination_parent=$(dirname "${destination}")
  [ -d "${destination_parent}" ] ||
    die "clone destination parent does not exist: ${destination_parent}"
  work=$(mktemp -d "${destination_parent}/.symbol-clone.XXXXXX") || exit 1
  archive=${work}/site.tar.gz
  extract=${work}/extract
  if ! archive_transfer GET "${HOST}" "${name}" "${archive}"; then
    rm -rf "${work}"
    return 1
  fi
  if ! extract_clone_archive "${archive}" "${extract}" "${work}"; then
    rm -rf "${work}"
    printf 'error: could not extract archive\n' >&2
    return 1
  fi
  if [ -e "${destination}" ]; then
    cp -RP "${extract}"/. "${destination}"/
  else
    mv "${extract}" "${destination}"
  fi
  rm -rf "${work}"
  printf 'cloned %s/%s/ -> %s\n' "${HOST}" "${name}" "${destination}"
}

expire_help() {
  cat <<'EOF'
symbol expire: opt-in expiration for sites and files

usage:
  symbol expire NAME [PATH]                  default decay policy
  symbol expire NAME [PATH] --decay
  symbol expire NAME [PATH] --in DURATION
  symbol expire NAME [PATH] --at TIMESTAMP
  symbol expire NAME [PATH] --never
  symbol expire NAME [PATH] --show

policy:
  --min-age DURATION   retention at max-size (default 30d)
  --max-age DURATION   retention at zero size (default 365d)
  --max-size SIZE      size reaching minimum retention (default 512MiB)
  --power NUMBER       curve exponent (default 3)

default retention
 365d |\
      | \
      |  \
      |   ....
 71.9d|------------------*  256 MiB
      |                   .....
  30d |                        ...............
      +---------------------------------------
       0            256 MiB             512 MiB

expiration is disabled until explicitly enabled.
expired content remains undoable for 4 hours.
--json writes the expiry report.

option aliases:
  --info and --graph are aliases for --show.
EOF
}

json_string() {
  key=$1
  awk -v key="${key}" '
    {
      text = text $0
    }
    END {
      token = "\"" key "\""
      start = index(text, token)
      if (!start) exit 1
      text = substr(text, start + length(token))
      sub(/^[[:space:]]*:[[:space:]]*/, "", text)
      if (substr(text, 1, 1) == "\"") {
        text = substr(text, 2)
        finish = index(text, "\"")
        print substr(text, 1, finish - 1)
      } else {
        split(text, values, /[,}]/)
        gsub(/[[:space:]]/, "", values[1])
        print values[1]
      }
    }
  '
}

json_limited_by() {
  awk '
    {
      text = text $0
    }
    END {
      token = "\"limited_by\":"
      start = index(text, token)
      if (!start) exit
      object = substr(text, start + length(token))
      sub(/^[[:space:]]*/, "", object)
      if (substr(object, 1, 4) == "null") exit
      kind = field(object, "kind")
      path = field(object, "path")
      if (path == "null" || path == "") print kind
      else print kind " " path
    }
    function field(object, key, token, value) {
      token = "\"" key "\":"
      value = substr(object, index(object, token) + length(token))
      sub(/^[[:space:]]*/, "", value)
      if (substr(value, 1, 1) == "\"") {
        value = substr(value, 2)
        sub(/".*$/, "", value)
      } else {
        sub(/[,}].*$/, "", value)
      }
      return value
    }
  '
}

human_duration() {
  awk -v seconds="${1:-0}" '
    BEGIN {
      seconds = int(seconds)
      days = int(seconds / 86400); seconds %= 86400
      hours = int(seconds / 3600); seconds %= 3600
      minutes = int(seconds / 60); seconds %= 60
      if (days) printf "%dd", days
      if (hours) printf "%s%dh", days ? " " : "", hours
      if (minutes) printf "%s%dm", (days || hours) ? " " : "", minutes
      if (seconds || !(days || hours || minutes))
        printf "%s%ds", (days || hours || minutes) ? " " : "", seconds
      print ""
    }
  '
}

human_bytes() {
  awk -v bytes="${1:-0}" '
    BEGIN {
      split("B KiB MiB GiB TiB PiB", units, " ")
      value = bytes + 0
      unit = 1
      while (value >= 1024 && unit < 6) {
        value /= 1024
        unit++
      }
      if (unit == 1) printf "%d %s\n", value, units[unit]
      else if (value >= 100) printf "%.0f %s\n", value, units[unit]
      else if (value >= 10) printf "%.1f %s\n", value, units[unit]
      else printf "%.2f %s\n", value, units[unit]
    }
  '
}

print_inherited_expiry_caps() {
  awk '
    {
      text = text $0
    }
    END {
      marker = "\"inherited_caps\":["
      start = index(text, marker)
      if (!start) exit
      rest = substr(text, start + length(marker))
      finish = index(rest, "]")
      if (!finish) exit
      rest = substr(rest, 1, finish - 1)
      while (match(rest, /\{[^}]*\}/)) {
        object = substr(rest, RSTART, RLENGTH)
        kind = field(object, "kind")
        path = field(object, "path")
        expires = field(object, "expires_at")
        if (path == "null" || path == "") path = "(site)"
        if (expires != "") printf "inherited cap:      %s %s at %s\n", kind, path, expires
        rest = substr(rest, RSTART + RLENGTH)
      }
      limit = text
      sub(/^.*"limited_by":[[:space:]]*/, "", limit)
      if (substr(limit, 1, 4) != "null") {
        kind = field(limit, "kind")
        path = field(limit, "path")
        if (path == "null" || path == "") path = "(site)"
        if (kind != "") printf "limited by:         %s %s\n", kind, path
      }
    }
    function field(object, key, token, value) {
      token = "\"" key "\":"
      value = substr(object, index(object, token) + length(token))
      sub(/^[[:space:]]*/, "", value)
      if (substr(value, 1, 1) == "\"") {
        value = substr(value, 2)
        sub(/".*$/, "", value)
      } else {
        sub(/[,}].*$/, "", value)
      }
      return value
    }
  ' "${HTTP_BODY}"
}

print_expiry_site_report() {
  awk '
    {
      text = text $0
    }
    END {
      site = string_field(text, "site")
      printf "expiry policies for %s\n", site
      printf "%-28s %-9s %-22s %s\n", "TARGET", "MODE", "EXPIRES", "LIMITED BY"
      marker = "\"entries\":["
      start = index(text, marker)
      if (!start) exit
      rest = substr(text, start + length(marker))
      depth = 0
      quoted = 0
      escaped = 0
      object = ""
      for (i = 1; i <= length(rest); i++) {
        character = substr(rest, i, 1)
        if (quoted) {
          if (escaped) escaped = 0
          else if (character == "\\") escaped = 1
          else if (character == "\"") quoted = 0
        } else if (character == "\"") {
          quoted = 1
        } else if (character == "{") {
          depth++
        } else if (character == "}") {
          depth--
        } else if (character == "]" && depth == 0) {
          break
        }
        if (depth > 0 || object != "") object = object character
        if (depth == 0 && object != "") {
          render(object, site)
          object = ""
        }
      }
    }
    function string_field(object, key, token, value) {
      token = "\"" key "\":"
      if (!index(object, token)) return ""
      value = substr(object, index(object, token) + length(token))
      sub(/^[[:space:]]*/, "", value)
      if (substr(value, 1, 1) != "\"") {
        sub(/[,}].*$/, "", value)
        return value
      }
      value = substr(value, 2)
      sub(/".*$/, "", value)
      return value
    }
    function human(seconds, days, hours, minutes, result) {
      seconds = int(seconds)
      days = int(seconds / 86400); seconds %= 86400
      hours = int(seconds / 3600); seconds %= 3600
      minutes = int(seconds / 60); seconds %= 60
      if (days) result = days "d"
      if (hours) result = result (result ? " " : "") hours "h"
      if (minutes) result = result (result ? " " : "") minutes "m"
      if (seconds || !result) result = result (result ? " " : "") seconds "s"
      return result
    }
    function render(object, site, path, target, mode, expires, remaining, limit, limited) {
      path = string_field(object, "path")
      target = site (path == "null" || path == "" ? "/" : "/" path)
      mode = string_field(object, "mode")
      expires = string_field(object, "effective_expires_at")
      remaining = string_field(object, "remaining_seconds")
      limit = object
      sub(/^.*"limited_by":[[:space:]]*/, "", limit)
      if (substr(limit, 1, 4) == "null") {
        limited = "-"
      } else {
        limited = string_field(limit, "kind")
        path = string_field(limit, "path")
        if (path != "null" && path != "") limited = limited " " path
      }
      if (expires == "null" || expires == "") expires = "disabled"
      else expires = expires " (in " human(remaining) ")"
      printf "%-28s %-9s %-22s %s\n", target, mode, expires, limited
    }
  ' "${HTTP_BODY}"
}

print_expire_report() {
  if awk 'index($0, "\"entries\":[") { found=1 } END { exit !found }' "${HTTP_BODY}"; then
    print_expiry_site_report
    return
  fi
  report_site=$(json_string site < "${HTTP_BODY}" 2>/dev/null || true)
  report_path=$(json_string path < "${HTTP_BODY}" 2>/dev/null || true)
  expires=$(json_string effective_expires_at < "${HTTP_BODY}" 2>/dev/null || true)
  remaining=$(json_string remaining_seconds < "${HTTP_BODY}" 2>/dev/null || true)
  mode=$(json_string mode < "${HTTP_BODY}" 2>/dev/null || true)
  size=$(json_string size < "${HTTP_BODY}" 2>/dev/null || true)
  refreshed=$(json_string refreshed_at < "${HTTP_BODY}" 2>/dev/null || true)
  own_expires=$(json_string expires_at < "${HTTP_BODY}" 2>/dev/null || true)
  retention=$(json_string retention_seconds < "${HTTP_BODY}" 2>/dev/null || true)
  min_age=$(json_string min_age_seconds < "${HTTP_BODY}" 2>/dev/null || true)
  max_age=$(json_string max_age_seconds < "${HTTP_BODY}" 2>/dev/null || true)
  max_size=$(json_string max_size_bytes < "${HTTP_BODY}" 2>/dev/null || true)
  power=$(json_string power < "${HTTP_BODY}" 2>/dev/null || true)
  if [ -z "${expires}" ] || [ "${expires}" = null ]; then
    printf 'expiration disabled\n'
    return
  fi
  size_h=$(human_bytes "${size:-0}")
  remaining_h=$(human_duration "${remaining:-0}")
  retention_h=
  [ -z "${retention}" ] || [ "${retention}" = null ] ||
    retention_h=$(human_duration "${retention}")
  min_h=
  [ -z "${min_age}" ] || [ "${min_age}" = null ] ||
    min_h=$(human_duration "${min_age}")
  max_h=
  [ -z "${max_age}" ] || [ "${max_age}" = null ] ||
    max_h=$(human_duration "${max_age}")
  max_size_h=
  [ -z "${max_size}" ] || [ "${max_size}" = null ] ||
    max_size_h=$(human_bytes "${max_size}")
  if [ -n "${report_site}" ] && [ "${report_site}" != null ]; then
    if [ -n "${report_path}" ] && [ "${report_path}" != null ]; then
      printf 'effective lifetime: %s/%s\n' "${report_site}" "${report_path}"
    else
      printf 'effective lifetime: %s/\n' "${report_site}"
    fi
  else
    printf 'effective lifetime\n'
  fi
  [ -z "${size}" ] || printf 'size:              %s\n' "${size_h}"
  if [ -n "${mode}" ] && [ "${mode}" != null ]; then
    if [ "${mode}" = decay ] && [ -n "${min_age}" ] && [ -n "${max_age}" ]; then
      printf 'policy:            decay (%s..%s @ %s ^%s)\n' \
        "${min_h}" "${max_h}" "${max_size_h:-unknown}" "${power:-unknown}"
    else
      printf 'policy:            %s\n' "${mode}"
    fi
  fi
  [ -z "${retention}" ] || [ "${retention}" = null ] ||
    printf 'policy retention:  %s\n' "${retention_h}"
  [ -z "${refreshed}" ] || [ "${refreshed}" = null ] ||
    printf 'refreshed:         %s\n' "${refreshed}"
  [ -z "${own_expires}" ] || [ "${own_expires}" = null ] ||
    printf 'own expiry:        %s\n' "${own_expires}"
  printf 'effective expiry:  %s (in %s)\n' "${expires}" "${remaining_h}"
  print_inherited_expiry_caps
  if [ "${mode}" = decay ]; then
    awk -v size="${size:-0}" -v maximum="${max_size:-0}" \
      -v min="${min_h:-min}" -v max="${max_h:-max}" \
      -v current="${size_h}" -v maximum_label="${max_size_h:-unknown}" '
      BEGIN {
        width=37
        position=maximum > 0 ? int(size / maximum * (width - 1)) : 0
        if (position < 0) position=0
        if (position >= width) position=width-1
        line=""
        for (i=0;i<width;i++) line=line (i==position ? "*" : (i<position ? "-" : "."))
        printf "\nretention by size\n"
        printf "%8s |\\\n", max
        printf "         | %s  you are here: %s\n", line, current
        printf "%8s |%s\n", min, "....................................."
        printf "      +-------------------------------------\n"
        printf "       0                           %s\n", maximum_label
      }
    '
  fi
  elapsed=0
  if [ -n "${retention}" ] && [ "${retention}" != null ] && [ -n "${remaining}" ]; then
    elapsed=$((retention > remaining ? retention - remaining : 0))
  fi
  elapsed_h=$(human_duration "${elapsed}")
  awk -v remaining="${remaining:-0}" -v retention="${retention:-0}" \
    -v elapsed_label="${elapsed_h}" -v remaining_label="${remaining_h}" '
    BEGIN {
      width=37
      elapsed=retention > remaining ? retention-remaining : 0
      position=retention > 0 ? int(elapsed/retention*(width-1)) : 0
      if (position < 0) position=0
      if (position >= width) position=width-1
      line=""
      for (i=0;i<width;i++) line=line (i==position ? "*" : (i<position ? "=" : "-"))
      printf "\neffective lifetime\n"
      printf "refreshed |%s| expires\n", line
      printf "          %s elapsed; %s remaining\n", elapsed_label, remaining_label
    }
  '
}

duration_valid() {
  case "$1" in
    *[!0-9smhdw]*|'') return 1 ;;
    *s|*m|*h|*d|*w) number=${1%?}; case "${number}" in ''|*[!0-9]*|0) return 1 ;; esac ;;
    *) return 1 ;;
  esac
}

size_valid() {
  case "$1" in
    [1-9][0-9]*|[1-9][0-9]*B|[1-9][0-9]*KB|[1-9][0-9]*MB|[1-9][0-9]*GB|\
    [1-9][0-9]*KiB|[1-9][0-9]*MiB|[1-9][0-9]*GiB) return 0 ;;
    *) return 1 ;;
  esac
}

manifest_files() {
  awk '
    /^[[:space:]]*\[files\][[:space:]]*$/ { in_files=1; next }
    /^[[:space:]]*\[/ { in_files=0 }
    in_files {
      line=$0
      eq=index(line, "=")
      if (!eq) next
      path=substr(line, 1, eq-1)
      hash=substr(line, eq+1)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", path)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", hash)
      if (path ~ /^".*"$/ && hash ~ /^".*"$/) {
        path=substr(path,2,length(path)-2)
        hash=substr(hash,2,length(hash)-2)
        print path "\t" hash
      }
    }
  ' "$1"
}

manifest_aliases() {
  awk '
    /^[[:space:]]*\[aliases\][[:space:]]*$/ { in_aliases=1; next }
    /^[[:space:]]*\[/ { in_aliases=0 }
    in_aliases {
      line=$0
      eq=index(line, "=")
      if (!eq) next
      path=substr(line, 1, eq-1)
      target=substr(line, eq+1)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", path)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", target)
      if (path ~ /^".*"$/ && target ~ /^".*"$/) {
        path=substr(path,2,length(path)-2)
        target=substr(target,2,length(target)-2)
        print path "\t" target
      }
    }
  ' "$1"
}

baseline_entry_map() {
  baseline_manifest=$1
  manifest_files "${baseline_manifest}" |
    awk -F '\t' '{ print $1 "\tF\t" $2 }'
  manifest_aliases "${baseline_manifest}" |
    awk -F '\t' '{ print $1 "\tA\t" $2 }'
}

filtered_local_files() (
  filtered_root=$1
  filtered_aliases=$2
  filtered_candidates=$(mktemp) || exit 1
  trap 'rm -f "${filtered_candidates}"' EXIT HUP INT TERM
  (
    cd "${filtered_root}"
    find . -type f ! -path './.git/*' ! -name symbol.toml \
      ! -name .symbol-token ! -name .symbol-claim -print | LC_ALL=C sort
  ) | while IFS= read -r filtered_path; do
    printf '%s\n' "${filtered_path#./}"
  done > "${filtered_candidates}"
  awk -F '\t' '
    FILENAME == ARGV[1] { aliases[$1] = 1; next }
    {
      for (alias in aliases)
        if ($0 == alias || index($0, alias "/") == 1) next
      print
    }
  ' "${filtered_aliases}" "${filtered_candidates}"
)

local_file_map_b3sum() (
  local_root=$1
  local_aliases=$2
  filtered_local_files "${local_root}" "${local_aliases}" |
  while IFS= read -r local_relative; do
    local_hash=$(b3sum "${local_root}/${local_relative}" |
      awk '{print "blake3:" $1}')
    printf '%s\tF\t%s\n' "${local_relative}" "${local_hash}"
  done
)

local_file_map_blake3() (
  local_root=$1
  local_aliases=$2
  filtered_local_files "${local_root}" "${local_aliases}" |
  while IFS= read -r local_relative; do
    local_hash=$(blake3 "${local_root}/${local_relative}" |
      awk '{print "blake3:" $1}')
    printf '%s\tF\t%s\n' "${local_relative}" "${local_hash}"
  done
)

local_file_map_remote() (
  local_root=$1
  local_aliases=$2
  local_baseline=$3
  local_base=$4
  local_site=$5
  filtered_local_files "${local_root}" "${local_aliases}" |
  while IFS= read -r local_relative; do
    local_hash=$(awk -F '\t' -v wanted="${local_relative}" \
      '$1 == wanted && $2 == "F" { print $3; exit }' "${local_baseline}")
    if [ -z "${local_hash}" ]; then
      local_hash="new:${local_relative}"
    else
      local_remote=$(mktemp) || exit 1
      local_encoded=$(urlencode_path "${local_relative}")
      curl -fsS "${local_base}/${local_site}/${local_encoded}" \
        -o "${local_remote}" ||
        die "could not compare local file with upstream: ${local_relative}"
      if ! cmp -s "${local_root}/${local_relative}" "${local_remote}"; then
        local_hash="changed:${local_relative}"
      fi
      rm -f "${local_remote}"
    fi
    printf '%s\tF\t%s\n' "${local_relative}" "${local_hash}"
  done
)

local_entry_map() (
  local_root=$1
  local_manifest=$2
  local_baseline=$3
  local_base=$4
  local_site=$5
  local_aliases=$(mktemp) || exit 1
  local_files=$(mktemp) || exit 1
  trap 'rm -f "${local_aliases}" "${local_files}"' EXIT HUP INT TERM
  local_alias_map "${local_root}" "${local_manifest}" > "${local_aliases}"
  if command -v b3sum >/dev/null 2>&1; then
    local_file_map_b3sum "${local_root}" "${local_aliases}" > "${local_files}"
  elif command -v blake3 >/dev/null 2>&1; then
    local_file_map_blake3 "${local_root}" "${local_aliases}" > "${local_files}"
  else
    local_file_map_remote "${local_root}" "${local_aliases}" \
      "${local_baseline}" "${local_base}" "${local_site}" > "${local_files}"
  fi
  cat "${local_files}"
  awk -F '\t' '{ print $1 "\tA\t" $2 }' "${local_aliases}"
)

upstream_entry_map() {
  awk '
    {
      text=text $0
    }
    END {
      rest=text
      while (match(rest, /\{"path"[[:space:]]*:[[:space:]]*"[^"]*"[^}]*"hash"[[:space:]]*:[[:space:]]*"[^"]*"/)) {
        object=substr(rest,RSTART,RLENGTH)
        path=object
        sub(/^.*"path"[[:space:]]*:[[:space:]]*"/,"",path)
        sub(/".*$/,"",path)
        hash=object
        sub(/^.*"hash"[[:space:]]*:[[:space:]]*"/,"",hash)
        sub(/".*$/,"",hash)
        if (path != "symbol.toml") print path "\tF\t" hash
        rest=substr(rest,RSTART+RLENGTH)
      }
      rest=text
      while (match(rest, /\{"path"[[:space:]]*:[[:space:]]*"[^"]*"[^}]*"target"[[:space:]]*:[[:space:]]*"[^"]*"/)) {
        object=substr(rest,RSTART,RLENGTH)
        path=object
        sub(/^.*"path"[[:space:]]*:[[:space:]]*"/,"",path)
        sub(/".*$/,"",path)
        target=object
        sub(/^.*"target"[[:space:]]*:[[:space:]]*"/,"",target)
        sub(/".*$/,"",target)
        print path "\tA\t" target
        rest=substr(rest,RSTART+RLENGTH)
      }
      rest=text
      while (match(rest, /\{"path"[[:space:]]*:[[:space:]]*"[^"]*"[^}]*"canonical_target"[[:space:]]*:[[:space:]]*"[^"]*"/)) {
        object=substr(rest,RSTART,RLENGTH)
        path=object
        sub(/^.*"path"[[:space:]]*:[[:space:]]*"/,"",path)
        sub(/".*$/,"",path)
        target=object
        sub(/^.*"canonical_target"[[:space:]]*:[[:space:]]*"/,"",target)
        sub(/".*$/,"",target)
        print path "\tA\t" target
        rest=substr(rest,RSTART+RLENGTH)
      }
    }
  ' "$1" | LC_ALL=C sort
}

json_quote() (
  LC_ALL=C
  export LC_ALL
  awk '
    {
      value = $0
      gsub(/\\/, "\\\\", value)
      gsub(/"/, "\\\"", value)
      printf "\"%s\"", value
    }
  '
)

put_alias_request() {
  alias_site=$1
  shift
  is_site_name "${alias_site}" ||
    usage_error "invalid site name: ${alias_site}"
  [ "$#" -ge 2 ] && [ $(( $# % 2 )) -eq 0 ] ||
    usage_error "usage: symbol alias SITE PATH TARGET [PATH TARGET ...]"
  alias_work=$(mktemp -d) || exit 1
  alias_specs=${alias_work}/specs
  alias_graph=${alias_work}/graph
  : > "${alias_specs}"
  : > "${alias_graph}"
  alias_count=0
  while [ "$#" -gt 0 ]; do
    alias_path=$1
    alias_target=$2
    shift 2
    validate_remote_path "${alias_path}"
    alias_canonical=$(canonical_alias_target "${alias_path}" "${alias_target}") ||
      usage_error "invalid alias target: ${alias_path} -> ${alias_target}"
    printf '%s\t%s\n' "${alias_path}" "${alias_target}" >> "${alias_specs}"
    printf '%s\t%s\n' "${alias_path}" "${alias_canonical}" >> "${alias_graph}"
    alias_count=$((alias_count + 1))
  done
  validate_alias_map "${alias_graph}" ||
    usage_error "alias batch contains a cycle or write through alias"
  alias_manifest=$(matching_manifest "${HOST}" "${alias_site}" 2>/dev/null || true)
  alias_conditional=
  if [ -n "${alias_manifest}" ]; then
    alias_conditional=$(manifest_value tree_hash "${alias_manifest}" 2>/dev/null || true)
  fi
  alias_idempotency=$(random_key)
  alias_auth=$(mktemp) || exit 1
  auth_args_file "${alias_auth}"
  set -- -H "Idempotency-Key: ${alias_idempotency}"
  [ -z "${alias_conditional}" ] ||
    set -- "$@" -H "If-Match: ${alias_conditional}"
  while IFS= read -r alias_arg; do set -- "$@" "${alias_arg}"; done < "${alias_auth}"
  rm -f "${alias_auth}"
  if [ "${alias_count}" -eq 1 ]; then
    IFS='	' read -r alias_path alias_target < "${alias_specs}"
    alias_encoded=$(urlencode_path "${alias_path}")
    alias_url=${HOST}/${alias_site}/${alias_encoded}
    set -- "$@" -H "Alias-Target: ${alias_target}"
    alias_attempt() {
      http_request ALIAS "${alias_url}" "$@"
    }
  else
    alias_body=${alias_work}/aliases.json
    {
      printf '{"aliases":['
      alias_separator=
      while IFS='	' read -r alias_path alias_target; do
        printf '%s{"path":%s,"target":%s}' "${alias_separator}" \
          "$(printf '%s\n' "${alias_path}" | json_quote)" \
          "$(printf '%s\n' "${alias_target}" | json_quote)"
        alias_separator=,
      done < "${alias_specs}"
      printf ']}\n'
    } > "${alias_body}"
    alias_url=${HOST}/${alias_site}/
    set -- "$@" -H 'Content-Type: application/json'
    alias_attempt() {
      http_request ALIAS "${alias_url}" "$@" \
        --data-binary "@${alias_body}"
    }
  fi
  alias_failed=0
  if ! alias_attempt "$@"; then
    if [ -z "${HTTP_STATUS:-}" ] || [ "${HTTP_STATUS}" = 000 ]; then
      alias_attempt "$@" || alias_failed=1
    else
      alias_failed=1
    fi
  fi
  if [ "${alias_failed}" -eq 1 ]; then
    rm -rf "${alias_work}"
    return 1
  fi
  if [ "${JSON_OUTPUT}" -eq 1 ]; then
    cat "${HTTP_BODY}"
    last=$(tail -c 1 "${HTTP_BODY}" 2>/dev/null || true)
    [ -z "${last}" ] || printf '\n'
  else
    print_mutation_result "${alias_site}" 1
    if [ "${alias_count}" -eq 1 ]; then
      printf 'aliased %s/%s/%s -> %s\n' \
        "${HOST}" "${alias_site}" "${alias_path}" "${alias_target}"
    else
      printf 'aliased %s paths atomically in %s/%s/\n' \
        "${alias_count}" "${HOST}" "${alias_site}"
    fi
  fi
  rm -rf "${alias_work}"
  [ -z "${alias_manifest}" ] ||
    refresh_local_manifest "${alias_manifest}" "${HOST}" "${alias_site}"
}

sync_project() {
  check=$1
  manifest_target
  baseline_tree=$(manifest_value tree_hash "${MANIFEST}") ||
    die "symbol.toml has no sync tree_hash baseline; clone the site again"
  baseline_revision=$(manifest_value content_revision "${MANIFEST}") ||
    die "symbol.toml has no content_revision baseline; clone the site again"
  work=$(mktemp -d) || exit 1
  baseline=${work}/baseline
  localmap=${work}/local
  upstream=${work}/upstream
  changed=${work}/changed
  baseline_entry_map "${MANIFEST}" | LC_ALL=C sort > "${baseline}"
  local_entry_map "${MANIFEST_DIR}" "${MANIFEST}" "${baseline}" \
    "${MANIFEST_HOST}" "${MANIFEST_NAME}" | LC_ALL=C sort > "${localmap}"
  if ! http_request GET "${MANIFEST_HOST}/${MANIFEST_NAME}/FILES" -H 'Accept: application/json'; then
    rm -rf "${work}"
    return 1
  fi
  upstream_tree=$(json_string tree_hash < "${HTTP_BODY}")
  upstream_revision=$(json_string content_revision < "${HTTP_BODY}")
  upstream_entry_map "${HTTP_BODY}" > "${upstream}"
  if [ "${upstream_tree}" != "${baseline_tree}" ] || ! cmp -s "${baseline}" "${upstream}"; then
    printf 'error: upstream changed since this checkout\n\n' >&2
    printf 'checkout base: revision %s  %s\n' "${baseline_revision}" "${baseline_tree}" >&2
    printf 'upstream now:  revision %s  %s\n' "${upstream_revision}" "${upstream_tree}" >&2
    printf '\nlocal changes:\n' >&2
    awk -F '\t' '
      function label(path, kind, value) {
        return kind == "A" ? path " -> " value : path
      }
      NR==FNR { base[$1]=$2 "\t" $3; base_kind[$1]=$2; base_value[$1]=$3; next }
      {
        local[$1]=$2 "\t" $3
        if (!($1 in base)) print "  + " label($1,$2,$3)
        else if (base[$1] != local[$1]) print "  M " label($1,$2,$3)
      }
      END {
        for (path in base) if (!(path in local))
          print "  - " label(path,base_kind[path],base_value[path])
      }
    ' "${baseline}" "${localmap}" | LC_ALL=C sort -k2,2 >&2
    printf '\nupstream changes:\n' >&2
    awk -F '\t' '
      function label(path, kind, value) {
        return kind == "A" ? path " -> " value : path
      }
      NR==FNR { base[$1]=$2 "\t" $3; base_kind[$1]=$2; base_value[$1]=$3; next }
      {
        remote[$1]=$2 "\t" $3
        if (!($1 in base)) print "  + " label($1,$2,$3)
        else if (base[$1] != remote[$1]) print "  M " label($1,$2,$3)
      }
      END {
        for (path in base) if (!(path in remote))
          print "  - " label(path,base_kind[path],base_value[path])
      }
    ' "${baseline}" "${upstream}" | LC_ALL=C sort -k2,2 >&2
    printf 'refusing to modify %s/%s/\n' "${MANIFEST_HOST}" "${MANIFEST_NAME}" >&2
    printf 'resolve manually with:\n' >&2
    printf '  symbol clone %s ../%s-upstream\n' "${MANIFEST_NAME}" "${MANIFEST_NAME}" >&2
    printf '  diff -ru . ../%s-upstream\n' "${MANIFEST_NAME}" >&2
    printf '  symbol put ...\n  symbol rm ...\n' >&2
    rm -rf "${work}"
    return 1
  fi
  awk -F '\t' '
    NR==FNR {
      base[$1]=$2 "\t" $3
      base_kind[$1]=$2
      base_value[$1]=$3
      next
    }
    {
      local[$1]=$2 "\t" $3
      if (!($1 in base)) print "+\t" $1 "\t" $2 "\t" $3
      else if (base[$1] != local[$1]) print "M\t" $1 "\t" $2 "\t" $3
    }
    END {
      for (path in base) if (!(path in local))
        print "-\t" path "\t" base_kind[path] "\t" base_value[path]
    }
  ' "${baseline}" "${localmap}" | LC_ALL=C sort -k2,2 > "${changed}"
  printf 'upstream: unchanged @ revision %s\n\n' "${upstream_revision}"
  additions=$(awk -F '\t' '$1=="+" || $1=="M" {n++} END {print n+0}' "${changed}")
  added=$(awk -F '\t' '$1=="+" {n++} END {print n+0}' "${changed}")
  modified=$(awk -F '\t' '$1=="M" {n++} END {print n+0}' "${changed}")
  alias_changes=$(awk -F '\t' \
    '($1=="+" || $1=="M") && $3=="A" {n++} END {print n+0}' "${changed}")
  if [ "${additions}" -gt 0 ]; then
    [ "${check}" -eq 0 ] && printf 'local changes:\n' || printf 'would sync:\n'
    awk -F '\t' '
      $1=="+" || $1=="M" {
        printf "  %s %s%s\n",$1,$2,($3=="A" ? " -> " $4 : "")
      }
    ' "${changed}"
  fi
  deletions=$(awk -F '\t' '$1=="-" {n++} END {print n+0}' "${changed}")
  if [ "${deletions}" -gt 0 ]; then
    printf '\n'
    [ "${check}" -eq 0 ] && printf 'local deletions ignored:\n' || printf 'would ignore local deletion:\n'
    awk -F '\t' -v name="${MANIFEST_NAME}" \
      '$1=="-" {
        printf "  - %s%s  (use: symbol rm %s %s)\n",
          $2,($3=="A" ? " -> " $4 : ""),name,$2
      }' "${changed}"
  fi
  if [ "${check}" -eq 1 ]; then
    printf '\nno changes made\n'
    rm -rf "${work}"
    return
  fi
  if [ "${additions}" -eq 0 ]; then
    printf '\nnothing to sync\n'
    rm -rf "${work}"
    return
  fi
  stage=${work}/stage
  mkdir "${stage}"
  awk -F '\t' '($1=="+" || $1=="M") && $3=="F" {print $2}' "${changed}" |
  while IFS= read -r path; do
    mkdir -p "${stage}/$(dirname "./${path}")"
    cp -P "${MANIFEST_DIR}/${path}" "${stage}/${path}"
  done
  awk -F '\t' '($1=="+" || $1=="M") && $3=="A" {print $2 "\t" $4}' \
    "${changed}" |
  while IFS='	' read -r path target; do
    mkdir -p "${stage}/$(dirname "./${path}")"
    relative_target=$(relative_alias_target "${path}" "${target}")
    safe_symlink "${relative_target}" "${stage}/${path}"
  done
  sync_single_root=$(awk -F '\t' '
    ($1=="+" || $1=="M") && $3=="F" {
      slash=index($2,"/")
      if (!slash) safe=1
      else roots[substr($2,1,slash-1)]=1
    }
    END {
      for (root in roots) { count++; only=root }
      if (!safe && count==1) print only
    }
  ' "${changed}")
  if [ -n "${sync_single_root}" ]; then
    printf '# partial sync root anchor\n' > "${stage}/symbol.toml"
  fi
  archive=${work}/changed.tar.gz
  tar -czf "${archive}" -C "${stage}" -- .
  printf '\nsyncing %s entries (%s added, %s modified, %s aliases)...\n' \
    "${additions}" "${added}" "${modified}" "${alias_changes}"
  auth=$(mktemp) || exit 1
  auth_args_file "${auth}"
  set -- -H 'Content-Type: application/gzip' -H 'Unpack: 1' \
    -H "If-Match: ${baseline_tree}" -H "Idempotency-Key: $(random_key)"
  while IFS= read -r arg; do set -- "$@" "${arg}"; done < "${auth}"
  rm -f "${auth}"
  sync_failed=0
  sync_response_lost=0
  sync_verified=0
  sync_retry_error=${work}/retry-error
  if ! http_request PUT "${MANIFEST_HOST}/${MANIFEST_NAME}" "$@" -T "${archive}"; then
    if [ -z "${HTTP_STATUS:-}" ] || [ "${HTTP_STATUS}" = 000 ]; then
      sync_response_lost=1
      http_request PUT "${MANIFEST_HOST}/${MANIFEST_NAME}" "$@" \
        -T "${archive}" 2>"${sync_retry_error}" || sync_failed=1
    else
      sync_failed=1
    fi
  fi
  if [ "${sync_failed}" -eq 1 ] && [ "${sync_response_lost}" -eq 1 ] &&
    [ "${HTTP_STATUS:-}" = 412 ]; then
    recovered_upstream=${work}/recovered-upstream
    if http_request GET "${MANIFEST_HOST}/${MANIFEST_NAME}/FILES" \
      -H 'Accept: application/json'; then
      upstream_entry_map "${HTTP_BODY}" | LC_ALL=C sort > "${recovered_upstream}"
      sync_verify_failed=0
      while IFS='	' read -r sync_action sync_path sync_kind sync_value; do
        [ "${sync_action}" != "-" ] || continue
        case "${sync_kind}" in
          F)
            sync_remote=$(mktemp) || exit 1
            sync_encoded=$(urlencode_path "${sync_path}")
            # -L for the same reason as the file PUT verification: an
            # unambiguous .html redirects to its canonical URL.
            if ! curl -fsSL \
              "${MANIFEST_HOST}/${MANIFEST_NAME}/${sync_encoded}" \
              -o "${sync_remote}"; then
              printf 'error: could not verify synced path after response loss: %s\n' \
                "${sync_path}" >&2
              sync_verify_failed=1
            elif ! cmp -s "${MANIFEST_DIR}/${sync_path}" "${sync_remote}"; then
              printf 'error: synced path differs after response loss: %s\n' \
                "${sync_path}" >&2
              sync_verify_failed=1
            fi
            rm -f "${sync_remote}"
            ;;
          A)
            if ! awk -F '	' -v path="${sync_path}" -v target="${sync_value}" '
              $1 == path && $2 == "A" && $3 == target { found=1 }
              END { exit !found }
            ' "${recovered_upstream}"; then
              sync_verify_failed=1
            fi
            ;;
        esac
      done < "${changed}"
      if [ "${sync_verify_failed}" -eq 0 ]; then
        sync_failed=0
        sync_verified=1
      else
        printf 'error: changed entries after response loss do not match the checkout\n' >&2
      fi
    fi
  fi
  if [ "${sync_failed}" -eq 1 ]; then
    [ ! -s "${sync_retry_error}" ] || cat "${sync_retry_error}" >&2
    if [ "${HTTP_STATUS:-}" = 412 ]; then
      printf 'error: upstream changed during sync; nothing was written\n' >&2
    fi
    rm -rf "${work}"
    return 1
  fi
  if [ "${sync_verified}" -eq 1 ]; then
    printf 'warning: sync response was lost; remote tree verified, but no undo token is available\n' >&2
  else
    print_mutation_result "${MANIFEST_NAME}"
  fi
  sync_work=${work}
  refresh_local_manifest "${MANIFEST}" "${MANIFEST_HOST}" "${MANIFEST_NAME}"
  printf 'synced %s/%s/ (%s added, %s modified)\n' \
    "${MANIFEST_HOST}" "${MANIFEST_NAME}" "${added}" "${modified}"
  rm -rf "${sync_work}"
}

if [ "${cmd}" != recover ]; then
  pending_root=${XDG_STATE_HOME:-${HOME}/.local/state}/symbol/claims
  for pending in "${pending_root}"/pending-*; do
    [ -d "${pending}" ] || continue
    printf 'pending Symbol operation found; run: symbol recover\n' >&2
    break
  done
fi

if [ "${cmd}" = help ] && [ "$#" -gt 0 ]; then
  help_command=$(resolve_command "$1") || exit $?
  command_help "${help_command}"
  exit 0
fi
case "${1:-}" in
  -h|--help)
    command_help "${cmd}"
    exit 0
    ;;
esac
select_token
start_update_check "${cmd}"
trap join_update_check EXIT

case "${cmd}" in
  put)
    unpack=0
    managed=0
    forced=
    REPLACE_SITE=0
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -u|--unpack) unpack=1; shift ;;
        --managed) managed=1; shift ;;
        --replace) REPLACE_SITE=1; shift ;;
        -f)
          [ "$#" -ge 2 ] || usage_error "-f requires a remote path"
          forced=$2
          [ "${forced}" != "-" ] || usage_error "remote file path cannot be -"
          shift 2
          ;;
        --) shift; break ;;
        -) break ;;
        -*) usage_error "unknown flag: $1" ;;
        *) break ;;
      esac
    done
    [ "$#" -le 3 ] || usage_error "put accepts at most NAME FILE DEST"
    piped=0
    explicit_stdin=0
    [ "$#" -eq 0 ] || [ "$1" != "-" ] || explicit_stdin=1
    resolve_imply_stdin
    if [ "${explicit_stdin}" -eq 1 ]; then
      stdin_to_temp
      piped=1
    elif { [ -n "${forced}" ] || [ "$#" -eq 0 ]; } && "${IMPLY_STDIN}"; then
      stdin_to_temp
      if [ -s "${STDIN_FILE}" ]; then
        piped=1
      else
        rm -f "${STDIN_FILE}"
        STDIN_FILE=
      fi
    fi
    if [ "${piped}" -eq 1 ]; then
      if [ "${explicit_stdin}" -eq 1 ]; then shift; fi
      if [ -n "${forced}" ]; then
        remote=${forced}
        if [ "$#" -eq 1 ]; then
          site=$1
          is_site_name "${site}" || usage_error "invalid site name: ${site}"
          base=${HOST}
          generated=0
        elif [ "$#" -eq 0 ]; then
          local_manifest=$(manifest_find 2>/dev/null || true)
          if [ -n "${local_manifest}" ]; then
            MANIFEST=${local_manifest}
            MANIFEST_DIR=$(dirname "${MANIFEST}")
            MANIFEST_HOST=$(manifest_value host "${MANIFEST}")
            MANIFEST_NAME=$(manifest_value name "${MANIFEST}")
            base=${MANIFEST_HOST}; site=${MANIFEST_NAME}; generated=0
          else
            base=${HOST}; site=; generated=1
          fi
        else
          usage_error "piped put -f accepts at most a site name"
        fi
      elif [ "$#" -eq 0 ]; then
        base=${HOST}; site=; remote=index.html; generated=1
      elif [ "$#" -eq 1 ]; then
        case "$1" in
          *.*)
            remote=$1
            local_manifest=$(manifest_find 2>/dev/null || true)
            if [ -n "${local_manifest}" ]; then
              MANIFEST=${local_manifest}
              MANIFEST_HOST=$(manifest_value host "${MANIFEST}")
              MANIFEST_NAME=$(manifest_value name "${MANIFEST}")
              base=${MANIFEST_HOST}; site=${MANIFEST_NAME}; generated=0
            else
              base=${HOST}; site=; generated=1
            fi
            ;;
          *)
            is_site_name "$1" || usage_error "invalid site name: $1"
            base=${HOST}; site=$1; remote=index.html; generated=0
            ;;
        esac
      elif [ "$#" -eq 2 ]; then
        site=$1; remote=$2; base=${HOST}; generated=0
        is_site_name "${site}" || usage_error "invalid site name: ${site}"
        [ "${remote}" != "-" ] || usage_error "remote file path cannot be -"
      else
        usage_error "piped put accepts at most NAME DEST"
      fi
      put_file_request "${base}" "${site}" "${STDIN_FILE}" "${remote}" 0 "${managed}" "" "${generated}"
      rm -f "${STDIN_FILE}"
    elif [ "$#" -eq 0 ]; then
      [ -z "${forced}" ] || usage_error "-f requires piped input"
      manifest_target
      put_file_request "${MANIFEST_HOST}" "${MANIFEST_NAME}" "${MANIFEST_DIR}" "" 1 "${managed}"
    elif [ "$#" -eq 1 ]; then
      [ -z "${forced}" ] || usage_error "-f requires piped input"
      put_file_request "${HOST}" "" "$1" "" "${unpack}" "${managed}" "" 1
    elif [ "$#" -eq 2 ]; then
      [ -z "${forced}" ] || usage_error "-f requires piped input"
      site=$1; source=$2
      is_site_name "${site}" || usage_error "invalid site name: ${site}"
      if [ -L "${source}" ]; then
        remote=$(basename "./${source}")
      elif [ -d "${source}" ] || [ "${unpack}" -eq 1 ]; then
        remote=
      else
        remote=$(basename "./${source}")
        case "${source}" in *.html|*.htm) remote=index.html ;; esac
      fi
      put_file_request "${HOST}" "${site}" "${source}" "${remote}" "${unpack}" "${managed}"
    else
      [ -z "${forced}" ] || usage_error "-f requires piped input"
      site=$1; source=$2; remote=$3
      is_site_name "${site}" || usage_error "invalid site name: ${site}"
      [ "${remote}" != "-" ] || usage_error "remote file path cannot be -"
      put_file_request "${HOST}" "${site}" "${source}" "${remote}" "${unpack}" "${managed}"
    fi
    ;;
  ls)
    links=0
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -l|--links) links=1; shift ;;
        --) shift; break ;;
        -*) usage_error "unknown flag: $1" ;;
        *) break ;;
      esac
    done
    [ "$#" -le 1 ] || usage_error "ls accepts at most one site name"
    if [ "$#" -eq 0 ]; then
      listing=$(request GET /FILES -H 'Accept: application/json')
      if [ "${JSON_OUTPUT}" -eq 1 ]; then
        printf '%s\n' "${listing}"
      else
        printf '%s\n' "${listing}" | print_listing "${HOST}" "${links}"
      fi
    else
      name=$1
      is_site_name "${name}" || usage_error "invalid site name: ${name}"
      inventory=$(request GET "/${name}/FILES" -H 'Accept: application/json')
      if [ "${JSON_OUTPUT}" -eq 1 ]; then
        printf '%s\n' "${inventory}"
      else
        printf '%s\n' "${inventory}" | print_inventory "${HOST}/${name}" "${links}"
      fi
    fi
    ;;
  get)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol get NAME [ARCHIVE]"
    name=$1
    dest=${2:-${name}.tar.gz}
    is_site_name "${name}" || usage_error "invalid site name: ${name}"
    archive_transfer GET "${HOST}" "${name}" "${dest}"
    [ "${dest}" = "-" ] || printf 'downloaded %s\n' "${dest}"
    ;;
  pop)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol pop NAME [ARCHIVE]"
    name=$1
    dest=${2:-${name}.tar.gz}
    is_site_name "${name}" || usage_error "invalid site name: ${name}"
    archive_transfer DELETE "${HOST}" "${name}" "${dest}"
    [ "${dest}" = "-" ] || printf 'popped %s\n' "${dest}"
    ;;
  clone)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol clone NAME [DIR]"
    clone_site "$1" "${2:-$1}"
    ;;
  copy)
    managed=0
    case "${1:-}" in --managed) managed=1; shift ;; esac
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol copy [--managed] SRC [DST]"
    copy_or_move_request COPY "$1" "${2:-}" "${managed}"
    ;;
  remix)
    managed=0
    case "${1:-}" in --managed) managed=1; shift ;; esac
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol remix [--managed] SRC [DST]"
    remix_destination=${2:-}
    if [ -n "${remix_destination}" ] && [ -e "${remix_destination}" ]; then
      [ -d "${remix_destination}" ] && [ -z "$(ls -A "${remix_destination}")" ] ||
        die "remix destination exists and is not empty: ${remix_destination}"
    fi
    copy_or_move_request COPY "$1" "${remix_destination}" "${managed}"
    copied=${RESULT_NAME}
    if ! clone_site "${copied}" "${copied}"; then
      printf 'server copy remains at %s/%s/\ncleanup with: symbol rm %s\n' \
        "${HOST}" "${copied}" "${copied}" >&2
      exit 1
    fi
    if [ -n "${RESULT_MANAGEMENT_TOKEN:-}" ]; then
      write_secret_sidecar token "${RESULT_MANAGEMENT_TOKEN}" "${copied}/symbol.toml"
    fi
    if [ -n "${RESULT_CLAIM_TOKEN:-}" ]; then
      write_secret_sidecar claim "${RESULT_CLAIM_TOKEN}" "${copied}/symbol.toml"
      remove_claim_recovery "${copied}"
    fi
    ;;
  move)
    [ "$#" -eq 2 ] || usage_error "usage: symbol move SRC DST"
    copy_or_move_request MOVE "$1" "$2" 0
    ;;
  alias)
    put_alias_request "$@"
    ;;
  sync)
    check=0
    case "${1:-}" in --check) check=1; shift ;; esac
    [ "$#" -eq 0 ] || usage_error "usage: symbol sync [--check]"
    sync_project "${check}"
    ;;
  undo)
    stack=0
    case "${1:-}" in -s|--stack) stack=1; shift ;; esac
    [ "$#" -le 2 ] || usage_error "usage: symbol undo [--stack] [NAME [TOKEN]]"
    if [ "$#" -eq 0 ]; then
      manifest_target
      base=${MANIFEST_HOST}; name=${MANIFEST_NAME}; token=
    else
      base=${HOST}; name=$1; token=${2:-}
    fi
    is_site_name "${name}" || usage_error "invalid site name: ${name}"
    if [ "${stack}" -eq 1 ]; then
      [ -z "${token}" ] || usage_error "--stack does not accept a token"
      stack_json=$(request GET "/${name}/UNDO")
      if [ "${JSON_OUTPUT}" -eq 1 ]; then
        printf '%s\n' "${stack_json}"
      else
      printf '%s\n' "${stack_json}" | awk '
        BEGIN { print "TOKEN      WOULD UNDO                         EXPIRES" }
        function human(seconds, days, hours, minutes, text) {
          seconds = int(seconds)
          days = int(seconds / 86400); seconds %= 86400
          hours = int(seconds / 3600); seconds %= 3600
          minutes = int(seconds / 60); seconds %= 60
          if (days) text = days "d"
          if (hours) text = text (text ? " " : "") hours "h"
          if (minutes) text = text (text ? " " : "") minutes "m"
          if (seconds || !text) text = text (text ? " " : "") seconds "s"
          return text
        }
        {
          text=text $0
        }
        END {
          rest=text
          while (match(rest, /\{"token"[^}]*\}/)) {
            object=substr(rest,RSTART,RLENGTH)
            token=object; sub(/^.*"token"[[:space:]]*:[[:space:]]*"/,"",token); sub(/".*$/,"",token)
            description=object; sub(/^.*"description"[[:space:]]*:[[:space:]]*"/,"",description); sub(/".*$/,"",description)
            expires=object; sub(/^.*"expires_at"[[:space:]]*:[[:space:]]*"/,"",expires); sub(/".*$/,"",expires)
            sub("T", " ", expires)
            sub(/:[0-9][0-9](\.[0-9]+)?Z$/, "Z", expires)
            remaining=object; sub(/^.*"remaining_seconds"[[:space:]]*:[[:space:]]*/,"",remaining); sub(/[^0-9].*$/,"",remaining)
            printf "%-10s %-34s %s (in %s)\n",token,description,expires,human(remaining)
            rest=substr(rest,RSTART+RLENGTH)
          }
        }
      '
      fi
    else
      auth=$(mktemp) || exit 1
      auth_args_file "${auth}"
      set --
      while IFS= read -r arg; do set -- "$@" "${arg}"; done < "${auth}"
      rm -f "${auth}"
      [ -z "${token}" ] || set -- "$@" -H "Undo-Token: ${token}"
      http_request UNDO "${base}/${name}" "$@" || exit 1
      print_mutation_result "${name}"
    fi
    ;;
  expire)
    if [ "$#" -eq 0 ] || [ "${1:-}" = --help ] || [ "${1:-}" = -h ]; then
      expire_help
      exit 0
    fi
    case "$1" in --show|--info|--graph) expire_help; exit 0 ;; esac
    name=$1; shift
    is_site_name "${name}" || usage_error "invalid site name: ${name}"
    path=
    case "${1:-}" in ''|--*) ;; *) path=$1; shift ;; esac
    mode=default
    min_age=; max_age=; max_size=; power=
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --decay) [ "${mode}" = default ] || usage_error "expiration modes are mutually exclusive"; mode=decay; shift ;;
        --in) [ "${mode}" = default ] || usage_error "expiration modes are mutually exclusive"; [ "$#" -ge 2 ] || usage_error "--in requires DURATION"; duration_valid "$2" || usage_error "invalid duration: $2"; mode=relative; in_value=$2; shift 2 ;;
        --at) [ "${mode}" = default ] || usage_error "expiration modes are mutually exclusive"; [ "$#" -ge 2 ] || usage_error "--at requires TIMESTAMP"; case "$2" in ????-??-??T??:??:??Z|????-??-??T??:??:??[+-]??:??) ;; *) usage_error "absolute time must be RFC 3339 with an offset" ;; esac; mode=absolute; at_value=$2; shift 2 ;;
        --never) [ "${mode}" = default ] || usage_error "expiration modes are mutually exclusive"; mode=never; shift ;;
        --show|--info|--graph) [ "${mode}" = default ] || usage_error "expiration modes are mutually exclusive"; mode=show; shift ;;
        --min-age) [ "$#" -ge 2 ] || usage_error "--min-age requires DURATION"; duration_valid "$2" || usage_error "invalid duration: $2"; min_age=$2; shift 2 ;;
        --max-age) [ "$#" -ge 2 ] || usage_error "--max-age requires DURATION"; duration_valid "$2" || usage_error "invalid duration: $2"; max_age=$2; shift 2 ;;
        --max-size) [ "$#" -ge 2 ] || usage_error "--max-size requires SIZE"; size_valid "$2" || usage_error "invalid size: $2"; max_size=$2; shift 2 ;;
        --power) [ "$#" -ge 2 ] || usage_error "--power requires NUMBER"; case "$2" in ''|*[!0-9.]*|0|0.0) usage_error "power must be greater than zero" ;; esac; power=$2; shift 2 ;;
        *) usage_error "unknown expire option: $1" ;;
      esac
    done
    [ "${mode}" = decay ] || {
      [ -z "${min_age}${max_age}${max_size}${power}" ] ||
        usage_error "decay constants require --decay"
    }
    target="/${name}"
    [ -z "${path}" ] || target="${target}/$(urlencode_path "${path}")"
    if [ "${mode}" = show ]; then
      http_request GET "${HOST}${target}/EXPIRES" -H 'Accept: application/json' || exit 1
      if [ "${JSON_OUTPUT}" -eq 1 ]; then
        cat "${HTTP_BODY}"
        last=$(tail -c 1 "${HTTP_BODY}" 2>/dev/null || true)
        [ -z "${last}" ] || printf '\n'
      else
        print_expire_report
      fi
    else
      auth=$(mktemp) || exit 1
      auth_args_file "${auth}"
      set --
      while IFS= read -r arg; do set -- "$@" "${arg}"; done < "${auth}"
      rm -f "${auth}"
      case "${mode}" in
        decay) set -- "$@" -H 'Expiry-Mode: decay' ;;
        relative) set -- "$@" -H 'Expiry-Mode: relative' -H "Expiry-In: ${in_value}" ;;
        absolute) set -- "$@" -H 'Expiry-Mode: absolute' -H "Expiry-At: ${at_value}" ;;
        never) set -- "$@" -H 'Expiry-Mode: never' ;;
      esac
      [ -z "${min_age}" ] || set -- "$@" -H "Expiry-Min-Age: ${min_age}"
      [ -z "${max_age}" ] || set -- "$@" -H "Expiry-Max-Age: ${max_age}"
      [ -z "${max_size}" ] || set -- "$@" -H "Expiry-Max-Size: ${max_size}"
      [ -z "${power}" ] || set -- "$@" -H "Expiry-Power: ${power}"
      http_request EXPIRE "${HOST}${target}" "$@" || exit 1
      if [ "${JSON_OUTPUT}" -eq 1 ]; then
        cat "${HTTP_BODY}"
        last=$(tail -c 1 "${HTTP_BODY}" 2>/dev/null || true)
        [ -z "${last}" ] || printf '\n'
      else
      print_mutation_result "${name}" 1
      if [ "${mode}" = never ]; then
        effective=$(json_string effective_expires_at < "${HTTP_BODY}" 2>/dev/null || true)
        remaining=$(json_string remaining_seconds < "${HTTP_BODY}" 2>/dev/null || true)
        limiting=$(json_limited_by < "${HTTP_BODY}" 2>/dev/null || true)
        if [ -n "${effective}" ] && [ "${effective}" != null ]; then
          printf 'effective expiry remains %s (in %s, limited by %s)\n' \
            "${effective}" "$(human_duration "${remaining:-0}")" "${limiting:-ancestor policy}"
        else
          printf 'expiration disabled for %s%s\n' "${HOST}" "${target}"
        fi
      elif [ -s "${HTTP_BODY}" ]; then
        print_expire_report
      fi
      fi
    fi
    ;;
  manage)
    if [ "$#" -eq 0 ] || [ "${1:-}" = --help ] || [ "${1:-}" = -h ]; then
      cat <<'EOF'
symbol manage: optional write ownership for a site

usage:
  symbol put --managed NAME SOURCE
  symbol manage NAME --claim
  symbol manage NAME --status
  symbol manage NAME --rotate
  symbol manage NAME --release

tokens:
  -t, --token TOKEN   explicit management token
  SYMBOL_TOKEN        environment fallback
  token = "..."       literal or manifest-relative file path

management protects writes; reads remain public.
tokens are shown once and cannot be recovered.
EOF
      exit 0
    fi
    [ "$#" -eq 2 ] || usage_error "usage: symbol manage NAME --claim|--status|--rotate|--release"
    name=$1; action=$2
    is_site_name "${name}" || usage_error "invalid site name: ${name}"
    case "${action}" in
      --claim) action=claim ;;
      --status) action=status ;;
      --rotate) action=rotate ;;
      --release) action=release ;;
      *) usage_error "unknown management action: ${action}" ;;
    esac
    auth=$(mktemp) || exit 1
    auth_args_file "${auth}"
    set -- -H "Management-Action: ${action}"
    while IFS= read -r arg; do set -- "$@" "${arg}"; done < "${auth}"
    rm -f "${auth}"
    if [ "${action}" = claim ]; then
      local_manifest=$(matching_manifest "${HOST}" "${name}" 2>/dev/null || true)
      claim=
      if [ -n "${local_manifest}" ]; then
        claim=$(claim_from_manifest "${local_manifest}" 2>/dev/null || true)
      fi
      [ -n "${claim}" ] || claim=$(claim_from_recovery "${name}" 2>/dev/null || true)
      [ -z "${claim}" ] || set -- "$@" -H "Creator-Claim: ${claim}"
      set -- "$@" -H "Idempotency-Key: $(random_key)"
    elif [ "${action}" = rotate ]; then
      set -- "$@" -H "Idempotency-Key: $(random_key)"
    fi
    http_request MANAGE "${HOST}/${name}" "$@" || exit 1
    cat "${HTTP_BODY}"
    last=$(tail -c 1 "${HTTP_BODY}" 2>/dev/null || true)
    [ -z "${last}" ] || printf '\n'
    save_response_secrets "${HOST}" "${name}"
    if [ "${action}" = claim ] || [ "${action}" = rotate ]; then
      management=$(header_value Management-Token)
      local_manifest=$(matching_manifest "${HOST}" "${name}" 2>/dev/null || true)
      if [ -n "${management}" ] && [ -z "${local_manifest}" ]; then
        save_management_recovery "${name}" "${management}"
      fi
    fi
    if [ "${action}" = release ]; then
      local_manifest=$(matching_manifest "${HOST}" "${name}" 2>/dev/null || true)
      if [ -n "${local_manifest}" ]; then
        dir=$(dirname "${local_manifest}")
        rm -f "${dir}/.symbol-token"
        tmp=$(mktemp "${dir}/.symbol.toml.XXXXXX") || exit 1
        awk '!/^[[:space:]]*token[[:space:]]*=/' "${local_manifest}" > "${tmp}"
        mv "${tmp}" "${local_manifest}"
      fi
    fi
    ;;
  recover)
    [ "$#" -eq 0 ] || usage_error "recover accepts no arguments"
    recover_pending_operations
    ;;
  rm)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol rm NAME [PATH]"
    name=$1
    remote_path=${2:-}
    remove_site=0
    [ "$#" -ne 1 ] || remove_site=1
    is_site_name "${name}" || usage_error "invalid site name: ${name}"
    auth=$(mktemp) || exit 1
    auth_args_file "${auth}"
    set --
    while IFS= read -r arg; do set -- "$@" "${arg}"; done < "${auth}"
    rm -f "${auth}"
    if [ "${remove_site}" -eq 1 ]; then
      http_request DELETE "${HOST}/${name}" "$@" || exit 1
      emit_status "deleted ${name}"
      print_mutation_result "${name}" 1
    else
      encoded=$(urlencode_path "${remote_path}")
      http_request DELETE "${HOST}/${name}/${encoded}" "$@" || exit 1
      emit_status "deleted ${name}/${remote_path}"
      print_mutation_result "${name}"
    fi
    ;;
  url)
    [ "$#" -eq 1 ] || usage_error "usage: symbol url NAME"
    is_site_name "$1" || usage_error "invalid site name: $1"
    printf '%s/%s/\n' "${HOST}" "$1"
    ;;
  update)
    [ "$#" -eq 0 ] || usage_error "update accepts no arguments"
    self=$(client_path)
    dest=$(dirname "${self}")
    hashfile="${dest}/.symbol.blake3"
    have=
    if [ -f "${hashfile}" ]; then
      have=$(tr -d ' \t\r\n' < "${hashfile}")
    fi
    tmp=$(mktemp)
    curl -fsSL "${HOST}/symbol.sh" -o "${tmp}"
    chmod +x "${tmp}"
    stats=$(diff_stats "${self}" "${tmp}")
    mv "${tmp}" "${self}"
    remote=$(curl -fsSL "${HOST}/symbol.sh/HASH" | tr -d ' \t\r\n')
    printf '%s\n' "${remote}" > "${hashfile}"
    rm -f "${dest}/.symbol.hash"
    if [ -n "${remote}" ] && [ "${remote}" = "${have}" ] && [ -z "${stats}" ]; then
      echo "already up to date"
    else
      echo "updated ${self}"
      if [ -n "${stats}" ]; then
        echo "  ${stats}"
      fi
    fi
    ;;
  api)
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --json) JSON_OUTPUT=1; shift ;;
        --) shift; break ;;
        -*) usage_error "unknown flag: $1" ;;
        *) break ;;
      esac
    done
    [ "$#" -eq 0 ] || usage_error "api accepts no arguments"
    api_json=$(request GET /API/VERSION)
    if [ "${JSON_OUTPUT}" -eq 1 ]; then
      printf '%s\n' "${api_json}"
    else
      printf '%s\n' "${api_json}" | print_api
    fi
    ;;
  stats)
    [ "$#" -eq 0 ] || usage_error "stats accepts no arguments"
    stats_json=$(request GET /STATS)
    if [ "${JSON_OUTPUT}" -eq 1 ]; then
      printf '%s\n' "${stats_json}"
    else
      printf '%s\n' "${stats_json}" | print_stats
    fi
    ;;
  help)
    usage
    ;;
esac

http_cleanup
join_update_check
trap - EXIT
