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

start_update_check "${1:-}"
trap join_update_check EXIT

usage() {
  cat <<EOF
symbol: static hosting on ${HOST}

usage:
  symbol put  [-u] FILE|DIR|ZIP        publish (short id)
  symbol put  [-u] NAME FILE|DIR|ZIP   publish as NAME (overwrites if it exists)
  symbol add  NAME FILE [DEST]         add or update one file
  symbol ls   [-l]                     list sites
  symbol ls   [-l] NAME                list files
  symbol get  NAME [ARCHIVE]           download a site without deleting it
  symbol pop  NAME [FILE]              delete a site, save it as tar.gz
  symbol rm   NAME                     delete a site
  symbol rm   NAME PATH                delete a file
  symbol url  NAME                     print the public url
  symbol update                        pull a new client if the server has one
  symbol stats                         server stats
  symbol help                          this message

  -u, --unpack   extract a zip/tar/tar.gz/gz into the site
  -l, --links    print full URLs in listings
  directories are always unpacked

env: SYMBOL_HOST  (default ${HOST})
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

cmd=${1:-}
if [ "$#" -gt 0 ]; then
  shift
fi

case "$cmd" in
  put)
    unpack=0
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -u|--unpack) unpack=1; shift ;;
        --) shift; break ;;
        -*)
          echo "error: unknown flag: $1" >&2
          usage >&2
          exit 2
          ;;
        *) break ;;
      esac
    done
    if [ "$#" -eq 1 ]; then
      put_body "$1" "/" "$unpack"
    elif [ "$#" -ge 2 ]; then
      put_body "$2" "/$1" "$unpack"
    else
      need 1 "$@"
    fi
    ;;
  add)
    need 2 "$@"
    name=$1
    src=$2
    dest=${3:-$(basename "$src")}
    curl -sS -T "$src" "${HOST}/${name}/${dest}"
    ;;
  ls|list)
    links=0
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -l|--links) links=1; shift ;;
        --) shift; break ;;
        -*)
          echo "error: unknown flag: $1" >&2
          usage >&2
          exit 2
          ;;
        *) break ;;
      esac
    done
    if [ "$#" -gt 1 ]; then
      echo "error: ls accepts at most one site name" >&2
      usage >&2
      exit 2
    fi
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
  get|download)
    need 1 "$@"
    name=$1
    dest=${2:-${name}.tar.gz}
    case "$dest" in
      *.tar.gz|*.tgz) suffix=.tar.gz ;;
      *.tar) suffix=.tar ;;
      *.zip) suffix=.zip ;;
      *)
        echo "error: archive must end in .tar.gz, .tgz, .tar, or .zip" >&2
        exit 2
        ;;
    esac
    tmp=$(mktemp)
    if curl -sS -f "${HOST}/${name}${suffix}" -o "$tmp"; then
      mv "$tmp" "$dest"
      echo "downloaded ${dest}"
    else
      rm -f "$tmp"
      exit 1
    fi
    ;;
  pop)
    need 1 "$@"
    name=$1
    dest=${2:-${name}.tar.gz}
    tmp=$(mktemp)
    if curl -sS -f -X DELETE "${HOST}/${name}" -o "$tmp"; then
      mv "$tmp" "$dest"
      echo "popped ${dest}"
    else
      cat "$tmp" >&2
      rm -f "$tmp"
      exit 1
    fi
    ;;
  rm|delete)
    need 1 "$@"
    name=$1
    if [ "$#" -eq 1 ]; then
      curl -sS -f -o /dev/null -X DELETE "${HOST}/${name}"
      echo "deleted ${name}"
    else
      request DELETE "/${name}/$2"
    fi
    ;;
  url)
    need 1 "$@"
    echo "${HOST}/$1/"
    ;;
  update|upgrade)
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
    stats_json=$(request GET /STATS)
    printf '%s\n' "$stats_json" | print_stats
    ;;
  -h|--help|help|"")
    usage
    ;;
  *)
    echo "error: unknown command: $cmd" >&2
    usage >&2
    exit 2
    ;;
esac

join_update_check
trap - EXIT
