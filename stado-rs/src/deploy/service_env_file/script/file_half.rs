//! The file half of the remote program: confine the path to the target home,
//! refuse everything it must not open, stat it without opening it, and run the
//! one `awk` pass that parses, classifies and redacts every assignment on the
//! host.
//!
//! This is the first of two pieces of one script, not a program of its own.
//! `REMOTE_ENV_FILE_HEAD` and `REMOTE_ENV_FILE_TAIL` are concatenated in that
//! order, at the blank line that already separated the file section from the
//! socket-table section, so the text delivered to a host is the script this
//! module has always sent.

/// Everything up to and including the parse of the file's assignments.
pub(super) const REMOTE_ENV_FILE_HEAD: &str = r##"set -eu
LC_ALL=C
export LC_ALL

home=$HOME
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
env_path=$(printf '%s' '@ENV_PATH_B64@' | /usr/bin/base64 "$decode")
reveal=$(printf '%s' '@REVEAL_B64@' | /usr/bin/base64 "$decode")
# The one key/value pair the caller wants checked. Both arrive base64-encoded
# in this script's own body — never in an argument vector — and the value is
# handed to awk through a command-scoped assignment prefix, so it enters that
# one process's environment and nothing else's. The value is already on this
# host by construction: `env-set` just wrote it into the file, so re-sending it
# to the reader that checks the write adds no exposure. What must never happen
# is the reverse trip, and it does not: the host answers with one word.
expect_key=$(printf '%s' '@EXPECT_KEY_B64@' | /usr/bin/base64 "$decode")
expect_value=$(printf '%s' '@EXPECT_VALUE_B64@' | /usr/bin/base64 "$decode")

# Every field below is either a compile-time constant of this script or has
# been through the awk sanitizer, so `report` never carries host text that
# could break the payload.
report() {
  printf '{"path":"%s","file_state":"%s","detail":"%s","mode":"%s","owner_only":%s,"bytes":%s,"entries_state":"%s",%s,"listeners_state":"%s","listeners":[%s]}\n' \
    "$1" "$2" "$3" "$4" "$5" "$6" "$7" "$8" "$9" "${10}"
}

# A refusal is a complete report with an explicit state, not an error exit:
# the caller has to be able to tell "this path is a symlink" from "the channel
# broke", and an empty entries list has to arrive with the reason beside it.
refuse() {
  report '' "$1" "$2" unknown false 0 unread '"entries":[],"entries_seen":'\
'0,"expected":"unverified"' unread ''
  exit 0
}

# The $HOME-confinement prelude of `service.rs::set_env_key_on_host`, word for
# word. The command that READS a managed env file must accept exactly the paths
# the command that WRITES one accepts; a reader with a wider rule would be a
# file-read primitive wearing an env-file's name.
case "$env_path" in
  '$HOME'/*) env_path="$home/${env_path#\$HOME/}" ;;
  "$home"/*) ;;
  /*) refuse refused_outside_home 'the target must be inside the target home' ;;
  *) env_path="$home/$env_path" ;;
esac
case "$env_path" in "$home"/*) ;; *) refuse refused_outside_home 'the target must be inside the target home' ;; esac
# -L before -f, never the other way round: -f follows the link, so a symlink
# would be reported as a present file and its target read instead.
if [ -L "$env_path" ]; then
  refuse refused_symlink 'the target is a symlink and was not followed'
fi
if [ ! -f "$env_path" ]; then
  refuse missing 'no regular file at the target'
fi
parent=$(/usr/bin/dirname "$env_path")
real_parent=$(/usr/bin/python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$parent")
if ! /usr/bin/python3 -c 'import os,sys; home=os.path.realpath(sys.argv[1]); parent=sys.argv[2]; sys.exit(0 if os.path.commonpath((home,parent)) == home else 1)' "$home" "$real_parent"; then
  refuse refused_outside_home 'the resolved target leaves the target home'
fi
if [ ! -r "$env_path" ]; then
  refuse unreadable 'the login user cannot read the target'
fi

# Both stat(1) dialects, BSD first, exactly as the vault section of
# host_inventory.rs probes them. Neither opens the file.
if facts=$(/usr/bin/stat -f '%z %Lp' "$env_path" 2>/dev/null); then
  :
elif facts=$(/usr/bin/stat -c '%s %a' "$env_path" 2>/dev/null); then
  :
else
  facts=""
fi
bytes=${facts%% *}
mode=${facts#* }
case "$bytes" in
  ''|*[!0-9]*) bytes=0 ;;
esac
if [ "$mode" = "$facts" ]; then
  mode=unknown
fi
case "$mode" in
  0???) mode=${mode#0} ;;
esac
case "$mode" in
  *[!0-7]*) mode=unknown ;;
esac
case "$mode" in
  *00) owner_only=true ;;
  *) owner_only=false ;;
esac

entries_state=read
if ! entries_fragment=$(STADO_EXPECT_KEY="$expect_key" STADO_EXPECT_VALUE="$expect_value" \
    /usr/bin/awk \
    -v reveal="$reveal" \
    -v max_entries=@MAX_ENTRIES@ \
    -v max_chars=@MAX_VALUE_CHARS@ '
# Printable ASCII only, minus the two bytes a JSON string cannot carry raw.
# Refusing them outright rather than escaping them is a guarantee that does
# not depend on getting the escaping right, and a corrupt or hostile file
# cannot emit a quote, a backslash, a newline or a control character into this
# report.
function jsonsafe(text) {
  gsub(/[^ -~]/, "?", text)
  gsub(/["\\]/, "?", text)
  return text
}
function clamp(text) {
  if (length(text) > max_chars) {
    return substr(text, 1, max_chars - 3) "..."
  }
  return text
}
# One layer of matching surrounding quotes, the way a shell would remove it.
# Only for the classification and the length: the reported text keeps its
# quotes, because whether a value is quoted is part of what the operator is
# reading the file to find out.
function unquote(text,   size, head, tail) {
  size = length(text)
  if (size >= 2) {
    head = substr(text, 1, 1)
    tail = substr(text, size, 1)
    if ((head == "\"" && tail == "\"") || (head == "'"'"'" && tail == "'"'"'")) {
      return substr(text, 2, size - 2)
    }
  }
  return text
}
# scheme://userinfo@host — a URL that carries a credential in its authority.
# Redacted whatever the key is called: DATABASE_URL names no secret and holds one.
function has_userinfo(text) {
  return (text ~ /^[A-Za-z][A-Za-z0-9+.-]*:\/\/[^\/@]*@/)
}
# An endpoint, a port, a plain flag, or a reference to another variable. Every
# one of these is a value an operator has to be able to read, and none of them
# can carry a credential: the URL form admits no @, ? or # by construction.
function inert(text) {
  if (text ~ /^\$\{?[A-Za-z_][A-Za-z0-9_]*\}?$/) return 1
  if (text ~ /^[0-9]+$/) return 1
  if (text ~ /^(true|false|yes|no|on|off|TRUE|FALSE|YES|NO|ON|OFF)$/) return 1
  if (text ~ /^[A-Za-z][A-Za-z0-9+.-]*:\/\/[A-Za-z0-9._-]+(:[0-9]+)?(\/[A-Za-z0-9._~\/-]*)?$/) return 1
  if (text ~ /^[A-Za-z0-9._-]+:[0-9]+$/) return 1
  return 0
}
function secretish(name) {
  return (name ~ /TOKEN|SECRET|PASSWORD|PASSWD|PASSPHRASE|CREDENTIAL|KEY|BEARER|PRIVATE|SIGNING|SIGNATURE|SALT|COOKIE|AUTH|SESSION/)
}
BEGIN {
  expect_key = ENVIRON["STADO_EXPECT_KEY"]
  expect_value = ENVIRON["STADO_EXPECT_VALUE"]
  expected = (expect_key == "" ? "not_asked" : "absent")
}
{
  line = $0
  sub(/\r$/, "", line)
  sub(/^[ \t]+/, "", line)
  if (line == "" || line ~ /^#/) next
  seen++
  form = "assignment"
  if (line ~ /^export[ \t]+/) {
    form = "export"
    sub(/^export[ \t]+/, "", line)
  }
  key = ""
  value = ""
  # The empty shown of a value this reader deliberately withholds.
  withheld = ""
  if (match(line, /^[A-Za-z_][A-Za-z0-9_]*=/)) {
    key = substr(line, 1, RLENGTH - 1)
    value = substr(line, RLENGTH + 1)
    sub(/[ \t]+$/, "", value)
  } else {
    form = "unparsable"
  }
  if (form == "unparsable") {
    # Shown, because `. other.env` or `set -a` changes what the whole file
    # means and hiding it is what made this class of fault unreadable. A line
    # that MENTIONS a credential is withheld anyway.
    chars = length(line)
    if (secretish(line)) {
      state = "redacted"
      shown = withheld
    } else {
      state = "shown"
      shown = line
    }
  } else {
    probe = unquote(value)
    chars = length(probe)
    if (probe == "") {
      state = "empty"
      shown = withheld
    } else if (key == reveal) {
      state = "revealed"
      shown = value
    } else if (has_userinfo(probe)) {
      state = "redacted"
      shown = withheld
    } else if (inert(probe)) {
      state = "shown"
      shown = value
    } else if (secretish(key)) {
      state = "redacted"
      shown = withheld
    } else {
      state = "shown"
      shown = value
    }
    # The one question the caller asked, answered against every assignment to
    # that key in turn so the LAST one decides — the assignment a sourced file
    # actually leaves behind. Compared after unquoting, because the writer that
    # overwrote env-set on this fleet wraps its value in shell quotes, and a
    # textual comparison would call an identical endpoint a mismatch.
    #
    # No apostrophe may appear in this program: it is delivered inside a
    # single-quoted shell word, so one would end the word and truncate the
    # program.
    if (expect_key != "" && key == expect_key) {
      expected = (probe == expect_value ? "matched" : "differs")
    }
  }
  # The cap bounds what is REPORTED, and is applied after the classification
  # above so a file past the cap still answers the question about its own key.
  if (seen > max_entries) next
  out = out sep sprintf("{\"line\":%d,\"form\":\"%s\",\"key\":\"%s\",\"value_state\":\"%s\",\"value\":\"%s\",\"chars\":%d}", \
    NR, form, jsonsafe(key), state, clamp(jsonsafe(shown)), chars)
  sep = ","
}
END {
  printf "\"entries\":[%s],\"entries_seen\":%d,\"expected\":\"%s\"", out, seen + 0, expected
}
' "$env_path"); then
  entries_state=parse_failed
  entries_fragment='"entries":[],"entries_seen":'\
'0,"expected":"unverified"'
fi
"##;
