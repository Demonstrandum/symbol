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
  symbol ls                            list sites
  symbol ls   NAME                     list files
  symbol pop  NAME [FILE]              delete a site, save it as tar.gz
  symbol rm   NAME                     delete a site
  symbol rm   NAME PATH                delete a file
  symbol url  NAME                     print the public url
  symbol update                        pull a new client if the server has one
  symbol stats                         server stats
  symbol help                          this message

  -u, --unpack   extract a zip/tar/tar.gz/gz into the site
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
  awk -F '[,:{}]' '
    function human(n, base, binary, labels, units, i) {
      labels = binary ? "B KiB MiB GiB TiB PiB" : "B kB MB GB TB PB"
      split(labels, units, " ")
      i = 1
      while (n >= base && i < 6) {
        n /= base
        i++
      }
      if (i == 1) return sprintf("%.0f %s", n, units[i])
      return sprintf("%.2f %s", n, units[i])
    }
    {
      for (i = 2; i < NF; i += 2) {
        key = $i
        value = $(i + 1)
        gsub(/["[:space:]]/, "", key)
        gsub(/[[:space:]]/, "", value)
        if (key == "sites") sites = value
        else if (key == "files") files = value
        else if (key == "blobs") blobs = value
        else if (key == "bytes") bytes = value
      }
    }
    END {
      printf "sites %s\nfiles %s\nblobs %s\n", sites, files, blobs
      printf "bytes %s (%s / %s)\n", bytes, human(bytes, 1000, 0), human(bytes, 1024, 1)
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
    if [ "$#" -eq 0 ]; then
      request GET /FILES
    else
      request GET "/$1/FILES"
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
