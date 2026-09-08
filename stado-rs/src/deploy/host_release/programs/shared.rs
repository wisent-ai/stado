/// The field sanitizer and the marker printer, shared by all three programs.
///
/// Lifted from [`crate::deploy::host_inventory::REMOTE_INVENTORY_SCRIPT`]
/// with its finding intact: this used to be a command substitution around
/// `tr`/`cut`, four forks per field, and on a host out of process slots the
/// forks failed, the substitution produced the empty string, and a report of
/// blanks went out claiming every field had been read. Builtins only, the
/// answer returned through a variable, and the sanitizer proves itself
/// against a fixed probe before anything is reported.
pub const SANITIZE_PRELUDE: &str = r##"set -eu
LC_ALL=C
export LC_ALL
field_limit=200
newline='
'

sanitize() {
  sanitize_rest="$1"
  sanitized=""
  sanitize_count=0
  while [ -n "$sanitize_rest" ] && [ "$sanitize_count" -lt "$field_limit" ]; do
    sanitize_tail=${sanitize_rest#?}
    sanitize_char=${sanitize_rest%"$sanitize_tail"}
    sanitize_rest=$sanitize_tail
    case "$sanitize_char" in
      [A-Za-z0-9]|' '|.|,|:|';'|/|@|_|+|=|%|'('|')'|-)
        sanitized="$sanitized$sanitize_char"
        ;;
      *)
        sanitized="$sanitized?"
        ;;
    esac
    sanitize_count=$((sanitize_count + 1))
  done
  if [ -z "$sanitized" ] && [ -n "$1" ]; then
    sanitized='?'
    sanitizer_state=broken
  fi
}

sanitizer_state=ok
sanitize 'probe-Value_1.2'
if [ "$sanitized" != 'probe-Value_1.2' ]; then
  sanitizer_state=broken
fi
sanitize 'a"b'
if [ "$sanitized" != 'a?b' ]; then
  sanitizer_state=broken
fi
sanitize_long=0123456789
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize "$sanitize_long"
if [ "${#sanitized}" -ne "$field_limit" ]; then
  sanitizer_state=broken
fi

# Every value leaving this host goes through here, so there is one place to
# forget and it is not a call site. The key is always a literal of the
# program below; only the value can come off the host.
say() {
  sanitize "$2"
  printf '%s\t%s\t%s\n' STADO_RELEASE "$1" "$sanitized"
}

# The bare version a managed binary declares. `stado --version` answers one
# plain line ("stado 0.5.1"); `skarbiec version` answers a JSON object. Both
# are read by shape and reduced to the bare coordinate, because the bare
# coordinate is what the registry declares and what this command compares.
read_version() {
  read_version_path="$1"
  read_version_value=""
  read_version_state=missing
  # -L first, never -f first: -f follows the link, so a symlink to another
  # binary would be executed as if it were the managed one.
  if [ -L "$read_version_path" ]; then
    read_version_state=refused_symlink
    return 0
  fi
  if [ ! -f "$read_version_path" ]; then
    return 0
  fi
  if [ ! -x "$read_version_path" ]; then
    read_version_state=not_executable
    return 0
  fi
  if read_version_output=$("$read_version_path" "$version_argument" 2>/dev/null); then
    :
  else
    read_version_state=version_failed
    return 0
  fi
  if [ -z "$read_version_output" ]; then
    read_version_state=version_empty
    return 0
  fi
  if [ "$version_shape" = json ]; then
    case "$read_version_output" in
      *'"version"'*)
        read_version_rest=${read_version_output#*'"version"'}
        read_version_gap=${read_version_rest%%'"'*}
        case "$read_version_rest" in
          *'"'*)
            case "$read_version_gap" in
              *[!:[:space:]]*) ;;
              *)
                read_version_rest=${read_version_rest#*'"'}
                read_version_value=${read_version_rest%%'"'*}
                ;;
            esac
            ;;
        esac
        ;;
    esac
  else
    read_version_line=${read_version_output%%"$newline"*}
    # The version is the word after the program name, not the last word. The
    # banner reads `stado 0.14.9 (rev 519ae967a13d-dirty)` since the build stamp
    # joined it, and reading the last word pulled `519ae967a13d-dirty)` out of
    # it, which failed the coordinate comparison as `layout version_mismatch` —
    # after the archive had been verified and staged. The 0.14.9 delivery to
    # charless-mac-mini died there on 2026-09-04. An annotation appended to a
    # banner is not a reason a verified delivery refuses itself.
    set -- $read_version_line
    case "$#" in
      0|1) read_version_value="" ;;
      *) read_version_value="$2" ;;
    esac
  fi
  if [ -z "$read_version_value" ]; then
    read_version_state=version_unparsable
  else
    read_version_state=reported
  fi
}
"##;

/// One release object, fetched whole, for either install shape.
///
/// The release route answers an unranged GET by streaming until its own
/// window closes, and it closes the body cleanly when it does. `curl` sees a
/// complete HTTP/2 response and exits 0 on a file that stopped short: the
/// darwin archive of 0.13.46 is 73,864,632 bytes, and two unranged reads of
/// it ended, both HTTP 200, at 22,925,186 and 15,318,446 bytes after about
/// 300 seconds each. Nothing in the fetch could see that. The short body
/// surfaced one step later as `verify mismatch`, which says the coordinate
/// holds bytes that disagree with its manifest -- the two-producer shape --
/// when the coordinate was whole and the transfer was not.
///
/// So the size is asked for before the body, and the body is read in bounded
/// ranges until it is that size. The route advertises `accept-ranges: bytes`
/// and answers `--range 0-0` with the object's total, and an 8 MiB range with
/// exactly 8 MiB in about 94 seconds, well inside the window that truncates
/// the whole-object read. A range that comes back short or refused is a
/// failure with its own marker instead of a digest that will not match.
pub const FETCH_PRELUDE: &str = r##"
fetch_chunk_bytes=8388608

# `release_resolve` is empty unless the origin is a tailnet name this fleet can
# place, in which case the route is pinned to the tailnet address while the URL,
# the SNI name and the certificate check stay exactly as they were. Every
# request this file makes goes through here, so the pin cannot be applied to the
# body read and forgotten for the size read.
release_object_curl() {
  if [ -n "${release_resolve:-}" ]; then
    /usr/bin/curl -fsS --get --resolve "$release_resolve" "$@" \
      "$release_api/api/release/object"
  else
    /usr/bin/curl -fsS --get "$@" \
      "$release_api/api/release/object"
  fi
}

release_object_total() {
  release_object_curl \
    --data-urlencode "uri=$1" \
    --range 0-0 \
    --dump-header "$2" \
    --output /dev/null || return 1
  /usr/bin/tr -d '\r' < "$2" |
    /usr/bin/awk 'tolower($1) == "content-range:" {
      if (split($2, part, "/") == 2) { total = part[2] }
    }
    END { print total }'
}

# Appends to "$2". The caller owns removing a partial file, because only the
# caller knows whether a partial file is worth resuming.
fetch_release_object() {
  fetch_uri=$1
  fetch_path=$2
  fetch_head="$fetch_path.head"
  fetch_part="$fetch_path.part"
  # The operator side read the published size before this program existed, so
  # the total is bound, not discovered. Deriving it from a `Range: 0-0`
  # answer's `Content-Range` only worked through the tailnet proxy: the
  # dashboard's own release route serves no ranges, so the host that serves
  # the store fetching over its own loopback got no `Content-Range` and
  # refused with `no_declared_size`. The probe remains for a bound of zero,
  # which is a caller that could not read the size.
  fetch_total=$archive_bytes
  if [ "$fetch_total" -eq 0 ]; then
    fetch_total=$(release_object_total "$fetch_uri" "$fetch_head") || {
      /bin/rm -f "$fetch_head"
      say fetch failed
      return 1
    }
    /bin/rm -f "$fetch_head"
  fi
  case "$fetch_total" in
    ''|*[!0-9]*)
      say fetch no_declared_size
      return 1
      ;;
  esac
  fetch_have=0
  while [ "$fetch_have" -lt "$fetch_total" ]; do
    fetch_end=$((fetch_have + fetch_chunk_bytes - 1))
    if [ "$fetch_end" -ge "$fetch_total" ]; then
      fetch_end=$((fetch_total - 1))
    fi
    fetch_want=$((fetch_end - fetch_have + 1))
    /bin/rm -f "$fetch_head" "$fetch_part"
    release_object_curl \
      --data-urlencode "uri=$fetch_uri" \
      --range "$fetch_have-$fetch_end" \
      --dump-header "$fetch_head" \
      --output "$fetch_part" || {
        /bin/rm -f "$fetch_head" "$fetch_part"
        say fetch "failed_at_$fetch_have"
        return 1
      }
    fetch_status=$(
      /usr/bin/tr -d '\r' < "$fetch_head" |
        /usr/bin/awk 'toupper($1) ~ /^HTTP/ { code = $2 } END { print code }'
    )
    if [ "$fetch_status" != 206 ]; then
      /bin/rm -f "$fetch_head" "$fetch_part"
      say fetch "refused_range_$fetch_status"
      return 1
    fi
    fetch_got=$(/usr/bin/wc -c < "$fetch_part" | /usr/bin/tr -d '[:space:]')
    if [ "$fetch_got" != "$fetch_want" ]; then
      /bin/rm -f "$fetch_head" "$fetch_part"
      say fetch "short_range_${fetch_have}_${fetch_got}_of_$fetch_want"
      return 1
    fi
    /bin/cat "$fetch_part" >> "$fetch_path" || {
      /bin/rm -f "$fetch_head" "$fetch_part"
      say fetch "cannot_append_at_$fetch_have"
      return 1
    }
    /bin/rm -f "$fetch_head" "$fetch_part"
    fetch_have=$((fetch_have + fetch_got))
  done
  fetch_size=$(/usr/bin/wc -c < "$fetch_path" | /usr/bin/tr -d '[:space:]')
  if [ "$fetch_size" != "$fetch_total" ]; then
    say fetch "truncated_${fetch_size}_of_$fetch_total"
    return 1
  fi
  say fetch ok
}
"##;
