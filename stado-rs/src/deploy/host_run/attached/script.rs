//! The two host scripts of an attached run: the wrapper that runs the program
//! and the one that forwards a signal to it.
//!
//! The program's output reaches the SSH session through two FIFOs the wrapper
//! relays, never through the session's own descriptors. A program that starts
//! a service (`release local install` restarting `stado serve`) left that
//! service holding the session's stdout, so the session stayed open after the
//! program exited and the attach waited until its caller killed it, ending
//! with `attached process signal marker is unavailable`. Now a descendant can
//! hold only a FIFO: once the program exits, the wrapper reads what is left,
//! names each process still holding the FIFOs ([`super::holders`]), stops
//! relaying and exits with the program's status, and the session closes with
//! it.
//!
//! The program's status reaches this machine on its own line, written the
//! moment the program ended ([`super::channel::PROGRAM_EXITED`]), and the
//! wrapper's own status on another as it exits
//! ([`super::channel::WRAPPER_EXITING`]). Before them the status travelled
//! only as the channel's, after the relays and the holder read: an ssh that
//! failed (255) or a channel cut in that time read the same as a program
//! that refused, and the upgrade called a validation that had printed
//! `config ok` the incoming Stado refusing the host's configuration.

use crate::deploy::shlex_quote;

use super::super::{confined_file_prelude, SIGNAL_AREA};
use super::channel::{PROGRAM_EXITED, WRAPPER_EXITING};

/// The wrapper: confine the program, publish its pid to the signal marker,
/// relay its output, say its status, and exit with it once it has exited.
pub(super) fn attached(program: &str, arguments: &[String], token: &str) -> String {
    let mut script = String::from("set -uo pipefail\numask 077\n");
    script.push_str(&confined_file_prelude(program, "-x", "program"));
    script.push_str(&format!(
        r#"signal_root="$declared_home/{SIGNAL_AREA}"
for component in "$declared_home/.stado" "$declared_home/.stado/work" "$signal_root"; do
  if [ -L "$component" ]; then printf '%s\n' "signal marker ancestor is a symlink: $component" >&2; exit 64; fi
  if [ ! -e "$component" ]; then /bin/mkdir "$component" || exit 73; fi
  if [ ! -d "$component" ] || [ ! -O "$component" ]; then printf '%s\n' "signal marker ancestor is not an owned directory: $component" >&2; exit 64; fi
done
/bin/chmod 700 "$signal_root" || exit 73
marker="$signal_root/{token}"
temporary="$marker.$$"
out_fifo="$marker.out"
err_fifo="$marker.err"
holders_file="$marker.holders"
child=''
out_relay=''
err_relay=''
cleanup() {{
  wrapper_status=$?
  /bin/rm -f -- "$temporary" "$marker" "$out_fifo" "$err_fifo" "$holders_file"
  printf '%s\n' "{WRAPPER_EXITING} $wrapper_status" >&2
}}
forward() {{ if [ -n "$child" ]; then /bin/kill "-$1" "$child" 2>/dev/null || :; fi; }}
trap cleanup EXIT
trap 'forward HUP' HUP
trap 'forward INT' INT
trap 'forward TERM' TERM
/usr/bin/mkfifo -m 600 "$out_fifo" "$err_fifo" || {{ printf '%s\n' "the output relays $out_fifo and $err_fifo could not be made" >&2; exit 73; }}
/bin/cat "$out_fifo" &
out_relay=$!
/bin/cat "$err_fifo" >&2 &
err_relay=$!
"#
    ));
    script.push_str("\"$path\"");
    for argument in arguments {
        script.push(' ');
        script.push_str(&shlex_quote(argument));
    }
    script.push_str(&format!(
        r#" <&0 >"$out_fifo" 2>"$err_fifo" &
child=$!
printf '%s\n' "$child" > "$temporary" || exit 73
/bin/chmod 600 "$temporary" || exit 73
/bin/mv -f -- "$temporary" "$marker" || exit 73
status=0
while :; do
  wait "$child"
  status=$?
  if ! /bin/kill -0 "$child" 2>/dev/null; then break; fi
done
printf '%s\n' "{PROGRAM_EXITED} $status" >&2
"#
    ));
    script.push_str(&super::holders::script(token));
    script.push_str(
        r#"if [ -n "$holders" ]; then
  printf '%s\n' "stado host run attach: the attached program exited with status $status; $holders still hold its output, which is no longer relayed" >&2
  /bin/kill "$out_relay" "$err_relay" 2>/dev/null || :
elif [ -n "${holders_unread:-}" ]; then
  printf '%s\n' "stado host run attach: the attached program exited with status $status; whether a process it started still holds its output is not known ($holders_unread)" >&2
  /bin/kill "$out_relay" "$err_relay" 2>/dev/null || :
fi
wait "$out_relay" "$err_relay" 2>/dev/null
exit "$status"
"#,
    );
    script
}

/// Forward `signal` to the attached program through its marker; a marker that
/// is gone means the program already ended, which is said with its path.
pub(super) fn signal(token: &str, signal: &str) -> String {
    format!(
        r#"set -eu
signal_root="$HOME/{SIGNAL_AREA}"
marker="$signal_root/{token}"
for component in "$HOME/.stado" "$HOME/.stado/work" "$signal_root"; do
  [ ! -L "$component" ] || {{ printf '%s\n' "signal marker ancestor is a symlink: $component" >&2; exit 64; }}
  [ -d "$component" ] && [ -O "$component" ] || {{ printf '%s\n' "signal marker ancestor is not an owned directory: $component" >&2; exit 64; }}
done
[ ! -L "$marker" ] && [ -f "$marker" ] && [ -O "$marker" ] || {{ printf '%s\n' "attached process signal marker $marker is unavailable: the wrapper removes it when the attached program has ended, so the program is no longer running" >&2; exit 66; }}
IFS= read -r pid < "$marker"
case "$pid" in ''|*[!0-9]*) printf '%s\n' "attached process signal marker $marker has an invalid pid" >&2; exit 65 ;; esac
/bin/kill -0 "$pid" 2>/dev/null || {{ printf '%s\n' 'attached process has already exited' >&2; exit 66; }}
/bin/kill -{signal} "$pid"
"#
    )
}
