//! The remote rewrite, and every guard it refuses on.

/// Replace one quarantine map, and leave the evidence that it happened.
///
/// Every guard here exists because the file being rewritten is the one the
/// release agent drives from, and the agent is writing it too, every tick:
///
/// - the live file's digest must still be the digest this command read, or some
///   other writer moved and this write would discard their work;
/// - the previous bytes are copied to a timestamped backup before anything is
///   written, so the state before the change is recoverable without this tool;
/// - the audit trail is proven appendable before the state is touched, because
///   an unaudited mutation is worse than a refused one;
/// - the staged document is hashed *after* it lands on the host's disk and
///   compared against what this command built, so a short or interrupted
///   transfer is discarded instead of renamed over a working rollout;
/// - only then does `mv` publish it, atomically, within one directory.
///
/// The staging file is a copy of the live one truncated in place, so the mode
/// and owner the agent gave its state file survive the rewrite.
pub(super) const CLEAR_TEMPLATE: &str = r#"set -euo pipefail
state=@STATE@
backup=@BACKUP@
staging=@STAGING@
audit=@AUDIT@
expected_live=@EXPECTED_LIVE@
expected_next=@EXPECTED_NEXT@

if [ ! -f "$state" ]; then
  printf '%s\n' "rollout state $state is missing" >&2
  exit 1
fi
if [ -e "$backup" ]; then
  printf '%s\n' "state backup $backup already exists" >&2
  exit 1
fi
if ! : >> "$audit"; then
  printf '%s\n' "cannot append to the quarantine audit trail $audit" >&2
  exit 1
fi
/bin/chmod u=rw,go= "$audit"
line=$(/usr/bin/openssl dgst -sha256 -r "$state")
live=${line%% *}
if [ "$live" != "$expected_live" ]; then
  printf '%s\n' "rollout state changed while this command ran: read $expected_live, found $live" >&2
  exit 1
fi
/bin/cp -p "$state" "$backup"
printf 'STADO_QUARANTINE\tbackup\t%s\n' "$backup"
/bin/rm -f "$staging"
/bin/cp -p "$state" "$staging"
printf '%s' @DOCUMENT@ | /usr/bin/openssl base64 -d -A > "$staging"
line=$(/usr/bin/openssl dgst -sha256 -r "$staging")
staged=${line%% *}
if [ "$staged" != "$expected_next" ]; then
  # The state was never changed, so the backup is a duplicate of the live file.
  # Leaving it behind would make an immediate retry refuse on its own debris.
  /bin/rm -f "$staging" "$backup"
  printf '%s\n' "staged rollout state reads back as $staged, not the $expected_next this command built" >&2
  exit 1
fi
/bin/mv "$staging" "$state"
printf 'STADO_QUARANTINE\tcommitted\t%s\n' "$state"
printf '%s' @RECORD@ | /usr/bin/openssl base64 -d -A >> "$audit"
printf '\n' >> "$audit"
printf 'STADO_QUARANTINE\taudited\t%s\n' "$audit"
"#;
