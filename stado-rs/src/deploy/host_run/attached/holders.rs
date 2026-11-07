//! The read, once the attached program has exited, of which processes still
//! hold the FIFOs its output is relayed through.
//!
//! On Linux each process's open files are links under `/proc/<pid>/fd`, and a
//! link to a relay names its path, so `find` reads only those links and keeps
//! the ones naming this run's relays, by the token in their names (the path a
//! link names is the physical one, not `$HOME`'s spelling of it); no file is
//! opened or stat'ed, so the read is bounded by the links it lists, not by
//! every open file of the host. lsof over the whole host stat'ed every open
//! file of every process: on a host running hundreds of containers with its
//! swap full it ran for more than twenty minutes after the program had
//! exited, the attach said nothing all that time, and it was cut as a hang.
//!
//! macOS has no `/proc`, and its lsof names no holder of a FIFO asked for by
//! its path (`lsof -t FIFO` answers nothing while a process writes to it), so
//! every open file is listed and the relays are matched among the FIFOs by
//! inode. A FIFO's lsof record carries no device field (`D`), so a match on
//! device and inode found no holder at all and the attach still waited for
//! the lingering child. lsof is looked for where each system keeps it, not on
//! a non-login PATH, and its filter runs outside any `$(...)`: macOS
//! `/bin/bash` 3.2 misparses a `case` pattern's `)` inside a command
//! substitution.
//!
//! The read runs to its end: nothing here cuts it short, as nothing in
//! `stado-wait` does. Its start and end are one stderr line each, beginning
//! with [`HOLDERS_READING`] and [`HOLDERS_READ`], which the attach turns into
//! a wait of its own ([`super::channel`]), so a long read is said, with the
//! moment it is due by the reads measured before it, while it runs, and
//! `stado waits` lists it.

/// The first words of the line the wrapper writes on stderr when it starts
/// reading which processes still hold the program's output.
pub(super) const HOLDERS_READING: &str = "stado host run attach: reading which processes hold";
/// The first words of the line it writes when that read ended with its
/// answer.
pub(super) const HOLDERS_READ: &str = "stado host run attach: read which processes hold";

/// What the wait on the read is, on `target`: one operation per target, so
/// each host's read is measured by its own.
pub(super) fn wait(target: &str) -> String {
    format!("which processes on {target} still hold an attached program's output")
}

/// The wrapper's part that sets `holders` (each pid still holding the relays
/// of run `token`, with its command) or `holders_unread` (why they are not
/// known). It runs after the program exited and expects the wrapper's
/// `out_fifo`, `err_fifo`, `out_relay`, `err_relay` and `holders_file`.
pub(super) fn script(token: &str) -> String {
    format!(
        r#"holders=''
holders_unread=''
holders_lister=''
system=$(/usr/bin/uname -s)
if [ "$system" = Linux ]; then
  if [ -d /proc/self/fd ]; then
    holders_lister='the /proc/<pid>/fd links naming its relays'
  else
    holders_unread='this Linux host has no /proc to read them from; mount proc at /proc to have them named'
  fi
else
  lister=''
  for candidate in /usr/sbin/lsof /usr/bin/lsof; do
    if [ -x "$candidate" ]; then lister=$candidate; break; fi
  done
  if [ -n "$lister" ] && inodes=$(/usr/bin/stat -f '%i' "$out_fifo" "$err_fifo" 2>/dev/null || /usr/bin/stat -c '%i' "$out_fifo" "$err_fifo" 2>/dev/null); then
    wanted=" $(printf '%s' "$inodes" | /usr/bin/tr '\n' ' ') "
    holders_lister="$lister over every open file on this host"
  else
    holders_unread='no lsof at /usr/sbin/lsof or /usr/bin/lsof names them on this host; install lsof to have them named'
  fi
fi
read_holders() {{
  if [ "$system" = Linux ]; then
    /usr/bin/find /proc/[0-9]*/fd -type l \( -lname '*/{token}.out' -o -lname '*/{token}.err' \) -printf '%h\n' |
      /usr/bin/sed -e 's|^/proc/||' -e 's|/fd$||' | /usr/bin/sort -u
  else
    "$lister" -nP -F pfti | /usr/bin/awk -v wanted="$wanted" '/^p/ {{ pid = substr($0, 2) }} /^f/ {{ type = "" }} /^t/ {{ type = substr($0, 2) }} /^i/ {{ if (type == "FIFO" && index(wanted, " " substr($0, 2) " ")) print pid }}' | /usr/bin/sort -u
  fi
}}
if [ -n "$holders_lister" ]; then
  printf '%s\n' "{HOLDERS_READING} its output with $holders_lister" >&2
  # stderr: find's and lsof's words about processes that ended or files they
  # cannot read while they list; the listing is the answer.
  read_holders </dev/null >"$holders_file" 2>/dev/null
  printf '%s\n' "{HOLDERS_READ} its output" >&2
  for pid in $(/bin/cat "$holders_file"); do
    if [ "$pid" = "$out_relay" ] || [ "$pid" = "$err_relay" ] || [ "$pid" = "$$" ]; then continue; fi
    holders="$holders$pid ($(/bin/ps -o comm= -p "$pid" 2>/dev/null)) "
  done
fi
"#
    )
}
