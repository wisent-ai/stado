//! The socket-table half of the remote program: read the listening TCP
//! sockets with `lsof`, or with `netstat` when the host has no `lsof`, and
//! emit the one line of JSON the whole report is.
//!
//! This is the second of two pieces of one script, not a program of its own.
//! It opens with the blank line that already separated the two sections, so
//! appending it to `REMOTE_ENV_FILE_HEAD` reproduces the delivered text byte
//! for byte.

/// The socket table and the final report line.
pub(super) const REMOTE_ENV_FILE_TAIL: &str = r##"
# The socket table, with the program that owns each port. lsof first, with the
# same fixed flags as the approved `host exec` entry so the two readers cannot
# disagree; netstat second, which answers the port question and not the owner
# question, and says so through its own state.
listeners_state=failed
listeners_json=""
listener_source=""
raw=""
# Judged on whether the reader produced a table, not on its exit status: lsof
# routinely exits non-zero after listing every socket it could see, because
# one file descriptor somewhere refused to be identified. Treating that as
# "the socket table could not be read" would report every endpoint on a
# healthy host as unjudged.
for candidate in /usr/sbin/lsof /usr/bin/lsof; do
  if [ -x "$candidate" ]; then
    raw=$("$candidate" -nP -iTCP -sTCP:LISTEN 2>/dev/null) || raw=""
    if [ -n "$raw" ]; then
      listener_source=lsof
      break
    fi
  fi
done
if [ -z "$listener_source" ]; then
  for candidate in /usr/sbin/netstat /bin/netstat /usr/bin/netstat; do
    if [ -x "$candidate" ]; then
      raw=$("$candidate" -anv -p tcp 2>/dev/null) || raw=""
      if [ -n "$raw" ]; then
        listener_source=netstat
        break
      fi
    fi
  done
fi
if [ "$listener_source" = lsof ]; then
  if listeners_json=$(printf '%s\n' "$raw" | /usr/bin/awk '
function jsonsafe(text) {
  gsub(/[^ -~]/, "?", text)
  gsub(/["\\]/, "?", text)
  return text
}
NR == 1 { next }
$NF == "(LISTEN)" {
  address = $(NF - 1)
  if (!match(address, /:[0-9]+$/)) next
  port = substr(address, RSTART + 1) + 0
  authority = substr(address, 1, RSTART - 1)
  if (authority != "*" && authority != "::1" && authority != "[::1]" && authority !~ /^127\./) next
  if (seen[port "/" $2]++) next
  printf "%s{\"address\":\"%s\",\"port\":%d,\"pid\":%d,\"process\":\"%s\"}", \
    (emitted++ ? "," : ""), jsonsafe(authority), port, $2 + 0, jsonsafe($1)
}
'); then
    listeners_state=read
  else
    listeners_json=""
  fi
elif [ "$listener_source" = netstat ]; then
  if listeners_json=$(printf '%s\n' "$raw" | /usr/bin/awk '
$6 != "LISTEN" { next }
{
  address = $4
  parts_count = split(address, parts, ".")
  port = parts[parts_count]
  if (port !~ /^[0-9]+$/) next
  authority = substr(address, 1, length(address) - length(port) - 1)
  if (authority != "*" && authority != "::1" && authority !~ /^127\./) next
  if (seen[port]++) next
  pid = 0
  for (field = 7; field <= NF; field++) {
    if ($field ~ /:[0-9]+$/) {
      pid = substr($field, index($field, ":") + 1)
      break
    }
  }
  printf "%s{\"address\":\"%s\",\"port\":%d,\"pid\":%d,\"process\":\"\"}", \
    (emitted++ ? "," : ""), authority, port + 0, pid + 0
}
'); then
    listeners_state=read_without_names
  else
    listeners_json=""
  fi
fi

report "$(printf '%s' "$env_path" | /usr/bin/awk '{ gsub(/[^ -~]/, "?"); gsub(/["\\]/, "?"); printf "%s", $0 }')" \
  read '' "$mode" "$owner_only" "$bytes" "$entries_state" "$entries_fragment" \
  "$listeners_state" "$listeners_json"
"##;
