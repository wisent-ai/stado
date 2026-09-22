//! The remote program this module sends to a host: one section per
//! measurement, and one builder that splices exactly the sections a caller
//! reads.
//!
//! Split out of `host_disk/mod.rs`, which had grown past the module line cap;
//! the scope a caller asks for, and the marks spliced into the text, stay
//! there.

use super::*;

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

/// Every device-backed filesystem and, on Linux, every block device the
/// kernel sees — the `volumes` and `block_devices` fields. Read by both
/// scopes. `df -Pk /` above measures the volume the fleet writes to; this
/// section answers where the rest of the host's storage is. On 2026-09-18 a
/// multi-terabyte disk was attached to ubuntu-server-rtx-pro-6000, `df`
/// showed nothing of it because nothing had mounted it, and the only
/// reading the product offered said the host had 29 GiB free. A disk with
/// no mountpoint is reported as attached and unmounted, never left out.
const VOLUMES_SECTION: &str = r#"/bin/df -Pk 2>/dev/null | while IFS= read -r row; do
  set -- $row
  case "${1:-}" in
    /dev/*) printf 'STADO_VOLUME\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "${1:-}" "${2:-}" "${3:-}" "${4:-}" "${5:-}" "${6:-}" ;;
  esac
done
if [ -x /usr/bin/lsblk ]; then
  /usr/bin/lsblk -b -P -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINT,UUID,MODEL 2>/dev/null | while IFS= read -r row; do
    printf 'STADO_BLOCK_DEVICE\t%s\n' "$row"
  done
  printf 'STADO_BLOCK_DEVICES_END\t%s\n' 'listed'
fi
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
  clone_temp=$(/usr/bin/getconf DARWIN_USER_TEMP_DIR 2>/dev/null)
  if [ -n "$clone_temp" ]; then
    clone_container=$(CDPATH= cd "$clone_temp/.." 2>/dev/null && /bin/pwd -P)
    if [ -n "$clone_container" ]; then
      printf 'STADO_CLONE_ROOT\t%s/__CLONE_CONTAINER__/__CLONE_ROOT__\n' "$clone_container"
    fi
  fi
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

/// Every directory a build tool tagged as regenerable, wherever it is, with
/// its size — the `build_caches` census.
///
/// The inventory above walks `$HOME` at depth two, which is the whole reason
/// 843 GB of build output went unwatched on `lukasz-macbook` on 2026-09-19:
/// `~/Documents/CodingProjects/Wisent` is one depth-two row, the per-repository
/// `target/` trees under it are four and five deep, and the coverage report
/// can only reason about paths the inventory named. The host declared the
/// `build_caches` cleaner all along; the cleaner's root reached none of it and
/// nothing said so, because nothing had measured it.
///
/// The marker is the same one the cleaner itself judges by: a `CACHEDIR.TAG`
/// written by the tool that produced the bytes. No directory-name matching and
/// no extension list. The walk is bounded at six levels below the home
/// directory, and the whole section shares the inventory's own budget.
/// The trailing marker is not decoration. Without it an empty census and a
/// census that never ran read identically, and a row that reports "this host
/// holds no build output" when nobody looked is the failure this whole change
/// exists to end. The section is emitted before the depth-bounded inventory
/// for the same reason: it is the targeted read, and a run cut short keeps it.
const BUILD_CACHE_SECTION: &str = r#"/usr/bin/find "$HOME" -maxdepth 6 -type f -name CACHEDIR.TAG 2>/dev/null |
  while IFS= read -r tag; do
    dir=${tag%/CACHEDIR.TAG}
    blocks=$(/usr/bin/du -sxk "$dir" 2>/dev/null | /usr/bin/cut -f1)
    [ -n "$blocks" ] || continue
    printf 'STADO_BUILD_CACHE_ITEM\t%s\t%s\n' "$blocks" "$dir"
  done
printf 'STADO_BUILD_CACHE_END\t%s\n' 'listed'
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
    if scope == DiskScope::UsageOnly {
        script.push_str(DISK_USAGE_SECTION);
        script.push_str(VOLUMES_SECTION);
        return script;
    }
    if scope != DiskScope::StateOnly {
        script.push_str(DISK_USAGE_SECTION);
        script.push_str(VOLUMES_SECTION);
    }
    // Memory belongs to the state read, not to the filesystem measurement:
    // `host gates` splits its host reads into `UsageOnly` for free space and
    // `StateOnly` for the janitor, and memory was in neither, so a verdict
    // that carries a memory gate had nothing to fill it with unless the host's
    // agent was publishing. `vm_stat`, `sysctl` and one small state file are
    // what this adds to a read that is already running the janitor's.
    script.push_str(MEMORY_SECTION);
    script.push_str(MEMORY_STATE_SECTION);
    script.push_str(CLEANUP_STATE_SECTION);
    if scope == DiskScope::Full {
        script.push_str(CLEANUP_LOCK_SECTION);
    }
    script.push_str(SNAPSHOT_SECTION);
    if scope == DiskScope::Full {
        script.push_str(BUILD_CACHE_SECTION);
        script.push_str(INVENTORY_SECTION);
    }
    script
        .replace(
            "__CLONE_CONTAINER__",
            disk_cleanup::chromium_clones::CLONE_CONTAINER,
        )
        .replace(
            "__CLONE_ROOT__",
            disk_cleanup::chromium_clones::CLONE_ROOT_NAME,
        )
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
