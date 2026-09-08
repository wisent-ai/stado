set -eu
LC_ALL=C
export LC_ALL

stado_home="$HOME/.stado"
bin_dir="$stado_home/bin"
forward_dir="$stado_home/forwards"
cargo_home="$HOME/.cargo"
cargo_bin="$cargo_home/bin"

kernel=$(/usr/bin/uname -s 2>/dev/null || :)
architecture=$(/usr/bin/uname -m 2>/dev/null || :)
case "$kernel:$architecture" in
  Darwin:arm64) release_platform=darwin-arm64 ;;
  Linux:x86_64|Linux:amd64) release_platform=linux-amd64 ;;
  *) release_platform=unsupported ;;
esac
field_limit=200
# The cap on how many files each vault section reports. A directory with a
# thousand files must not produce an unbounded report; what was matched
# beyond the cap is counted, not silently dropped.
vault_limit=64
# A Cargo bin directory is normally small (rustup's proxies plus explicitly
# installed tools), but it is still owner-writable input and may not make the
# report unbounded. Every member is counted even after this output cap.
cargo_entry_limit=512
# A literal newline, for the first-line-only expansions below. Written as a
# quoted line break rather than bash's $'\n' so nothing here needs a dialect.
newline='
'

# Reduce one value to a bounded, JSON-inert token: every character outside a
# conservative allowlist becomes '?', and what is left is cut to field_limit
# characters. Escaping would also work; refusing the dangerous characters
# outright is a guarantee that does not depend on getting the escaping right,
# and it means a corrupt or hostile file under ~/.stado cannot emit quotes,
# backslashes, newlines or unbounded text into this report.
#
# Shell builtins only, and the answer comes back in "sanitized" instead of on
# stdout. That is the whole point of this function's shape, not a style
# choice. It used to be "$(printf | tr -d | tr -c | cut)": a command
# substitution plus three external programs, four forks for every field in
# the report. On a host that had run out of per-user process slots the inner
# forks failed, the subshell died, and the substitution produced the empty
# string with status 128 — and because a command substitution in an argument
# position is invisible to `set -e`, the report went out claiming it had read
# names, modes, versions and marker URLs while emitting "" for every one of
# them. A sanitizer that forks nothing cannot fail that way, and returning
# through a variable is what removes the last fork.
sanitize() {
  sanitize_rest="$1"
  sanitized=""
  sanitize_count=0
  while [ -n "$sanitize_rest" ] && [ "$sanitize_count" -lt "$field_limit" ]; do
    # The leading character, taken by stripping the tail that follows it.
    # Under LC_ALL=C '?' is one byte, so this walks bytes the way `cut -c` did.
    sanitize_tail=${sanitize_rest#?}
    sanitize_char=${sanitize_rest%"$sanitize_tail"}
    sanitize_rest=$sanitize_tail
    case "$sanitize_char" in
      [A-Za-z0-9]|' '|.|,|:|';'|/|@|_|+|=|%|'('|')'|-)
        sanitized="$sanitized$sanitize_char"
        ;;
      *)
        # Control characters land here too. The pipeline this replaced
        # deleted them; mapping them to '?' instead keeps the invariant
        # below true and shows the operator that something was removed.
        sanitized="$sanitized?"
        ;;
    esac
    sanitize_count=$((sanitize_count + 1))
  done
  # A non-empty value must never leave here as an empty field, and if it ever
  # does that is a fault of the host and gets reported as one. The loop
  # appends a character for every character it consumes, so this is reachable
  # only when field_limit or the shell's arithmetic has gone wrong — which is
  # exactly the class of failure that once shipped a report of blanks with
  # every state beside them saying the value had been read. Quiet emptiness is
  # the one outcome this function may not have.
  if [ -z "$sanitized" ] && [ -n "$1" ]; then
    sanitized='?'
    sanitizer_state=broken
  fi
}

# Emit lstat metadata for one fixed path or one member discovered below the
# fixed Cargo bin directory. Both stat dialects report the directory entry
# itself by default, so a symlink is never followed. Numeric fields become
# JSON null unless every one was read and validated; zero is a real size and
# must not double as "stat failed".
emit_filesystem_metadata() {
  metadata_path="$1"
  metadata_name="$2"
  if [ -L "$metadata_path" ]; then
    metadata_kind=symlink
  elif [ -d "$metadata_path" ]; then
    metadata_kind=directory
  elif [ -f "$metadata_path" ]; then
    metadata_kind=regular
  elif [ -e "$metadata_path" ]; then
    metadata_kind=other
  else
    metadata_kind=uninspected
  fi

  metadata_complete=true
  metadata_state=unavailable
  metadata_bytes=null
  metadata_mode=unknown
  metadata_uid=null
  metadata_gid=null
  metadata_modified_epoch=null
  # A failed -e check also means denied traversal; only stat's ENOENT
  # diagnosis proves absence. LC_ALL=C above fixes both stat error dialects.
  metadata_facts_read=false
  if [ "$kernel" = Darwin ]; then
    if metadata_facts=$(/usr/bin/stat -f '%z %Lp %u %g %m' "$metadata_path" 2>&1); then
      metadata_facts_read=true
    fi
  else
    if metadata_facts=$(/usr/bin/stat -c '%s %a %u %g %Y' "$metadata_path" 2>&1); then
      metadata_facts_read=true
    fi
  fi
  if [ "$metadata_facts_read" = true ]; then
    metadata_state=malformed
    # Function-local positional parameters keep the split out of the caller's
    # state. A modification epoch may precede 1970; ownership and size may not.
    set -- $metadata_facts
    if [ "$#" -eq 5 ] && [ -n "${5#-}" ]; then
      case "$1:$3:$4:${5#-}" in
        *[!0-9:]*) ;;
        *)
          case "$2" in
            ''|*[!0-7]*) ;;
            *)
              metadata_state=read
              metadata_bytes=$1
              metadata_mode=$2
              metadata_uid=$3
              metadata_gid=$4
              metadata_modified_epoch=$5
              ;;
          esac
          ;;
      esac
    fi
  else
    case "$metadata_facts" in
      *': No such file or directory')
        metadata_kind=missing
        metadata_state=missing
        ;;
    esac
  fi
  if [ "$metadata_kind" = uninspected ] || \
     { [ "$metadata_state" != read ] && [ "$metadata_state" != missing ]; }; then
    metadata_complete=false
  fi

  metadata_symlink_target=""
  metadata_symlink_target_state=not_symlink
  if [ "$metadata_kind" = symlink ]; then
    metadata_symlink_target_state=unavailable
    if metadata_symlink_target=$(/usr/bin/readlink "$metadata_path" 2>/dev/null); then
      metadata_symlink_target_state=read
    else
      metadata_symlink_target=""
      metadata_complete=false
    fi
  fi
  sanitize "$metadata_name"
  metadata_name_safe=$sanitized
  metadata_name_state=read
  if [ "$metadata_name_safe" != "$metadata_name" ]; then
    metadata_name_state=sanitized
    metadata_complete=false
  fi
  sanitize "$metadata_mode"
  metadata_mode_safe=$sanitized
  sanitize "$metadata_symlink_target"
  metadata_symlink_target_safe=$sanitized
  if [ "$metadata_symlink_target_safe" != "$metadata_symlink_target" ]; then
    metadata_symlink_target_state=sanitized
    metadata_complete=false
  fi
  printf '{"name":"%s","name_state":"%s","kind":"%s","metadata_state":"%s","bytes":%s,"mode":"%s","uid":%s,"gid":%s,"modified_epoch":%s,"symlink_target":"%s","symlink_target_state":"%s"}' \
    "$metadata_name_safe" "$metadata_name_state" "$metadata_kind" "$metadata_state" \
    "$metadata_bytes" "$metadata_mode_safe" "$metadata_uid" "$metadata_gid" \
    "$metadata_modified_epoch" "$metadata_symlink_target_safe" \
    "$metadata_symlink_target_state"
}

# A non-directory parent refused before traversal still gets one whole typed
# row. This is distinct from `missing`: no claim was made about what sits
# below the non-directory.
emit_refused_filesystem_metadata() {
  metadata_name="$1"
  metadata_state="$2"
  sanitize "$metadata_name"
  metadata_name_safe=$sanitized
  printf '{"name":"%s","name_state":"read","kind":"uninspected","metadata_state":"%s","bytes":null,"mode":"unknown","uid":null,"gid":null,"modified_epoch":null,"symlink_target":"","symlink_target_state":"not_symlink"}' \
    "$metadata_name_safe" "$metadata_state"
  metadata_complete=false
  metadata_kind=uninspected
}

