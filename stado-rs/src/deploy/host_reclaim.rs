//! `stado host reclaim HOST [--dry-run|--apply --reason TEXT]` — get the disk
//! back, in declared stages, measuring each one.
//!
//! NO Python original. The incident: the Mac mini's data volume sat at roughly
//! 2 GiB free against a 55 GiB registry policy. Its queue agent publishes
//! `disk_pressure_unresolved` and fails admission closed while that is true, so
//! it claimed nothing for hours and every release build queued behind it. The
//! janitor's declared cleaners did not cover what had actually filled the disk,
//! and there was no command for the rest of it: the space came back by hand,
//! over ssh, from a shell script written during the outage. This is that
//! reclamation as a product command — same stages, same measurements, same
//! refusals, through the registry-authorized channel and with an audit record
//! left on the machine whose disk changed. [`crate::deploy::host_gates`] is the
//! read half that says the space is the reason nothing is being claimed.
//!
//! Four stages, in this order, and nothing else:
//!
//! 1. `registry_cleanup` — the host's OWN janitor
//!    ([`crate::providers::local::disk_cleanup`]), invoked exactly the way
//!    [`crate::deploy::host_cleanup`] invokes it, so the policy stays the one
//!    the registry declares and this module contains no cleanup policy of its
//!    own. `--dry-run` runs its planning phase; `--apply` runs the enforcing
//!    pass. The item count is the janitor's own.
//! 2. `build_scratch` — `$HOME/`[`BUILD_WORK_ROOT`], the release build scratch
//!    tree. `scripts/build-stado-linux-host.sh` works there and does not remove
//!    what it wrote; a from-scratch release build leaves its checkout and its
//!    vendored sources behind every time.
//! 3. `delivered_trees` — the version directories under `$HOME/`[`SERVICES_ROOT`],
//!    where every `service deploy` and every artifact install stages one tree
//!    per version and keeps the previous one beside it as
//!    `current.before-<version>` so a rollback is a rename; plus every
//!    superseded delivery root the product catalog declares
//!    ([`crate::deploy::products::Product::superseded_roots`]). The mini
//!    carries 20 `weles-worker` versions, 9.7 GiB, under
//!    `$HOME/.local/share/weles-worker` from the installer that predates
//!    [`crate::deploy::artifact_install`], while the worker runs from its own
//!    checkout — trees no rollback will reach and, until the catalog declared
//!    that root, trees no command could see. Same rules for both: the roots
//!    come from declarations so the next delivery path change is a data
//!    change, and a product's LIVE install root is never one of them.
//! 4. `chromium_clones` — the per-launch bundle clones under this account's
//!    macOS temporary container, `<container>/`[`CLONE_CONTAINER`]`/`
//!    [`CLONE_ROOT_NAME`]. macOS clones the whole browser bundle every time
//!    Chromium starts so it can validate a signature nobody can swap
//!    underneath it, Weles drives Chromium for browser automation, and a run
//!    that is killed leaves its clone behind. On the mini the day this landed:
//!    137 clones, 130 of them untouched for more than a day. Last of the four
//!    because the first three are space the fleet's own software wrote, and
//!    this is space the operating system wrote — the same order, and the same
//!    reason, as the janitor's cleaner
//!    ([`crate::providers::local::disk_cleanup::chromium_clones`], which is
//!    what removes these when the registry declares that cleaner; this stage
//!    is for the hosts and the moments where it has not).
//!
//! Five rules, encoded here rather than left to whoever is at the keyboard:
//!
//! - **nothing outside those roots.** Every candidate is produced by globbing
//!   one of the three declared roots; no path arrives from the registry, from
//!   the operator, or from the host's own output. The one exception is named
//!   and constrained: the clone container is the OS's own answer for this
//!   account (`$TMPDIR`, `getconf DARWIN_USER_TEMP_DIR` behind it), and the
//!   stage refuses it unless it is under `/var/folders`, which is the only
//!   place macOS puts one.
//! - **one enumeration, not two.** Candidates and the newest-tree guard come
//!   from the SAME glob, and the age gate is asked per candidate. A `find` for
//!   one and a glob for the other differ by exactly the dotted entries, and on
//!   this control plane's own host that difference named
//!   `.macos-capability-backup-20260803` — an operator's state backup sitting
//!   beside the deliveries, which no delivery created and no reclamation may
//!   take. A delivery and a build both produce plainly named directories, so
//!   the glob IS the set.
//! - **never a path a live process holds.** One `ps` snapshot is taken before
//!   any stage and every candidate is checked against it. Taken once, into a
//!   variable, because `ps | grep <path>` matches the grep's own argv and would
//!   report every candidate as held.
//! - **never the newest tree of a product, and never what `current` resolves
//!   to.** Both are kept even when they are the largest thing there, and
//!   nothing younger than [`MIN_AGE_DAYS`] is touched at all, which is what
//!   makes the stage safe against a delivery that is mid-flight: its tree is
//!   the newest one and the youngest one.
//! - **`--dry-run` deletes nothing.** It is the default, and the same script
//!   runs in both modes with the removal itself behind the mode flag, so a
//!   preview walks exactly the paths an apply would remove rather than a
//!   second implementation's guess at them.
//!
//! The remote program is a raw Rust string: the `\t` and `\n` in its `printf`
//! formats are the literal backslash sequences the remote shell expands, and
//! spelling a program this size through escaped quotes is how a marker gets
//! silently mistyped.

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::host_channel;
use super::host_cleanup::cleaner_plans;
use super::host_disk::gib_from_blocks;
use super::host_recovery::WC_CANDIDATES;
use super::products;
use super::{shlex_quote, DeployError, Runner};
use crate::deploy::artifact_install::SERVICES_ROOT;
use crate::providers::local::disk_cleanup::chromium_clones;
use crate::targets::ComputeTarget;

/// The release build scratch tree, relative to the target account's home.
///
/// Its own root under `.stado`, and the one the fleet's checked-in build
/// helper already uses (`scripts/build-stado-linux-host.sh`): a stage that
/// reclaimed a directory that helper does not write would be reclaiming
/// something else.
pub const BUILD_WORK_ROOT: &str = ".stado/build-work";

/// Nothing younger than this is a candidate, in any stage.
///
/// A build in flight and a delivery in flight both keep their own directory
/// fresh, so age is the guard that does not depend on a process being visible
/// to `ps` at the instant the sweep runs.
pub const MIN_AGE_DAYS: &str = "1";
/// Chromium creates more than 100 full-bundle clones in a day on an active
/// Weles host. Process ownership and newest-clone guards make one hour enough
/// to survive launch races without allowing the clone root to fill the disk.
pub const CLONE_MIN_AGE_MINUTES: &str = "60";
/// A reclaim includes the registry janitor (whose declared pass may take up
/// to ten minutes) and removal of large, already-enumerated trees. The generic
/// two-minute host-read bound killed the transport mid-pass and left the
/// remote janitor running without a caller. This explicit operator command is
/// bounded independently at one hour.
const RECLAIM_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// `mode` for a run that measured and removed nothing.
pub const DRY_RUN_MODE: &str = "dry_run";
/// `mode` for a run that removed what its stages named.
pub const APPLY_MODE: &str = "apply";

/// The stage name for the host's own janitor pass.
pub const REGISTRY_CLEANUP_STAGE: &str = "registry_cleanup";
/// The stage name for the release build scratch tree.
pub const BUILD_SCRATCH_STAGE: &str = "build_scratch";
/// The stage name for delivered product trees.
pub const DELIVERED_TREES_STAGE: &str = "delivered_trees";
/// The stage name for the macOS per-launch Chromium bundle clones.
pub const CHROMIUM_CLONES_STAGE: &str = "chromium_clones";
/// The stage name for terminal queue-job workdirs and bootstrap scratch.
pub const QUEUE_WORKDIRS_STAGE: &str = "queue_workdirs";
/// Rebuildable package/browser caches owned by build tooling.
pub const REBUILDABLE_CACHES_STAGE: &str = "rebuildable_caches";
/// The stage name for macOS-style home trees found on a Linux host.
pub const FOREIGN_HOME_TREES_STAGE: &str = "foreign_home_trees";
/// The stage name for eligible local Time Machine APFS snapshots.
pub const LOCAL_APFS_SNAPSHOTS_STAGE: &str = "local_apfs_snapshots";

/// The only prefix a macOS temporary container has, and the guard on the one
/// root this module does not spell itself.
///
/// The container comes from the OS (`$TMPDIR`, `getconf DARWIN_USER_TEMP_DIR`
/// behind it) because nothing else knows where it is — its name carries a
/// per-account hash. A value from outside that is not under this prefix is not
/// a container, and the stage walks nothing rather than trusting it.
pub const CONTAINER_PREFIX: &str = "/var/folders/";

/// Suffix a stage name carries when the host could not run it at all.
///
/// A stage that did not run is reported UNDER ITS OWN NAME plus this suffix,
/// with null measurements, rather than omitted or folded into a zero-item
/// success. "Nobody looked" rendered as "nothing was there" is the fold this
/// fleet has already paid for once.
pub const UNAVAILABLE_SUFFIX: &str = "_unavailable";

/// Where the record of an applied reclamation lands on the host whose disk
/// changed, relative to that account's home.
///
/// On the machine, next to the state it changed, and not in a central ledger:
/// the disk that moved is the host's, the operator who moved it may never touch
/// this control plane again, and a record kept anywhere else is a record that
/// can be missing exactly when someone asks what happened to that box.
pub const AUDIT_LOG: &str = ".stado/audit/host-reclaim.jsonl";

/// Substitution points in [`REMOTE_SCRIPT_TEMPLATE`]. Every value spliced in is
/// a crate constant, never registry or operator data.
const APPLY_MARK: &str = "@APPLY@";
const WC_WORDS_MARK: &str = "@WC_WORDS@";
const SERVICES_ROOT_MARK: &str = "@SERVICES_ROOT@";
const BUILD_WORK_MARK: &str = "@BUILD_WORK@";
const CLONE_AGE_MINUTES_MARK: &str = "@CLONE_AGE_MINUTES@";
const AGE_DAYS_MARK: &str = "@AGE_DAYS@";
const LIVE_JOBS_MARK: &str = "@LIVE_JOBS@";
/// Whether the operator side could read the queue, and therefore whether the
/// workdir sweep may run at all.
const KEEP_LIST_MARK: &str = "@KEEP_LIST@";
const WORK_ROOTS_MARK: &str = "@WORK_ROOTS@";
/// Where queue workdirs live in production: the fixed POSIX temp root plus
/// whatever the login shell's TMPDIR names (the macOS per-user container).
pub const DEFAULT_WORK_ROOTS: &str = "/tmp \"${TMPDIR:-}\"";
const CLONE_CONTAINER_MARK: &str = "@CLONE_CONTAINER@";
const CLONE_ROOT_MARK: &str = "@CLONE_ROOT@";
const CLONE_PREFIX_MARK: &str = "@CLONE_PREFIX@";
const CONTAINER_PREFIX_MARK: &str = "@CONTAINER_PREFIX@";
const SUPERSEDED_ROOTS_MARK: &str = "@SUPERSEDED_ROOTS@";
const TARGET_FREE_KB_MARK: &str = "@TARGET_FREE_KB@";

/// The fixed remote program.
///
/// stderr is deliberately NOT redirected, for the reason
/// [`crate::deploy::host_cleanup`] gives: it travels back into the channel's
/// own stderr, which is where [`host_channel::finish_report`] reads the last
/// line from, and it is the one sentence explaining why a stage failed.
const REMOTE_SCRIPT_TEMPLATE: &str = r#"set -u
apply=@APPLY@
scratch="$HOME/@BUILD_WORK@"
services="$HOME/@SERVICES_ROOT@"
target_free_kb=@TARGET_FREE_KB@

free_kb() { /bin/df -Pk / 2>/dev/null | /usr/bin/awk 'NR==2 {print $4}'; }

# Every live process's argv, taken ONCE. Asking `ps` per candidate through a
# pipeline matches the grep's own argv and reports every path as held.
snapshot=$(/bin/ps -Ao args= 2>/dev/null || true)

held() {
  case "$snapshot" in
    *"$1"*) return 0 ;;
  esac
  return 1
}

# Older than the age gate. Asked per candidate rather than by sweeping a root,
# so the candidates come from exactly one enumeration -- see below.
stale() {
  [ -n "$(/usr/bin/find "$1" -maxdepth 0 -mtime +@AGE_DAYS@ 2>/dev/null)" ]
}

stale_minutes() {
  [ -n "$(/usr/bin/find "$1" -maxdepth 0 -mmin +@CLONE_AGE_MINUTES@ 2>/dev/null)" ]
}

# The only place anything is removed. A held path is skipped silently -- it is
# not a failure, it is the rule -- and in dry-run mode the path is reported
# without being touched, so a preview names exactly what an apply would take.
reclaim() {
  if held "$1"; then return 1; fi
  if [ "$apply" = 1 ]; then
    /bin/rm -rf -- "$1" 2>/dev/null || return 1
  fi
  printf 'STADO_RECLAIM_ITEM\t%s\t%s\n' "$2" "$1"
  return 0
}

printf 'STADO_RECLAIM_FREE\tbefore\t%s\n' "$(free_kb)"

before=$(free_kb)
wc_bin=""
for candidate in @WC_WORDS@; do
  if [ -x "$candidate" ]; then wc_bin="$candidate"; break; fi
done
if [ -z "$wc_bin" ]; then
  printf 'STADO_RECLAIM_UNAVAILABLE\tregistry_cleanup\t%s\n' 'no stado binary on this host'
else
  if [ "$apply" = 1 ]; then
    plan=$("$wc_bin" disk-cleanup --once)
  else
    plan=$("$wc_bin" disk-cleanup --once --dry-run)
  fi
  printf 'STADO_RECLAIM_CLEANUP\t%s\t%s\t%s\n' "$before" "$(free_kb)" "$plan"
fi

before=$(free_kb)
if [ -d "$scratch" ]; then
  for entry in "$scratch"/*; do
    [ -e "$entry" ] || continue
    stale "$entry" || continue
    reclaim "$entry" build_scratch
  done
fi
printf 'STADO_RECLAIM_STAGE\tbuild_scratch\t%s\t%s\n' "$before" "$(free_kb)"

before=$(free_kb)
# Workdirs of queue jobs that are neither queued nor running: a terminal job
# never returns to its workdir, so age is irrelevant and the keep-list is the
# small live set the operator side read from the queue store. Bootstrap
# scratch (stado-bootstrap-*) is one-off provisioning debris by definition.
# On 2026-08-19 these trees filled the linux builder to 0 GiB free and the
# fleet starved on a host that looked merely busy.
if [ "@KEEP_LIST@" = yes ]; then
  for workroot in @WORK_ROOTS@; do
    [ -n "$workroot" ] && [ -d "$workroot" ] || continue
    for entry in "$workroot"/wc-* "$workroot"/stado-bootstrap-*; do
      [ -d "$entry" ] || continue
      id=$(basename "$entry")
      id="${id#wc-}"
      case " @LIVE_JOBS@ " in
        *" $id "*) continue ;;
      esac
      reclaim "$entry" queue_workdirs
    done
  done
fi
printf 'STADO_RECLAIM_STAGE\tqueue_workdirs\t%s\t%s\n' "$before" "$(free_kb)"

before=$(free_kb)
# macOS-style home trees on a Linux host. `/Users/<name>` exists on Linux only
# as debris of a job or delivery that carried a hard-wired Mac path — on
# 2026-08-19 one such tree held 10.9 GiB of build cache on the GPU builder.
# The uname gate makes this stage a no-op on every macOS host, where /Users is
# the real home root; held() still protects a tree a live process names.
if [ "$(/usr/bin/uname 2>/dev/null || /bin/uname)" = "Linux" ] && [ -d /Users ]; then
  for entry in /Users/*; do
    [ -d "$entry" ] || continue
    reclaim "$entry" foreign_home_trees
  done
fi
printf 'STADO_RECLAIM_STAGE\tforeign_home_trees\t%s\t%s\n' "$before" "$(free_kb)"

before=$(free_kb)
# One directory of versions: keep what `current` resolves to, keep the newest,
# take the stale unheld rest. A function because the same rules have to hold
# for the services root and for every superseded delivery root a product
# declares -- two copies would be two policies, and only one of them would be
# the tested one.
sweep_versions() {
  product="$1"
  # What `current` resolves to, in the spelling the version glob produces,
  # so the comparison below is an equality and not a guess.
  keep=""
  if [ -L "$product/current" ]; then
    keep=$(/usr/bin/readlink "$product/current" 2>/dev/null || true)
    case "$keep" in
      "") ;;
      /*) ;;
      *) keep="$product/$keep" ;;
    esac
  fi
  # The newest version directory, `current` itself excluded so a product
  # whose link is stale still keeps its most recent delivery. `ls -td` sorts
  # newest first; the listing is walked with globbing off and IFS on newline
  # so a directory name with a space in it cannot become two words.
  newest=""
  listing=$(/bin/ls -td -- "$product"/*/ 2>/dev/null || true)
  saved_ifs=$IFS
  set -f
  IFS='
'
  for candidate in $listing; do
    candidate=${candidate%/}
    case "$candidate" in
      */current) continue ;;
    esac
    if [ -L "$candidate" ]; then continue; fi
    newest="$candidate"
    break
  done
  IFS=$saved_ifs
  set +f
  # The SAME glob the newest above came from. A `find` here would also list
  # dotted entries the glob cannot see, and the two halves would disagree
  # about the set: on this control plane's own host that difference named
  # `.macos-capability-backup-20260803` -- an operator's state backup living
  # beside the deliveries, which no delivery created and no reclamation may
  # take. A delivery and a build both produce plainly named directories, so
  # the glob IS the set.
  for tree in "$product"/*; do
    [ -d "$tree" ] || continue
    if [ -L "$tree" ]; then continue; fi
    case "$tree" in
      */current) continue ;;
    esac
    [ "$tree" = "$keep" ] && continue
    [ "$tree" = "$newest" ] && continue
    stale "$tree" || continue
    reclaim "$tree" delivered_trees
  done
}

if [ -d "$services" ]; then
  for product in "$services"/*; do
    [ -d "$product" ] || continue
    sweep_versions "$product"
  done
fi
# Where an EARLIER delivery mechanism staged one directory per version, taken
# from what `data/products.json` declares per product, so a delivery path that
# changes again is a declaration change and not a change here. Each root IS one
# product's version directory, one level shallower than the services layout.
for superseded in @SUPERSEDED_ROOTS@; do
  [ -d "$superseded" ] || continue
  sweep_versions "$superseded"
done
printf 'STADO_RECLAIM_STAGE\tdelivered_trees\t%s\t%s\n' "$before" "$(free_kb)"

before=$(free_kb)
# The account's macOS temporary container, as the OS reports it: its name
# carries a per-account hash, so nothing on this side can spell it. $TMPDIR is
# that answer inside a session; getconf is the same answer when a session
# stripped the variable. Anything not under the one prefix macOS uses is not a
# container and is refused.
before=$(free_kb)
# Exact cache roots, not a general cache sweep. Cargo recreates git checkouts
# from its bare db and Playwright reinstalls browser bundles from package pins.
# Age and process guards keep active builds untouched.
for cache_root in "$HOME/.cargo/git/checkouts" "$HOME/Library/Caches/ms-playwright"; do
  [ -d "$cache_root" ] || continue
  for entry in "$cache_root"/*; do
    [ -d "$entry" ] || continue
    if [ -L "$entry" ]; then continue; fi
    stale "$entry" || continue
    if [ "$apply" = 1 ] && [ "$target_free_kb" -gt 0 ] && [ "$(free_kb)" -ge "$target_free_kb" ]; then
      break
    fi
    reclaim "$entry" rebuildable_caches
  done
done
# Old release probes are complete throwaway workspaces.
for entry in "$HOME/.local/share/weles-release-probe"/* "$HOME/.npm/_cacache"/*; do
  [ -d "$entry" ] || continue
  if [ -L "$entry" ]; then continue; fi
  stale "$entry" || continue
  if [ "$apply" = 1 ] && [ "$target_free_kb" -gt 0 ] && [ "$(free_kb)" -ge "$target_free_kb" ]; then
    break
  fi
  reclaim "$entry" rebuildable_caches
done
# Interrupted worker downloads are disposable staging directories. An hour is
# enough to exclude an active delivery while preventing today's failed
# downloads from surviving until tomorrow under disk pressure.
for entry in "$HOME/.local/share/weles-worker"/.worker-download.*; do
  [ -d "$entry" ] || continue
  if [ -L "$entry" ]; then continue; fi
  stale_minutes "$entry" || continue
  if [ "$apply" = 1 ] && [ "$target_free_kb" -gt 0 ] && [ "$(free_kb)" -ge "$target_free_kb" ]; then
    break
  fi
  reclaim "$entry" rebuildable_caches
done
# Git dependency checkouts are also rebuildable. The process snapshot remains
# the ownership gate; the shorter age only allows the cache to recover from a
# same-day build storm once no compiler names the checkout anymore.
for entry in "$HOME/.cargo/git/checkouts"/*; do
  [ -d "$entry" ] || continue
  if [ -L "$entry" ]; then continue; fi
  stale_minutes "$entry" || continue
  if [ "$apply" = 1 ] && [ "$target_free_kb" -gt 0 ] && [ "$(free_kb)" -ge "$target_free_kb" ]; then
    break
  fi
  reclaim "$entry" rebuildable_caches
done
printf 'STADO_RECLAIM_STAGE\trebuildable_caches\t%s\t%s\n' "$before" "$(free_kb)"
before=$(free_kb)

container=${TMPDIR:-$(/usr/bin/getconf DARWIN_USER_TEMP_DIR 2>/dev/null || true)}
clones=""
case "$container" in
  @CONTAINER_PREFIX@*) clones="$(/usr/bin/dirname "${container%/}")/@CLONE_CONTAINER@/@CLONE_ROOT@" ;;
esac
if [ -n "$clones" ] && [ -d "$clones" ]; then
  # The newest clone, kept whatever its age: macOS makes one per launch and
  # says nothing about which process owns which, so a browser that has been up
  # longer than the age gate is exactly the owner of the most recent one. Taken
  # from the SAME glob the candidate loop below uses -- a directory nobody
  # launched, sitting in the root with the freshest mtime, would otherwise
  # shield itself and leave the live browser's clone the newest thing eligible.
  newest=""
  listing=$(/bin/ls -td -- "$clones"/@CLONE_PREFIX@*/ 2>/dev/null || true)
  saved_ifs=$IFS
  set -f
  IFS='
'
  for candidate in $listing; do
    candidate=${candidate%/}
    if [ -L "$candidate" ]; then continue; fi
    newest="$candidate"
    break
  done
  IFS=$saved_ifs
  set +f
  # Only entries the OS itself named. A clone root is a directory this stage
  # did not create and does not own, so the entry name is what says an entry is
  # a clone rather than something that merely lives there.
  for clone in "$clones"/@CLONE_PREFIX@*; do
    [ -d "$clone" ] || continue
    if [ -L "$clone" ]; then continue; fi
    [ "$clone" = "$newest" ] && continue
    stale_minutes "$clone" || continue
    if [ "$apply" = 1 ] && [ "$target_free_kb" -gt 0 ] && [ "$(free_kb)" -ge "$target_free_kb" ]; then
      break
    fi
    reclaim "$clone" chromium_clones
  done
fi
printf 'STADO_RECLAIM_STAGE\tchromium_clones\t%s\t%s\n' "$before" "$(free_kb)"

before=$(free_kb)
if [ "$(/usr/bin/uname 2>/dev/null || /bin/uname)" != "Darwin" ]; then
  printf 'STADO_RECLAIM_UNAVAILABLE\tlocal_apfs_snapshots\t%s\n' 'host is not macOS'
elif [ "$target_free_kb" -le 0 ]; then
  printf 'STADO_RECLAIM_UNAVAILABLE\tlocal_apfs_snapshots\t%s\n' 'registry declares no disk cleanup target'
elif [ ! -x /usr/bin/tmutil ]; then
  printf 'STADO_RECLAIM_UNAVAILABLE\tlocal_apfs_snapshots\t%s\n' 'tmutil is unavailable'
else
  snapshots=$(/usr/bin/tmutil listlocalsnapshots / 2>/dev/null) || {
    printf 'STADO_RECLAIM_UNAVAILABLE\tlocal_apfs_snapshots\t%s\n' 'tmutil could not enumerate local snapshots'
    snapshots=""
  }
  saved_ifs=$IFS
  IFS='
'
  for line in $snapshots; do
    case "$line" in
      com.apple.TimeMachine.*.local)
        stamp=${line#com.apple.TimeMachine.}
        stamp=${stamp%.local}
        case "$stamp" in
          ????-??-??-??????) ;;
          *)
            printf 'STADO_RECLAIM_REFUSED\tlocal_apfs_snapshots\t%s\t%s\n' "$line" 'unrecognized Time Machine snapshot identifier'
            continue
            ;;
        esac
        printf 'STADO_RECLAIM_ITEM\tlocal_apfs_snapshots\t%s\n' "$line"
        if [ "$apply" = 1 ] && [ "$(free_kb)" -lt "$target_free_kb" ]; then
          if ! result=$(/usr/bin/tmutil deletelocalsnapshots "$stamp" 2>&1); then
            printf 'STADO_RECLAIM_REFUSED\tlocal_apfs_snapshots\t%s\t%s\n' "$line" "$result"
          fi
        fi
        ;;
      Snapshot*|Snapshots*|"") ;;
      *) printf 'STADO_RECLAIM_REFUSED\tlocal_apfs_snapshots\t%s\t%s\n' "$line" 'snapshot is not an eligible local Time Machine snapshot' ;;
    esac
  done
  IFS=$saved_ifs
fi
printf 'STADO_RECLAIM_STAGE\tlocal_apfs_snapshots\t%s\t%s\n' "$before" "$(free_kb)"

printf 'STADO_RECLAIM_FREE\tafter\t%s\n' "$(free_kb)"
"#;

/// Every superseded delivery root the product catalog declares, as shell words.
///
/// Read from [`products::declared`] rather than spelled here: the paths are
/// facts about each product's delivery history, they live in
/// `data/products.json`, and a reclamation that carried its own copy would go
/// stale the next time a delivery path moves. A catalog that will not parse
/// yields no roots, so the stage sweeps the services root alone rather than
/// guessing.
fn superseded_words() -> String {
    products::declared()
        .unwrap_or_default()
        .iter()
        .flat_map(|product| product.superseded_roots.iter())
        .map(|root| format!("\"{root}\""))
        .collect::<Vec<String>>()
        .join(" ")
}

/// The remote program for one mode, with every substitution in place.
///
/// The stado candidates are quoted exactly the way
/// [`crate::deploy::host_recovery::remote_script`] quotes them, so `$HOME`
/// still expands on the remote side while the word stays one word.
/// `work_roots` is substituted verbatim into the queue-workdirs sweep;
/// production callers pass [`DEFAULT_WORK_ROOTS`], tests pass their scratch
/// directory so an executed apply can never leave the fixture.
pub fn remote_script(
    apply: bool,
    live_jobs: Option<&[String]>,
    work_roots: &str,
    target_free_gb: Option<i64>,
) -> String {
    let wc_words = WC_CANDIDATES
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<String>>()
        .join(" ");
    // `None` is "nobody could read the queue", and the workdir sweep is then
    // not run at all. An empty keep-list would mean the opposite — no job is
    // live, so every workdir is terminal — and that is how a fail-closed
    // stage becomes a delete-everything stage.
    let live_words = live_jobs
        .unwrap_or_default()
        .iter()
        .filter(|id| !id.is_empty() && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-'))
        .cloned()
        .collect::<Vec<String>>()
        .join(" ");
    REMOTE_SCRIPT_TEMPLATE
        .replace(APPLY_MARK, if apply { "1" } else { "0" })
        .replace(WC_WORDS_MARK, &wc_words)
        .replace(SERVICES_ROOT_MARK, SERVICES_ROOT)
        .replace(BUILD_WORK_MARK, BUILD_WORK_ROOT)
        .replace(AGE_DAYS_MARK, MIN_AGE_DAYS)
        .replace(LIVE_JOBS_MARK, &live_words)
        .replace(
            KEEP_LIST_MARK,
            if live_jobs.is_some() { "yes" } else { "no" },
        )
        .replace(CLONE_AGE_MINUTES_MARK, CLONE_MIN_AGE_MINUTES)
        .replace(WORK_ROOTS_MARK, work_roots)
        .replace(CONTAINER_PREFIX_MARK, CONTAINER_PREFIX)
        .replace(CLONE_CONTAINER_MARK, chromium_clones::CLONE_CONTAINER)
        .replace(CLONE_ROOT_MARK, chromium_clones::CLONE_ROOT_NAME)
        .replace(CLONE_PREFIX_MARK, chromium_clones::CLONE_ENTRY_PREFIX)
        .replace(SUPERSEDED_ROOTS_MARK, &superseded_words())
        .replace(
            TARGET_FREE_KB_MARK,
            &target_free_gb
                .and_then(|gb| gb.checked_mul(1024 * 1024))
                .unwrap_or_default()
                .to_string(),
        )
}

/// One stage, as the host measured it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stage {
    pub stage: String,
    /// `df -Pk` available blocks either side of the stage, or `None` for a
    /// stage that never ran.
    pub free_kb_before: Option<i64>,
    pub free_kb_after: Option<i64>,
    /// How many items the stage reclaimed, or in dry-run mode would reclaim.
    pub items: usize,
    /// The paths behind that count, when the stage produced them. The janitor
    /// reports per-cleaner counts and not paths, so this is empty for that
    /// stage rather than filled with placeholders.
    pub paths: Vec<String>,
    /// Why the stage could not run, for the host's own words in the rendering.
    pub detail: Option<String>,
    /// Snapshot identifiers refused by the native ownership/type checks.
    pub refused: Vec<String>,
}

/// Everything one reclamation did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reclamation {
    pub mode: String,
    pub stages: Vec<Stage>,
    pub free_kb_before: Option<i64>,
    pub free_kb_after: Option<i64>,
    /// The janitor's canonical report, kept for the per-cleaner rendering.
    pub janitor_plan: Option<Value>,
    /// Stages that did not run, and why. A stage nobody could judge must say
    /// so; reporting it as a stage that removed nothing is the same sentence
    /// as a clean host.
    pub skipped: Vec<(String, String)>,
}

/// A marker field that has to be a `df` block count.
fn blocks(field: &str) -> Option<i64> {
    field.trim().parse::<i64>().ok()
}

/// Take every pending item line that names `stage`, leaving the rest.
fn drain(pending: &mut Vec<(String, String)>, stage: &str) -> Vec<String> {
    let mut mine = Vec::new();
    pending.retain(|(named, path)| {
        if named == stage {
            mine.push(path.clone());
            return false;
        }
        true
    });
    mine
}

/// Fold the marker lines of stdout into a reclamation.
///
/// Item lines always precede the stage marker that closes their stage, because
/// each stage prints its own totals after its loop, so items are attached to
/// the stage that names them without buffering the whole stream.
pub fn parse_output(stdout: &str, apply: bool) -> Reclamation {
    let mut reclamation = Reclamation {
        mode: if apply { APPLY_MODE } else { DRY_RUN_MODE }.to_string(),
        ..Reclamation::default()
    };
    let mut pending: Vec<(String, String)> = Vec::new();
    let mut refused: Vec<(String, String)> = Vec::new();
    // Item lines are drained by the stage marker that closes them, through a
    // free function so the pending list stays borrowable by the arm that fills
    // it.
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_RECLAIM_FREE", "before", free] => reclamation.free_kb_before = blocks(free),
            ["STADO_RECLAIM_FREE", "after", free] => reclamation.free_kb_after = blocks(free),
            ["STADO_RECLAIM_ITEM", stage, path] => {
                pending.push(((*stage).to_string(), (*path).to_string()));
            }
            ["STADO_RECLAIM_REFUSED", stage, item, detail] => {
                refused.push(((*stage).to_string(), format!("{item}: {detail}")));
            }
            ["STADO_RECLAIM_STAGE", stage, before, after] => {
                let paths = drain(&mut pending, stage);
                let stage_refused = drain(&mut refused, stage);
                reclamation.stages.push(Stage {
                    stage: (*stage).to_string(),
                    free_kb_before: blocks(before),
                    free_kb_after: blocks(after),
                    items: paths.len(),
                    paths,
                    detail: None,
                    refused: stage_refused,
                });
            }
            ["STADO_RECLAIM_CLEANUP", before, after, plan] => {
                // The janitor's own numbers, never recounted here: what a pass
                // would remove in preview mode, what it did remove in apply
                // mode. A report that will not parse is a stage that did not
                // run, not a stage that freed nothing.
                let parsed: Option<Value> = serde_json::from_str(plan).ok();
                let Some(parsed) = parsed else {
                    reclamation.stages.push(unavailable(
                        REGISTRY_CLEANUP_STAGE,
                        "the host janitor produced no parseable report",
                    ));
                    continue;
                };
                let counted: i64 = cleaner_plans(&parsed)
                    .iter()
                    .map(|cleaner| {
                        if apply {
                            cleaner.deleted_items
                        } else {
                            cleaner.eligible_items
                        }
                    })
                    .sum();
                reclamation.janitor_plan = Some(parsed);
                reclamation.stages.push(Stage {
                    stage: REGISTRY_CLEANUP_STAGE.to_string(),
                    free_kb_before: blocks(before),
                    free_kb_after: blocks(after),
                    items: usize::try_from(counted).unwrap_or_default(),
                    paths: Vec::new(),
                    detail: None,
                    refused: Vec::new(),
                });
            }
            ["STADO_RECLAIM_UNAVAILABLE", stage, detail] => {
                reclamation.stages.push(unavailable(stage, detail));
            }
            _ => {}
        }
    }
    reclamation
}

/// A stage the host could not run, named as itself.
fn unavailable(stage: &str, detail: &str) -> Stage {
    Stage {
        stage: format!("{stage}{UNAVAILABLE_SUFFIX}"),
        free_kb_before: None,
        free_kb_after: None,
        items: 0,
        paths: Vec::new(),
        detail: Some(detail.to_string()),
        refused: Vec::new(),
    }
}

/// The reclamation as the `--json` report, in the exact shape the operator
/// console consumes.
pub fn to_report(target: &ComputeTarget, reclamation: &Reclamation) -> Map<String, Value> {
    let free = |blocks: Option<i64>| match blocks {
        Some(blocks) => json!(gib_from_blocks(blocks as f64)),
        None => Value::Null,
    };
    let mut report = Map::new();
    report.insert("host".to_string(), json!(target.name));
    report.insert("mode".to_string(), json!(reclamation.mode));
    report.insert(
        "stages".to_string(),
        Value::Array(
            reclamation
                .stages
                .iter()
                .map(|stage| {
                    json!({
                        "stage": stage.stage,
                        "free_gb_before": free(stage.free_kb_before),
                        "free_gb_after": free(stage.free_kb_after),
                        "items": stage.items,
                        "paths": stage.paths,
                        "refused": stage.refused,
                        "detail": stage.detail,
                    })
                })
                .collect(),
        ),
    );
    // Named in the receipt, not only on the terminal: a consumer that reads
    // `stages` alone cannot tell a stage that ran and found nothing from a
    // stage nobody could judge.
    report.insert(
        "skipped".to_string(),
        Value::Array(
            reclamation
                .skipped
                .iter()
                .map(|(stage, reason)| json!({"stage": stage, "reason": reason}))
                .collect(),
        ),
    );
    report.insert(
        "free_gb_before".to_string(),
        free(reclamation.free_kb_before),
    );
    report.insert("free_gb_after".to_string(), free(reclamation.free_kb_after));
    report
}

/// The script that appends one audit record on the host whose disk changed.
///
/// The record is one line of JSON built by `serde_json` on this side and
/// spliced in as a single shell-quoted word, so operator prose cannot become
/// shell syntax and cannot break the line format either. The directory is
/// owner-only, the same way every other thing Stado keeps under `.stado` is.
const AUDIT_SCRIPT_TEMPLATE: &str = r#"set -u
umask 077
log="$HOME/@AUDIT_LOG@"
/bin/mkdir -p "$(/usr/bin/dirname "$log")" || exit 1
printf '%s\n' @RECORD@ >> "$log" || exit 1
printf 'STADO_RECLAIM_AUDITED\t%s\n' "$log"
"#;

/// Append the audit record for an applied reclamation, and return where it
/// landed on the host.
///
/// Separate from the reclamation script because the record states what the
/// reclamation actually did: the measurements have to exist before the record
/// can be true, and a record written up front would be a record of an
/// intention.
///
/// `actor` arrives from the caller rather than being read here, so that this
/// binary has ONE spelling of "who did this" — `cli/autonomy_cmd::actor`, the
/// same one `service ensure` stamps its own record with.
pub async fn record_audit(
    target: &ComputeTarget,
    reclamation: &Reclamation,
    reason: &str,
    actor: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    let record = json!({
        "at": crate::models::isoformat_utc(chrono::Utc::now()),
        "command": "stado host reclaim",
        "mode": reclamation.mode,
        "actor": actor,
        "reason": reason,
        "free_gb_before": reclamation.free_kb_before.map(|kb| gib_from_blocks(kb as f64)),
        "free_gb_after": reclamation.free_kb_after.map(|kb| gib_from_blocks(kb as f64)),
        "stages": reclamation
            .stages
            .iter()
            .map(|stage| json!({
                "stage": stage.stage,
                "items": stage.items,
                "paths": stage.paths,
                "refused": stage.refused,
                "free_gb_before": stage.free_kb_before.map(|kb| gib_from_blocks(kb as f64)),
                "free_gb_after": stage.free_kb_after.map(|kb| gib_from_blocks(kb as f64)),
            }))
            .collect::<Vec<Value>>(),
    });
    let script = AUDIT_SCRIPT_TEMPLATE
        .replace("@AUDIT_LOG@", AUDIT_LOG)
        .replace("@RECORD@", &shlex_quote(&record.to_string()));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the audit record could not be written",
        )));
    }
    for line in output.stdout.lines() {
        if let ["STADO_RECLAIM_AUDITED", path] = host_channel::marker_fields(line).as_slice() {
            return Ok((*path).to_string());
        }
    }
    Err(DeployError(
        "the host did not confirm the audit record".to_string(),
    ))
}

/// Run the reclamation on one canonical registry host.
///
/// Returns the resolved target alongside the reclamation because the caller
/// needs it for the report and for the audit record, and resolving it twice
/// would be two registry reads that could disagree.
pub async fn reclaim_host(
    target_name: &str,
    apply: bool,
    runner: &Runner,
) -> Result<(ComputeTarget, Reclamation), DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    // The keep-list for the queue_workdirs stage: jobs still queued or
    // running are the only ones that may return to their workdirs.
    //
    // An unreadable queue store costs exactly that one stage. It used to
    // refuse the whole reclamation, and on 2026-09-03 that turned a listing
    // defect into a release outage: a migration created `queue_priority/`
    // beside `queue/`, the gateway answered `prefix=queue/` with both, the
    // keep-list could not be built, and `release-capacity` failed the
    // 0.13.50 train before a single object was published — while the build
    // caches and superseded trees the pass exists to free were never even
    // looked at. Skipping the stage keeps every workdir, which is the
    // fail-closed behaviour this comment always claimed, and says so in the
    // report instead of deleting blind or refusing everything.
    //
    // A stage that skips silently is the other half of that same outage, so
    // the store's own sentence travels into the skip. It used to be
    // discarded (`Err(_)`) at the only place that saw it, and that is why the
    // release train spent an hour and three attempts: nothing anywhere said
    // which read failed or why. The queue held 13 parseable records and
    // `running/` was empty, both verified object by object through the same
    // CLI, so the sentence was the only thing that could have named the real
    // failure. Carried through, `HTTP 502: upstream unavailable` classes as
    // `infra_down`, `retryable=true` — which is the truth about a status
    // `deploy.yml` retries three times — where a discarded error left
    // `error_code="unknown"`, `retryable=false`. A refusal, or a skip, an
    // operator cannot act on is the defect this fleet keeps paying for.
    let mut unreadable: Vec<String> = Vec::new();
    let live_jobs = match crate::queue::JobStorage::new().await {
        Ok(store) => {
            let mut ids = Vec::new();
            for state in ["queue", "running"] {
                match store.list_jobs(state, 0).await {
                    Ok(jobs) => ids.extend(jobs.into_iter().map(|job| job.job_id)),
                    Err(error) => unreadable.push(format!("{state}/: {error}")),
                }
            }
            if unreadable.is_empty() {
                Some(ids)
            } else {
                None
            }
        }
        Err(error) => {
            unreadable.push(format!("opening the queue store: {error}"));
            None
        }
    };
    let target_free_gb = target
        .disk_cleanup
        .as_ref()
        .map(|policy| policy.target_free_gb);
    let output = host_channel::run_script_with_timeout(
        &target,
        &remote_script(
            apply,
            live_jobs.as_deref(),
            DEFAULT_WORK_ROOTS,
            target_free_gb,
        ),
        RECLAIM_TIMEOUT,
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the reclamation did not run",
        )));
    }
    let mut reclamation = parse_output(&output.stdout, apply);
    if live_jobs.is_none() {
        reclamation.skipped.push((
            "queue_workdirs".to_string(),
            format!(
                "the queue store did not answer, so no workdir could be shown to be terminal; \
                 every workdir was kept — {}",
                unreadable.join("; ")
            ),
        ));
    }
    Ok((target, reclamation))
}
