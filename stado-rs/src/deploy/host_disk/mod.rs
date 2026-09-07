//! The host reader behind `stado space report TARGET`: current filesystem and
//! memory usage beside the registry cleanup policy and janitor state.
//!
//! NO Python original: item four of `stado.wisent.com/docs/missing-commands`. Shape and
//! rules come from [`crate::deploy::host_reboot`] via
//! [`crate::deploy::host_channel`].
//!
//! Three parts, deliberately reported together. "97% full" on its own does
//! not tell an operator whether anything is going to be done about it, and
//! "the janitor last ran at 04:12" on its own does not say whether it
//! helped. The July incident was precisely the pair coming apart: a box at
//! zero free bytes whose cleanup policy looked fine in the registry.
//!
//! No part invents a schema.
//!
//! - Usage comes from `df -Pk /` — the POSIX output format, so the columns
//!   are the same on macOS and Linux, unlike the default macOS layout,
//!   which inserts three inode columns before the mount point.
//! - Policy comes from the registry's own
//!   [`crate::targets::DiskCleanupPolicy`], serialized as it stands.
//! - State comes from the janitor's own state file, named by
//!   [`crate::providers::local::disk_cleanup::state_relative_path`] and
//!   written by that module's `write_state`. The `last pass`, `freed
//!   bytes` and `next scheduled pass` this command reports are all derived
//!   from that document; nothing here re-implements the janitor's
//!   bookkeeping.
//! - Local APFS snapshots come from `tmutil listlocalsnapshots /`, and they
//!   are here because NOTHING in this product can reclaim them and their
//!   blocks are already inside the `used` figure above. On
//!   `control-host` on 2026-08-18 the janitor's cleaners and the declared
//!   space-reclamation filesystem stages between them accounted for every
//!   consumer an operator could act on, and three OS-update snapshots sat
//!   outside all of it — the kind of thing that holds tens of GiB and turns
//!   "the product says the disk is accounted for" into a false statement.
//!   Reported, never touched. macOS publishes no size for a snapshot:
//!   `tmutil`, `diskutil apfs listSnapshots` and `diskutil info` all name
//!   them and none of them measures them (checked on macOS 26.5 on both this
//!   control plane's host and the mini), so the count and the host's own
//!   names are reported and no byte figure is invented from them.
//!
//! Like [`crate::deploy::host_recovery`]'s script, the remote program is
//! written as an escaped string: `\\t` / `\\n` are the literal backslash
//! sequences the remote `printf` expands.

use chrono::{DateTime, TimeDelta};
use serde_json::{json, Map, Value};

use super::host_channel;
use super::{shlex_quote, DeployError, Runner};
use crate::providers::local::disk_cleanup;
use crate::targets::ComputeTarget;

mod memory;
mod reading;
mod report;

pub use memory::*;
pub use reading::*;
pub use report::*;

/// `status` for a clean read.
pub const OK_STATUS: &str = "ok";

/// Substitution point for the janitor's state path in [`REMOTE_SCRIPT`].
/// The value is a crate constant, never registry or operator data, and it
/// is shell-quoted before it is spliced.
const STATE_PATH_MARK: &str = "@STATE_PATH@";

/// Substitution point for the janitor's lock path in [`REMOTE_SCRIPT`], on
/// the same terms: a crate constant, shell-quoted before it is spliced.
const LOCK_PATH_MARK: &str = "@LOCK_PATH@";

/// Substitution point for the memory pass's own state path, on the same
/// terms as [`STATE_PATH_MARK`]: a crate constant, shell-quoted before it is
/// spliced. `targets[].memory_reclaim` has a janitor state file exactly as
/// `targets[].disk_cleanup` does, and this report reads both.
const MEMORY_STATE_PATH_MARK: &str = "@MEMORY_STATE_PATH@";

/// What the caller of [`remote_script_for`] is going to read.
///
/// The script is the definition of every measurement here, so a caller that
/// consumes fewer fields must not get a second implementation of the ones it
/// shares: each section below is one constant, and every scope splices the
/// same constants. Two scopes therefore cannot drift on a field they both
/// keep, because there is only ever one text producing it.
///
/// This exists because the cost is not evenly spread. [`INVENTORY_SECTION`]
/// walks the managed home and selected system roots deeply enough to attribute
/// disk pressure; the depth caps the OUTPUT, never the traversal, so it walks the whole selected
/// tree. Measured on `lukasz-macbook` on 2026-09-02: the three fields
/// `host gates` reads take 0.8s together, while the full script had not
/// finished after 180s and burned `user 7m27s` of CPU, so
/// `stado host gates lukasz-macbook` died on the two-minute
/// [`host_channel::remote_timeout`] having computed nothing an operator
/// could read — `disk_cleanup_stalled` and `cleanup_success_age_seconds`
/// were unobtainable on the machine the command was running on. The work
/// was performed for a consumer that does not exist: `host gates` never
/// reads `inventory`, `clone_summaries` or `lock_holders`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskScope {
    /// Every field. `space report`'s [`to_report`] reads all eight, so its
    /// script's cost is the cost of what it prints.
    Full,
    /// `usage`, `state` and `snapshots` only: exactly the three fields
    /// `deploy::host_gates::assemble` reads. The omitted sections are
    /// independent commands, so the kept fields are produced by the same
    /// text, in the same order, as under [`DiskScope::Full`].
    GateInputs,
}

/// `df` — the `usage` field. Read by both scopes.
const DISK_USAGE_SECTION: &str = r#"/bin/df -Pk / 2>/dev/null | while IFS= read -r row; do
  set -- $row
  case "${1:-}" in
    Filesystem|"") continue ;;
  esac
  printf 'STADO_DISK\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "${1:-}" "${2:-}" "${3:-}" "${4:-}" "${5:-}" "${6:-}"
done
"#;

/// The janitor's state file — the `state` field, which carries
/// `last_success_at` and `low_bytes`. Read by both scopes: it is where
/// `cleanup_success_age_seconds` and `disk_cleanup_stalled` come from.
const CLEANUP_STATE_SECTION: &str = r#"state="$HOME/@STATE_PATH@"
if [ -r "$state" ]; then
  printf 'STADO_CLEANUP_STATE\t%s\n' "$(/usr/bin/tr -d '\t\r\n' < "$state")"
else
  printf 'STADO_CLEANUP_STATE_MISSING\t%s\n' "$state"
fi
"#;

/// `lsof` on the run lock — `lock_holders`, `lock_read`, `lock_path`. Read
/// only by `space report`; `host gates` never looks at them.
const CLEANUP_LOCK_SECTION: &str = r#"lock="$HOME/@LOCK_PATH@"
# Who holds the janitor's run lock. `lock_busy` in a cleanup report and
# `cleanup_in_progress` in an agent's capacity broadcast are the same fact
# seen from two sides, and neither one names the holder -- so a host can
# report both for hours, scan nothing, and refuse to admit work, with no
# command able to say which process to look at. On charless-mac-mini that
# cost most of a day. `lsof` is the only reader that answers it; the path is
# fixed by the product, never supplied by an operator.
if [ -e "$lock" ] && [ -x /usr/sbin/lsof ]; then
  /usr/sbin/lsof -Fpc -- "$lock" 2>/dev/null | {
    holder_pid=''
    while IFS= read -r field; do
      case "$field" in
        p*) holder_pid=${field#p} ;;
        c*)
          if [ -n "$holder_pid" ]; then
            printf 'STADO_CLEANUP_LOCK\t%s\t%s\n' "$holder_pid" "${field#c}"
            holder_pid=''
          fi
          ;;
      esac
    done
  }
  printf 'STADO_CLEANUP_LOCK_END\t%s\n' "$lock"
fi
"#;

/// `tmutil` — the `snapshots` field. Read by both scopes:
/// `local_snapshots_unreclaimable` counts them.
const SNAPSHOT_SECTION: &str = r#"if [ -x /usr/bin/tmutil ]; then
  /usr/bin/tmutil listlocalsnapshots / 2>/dev/null | while IFS= read -r row; do
    case "$row" in
      com.apple.*) printf 'STADO_SNAPSHOT\t%s\n' "$row" ;;
    esac
  done
  printf 'STADO_SNAPSHOT_END\t%s\n' 'listed'
fi
"#;

/// The `du` inventory and the Chromium clone census — `inventory` and
/// `clone_summaries`. Read only by `space report`, and the whole cost
/// of this script: the depth caps the output, not the traversal. macOS needs
/// its clone and application-state roots; Linux needs the managed home plus
/// `/home`, `/mnt`, `/var`, and `/opt` because a depth-two root report only
/// named `/root/.stado` while leaving the directory consuming the disk hidden.
const INVENTORY_SECTION: &str = r#"if [ "$(/usr/bin/uname 2>/dev/null || /bin/uname)" = "Darwin" ]; then
  for spec in "$HOME:2" "/private/var:2" "/private/var/folders:5" "$HOME/.local/share:4" "$HOME/.local/state:4" "$HOME/Library/Caches:3" "$HOME/.cargo/git:3" "$HOME/.stado/local-storage:4" "$HOME/.stado/local-backup:4"; do
    root=${spec%:*}
    depth=${spec##*:}
    [ -d "$root" ] || continue
    /usr/bin/du -xk -d "$depth" "$root" 2>/dev/null |
      /usr/bin/sort -nr |
      /usr/bin/head -n 40 |
      while IFS='	' read -r blocks path; do
        [ -n "$blocks" ] && [ -n "$path" ] || continue
        printf 'STADO_DISK_ITEM\t%s\t%s\n' "$blocks" "$path"
      done
  done
  for clone_root in /private/var/folders/*/*/X/org.chromium.Chromium.code_sign_clone; do
    [ -d "$clone_root" ] || continue
    total=$(/usr/bin/find "$clone_root" -maxdepth 1 -type d -name 'code_sign_clone.*' 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
    day_old=$(/usr/bin/find "$clone_root" -maxdepth 1 -type d -name 'code_sign_clone.*' -mtime +0 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
    hour_old=$(/usr/bin/find "$clone_root" -maxdepth 1 -type d -name 'code_sign_clone.*' -mmin +60 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
    printf 'STADO_CLONE_SUMMARY\t%s\t%s\t%s\t%s\n' "$clone_root" "$total" "$hour_old" "$day_old"
  done
else
  for spec in "$HOME:3" "/home:3" "/mnt:3" "/var:3" "/opt:3"; do
    root=${spec%:*}
    depth=${spec##*:}
    [ -d "$root" ] || continue
    /usr/bin/du -xk -d "$depth" "$root" 2>/dev/null |
      /usr/bin/sort -nr |
      /usr/bin/head -n 40 |
      while IFS='	' read -r blocks path; do
        [ -n "$blocks" ] && [ -n "$path" ] || continue
        printf 'STADO_DISK_ITEM\t%s\t%s\n' "$blocks" "$path"
      done
  done
fi
"#;

/// The remote program for a caller that reads every field.
///
/// Retained as the name the existing callers use, and defined in terms of
/// [`remote_script_for`] so there is one builder and one set of sections.
pub fn remote_script() -> String {
    remote_script_for(DiskScope::Full)
}

/// The remote program carrying exactly the sections SCOPE consumes, with the
/// janitor's state and lock paths in place.
///
/// Assembled from the section constants rather than from per-scope text: a
/// field two scopes share is produced by the same bytes in both, so the
/// cheap script cannot report a different `usage`, `state` or `snapshots`
/// than the full one. The sections are independent commands reading
/// independent sources, so dropping one cannot alter another's output.
pub fn remote_script_for(scope: DiskScope) -> String {
    let mut script = String::from("set -u\n");
    script.push_str(DISK_USAGE_SECTION);
    script.push_str(MEMORY_SECTION);
    script.push_str(MEMORY_STATE_SECTION);
    script.push_str(CLEANUP_STATE_SECTION);
    if scope == DiskScope::Full {
        script.push_str(CLEANUP_LOCK_SECTION);
    }
    script.push_str(SNAPSHOT_SECTION);
    if scope == DiskScope::Full {
        script.push_str(INVENTORY_SECTION);
    }
    script
        .replace(
            STATE_PATH_MARK,
            &shlex_quote(&disk_cleanup::state_relative_path()),
        )
        .replace(
            LOCK_PATH_MARK,
            &shlex_quote(&disk_cleanup::lock_relative_path()),
        )
        .replace(
            MEMORY_STATE_PATH_MARK,
            &shlex_quote(&crate::providers::local::host_memory::state::state_relative_path()),
        )
}
