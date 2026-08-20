#!/bin/sh
# symbol: put a static site on the tailnet
# install: curl -fsSL __HOST__/install.sh | sh
set -eu

HOST="${SYMBOL_HOST:-__HOST__}"

UPDATE_PID=
UPDATE_NOTE=

client_path() {
  cmd=$0
  case "$cmd" in
    /*)
      printf '%s\n' "$cmd"
      return
      ;;
    */*)
      printf '%s\n' "$(pwd)/$cmd"
      return
      ;;
  esac
  oldifs=$IFS
  IFS=:
  for dir in $PATH; do
    if [ -n "$dir" ] && [ -f "$dir/$cmd" ] && [ -x "$dir/$cmd" ]; then
      IFS=$oldifs
      printf '%s\n' "$dir/$cmd"
      return
    fi
  done
  IFS=$oldifs
  printf '%s\n' "$cmd"
}

start_update_check() {
  case "${1:-}" in
    update|upgrade|-h|--help|help|"") return 0 ;;
  esac
  case "$HOST" in
    http://*|https://*) ;;
    *) return 0 ;;
  esac
  hashfile=$(dirname "$(client_path)")/.symbol.blake3
  UPDATE_NOTE=$(mktemp) || return 0
  {
    if [ ! -f "$hashfile" ]; then
      printf '\nsymbol client is missing its hash file. run:\n  symbol update\n' > "$UPDATE_NOTE"
      exit 0
    fi
    remote=$(curl -fsS --max-time 2 "${HOST}/symbol.sh/HASH" 2>/dev/null || true)
    remote=$(printf '%s' "$remote" | tr -d ' \t\r\n')
    [ -n "$remote" ] || exit 0
    have=$(tr -d ' \t\r\n' < "$hashfile")
    if [ "$remote" != "$have" ]; then
      printf '\nsymbol client is out of date. run:\n  symbol update\n' > "$UPDATE_NOTE"
    fi
  } &
  UPDATE_PID=$!
}

join_update_check() {
  if [ -n "${UPDATE_PID:-}" ]; then
    wait "$UPDATE_PID" 2>/dev/null || true
    UPDATE_PID=
  fi
  if [ -n "${UPDATE_NOTE:-}" ]; then
    if [ -s "$UPDATE_NOTE" ]; then
      if update_check_color; then
        printf '\033[33m' >&2
        cat "$UPDATE_NOTE" >&2
        printf '\033[0m' >&2
      else
        cat "$UPDATE_NOTE" >&2
      fi
    fi
    rm -f "$UPDATE_NOTE"
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
  symbol put [-u] [--managed] [NAME [FILE [DEST]]]
  symbol clone NAME [DIR]
  symbol get NAME [ARCHIVE]
  symbol pop NAME [ARCHIVE]
  symbol copy [--managed] SRC [DST]
  symbol remix [--managed] SRC [DST]
  symbol move SRC DST
  symbol sync [--check]
  symbol undo [--stack] [NAME [TOKEN]]
  symbol expire [NAME [PATH]] [POLICY]
  symbol manage [NAME ACTION]
  symbol ls [-l] [NAME]
  symbol rm NAME [PATH]
  symbol url NAME
  symbol stats
  symbol update
  symbol help

global:
  -t, --token TOKEN   management token (also after the command, before --)

put:
  -u, --unpack        unpack archives; directories are always unpacked
  -f PATH             force piped input's remote file path
  --managed           create a managed site
  with no arguments, publish the nearest symbol.toml project

streams:
  - in a source position reads stdin
  - as get/pop output writes archive bytes to stdout

aliases:
  push -> put; pull -> clone; rename -> move; add -> put
  list -> ls; download -> get; delete -> rm; upgrade -> update

env: SYMBOL_HOST (default ${HOST}); SYMBOL_TOKEN
EOF
}

need() {
  if [ "$#" -lt "$1" ]; then
    usage >&2
    exit 2
  fi
}

diff_stats() {
  out=$(mktemp) || return 0
  diff -u "$1" "$2" > "$out" || true
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
  ' "$out"
  rm -f "$out"
}

request() {
  method=$1
  path=$2
  shift 2
  curl -sS -X "$method" "$@" "${HOST}${path}"
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

print_links() {
  base=${1%/}
  nested=$2
  awk -v base="$base" -v nested="$nested" '
    function spaces(n, out) {
      out = ""
      while (n-- > 0) out = out " "
      return out
    }
    function trim(value) {
      sub(/^[[:space:]]+/, "", value)
      sub(/[[:space:]]+$/, "", value)
      return value
    }
    BEGIN {
      size = "[0-9]+(\\.[0-9]+)?[[:space:]]+(B|KiB|MiB|GiB|TiB|PiB)"
    }
    {
      line = $0
      count_pattern = "[[:space:]]+[0-9]+ files[[:space:]]+" size "([[:space:]]+total)?[[:space:]]*$"
      size_pattern = "[[:space:]]+" size "[[:space:]]*$"
      if (match(line, count_pattern) || match(line, size_pattern)) {
        line_match_start = RSTART
        matched = substr(line, line_match_start, RLENGTH)
        match(matched, /[^[:space:]]/)
        starts[NR] = line_match_start + RSTART - 1
        names[NR] = trim(substr(line, 1, line_match_start - 1))
        metadata[NR] = trim(matched)
        if (minimum_start == 0 || starts[NR] < minimum_start) minimum_start = starts[NR]
      } else {
        names[NR] = trim(line)
      }

      name = names[NR]
      if (name == "") {
        links[NR] = ""
      } else if (nested && NR == 1) {
        links[NR] = base
      } else if (name == "../") {
        links[NR] = base "/.."
      } else {
        sub(/\/$/, "", name)
        links[NR] = base "/" name
      }
      if (length(links[NR]) > link_width) link_width = length(links[NR])
      lines = NR
    }
    END {
      for (i = 1; i <= lines; i++) {
        if (metadata[i] == "") {
          print links[i]
        } else {
          printf "%-*s  %s%s\n",
            link_width,
            links[i],
            spaces(starts[i] - minimum_start),
            metadata[i]
        }
      }
    }
  '
}

put_body() {
  src=$1
  dest=$2
  unpack=$3
  if [ -d "$src" ]; then
    tar -czf - -C "$src" . | curl -sS -T - \
      -H 'Content-Type: application/gzip' \
      -H unpack:1 \
      "${HOST}${dest}"
    return
  fi
  if [ ! -f "$src" ]; then
    echo "error: not a file or directory: $src" >&2
    exit 1
  fi
  base=$(basename "$src")
  case "$src" in
    *.zip) ctype=application/zip ;;
    *.tgz|*.tar.gz) ctype=application/gzip ;;
    *.gz) ctype=application/gzip ;;
    *.tar) ctype=application/x-tar ;;
    *.html|*.htm) ctype=text/html ;;
    *.mp3) ctype=audio/mpeg ;;
    *.m4a) ctype=audio/mp4 ;;
    *.mp4|*.m4v) ctype=video/mp4 ;;
    *.webm) ctype=video/webm ;;
    *.ogg|*.oga) ctype=audio/ogg ;;
    *.wav) ctype=audio/wav ;;
    *) ctype=application/octet-stream ;;
  esac
  if [ "$unpack" -eq 1 ]; then
    curl -sS -T "$src" \
      -H "Content-Type: $ctype" \
      -H "Content-Disposition: attachment; filename=\"${base}\"" \
      -H unpack:1 \
      "${HOST}${dest}"
  else
    curl -sS -T "$src" \
      -H "Content-Type: $ctype" \
      -H "Content-Disposition: attachment; filename=\"${base}\"" \
      "${HOST}${dest}"
  fi
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
  case "$dir" in
    /*) ;;
    *) dir=$(cd "$dir" 2>/dev/null && pwd) || return 1 ;;
  esac
  if stat -c %d "$dir" >/dev/null 2>&1; then
    device=$(stat -c %d "$dir")
    stat_device() { stat -c %d "$1"; }
  else
    device=$(stat -f %d "$dir")
    stat_device() { stat -f %d "$1"; }
  fi
  while :; do
    if [ -f "$dir/symbol.toml" ]; then
      printf '%s\n' "$dir/symbol.toml"
      return 0
    fi
    parent=${dir%/*}
    [ -n "$parent" ] || parent=/
    [ "$parent" != "$dir" ] || return 1
    [ "$(stat_device "$parent")" = "$device" ] || return 1
    dir=$parent
  done
}

manifest_value() {
  key=$1
  file=$2
  awk -v wanted="$key" '
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
  ' "$file"
}

manifest_target() {
  MANIFEST=$(manifest_find) || die "no symbol.toml found; specify a site"
  MANIFEST_DIR=$(dirname "$MANIFEST")
  MANIFEST_HOST=$(manifest_value host "$MANIFEST") ||
    die "invalid symbol.toml: missing top-level host"
  MANIFEST_NAME=$(manifest_value name "$MANIFEST") ||
    die "invalid symbol.toml: missing top-level name"
  case "$MANIFEST_HOST" in http://*|https://*) ;; *) die "invalid symbol.toml host" ;; esac
  is_site_name "$MANIFEST_NAME" || die "invalid symbol.toml site name"
}

token_from_manifest() {
  file=$1
  value=$(manifest_value token "$file" 2>/dev/null || true)
  [ -n "$value" ] || return 1
  case "$value" in
    sym_mgmt_*) printf '%s\n' "$value" ;;
    *)
      case "$value" in /*) path=$value ;; *) path=$(dirname "$file")/$value ;; esac
      [ -f "$path" ] || die "management token file not found: $path"
      tr -d '\r\n' < "$path"
      ;;
  esac
}

claim_from_manifest() {
  file=$1
  value=$(manifest_value claim "$file" 2>/dev/null || true)
  [ -n "$value" ] || return 1
  case "$value" in
    sym_claim_*) printf '%s\n' "$value" ;;
    *)
      case "$value" in /*) path=$value ;; *) path=$(dirname "$file")/$value ;; esac
      [ -f "$path" ] || die "creator claim file not found: $path"
      tr -d '\r\n' < "$path"
      ;;
  esac
}

TOKEN_EXPLICIT=
TOKEN_SEEN=0
parse_global_tokens() {
  args=$(mktemp) || exit 1
  : > "$args"
  after_dash=0
  while [ "$#" -gt 0 ]; do
    if [ "$after_dash" -eq 0 ]; then
      case "$1" in
        --)
          after_dash=1
          printf '%s\n' "$1" >> "$args"
          shift
          continue
          ;;
        -t|--token)
          [ "$TOKEN_SEEN" -eq 0 ] || usage_error "management token specified more than once"
          [ "$#" -ge 2 ] || usage_error "$1 requires a token"
          TOKEN_EXPLICIT=$2
          TOKEN_SEEN=1
          shift 2
          continue
          ;;
        --token=*)
          [ "$TOKEN_SEEN" -eq 0 ] || usage_error "management token specified more than once"
          TOKEN_EXPLICIT=${1#*=}
          [ -n "$TOKEN_EXPLICIT" ] || usage_error "--token requires a token"
          TOKEN_SEEN=1
          shift
          continue
          ;;
      esac
    fi
    printf '%s\n' "$1" >> "$args"
    shift
  done
  set --
  while IFS= read -r arg; do
    set -- "$@" "$arg"
  done < "$args"
  rm -f "$args"
  PARSED_ARGC=$#
  PARSED_ARGS=$(mktemp) || exit 1
  : > "$PARSED_ARGS"
  for arg do printf '%s\n' "$arg" >> "$PARSED_ARGS"; done
}

select_token() {
  TOKEN=
  if [ "$TOKEN_SEEN" -eq 1 ]; then
    TOKEN=$TOKEN_EXPLICIT
  elif [ -n "${SYMBOL_TOKEN:-}" ]; then
    TOKEN=$SYMBOL_TOKEN
  else
    local_manifest=$(manifest_find 2>/dev/null || true)
    if [ -n "$local_manifest" ]; then
      if manifest_value token "$local_manifest" >/dev/null 2>&1; then
        TOKEN=$(token_from_manifest "$local_manifest")
      fi
    fi
  fi
}

auth_args_file() {
  file=$1
  : > "$file"
  if [ -n "${TOKEN:-}" ]; then
    printf '%s\n%s\n' '-H' "Authorization: Bearer $TOKEN" >> "$file"
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
  [ -z "${HTTP_DIR:-}" ] || rm -rf "$HTTP_DIR"
  HTTP_DIR=
}

http_request() {
  method=$1
  url=$2
  shift 2
  http_cleanup
  HTTP_DIR=$(mktemp -d) || exit 1
  HTTP_HEADERS=$HTTP_DIR/headers
  HTTP_BODY=$HTTP_DIR/body
  HTTP_STATUS=$(curl -sS -X "$method" -D "$HTTP_HEADERS" -o "$HTTP_BODY" \
    -w '%{http_code}' "$@" "$url") || {
      status=$?
      [ -s "$HTTP_BODY" ] && cat "$HTTP_BODY" >&2
      return "$status"
    }
  case "$HTTP_STATUS" in
    2??) return 0 ;;
    *) [ -s "$HTTP_BODY" ] && cat "$HTTP_BODY" >&2; return 1 ;;
  esac
}

header_value() {
  wanted=$1
  awk -v wanted="$wanted" '
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
  ' "$HTTP_HEADERS"
}

print_mutation_result() {
  suppress_body=${2:-0}
  if [ "$suppress_body" -eq 0 ] && [ -s "$HTTP_BODY" ]; then
    cat "$HTTP_BODY"
    last=$(tail -c 1 "$HTTP_BODY" 2>/dev/null || true)
    [ -z "$last" ] || printf '\n'
  fi
  undo=$(header_value Undo-Token)
  undo_expires=$(header_value Undo-Expires)
  if [ -n "$undo" ]; then
    printf 'undo until %s: symbol undo %s %s\n' \
      "${undo_expires:-unknown}" "$1" "$undo"
  fi
  management_count=$(header_value Sanitized-Management-Tokens)
  claim_count=$(header_value Sanitized-Creator-Claims)
  management_count=${management_count:-0}
  claim_count=${claim_count:-0}
  if [ "$management_count" != 0 ] || [ "$claim_count" != 0 ]; then
    printf 'warning: redacted %s management tokens and %s creator claims from uploaded files\n' \
      "$management_count" "$claim_count" >&2
  fi
}

command_registry() {
  cat <<'EOF'
put put push add
pop pop
clone clone pull
get get download
copy copy
remix remix
move move rename
stats stats
sync sync
undo undo
expire expire
manage manage
ls ls list
rm rm delete
url url
update update upgrade
help help -h --help
EOF
}

resolve_command() {
  query=$1
  command_registry | awk -v q="$query" '
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
while IFS= read -r arg; do set -- "$@" "$arg"; done < "$PARSED_ARGS"
rm -f "$PARSED_ARGS"
raw_cmd=${1:-help}
[ "$#" -eq 0 ] || shift
cmd=$(resolve_command "$raw_cmd") || exit $?
select_token
start_update_check "$cmd"
trap join_update_check EXIT

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
  suffix=$(archive_suffix "$dest")
  url="${base}/${name}${suffix}"
  auth=$(mktemp) || exit 1
  archive_headers=$(mktemp) || exit 1
  auth_args_file "$auth"
  set -- -D "$archive_headers"
  while IFS= read -r arg; do set -- "$@" "$arg"; done < "$auth"
  rm -f "$auth"
  if [ "$dest" = "-" ]; then
    if ! curl -sS -f -X "$method" "$@" "$url"; then
      rm -f "$archive_headers"
      die "archive transfer failed"
    fi
    if [ "$method" = DELETE ]; then
      HTTP_HEADERS=$archive_headers
      undo=$(header_value Undo-Token)
      undo_expires=$(header_value Undo-Expires)
      [ -z "$undo" ] || printf 'undo until %s: symbol undo %s %s\n' \
        "${undo_expires:-unknown}" "$name" "$undo" >&2
    fi
    rm -f "$archive_headers"
    return
  fi
  tmp=$(mktemp) || exit 1
  if curl -sS -f -X "$method" "$@" "$url" -o "$tmp"; then
    mv "$tmp" "$dest"
    if [ "$method" = DELETE ]; then
      HTTP_HEADERS=$archive_headers
      undo=$(header_value Undo-Token)
      undo_expires=$(header_value Undo-Expires)
      [ -z "$undo" ] || printf 'undo until %s: symbol undo %s %s\n' \
        "${undo_expires:-unknown}" "$name" "$undo" >&2
    fi
    rm -f "$archive_headers"
  else
    rm -f "$tmp"
    rm -f "$archive_headers"
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
  [ -n "$remote" ] || return 0
  case "/$remote/" in
    */../*|*/./*|*'//'*) usage_error "invalid remote path: $remote" ;;
  esac
  terminal=$(basename "$remote")
  case "$terminal" in
    symbol.toml|.symbol-token|.symbol-claim|FILES|HASH|UNDO|EXPIRES)
      usage_error "reserved remote path: $remote"
      ;;
  esac
}

stage_upload_directory() {
  source=$1
  STAGE_ROOT=$(mktemp -d) || exit 1
  (
    cd "$source"
    find . -type f ! -path './.git/*' \
      ! -name '.symbol-token' ! -name '.symbol-claim' \
      ! -name 'symbol.toml' -print
  ) | while IFS= read -r path; do
    relative=${path#./}
    mkdir -p "$STAGE_ROOT/$(dirname "$relative")"
    cp "$source/$relative" "$STAGE_ROOT/$relative"
  done
}

make_archive_from_directory() {
  source=$1
  stage_upload_directory "$source"
  ARCHIVE_FILE=$(mktemp) || exit 1
  tar -czf "$ARCHIVE_FILE" -C "$STAGE_ROOT" .
  rm -rf "$STAGE_ROOT"
  STAGE_ROOT=
}

write_secret_sidecar() {
  kind=$1
  secret=$2
  manifest=$3
  dir=$(dirname "$manifest")
  case "$kind" in
    token) sidecar=.symbol-token; key=token ;;
    claim) sidecar=.symbol-claim; key=claim ;;
  esac
  (umask 077; printf '%s\n' "$secret" > "$dir/$sidecar")
  tmp=$(mktemp "$dir/.symbol.toml.XXXXXX") || exit 1
  awk -v key="$key" -v value="./$sidecar" '
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
  ' "$manifest" > "$tmp"
  mv "$tmp" "$manifest"
  if [ -d "$dir/.git" ] || [ -f "$dir/.gitignore" ]; then
    touch "$dir/.gitignore"
    if ! awk -v line="/$sidecar" '$0 == line { found=1 } END { exit !found }' "$dir/.gitignore"; then
      printf '/%s\n' "$sidecar" >> "$dir/.gitignore"
    fi
  fi
  printf 'saved %s to %s/%s (mode 0600)\n' "$kind" "$dir" "$sidecar"
}

matching_manifest() {
  expected_host=$1
  expected_name=$2
  found=$(manifest_find 2>/dev/null || true)
  [ -n "$found" ] || return 1
  found_host=$(manifest_value host "$found" 2>/dev/null || true)
  found_name=$(manifest_value name "$found" 2>/dev/null || true)
  [ "${found_host%/}" = "${expected_host%/}" ] && [ "$found_name" = "$expected_name" ] || return 1
  printf '%s\n' "$found"
}

save_response_secrets() {
  base=$1
  name=$2
  management=$(header_value Management-Token)
  claim=$(header_value Creator-Claim)
  [ -z "$management" ] || {
    printf 'management token (shown once):\n  %s\n' "$management"
    local_manifest=$(matching_manifest "$base" "$name" 2>/dev/null || true)
    [ -z "$local_manifest" ] || write_secret_sidecar token "$management" "$local_manifest"
  }
  [ -z "$claim" ] || {
    printf 'creator claim (shown once):\n  %s\n' "$claim"
    local_manifest=$(matching_manifest "$base" "$name" 2>/dev/null || true)
    [ -z "$local_manifest" ] || write_secret_sidecar claim "$claim" "$local_manifest"
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
  validate_remote_path "$remote"
  is_site_name "$site" || [ "$generated" -eq 1 ] ||
    usage_error "invalid site name: $site"
  auth=$(mktemp) || exit 1
  auth_args_file "$auth"
  set -- -H "Idempotency-Key: $(random_key)"
  while IFS= read -r arg; do set -- "$@" "$arg"; done < "$auth"
  rm -f "$auth"
  [ -z "$conditional" ] || set -- "$@" -H "If-Match: $conditional"
  if [ "$managed" -eq 1 ]; then
    claim_manifest=$(matching_manifest "$base" "$site" 2>/dev/null || true)
    if [ -z "$claim_manifest" ] && [ -d "$source" ] && [ -f "$source/symbol.toml" ]; then
      candidate_host=$(manifest_value host "$source/symbol.toml" 2>/dev/null || true)
      candidate_name=$(manifest_value name "$source/symbol.toml" 2>/dev/null || true)
      if [ "${candidate_host%/}" = "${base%/}" ] && [ "$candidate_name" = "$site" ]; then
        claim_manifest=$source/symbol.toml
      fi
    fi
    claim=
    if [ -n "$claim_manifest" ] && manifest_value claim "$claim_manifest" >/dev/null 2>&1; then
      claim=$(claim_from_manifest "$claim_manifest")
    fi
    if [ -z "$claim" ]; then
      claim="sym_claim_$(random_key)$(random_key)"
      [ -z "$claim_manifest" ] || write_secret_sidecar claim "$claim" "$claim_manifest"
    fi
    set -- "$@" -H 'Management-Action: claim' -H "Creator-Claim: $claim"
  fi
  if [ "$generated" -eq 1 ] && [ -n "$remote" ]; then
    upload_stage=$(mktemp -d) || exit 1
    mkdir -p "$upload_stage/$(dirname "$remote")"
    cp "$source" "$upload_stage/$remote"
    ARCHIVE_FILE=$(mktemp) || exit 1
    tar -czf "$ARCHIVE_FILE" -C "$upload_stage" .
    rm -rf "$upload_stage"
    source=$ARCHIVE_FILE
    set -- "$@" -H 'Content-Type: application/gzip' -H 'Unpack: 1'
  elif [ -d "$source" ]; then
    make_archive_from_directory "$source"
    source=$ARCHIVE_FILE
    set -- "$@" -H 'Content-Type: application/gzip' -H 'Unpack: 1'
  else
    [ -f "$source" ] || die "not a file or directory: $source"
    if [ -n "$remote" ]; then
      ctype=$(content_type "$remote")
      set -- "$@" -H "Content-Disposition: attachment; filename=\"$(basename "$remote")\""
    else
      ctype=$(content_type "$source")
    fi
    set -- "$@" -H "Content-Type: $ctype"
    [ "$unpack" -eq 0 ] || set -- "$@" -H 'Unpack: 1'
  fi
  if [ "$generated" -eq 1 ]; then
    url="${base}/"
  elif [ -n "$remote" ]; then
    encoded=$(urlencode_path "$remote")
    url="${base}/${site}/${encoded}"
  else
    url="${base}/${site}"
  fi
  http_request PUT "$url" "$@" -T "$source" || {
    [ -z "${ARCHIVE_FILE:-}" ] || rm -f "$ARCHIVE_FILE"
    return 1
  }
  [ -z "${ARCHIVE_FILE:-}" ] || rm -f "$ARCHIVE_FILE"
  ARCHIVE_FILE=
  location=$(header_value Location)
  final_name=$site
  if [ "$generated" -eq 1 ]; then
    [ -n "$location" ] || die "server omitted Location for generated site"
    final_name=${location%/}
    final_name=${final_name##*/}
  fi
  print_mutation_result "$final_name"
  save_response_secrets "$base" "$final_name"
  PUT_NAME=$final_name
}

refresh_local_manifest() {
  manifest=$1
  base=$2
  name=$3
  dir=$(dirname "$manifest")
  work=$(mktemp -d) || exit 1
  preserved=$work/preserved
  awk '/^[[:space:]]*(token|claim)[[:space:]]*=/{print}' "$manifest" > "$preserved"
  fresh=$work/symbol.toml
  curl -sS -f "${base}/${name}/symbol.toml" -o "$fresh" || {
    rm -rf "$work"
    die "publish succeeded, but could not refresh local symbol.toml"
  }
  if [ -s "$preserved" ]; then
    merged=$work/merged
    awk 'BEGIN{done=0} /^[[:space:]]*\[/ && !done {while((getline l < p)>0) print l; done=1} {print}
      END{if(!done) while((getline l < p)>0) print l}' p="$preserved" "$fresh" > "$merged"
    mv "$merged" "$fresh"
  fi
  tmp=$(mktemp "$dir/.symbol.toml.XXXXXX") || exit 1
  cp "$fresh" "$tmp"
  mv "$tmp" "$manifest"
  rm -rf "$work"
}

stdin_to_temp() {
  STDIN_FILE=$(mktemp) || exit 1
  cat > "$STDIN_FILE"
}

copy_or_move_request() {
  method=$1
  source=$2
  destination=$3
  managed=$4
  is_site_name "$source" || usage_error "invalid source site: $source"
  [ -z "$destination" ] || is_site_name "$destination" ||
    usage_error "invalid destination site: $destination"
  auth=$(mktemp) || exit 1
  auth_args_file "$auth"
  set --
  while IFS= read -r arg; do set -- "$@" "$arg"; done < "$auth"
  rm -f "$auth"
  [ -z "$destination" ] || set -- "$@" -H "Destination: /$destination"
  if [ "$method" = COPY ]; then
    set -- "$@" -H "Idempotency-Key: $(random_key)"
    if [ "$managed" -eq 1 ]; then
      claim="sym_claim_$(random_key)$(random_key)"
      set -- "$@" -H 'Management-Action: claim' -H "Creator-Claim: $claim"
    fi
  fi
  http_request "$method" "${HOST}/${source}" "$@" || return 1
  location=$(header_value Location)
  [ -n "$location" ] || die "server omitted Location"
  RESULT_NAME=${location%/}
  RESULT_NAME=${RESULT_NAME##*/}
  print_mutation_result "$RESULT_NAME"
  save_response_secrets "$HOST" "$RESULT_NAME"
}

clone_site() {
  name=$1
  destination=${2:-$name}
  is_site_name "$name" || usage_error "invalid site name: $name"
  [ "$destination" != "-" ] || usage_error "clone destination cannot be -; use get $name -"
  if [ -e "$destination" ]; then
    [ -d "$destination" ] && [ -z "$(ls -A "$destination")" ] ||
      die "clone destination exists and is not empty: $destination"
  fi
  work=$(mktemp -d) || exit 1
  archive=$work/site.tar.gz
  extract=$work/extract
  mkdir "$extract"
  if ! archive_transfer GET "$HOST" "$name" "$archive"; then
    rm -rf "$work"
    return 1
  fi
  if ! tar -xzf "$archive" -C "$extract"; then
    rm -rf "$work"
    die "could not extract archive"
  fi
  if [ -e "$destination" ]; then
    cp -R "$extract"/. "$destination"/
  else
    mv "$extract" "$destination"
  fi
  rm -rf "$work"
  printf 'cloned %s/%s/ -> %s\n' "$HOST" "$name" "$destination"
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

option aliases:
  --info and --graph are aliases for --show.
EOF
}

json_string() {
  key=$1
  awk -v key="$key" '
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

print_expire_report() {
  expires=$(json_string effective_expires_at < "$HTTP_BODY" 2>/dev/null || true)
  remaining=$(json_string remaining_seconds < "$HTTP_BODY" 2>/dev/null || true)
  mode=$(json_string mode < "$HTTP_BODY" 2>/dev/null || true)
  size=$(json_string size < "$HTTP_BODY" 2>/dev/null || true)
  refreshed=$(json_string refreshed_at < "$HTTP_BODY" 2>/dev/null || true)
  own_expires=$(json_string expires_at < "$HTTP_BODY" 2>/dev/null || true)
  retention=$(json_string retention_seconds < "$HTTP_BODY" 2>/dev/null || true)
  min_age=$(json_string min_age_seconds < "$HTTP_BODY" 2>/dev/null || true)
  max_age=$(json_string max_age_seconds < "$HTTP_BODY" 2>/dev/null || true)
  max_size=$(json_string max_size_bytes < "$HTTP_BODY" 2>/dev/null || true)
  power=$(json_string power < "$HTTP_BODY" 2>/dev/null || true)
  if [ -z "$expires" ] || [ "$expires" = null ]; then
    printf 'expiration disabled\n'
    return
  fi
  size_h=$(human_bytes "${size:-0}")
  remaining_h=$(human_duration "${remaining:-0}")
  retention_h=
  [ -z "$retention" ] || [ "$retention" = null ] ||
    retention_h=$(human_duration "$retention")
  min_h=
  [ -z "$min_age" ] || [ "$min_age" = null ] ||
    min_h=$(human_duration "$min_age")
  max_h=
  [ -z "$max_age" ] || [ "$max_age" = null ] ||
    max_h=$(human_duration "$max_age")
  max_size_h=
  [ -z "$max_size" ] || [ "$max_size" = null ] ||
    max_size_h=$(human_bytes "$max_size")
  printf 'effective lifetime\n'
  [ -z "$size" ] || printf 'size:              %s\n' "$size_h"
  if [ -n "$mode" ] && [ "$mode" != null ]; then
    if [ "$mode" = decay ] && [ -n "$min_age" ] && [ -n "$max_age" ]; then
      printf 'policy:            decay (%s..%s @ %s ^%s)\n' \
        "$min_h" "$max_h" "${max_size_h:-unknown}" "${power:-unknown}"
    else
      printf 'policy:            %s\n' "$mode"
    fi
  fi
  [ -z "$retention" ] || [ "$retention" = null ] ||
    printf 'policy retention:  %s\n' "$retention_h"
  [ -z "$refreshed" ] || [ "$refreshed" = null ] ||
    printf 'refreshed:         %s\n' "$refreshed"
  [ -z "$own_expires" ] || [ "$own_expires" = null ] ||
    printf 'own expiry:        %s\n' "$own_expires"
  printf 'effective expiry:  %s (in %s)\n' "$expires" "$remaining_h"
  if [ "$mode" = decay ]; then
    awk -v size="${size:-0}" -v maximum="${max_size:-0}" \
      -v min="${min_h:-min}" -v max="${max_h:-max}" \
      -v current="$size_h" -v maximum_label="${max_size_h:-unknown}" '
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
  if [ -n "$retention" ] && [ "$retention" != null ] && [ -n "$remaining" ]; then
    elapsed=$((retention > remaining ? retention - remaining : 0))
  fi
  elapsed_h=$(human_duration "$elapsed")
  awk -v remaining="${remaining:-0}" -v retention="${retention:-0}" \
    -v elapsed_label="$elapsed_h" -v remaining_label="$remaining_h" '
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
    *s|*m|*h|*d|*w) number=${1%?}; case "$number" in ''|*[!0-9]*|0) return 1 ;; esac ;;
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

blake3_file() {
  if command -v b3sum >/dev/null 2>&1; then
    b3sum "$1" | awk '{print "blake3:" $1}'
  elif command -v blake3 >/dev/null 2>&1; then
    blake3 "$1" | awk '{print "blake3:" $1}'
  else
    die "sync requires b3sum (or blake3)"
  fi
}

local_file_map() {
  root=$1
  (
    cd "$root"
    find . -type f ! -path './.git/*' ! -name symbol.toml \
      ! -name .symbol-token ! -name .symbol-claim -print | LC_ALL=C sort
  ) | while IFS= read -r path; do
    relative=${path#./}
    hash=$(blake3_file "$root/$relative")
    printf '%s\t%s\n' "$relative" "$hash"
  done
}

upstream_file_map() {
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
        if (path != "symbol.toml") print path "\t" hash
        rest=substr(rest,RSTART+RLENGTH)
      }
    }
  ' "$1" | LC_ALL=C sort
}

sync_project() {
  check=$1
  manifest_target
  baseline_tree=$(manifest_value tree_hash "$MANIFEST") ||
    die "symbol.toml has no sync tree_hash baseline; clone the site again"
  baseline_revision=$(manifest_value content_revision "$MANIFEST") ||
    die "symbol.toml has no content_revision baseline; clone the site again"
  work=$(mktemp -d) || exit 1
  baseline=$work/baseline
  localmap=$work/local
  upstream=$work/upstream
  changed=$work/changed
  manifest_files "$MANIFEST" | LC_ALL=C sort > "$baseline"
  local_file_map "$MANIFEST_DIR" > "$localmap"
  if ! http_request GET "${MANIFEST_HOST}/${MANIFEST_NAME}/FILES" -H 'Accept: application/json'; then
    rm -rf "$work"
    return 1
  fi
  upstream_tree=$(json_string tree_hash < "$HTTP_BODY")
  upstream_revision=$(json_string content_revision < "$HTTP_BODY")
  upstream_file_map "$HTTP_BODY" > "$upstream"
  if [ "$upstream_tree" != "$baseline_tree" ] || ! cmp -s "$baseline" "$upstream"; then
    printf 'error: upstream changed since this checkout\n\n' >&2
    printf 'checkout base: revision %s  %s\n' "$baseline_revision" "$baseline_tree" >&2
    printf 'upstream now:  revision %s  %s\n' "$upstream_revision" "$upstream_tree" >&2
    printf 'refusing to modify %s/%s/\n' "$MANIFEST_HOST" "$MANIFEST_NAME" >&2
    rm -rf "$work"
    return 1
  fi
  awk -F '\t' '
    NR==FNR { base[$1]=$2; next }
    { local[$1]=$2; if (!($1 in base)) print "+\t" $1; else if (base[$1] != $2) print "M\t" $1 }
    END { for (path in base) if (!(path in local)) print "-\t" path }
  ' "$baseline" "$localmap" | LC_ALL=C sort -k2,2 > "$changed"
  printf 'upstream: unchanged @ revision %s\n\n' "$upstream_revision"
  additions=$(awk -F '\t' '$1=="+" || $1=="M" {n++} END {print n+0}' "$changed")
  if [ "$additions" -gt 0 ]; then
    [ "$check" -eq 0 ] && printf 'local changes:\n' || printf 'would sync:\n'
    awk -F '\t' '$1=="+" || $1=="M" {printf "  %s %s\n",$1,$2}' "$changed"
  fi
  deletions=$(awk -F '\t' '$1=="-" {n++} END {print n+0}' "$changed")
  if [ "$deletions" -gt 0 ]; then
    printf '\n'
    [ "$check" -eq 0 ] && printf 'local deletions ignored:\n' || printf 'would ignore local deletion:\n'
    awk -F '\t' '$1=="-" {printf "  - %s\n",$2}' "$changed"
  fi
  if [ "$check" -eq 1 ]; then
    printf '\nno changes made\n'
    rm -rf "$work"
    return
  fi
  if [ "$additions" -eq 0 ]; then
    printf '\nnothing to sync\n'
    rm -rf "$work"
    return
  fi
  stage=$work/stage
  mkdir "$stage"
  awk -F '\t' '$1=="+" || $1=="M" {print $2}' "$changed" |
  while IFS= read -r path; do
    mkdir -p "$stage/$(dirname "$path")"
    cp "$MANIFEST_DIR/$path" "$stage/$path"
  done
  archive=$work/changed.tar.gz
  tar -czf "$archive" -C "$stage" .
  auth=$(mktemp) || exit 1
  auth_args_file "$auth"
  set -- -H 'Content-Type: application/gzip' -H 'Unpack: 1' \
    -H "If-Match: $baseline_tree" -H "Idempotency-Key: $(random_key)"
  while IFS= read -r arg; do set -- "$@" "$arg"; done < "$auth"
  rm -f "$auth"
  if ! http_request PUT "${MANIFEST_HOST}/${MANIFEST_NAME}" "$@" -T "$archive"; then
    if [ "$HTTP_STATUS" = 412 ]; then
      printf 'error: upstream changed during sync; nothing was written\n' >&2
    fi
    rm -rf "$work"
    return 1
  fi
  print_mutation_result "$MANIFEST_NAME"
  preserved=$work/preserved
  awk '/^[[:space:]]*(token|claim)[[:space:]]*=/{print}' "$MANIFEST" > "$preserved"
  fresh=$work/symbol.toml
  curl -sS -f "${MANIFEST_HOST}/${MANIFEST_NAME}/symbol.toml" -o "$fresh" ||
    die "sync succeeded, but could not refresh local symbol.toml"
  if [ -s "$preserved" ]; then
    merged=$work/merged
    awk 'BEGIN{done=0} /^[[:space:]]*\[/ && !done {while((getline l < p)>0) print l; done=1} {print}
      END{if(!done) while((getline l < p)>0) print l}' p="$preserved" "$fresh" > "$merged"
    mv "$merged" "$fresh"
  fi
  mv "$fresh" "$MANIFEST"
  printf 'synced %s/%s/\n' "$MANIFEST_HOST" "$MANIFEST_NAME"
  rm -rf "$work"
}

case "$cmd" in
  put)
    unpack=0
    managed=0
    forced=
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -u|--unpack) unpack=1; shift ;;
        --managed) managed=1; shift ;;
        -f)
          [ "$#" -ge 2 ] || usage_error "-f requires a remote path"
          forced=$2
          [ "$forced" != "-" ] || usage_error "remote file path cannot be -"
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
    if [ ! -t 0 ] || [ "$explicit_stdin" -eq 1 ]; then
      stdin_to_temp
      if [ "$explicit_stdin" -eq 1 ] || [ -s "$STDIN_FILE" ]; then
        piped=1
      else
        rm -f "$STDIN_FILE"
        STDIN_FILE=
      fi
    fi
    if [ "$piped" -eq 1 ]; then
      if [ "$explicit_stdin" -eq 1 ]; then shift; fi
      if [ -n "$forced" ]; then
        remote=$forced
        if [ "$#" -eq 1 ]; then
          site=$1
          is_site_name "$site" || usage_error "invalid site name: $site"
          base=$HOST
          generated=0
        elif [ "$#" -eq 0 ]; then
          local_manifest=$(manifest_find 2>/dev/null || true)
          if [ -n "$local_manifest" ]; then
            MANIFEST=$local_manifest
            MANIFEST_DIR=$(dirname "$MANIFEST")
            MANIFEST_HOST=$(manifest_value host "$MANIFEST")
            MANIFEST_NAME=$(manifest_value name "$MANIFEST")
            base=$MANIFEST_HOST; site=$MANIFEST_NAME; generated=0
          else
            base=$HOST; site=; generated=1
          fi
        else
          usage_error "piped put -f accepts at most a site name"
        fi
      elif [ "$#" -eq 0 ]; then
        base=$HOST; site=; remote=index.html; generated=1
      elif [ "$#" -eq 1 ]; then
        case "$1" in
          *.*)
            remote=$1
            local_manifest=$(manifest_find 2>/dev/null || true)
            if [ -n "$local_manifest" ]; then
              MANIFEST=$local_manifest
              MANIFEST_HOST=$(manifest_value host "$MANIFEST")
              MANIFEST_NAME=$(manifest_value name "$MANIFEST")
              base=$MANIFEST_HOST; site=$MANIFEST_NAME; generated=0
            else
              base=$HOST; site=; generated=1
            fi
            ;;
          *)
            is_site_name "$1" || usage_error "invalid site name: $1"
            base=$HOST; site=$1; remote=index.html; generated=0
            ;;
        esac
      elif [ "$#" -eq 2 ]; then
        site=$1; remote=$2; base=$HOST; generated=0
        is_site_name "$site" || usage_error "invalid site name: $site"
        [ "$remote" != "-" ] || usage_error "remote file path cannot be -"
      else
        usage_error "piped put accepts at most NAME DEST"
      fi
      put_file_request "$base" "$site" "$STDIN_FILE" "$remote" 0 "$managed" "" "$generated"
      rm -f "$STDIN_FILE"
    elif [ "$#" -eq 0 ]; then
      [ -z "$forced" ] || usage_error "-f requires piped input"
      manifest_target
      put_file_request "$MANIFEST_HOST" "$MANIFEST_NAME" "$MANIFEST_DIR" "" 1 "$managed"
      refresh_local_manifest "$MANIFEST" "$MANIFEST_HOST" "$MANIFEST_NAME"
    elif [ "$#" -eq 1 ]; then
      [ -z "$forced" ] || usage_error "-f requires piped input"
      put_file_request "$HOST" "" "$1" "" "$unpack" "$managed" "" 1
    elif [ "$#" -eq 2 ]; then
      [ -z "$forced" ] || usage_error "-f requires piped input"
      site=$1; source=$2
      is_site_name "$site" || usage_error "invalid site name: $site"
      if [ -d "$source" ] || [ "$unpack" -eq 1 ]; then
        remote=
      else
        remote=$(basename "$source")
        case "$source" in *.html|*.htm) remote=index.html ;; esac
      fi
      put_file_request "$HOST" "$site" "$source" "$remote" "$unpack" "$managed"
    else
      [ -z "$forced" ] || usage_error "-f requires piped input"
      site=$1; source=$2; remote=$3
      is_site_name "$site" || usage_error "invalid site name: $site"
      [ "$remote" != "-" ] || usage_error "remote file path cannot be -"
      put_file_request "$HOST" "$site" "$source" "$remote" "$unpack" "$managed"
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
      if [ "$links" -eq 1 ]; then
        request GET /FILES | print_links "$HOST" 0
      else
        request GET /FILES
      fi
    else
      name=$1
      if [ "$links" -eq 1 ]; then
        request GET "/${name}/FILES" | print_links "${HOST}/${name}" 1
      else
        request GET "/${name}/FILES"
      fi
    fi
    ;;
  get)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol get NAME [ARCHIVE]"
    name=$1
    dest=${2:-${name}.tar.gz}
    is_site_name "$name" || usage_error "invalid site name: $name"
    archive_transfer GET "$HOST" "$name" "$dest"
    [ "$dest" = "-" ] || printf 'downloaded %s\n' "$dest"
    ;;
  pop)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol pop NAME [ARCHIVE]"
    name=$1
    dest=${2:-${name}.tar.gz}
    is_site_name "$name" || usage_error "invalid site name: $name"
    archive_transfer DELETE "$HOST" "$name" "$dest"
    [ "$dest" = "-" ] || printf 'popped %s\n' "$dest"
    ;;
  clone)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol clone NAME [DIR]"
    clone_site "$1" "${2:-$1}"
    ;;
  copy)
    managed=0
    case "${1:-}" in --managed) managed=1; shift ;; esac
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol copy [--managed] SRC [DST]"
    copy_or_move_request COPY "$1" "${2:-}" "$managed"
    ;;
  remix)
    managed=0
    case "${1:-}" in --managed) managed=1; shift ;; esac
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol remix [--managed] SRC [DST]"
    copy_or_move_request COPY "$1" "${2:-}" "$managed"
    copied=$RESULT_NAME
    if ! clone_site "$copied" "$copied"; then
      printf 'server copy remains at %s/%s/\ncleanup with: symbol rm %s\n' \
        "$HOST" "$copied" "$copied" >&2
      exit 1
    fi
    ;;
  move)
    [ "$#" -eq 2 ] || usage_error "usage: symbol move SRC DST"
    copy_or_move_request MOVE "$1" "$2" 0
    ;;
  sync)
    check=0
    case "${1:-}" in --check) check=1; shift ;; esac
    [ "$#" -eq 0 ] || usage_error "usage: symbol sync [--check]"
    sync_project "$check"
    ;;
  undo)
    stack=0
    case "${1:-}" in -s|--stack) stack=1; shift ;; esac
    [ "$#" -le 2 ] || usage_error "usage: symbol undo [--stack] [NAME [TOKEN]]"
    if [ "$#" -eq 0 ]; then
      manifest_target
      base=$MANIFEST_HOST; name=$MANIFEST_NAME; token=
    else
      base=$HOST; name=$1; token=${2:-}
    fi
    is_site_name "$name" || usage_error "invalid site name: $name"
    if [ "$stack" -eq 1 ]; then
      [ -z "$token" ] || usage_error "--stack does not accept a token"
      request GET "/${name}/UNDO" | awk '
        BEGIN { print "TOKEN      WOULD UNDO                         EXPIRES" }
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
            remaining=object; sub(/^.*"remaining_seconds"[[:space:]]*:[[:space:]]*/,"",remaining); sub(/[^0-9].*$/,"",remaining)
            printf "%-10s %-34s %s (%ss)\n",token,description,expires,remaining
            rest=substr(rest,RSTART+RLENGTH)
          }
        }
      '
    else
      auth=$(mktemp) || exit 1
      auth_args_file "$auth"
      set --
      while IFS= read -r arg; do set -- "$@" "$arg"; done < "$auth"
      rm -f "$auth"
      [ -z "$token" ] || set -- "$@" -H "Undo-Token: $token"
      http_request UNDO "${base}/${name}" "$@" || exit 1
      cat "$HTTP_BODY"
    fi
    ;;
  expire)
    if [ "$#" -eq 0 ] || [ "${1:-}" = --help ] || [ "${1:-}" = -h ]; then
      expire_help
      exit 0
    fi
    case "$1" in --show|--info|--graph) expire_help; exit 0 ;; esac
    name=$1; shift
    is_site_name "$name" || usage_error "invalid site name: $name"
    path=
    case "${1:-}" in ''|--*) ;; *) path=$1; shift ;; esac
    mode=default
    min_age=; max_age=; max_size=; power=
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --decay) [ "$mode" = default ] || usage_error "expiration modes are mutually exclusive"; mode=decay; shift ;;
        --in) [ "$mode" = default ] || usage_error "expiration modes are mutually exclusive"; [ "$#" -ge 2 ] || usage_error "--in requires DURATION"; duration_valid "$2" || usage_error "invalid duration: $2"; mode=relative; in_value=$2; shift 2 ;;
        --at) [ "$mode" = default ] || usage_error "expiration modes are mutually exclusive"; [ "$#" -ge 2 ] || usage_error "--at requires TIMESTAMP"; case "$2" in ????-??-??T??:??:??Z|????-??-??T??:??:??[+-]??:??) ;; *) usage_error "absolute time must be RFC 3339 with an offset" ;; esac; mode=absolute; at_value=$2; shift 2 ;;
        --never) [ "$mode" = default ] || usage_error "expiration modes are mutually exclusive"; mode=never; shift ;;
        --show|--info|--graph) [ "$mode" = default ] || usage_error "expiration modes are mutually exclusive"; mode=show; shift ;;
        --min-age) [ "$#" -ge 2 ] || usage_error "--min-age requires DURATION"; duration_valid "$2" || usage_error "invalid duration: $2"; min_age=$2; shift 2 ;;
        --max-age) [ "$#" -ge 2 ] || usage_error "--max-age requires DURATION"; duration_valid "$2" || usage_error "invalid duration: $2"; max_age=$2; shift 2 ;;
        --max-size) [ "$#" -ge 2 ] || usage_error "--max-size requires SIZE"; size_valid "$2" || usage_error "invalid size: $2"; max_size=$2; shift 2 ;;
        --power) [ "$#" -ge 2 ] || usage_error "--power requires NUMBER"; case "$2" in ''|*[!0-9.]*|0|0.0) usage_error "power must be greater than zero" ;; esac; power=$2; shift 2 ;;
        *) usage_error "unknown expire option: $1" ;;
      esac
    done
    [ "$mode" = decay ] || {
      [ -z "$min_age$max_age$max_size$power" ] ||
        usage_error "decay constants require --decay"
    }
    target="/${name}"
    [ -z "$path" ] || target="$target/$(urlencode_path "$path")"
    if [ "$mode" = show ]; then
      http_request GET "${HOST}${target}/EXPIRES" -H 'Accept: application/json' || exit 1
      print_expire_report
    else
      auth=$(mktemp) || exit 1
      auth_args_file "$auth"
      set --
      while IFS= read -r arg; do set -- "$@" "$arg"; done < "$auth"
      rm -f "$auth"
      case "$mode" in
        decay) set -- "$@" -H 'Expiry-Mode: decay' ;;
        relative) set -- "$@" -H 'Expiry-Mode: relative' -H "Expiry-In: $in_value" ;;
        absolute) set -- "$@" -H 'Expiry-Mode: absolute' -H "Expiry-At: $at_value" ;;
        never) set -- "$@" -H 'Expiry-Mode: never' ;;
      esac
      [ -z "$min_age" ] || set -- "$@" -H "Expiry-Min-Age: $min_age"
      [ -z "$max_age" ] || set -- "$@" -H "Expiry-Max-Age: $max_age"
      [ -z "$max_size" ] || set -- "$@" -H "Expiry-Max-Size: $max_size"
      [ -z "$power" ] || set -- "$@" -H "Expiry-Power: $power"
      http_request EXPIRE "${HOST}${target}" "$@" || exit 1
      print_mutation_result "$name" 1
      [ -s "$HTTP_BODY" ] && print_expire_report
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
    is_site_name "$name" || usage_error "invalid site name: $name"
    case "$action" in
      --claim) action=claim ;;
      --status) action=status ;;
      --rotate) action=rotate ;;
      --release) action=release ;;
      *) usage_error "unknown management action: $action" ;;
    esac
    auth=$(mktemp) || exit 1
    auth_args_file "$auth"
    set -- -H "Management-Action: $action"
    while IFS= read -r arg; do set -- "$@" "$arg"; done < "$auth"
    rm -f "$auth"
    if [ "$action" = claim ]; then
      local_manifest=$(matching_manifest "$HOST" "$name" 2>/dev/null || true)
      if [ -n "$local_manifest" ]; then
        claim=$(claim_from_manifest "$local_manifest" 2>/dev/null || true)
        [ -z "$claim" ] || set -- "$@" -H "Creator-Claim: $claim"
      fi
      set -- "$@" -H "Idempotency-Key: $(random_key)"
    elif [ "$action" = rotate ]; then
      set -- "$@" -H "Idempotency-Key: $(random_key)"
    fi
    http_request MANAGE "${HOST}/${name}" "$@" || exit 1
    cat "$HTTP_BODY"
    save_response_secrets "$HOST" "$name"
    if [ "$action" = release ]; then
      local_manifest=$(matching_manifest "$HOST" "$name" 2>/dev/null || true)
      if [ -n "$local_manifest" ]; then
        dir=$(dirname "$local_manifest")
        rm -f "$dir/.symbol-token"
        tmp=$(mktemp "$dir/.symbol.toml.XXXXXX") || exit 1
        awk '!/^[[:space:]]*token[[:space:]]*=/' "$local_manifest" > "$tmp"
        mv "$tmp" "$local_manifest"
      fi
    fi
    ;;
  rm)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_error "usage: symbol rm NAME [PATH]"
    name=$1
    remote_path=${2:-}
    remove_site=0
    [ "$#" -ne 1 ] || remove_site=1
    is_site_name "$name" || usage_error "invalid site name: $name"
    auth=$(mktemp) || exit 1
    auth_args_file "$auth"
    set --
    while IFS= read -r arg; do set -- "$@" "$arg"; done < "$auth"
    rm -f "$auth"
    if [ "$remove_site" -eq 1 ]; then
      http_request DELETE "${HOST}/${name}" "$@" || exit 1
      printf 'deleted %s\n' "$name"
      print_mutation_result "$name" 1
    else
      encoded=$(urlencode_path "$remote_path")
      http_request DELETE "${HOST}/${name}/${encoded}" "$@" || exit 1
      print_mutation_result "$name"
    fi
    ;;
  url)
    [ "$#" -eq 1 ] || usage_error "usage: symbol url NAME"
    is_site_name "$1" || usage_error "invalid site name: $1"
    printf '%s/%s/\n' "$HOST" "$1"
    ;;
  update)
    [ "$#" -eq 0 ] || usage_error "update accepts no arguments"
    self=$(client_path)
    dest=$(dirname "$self")
    hashfile="${dest}/.symbol.blake3"
    have=
    if [ -f "$hashfile" ]; then
      have=$(tr -d ' \t\r\n' < "$hashfile")
    fi
    tmp=$(mktemp)
    curl -fsSL "${HOST}/symbol.sh" -o "$tmp"
    chmod +x "$tmp"
    stats=$(diff_stats "$self" "$tmp")
    mv "$tmp" "$self"
    remote=$(curl -fsSL "${HOST}/symbol.sh/HASH" | tr -d ' \t\r\n')
    printf '%s\n' "$remote" > "$hashfile"
    rm -f "${dest}/.symbol.hash"
    if [ -n "$remote" ] && [ "$remote" = "$have" ] && [ -z "$stats" ]; then
      echo "already up to date"
    else
      echo "updated ${self}"
      if [ -n "$stats" ]; then
        echo "  ${stats}"
      fi
    fi
    ;;
  stats)
    [ "$#" -eq 0 ] || usage_error "stats accepts no arguments"
    stats_json=$(request GET /STATS)
    printf '%s\n' "$stats_json" | print_stats
    ;;
  help)
    usage
    ;;
esac

http_cleanup
join_update_check
trap - EXIT
