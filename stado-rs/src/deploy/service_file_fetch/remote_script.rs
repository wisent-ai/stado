//! The remote program and the one substitution that binds a request into it.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

/// The remote program.
///
/// One `stat`, one hash, one base64. Nothing here parses or classifies the
/// content: this command's whole contract is that the bytes arrive unaltered,
/// and a script that looked at them would be a place for that to stop being
/// true.
const REMOTE_FETCH_BODY: &str = r##"set -eu
LC_ALL=C
export LC_ALL

home=$HOME
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
fetch_path=$(printf '%s' '@FETCH_PATH_B64@' | /usr/bin/base64 "$decode")

# Every field below is either a compile-time constant of this script, a
# digit string, a mode string this script itself validated, or base64 — so the
# payload can never carry host text that breaks it. The path is the one field
# that could, which is why it travels back base64 too and is decoded here; the
# entry names of a listed directory travel the same way, set in `entries`
# before a report that carries them.
entries=''
report() {
  printf '{"path":"%s","file_state":"%s","detail":"%s","mode":"%s","owner_only":%s,"bytes":%s,"digest":"%s","content_b64":"%s","entries_b64":"%s"}\n' \
    "$1" "$2" "$3" "$4" "$5" "$6" "$7" "$8" "$entries"
}

# A refusal is a complete report with an explicit state, not an error exit:
# the caller has to be able to tell "this path is a symlink" from "the channel
# broke".
refuse() {
  report '' "$1" "$2" unknown false 0 '' ''
  exit 0
}

# The names in the directory `listed` names, one per line, a directory's
# ending in `/`, base64 on one line. Names only: nothing is opened.
listing() {
  (cd "$listed" && /bin/ls -Ap) | /usr/bin/base64 | /usr/bin/tr -d '\n'
}

# Whether the directory `listed` names resolves inside the target home.
inside_home() {
  real=$(cd "$listed" && pwd -P) || return
  case "$real/" in "$home_real"/*) true ;; *) false ;; esac
}

# The $HOME-confinement prelude of `service_env_file.rs`, word for word. The
# command that COPIES a managed file must accept exactly the paths the commands
# that READ and WRITE one accept; a copier with a wider rule would be a
# file-read primitive wearing a service verb's name.
case "$fetch_path" in
  '$HOME'/*) fetch_path="$home/${fetch_path#\$HOME/}" ;;
  "$home"/*) ;;
  /*) refuse refused_outside_home 'the target must be inside the target home' ;;
  *) fetch_path="$home/$fetch_path" ;;
esac
case "$fetch_path" in "$home"/*) ;; *) refuse refused_outside_home 'the target must be inside the target home' ;; esac
# -L before -f, never the other way round: -f follows the link, so a symlink
# would be reported as a present file and its target copied instead.
if [ -L "$fetch_path" ]; then
  refuse refused_symlink 'the target is a symlink and was not followed'
fi
# A directory, or a path with no file whose directory exists inside the home,
# is answered with the names that are there, so a caller who guessed a file's
# name learns the real one from the same read instead of guessing again.
home_real=$(cd "$home" && pwd -P) || refuse refused_outside_home 'the target home cannot be resolved'
if [ -d "$fetch_path" ]; then
  listed=$fetch_path
  if inside_home; then
    entries=$(listing)
    refuse directory 'the target is a directory; its entries are listed'
  fi
  refuse refused_outside_home 'the resolved target leaves the target home'
fi
if [ ! -f "$fetch_path" ]; then
  listed=$(/usr/bin/dirname "$fetch_path")
  if [ -d "$listed" ] && [ ! -L "$listed" ] && inside_home; then
    entries=$(listing)
    refuse missing 'no regular file at the target; the entries of its directory are listed'
  fi
  refuse missing 'no regular file at the target'
fi
parent=$(/usr/bin/dirname "$fetch_path")
# `cd … && pwd -P` resolves every symbolic link in a directory path, so the
# comparison is between physical directories.
real_parent=$(cd "$parent" 2>/dev/null && pwd -P) || refuse missing 'the target directory cannot be resolved'
real_home=$(cd "$home" 2>/dev/null && pwd -P) || refuse refused_outside_home 'the target home cannot be resolved'
case "$real_parent/" in
  "$real_home"/*) ;;
  *) refuse refused_outside_home 'the resolved target leaves the target home' ;;
esac
if [ ! -r "$fetch_path" ]; then
  refuse unreadable 'the login user cannot read the target'
fi

path_b64=$(printf '%s' "$fetch_path" | /usr/bin/base64 | /usr/bin/tr -d '\n')

# Both stat(1) dialects, BSD first, exactly as `service_env_file.rs` probes
# them. Neither opens the file.
if facts=$(/usr/bin/stat -f '%z %Lp' "$fetch_path" 2>/dev/null); then
  :
elif facts=$(/usr/bin/stat -c '%s %a' "$fetch_path" 2>/dev/null); then
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

# The digest is the host's own, over the file itself, before any encoding.
# `shasum` where macOS keeps it, `sha256sum` where Linux keeps it, and NEVER a
# fabricated or skipped digest: the whole guarantee of this command is that two
# independently computed hashes of the same bytes agree.
digest=''
if [ -x /usr/bin/shasum ]; then
  digest=$(/usr/bin/shasum -a 256 "$fetch_path" | /usr/bin/awk '{print $1}')
elif command -v sha256sum >/dev/null 2>&1; then
  digest=$(sha256sum "$fetch_path" | /usr/bin/awk '{print $1}')
fi
case "$digest" in
  [0-9a-f]*) ;;
  *) digest='' ;;
esac
if [ -z "$digest" ]; then
  report "$path_b64" no_hasher 'the host has neither shasum nor sha256sum, so no digest could be computed' \
    "$mode" "$owner_only" "$bytes" '' ''
  exit 0
fi

# One line, always: `base64` wraps at 76 columns on some hosts and not others,
# and the report is parsed as a single JSON line.
if ! content=$(/usr/bin/base64 < "$fetch_path" | /usr/bin/tr -d '\n'); then
  report "$path_b64" encode_failed 'the file was readable and its bytes could not be encoded' \
    "$mode" "$owner_only" "$bytes" "$digest" ''
  exit 0
fi

report "$path_b64" read '' "$mode" "$owner_only" "$bytes" "$digest" "$content"
"##;

/// The remote program for one file, with this request's path bound in.
///
/// The path travels base64-encoded inside the script's own body, never in an
/// argument vector, for the same reason `env set` encodes its value: the
/// script text is the only thing that reaches the host.
pub fn remote_fetch_script(fetch_path: &str) -> String {
    REMOTE_FETCH_BODY.replace("@FETCH_PATH_B64@", &STANDARD.encode(fetch_path.as_bytes()))
}
