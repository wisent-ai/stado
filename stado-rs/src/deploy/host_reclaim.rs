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
const AGE_DAYS_MARK: &str = "@AGE_DAYS@";
const LIVE_JOBS_MARK: &str = "@LIVE_JOBS@";
const WORK_ROOTS_MARK: &str = "@WORK_ROOTS@";
/// Where queue workdirs live in production: the fixed POSIX temp root plus
/// whatever the login shell's TMPDIR names (the macOS per-user container).
pub const DEFAULT_WORK_ROOTS: &str = "/tmp \"${TMPDIR:-}\"";
const CLONE_CONTAINER_MARK: &str = "@CLONE_CONTAINER@";
const CLONE_ROOT_MARK: &str = "@CLONE_ROOT@";
const CLONE_PREFIX_MARK: &str = "@CLONE_PREFIX@";
const CONTAINER_PREFIX_MARK: &str = "@CONTAINER_PREFIX@";
const SUPERSEDED_ROOTS_MARK: &str = "@SUPERSEDED_ROOTS@";

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
printf 'STADO_RECLAIM_STAGE\tqueue_workdirs\t%s\t%s\n' "$before" "$(free_kb)"

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
    stale "$clone" || continue
    reclaim "$clone" chromium_clones
  done
fi
printf 'STADO_RECLAIM_STAGE\tchromium_clones\t%s\t%s\n' "$before" "$(free_kb)"

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
pub fn remote_script(apply: bool, live_jobs: &[String], work_roots: &str) -> String {
    let wc_words = WC_CANDIDATES
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<String>>()
        .join(" ");
    // Job ids are hex identifiers; anything else is dropped rather than
    // spliced into a shell word. Losing a malformed id from the KEEP list is
    // fail-safe only because held() still protects a workdir some live
    // process names in its argv.
    let live_words = live_jobs
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
        .replace(WORK_ROOTS_MARK, work_roots)
        .replace(CONTAINER_PREFIX_MARK, CONTAINER_PREFIX)
        .replace(CLONE_CONTAINER_MARK, chromium_clones::CLONE_CONTAINER)
        .replace(CLONE_ROOT_MARK, chromium_clones::CLONE_ROOT_NAME)
        .replace(CLONE_PREFIX_MARK, chromium_clones::CLONE_ENTRY_PREFIX)
        .replace(SUPERSEDED_ROOTS_MARK, &superseded_words())
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
            ["STADO_RECLAIM_STAGE", stage, before, after] => {
                let paths = drain(&mut pending, stage);
                reclamation.stages.push(Stage {
                    stage: (*stage).to_string(),
                    free_kb_before: blocks(before),
                    free_kb_after: blocks(after),
                    items: paths.len(),
                    paths,
                    detail: None,
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
                    })
                })
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
            .map(|stage| json!({"stage": stage.stage, "items": stage.items}))
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
    // running are the only ones that may return to their workdirs. Read
    // best-effort — an unreadable queue store must not turn a disk repair
    // into an outage — but a read failure keeps EVERY workdir (empty
    // keep-list would keep none), so the stage fails closed.
    let live_jobs = match crate::queue::JobStorage::new().await {
        Ok(store) => {
            let mut ids = Vec::new();
            let mut readable = true;
            for state in ["queue", "running"] {
                match store.list_jobs(state, 0).await {
                    Ok(jobs) => ids.extend(jobs.into_iter().map(|job| job.job_id)),
                    Err(_) => readable = false,
                }
            }
            if readable {
                Some(ids)
            } else {
                None
            }
        }
        Err(_) => None,
    };
    let Some(live_jobs) = live_jobs else {
        return Err(DeployError(
            "the queue store is unreadable, so the terminal-workdir keep-list cannot be built; \
             refusing to reclaim workdirs blind"
                .to_string(),
        ));
    };
    let output = host_channel::run_script(
        &target,
        &remote_script(apply, &live_jobs, DEFAULT_WORK_ROOTS),
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the reclamation did not run",
        )));
    }
    Ok((target, parse_output(&output.stdout, apply)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    // -----------------------------------------------------------------
    // The shipped program itself, run against a scratch $HOME. No ssh, no
    // registry, no stado on that scratch host — so the janitor stage reports
    // itself unrun instead of touching the machine running the tests. Same
    // shape `host_inventory`'s tests use, for the same reason: the thing under
    // test is the program the channel sends, not a paraphrase of it.
    // -----------------------------------------------------------------

    /// `/bin/bash -s` with the program on stdin, which is byte for byte what
    /// [`host_channel::run_script`] sends.
    ///
    /// `TMPDIR` points at a temporary container INSIDE the scratch home, so the
    /// clones stage walks a root this test made. Pointed at the real container,
    /// an apply here would take the code-sign clones of whatever this machine's
    /// own browsers are doing, and a test is not entitled to one of them.
    fn run_locally(home: &Path, apply: bool) -> String {
        let mut child = Command::new("/bin/bash")
            .arg("-s")
            .env("HOME", home)
            .env("TMPDIR", container(home).join("T"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("bash");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(remote_script(apply, &[], "\"$HOME/.stado-test-workroot\"").as_bytes())
            .expect("the program reached the shell");
        let output = child.wait_with_output().expect("bash finished");
        assert!(
            output.status.success(),
            "the program failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Push a path's mtime back past the age gate, to an exact timestamp so
    /// `ls -td` has a defined order to report.
    fn backdate(path: &Path, stamp: &str) {
        let status = Command::new("/usr/bin/touch")
            .args(["-t", stamp])
            .arg(path)
            .status()
            .expect("touch");
        assert!(status.success(), "could not backdate {}", path.display());
    }

    /// The scratch temporary container of a scratch home.
    fn container(home: &Path) -> PathBuf {
        home.join("container")
    }

    /// The Chromium clone root inside that container, in the layout macOS uses.
    fn clone_root(home: &Path) -> PathBuf {
        container(home)
            .join(chromium_clones::CLONE_CONTAINER)
            .join(chromium_clones::CLONE_ROOT_NAME)
    }

    /// One clone bundle in that root, at an exact mtime.
    fn make_clone(root: &Path, name: &str, stamp: Option<&str>) -> PathBuf {
        let clone = root.join(name);
        std::fs::create_dir_all(clone.join("Chromium.app.bundle")).expect("clone bundle");
        if let Some(stamp) = stamp {
            backdate(&clone, stamp);
        }
        clone
    }

    /// A scratch host carrying exactly what the three filesystem stages have to
    /// decide about: one stale build tree and one fresh one, three delivered
    /// versions of one product with `current` on the middle one, — beside
    /// them — the dotted state backup this control plane's own host actually
    /// carries, which is older than every gate and must still survive, and a
    /// temporary container holding two stale Chromium clones plus a directory
    /// macOS did not name.
    fn scratch_host() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().expect("scratch home");
        let scratch = home.path().join(BUILD_WORK_ROOT);
        for entry in ["stado", "fresh"] {
            std::fs::create_dir_all(scratch.join(entry)).expect("build scratch");
        }
        backdate(&scratch.join("stado"), "202601010000");
        let product = home.path().join(SERVICES_ROOT).join("weles-worker");
        for (version, stamp) in [
            ("0.4.9", "202601010000"),
            ("0.5.0", "202601020000"),
            ("0.5.1", "202601030000"),
        ] {
            let tree = product.join(version);
            std::fs::create_dir_all(&tree).expect("delivered tree");
            backdate(&tree, stamp);
        }
        std::os::unix::fs::symlink(product.join("0.5.0"), product.join("current"))
            .expect("current link");
        // The regression a live dry run against this control plane's own host
        // found: `.macos-capability-backup-20260803`, an operator's state
        // backup living in the product root. Stale, plainly a directory, and
        // neither `current` nor the newest — every other rule would let it go.
        let backup = product.join(".macos-capability-backup-20260803");
        std::fs::create_dir_all(&backup).expect("host-local backup");
        backdate(&backup, "202512310000");
        let clones = clone_root(home.path());
        std::fs::create_dir_all(container(home.path()).join("T")).expect("scratch container");
        make_clone(&clones, "code_sign_clone.older", Some("202601010000"));
        make_clone(&clones, "code_sign_clone.newest", Some("202601020000"));
        // Not a clone: nothing macOS named, so nothing this stage may take,
        // however stale it is. The clone root is the one root the fleet's own
        // software did not create, so its contents are not the fleet's to
        // assume about.
        let foreign = clones.join("someone-elses-directory");
        std::fs::create_dir_all(&foreign).expect("foreign directory");
        backdate(&foreign, "202512310000");
        (home, product)
    }

    /// The stage spans the superseded delivery root the catalog declares, with
    /// the same rules it applies to the services root: the mini's 20 inert
    /// `weles-worker` trees under `$HOME/.local/share/weles-worker` were
    /// invisible to every command until that root was declared, and the live
    /// worker runs from its own checkout, never from there.
    ///
    /// The root is read from `data/products.json` rather than spelled here, so
    /// this fails the day a declaration is dropped — which is the point of
    /// declaring it.
    #[test]
    fn the_stage_spans_every_declared_superseded_root() {
        let (home, _product) = scratch_host();
        let declared: Vec<String> = products::declared()
            .expect("the product catalog parses")
            .iter()
            .flat_map(|product| product.superseded_roots.clone())
            .collect();
        assert!(
            declared.contains(&"$HOME/.local/share/weles-worker".to_string()),
            "no superseded root is declared: {declared:?}"
        );
        // One legacy root, laid out the way that installer left it: versions
        // directly under the root and no `current` link at all.
        let legacy = home.path().join(".local/share/weles-worker");
        // Two stale versions and one written today — the shape the mini is in,
        // where every one of the 20 trees was written today and an apply
        // therefore takes none of them yet.
        for (version, stamp) in [
            ("0.5.20", "202601010000"),
            ("0.5.21", "202601020000"),
            ("0.5.22", "202608180000"),
        ] {
            let tree = legacy.join(version);
            std::fs::create_dir_all(&tree).expect("legacy tree");
            backdate(&tree, stamp);
        }
        let named = parse_output(&run_locally(home.path(), false), false);
        let stage = named
            .stages
            .iter()
            .find(|stage| stage.stage == DELIVERED_TREES_STAGE)
            .expect("the trees stage ran");
        assert!(
            stage
                .paths
                .contains(&legacy.join("0.5.20").display().to_string()),
            "the declared superseded root was not walked: {:?}",
            stage.paths
        );
        assert!(
            !stage
                .paths
                .contains(&legacy.join("0.5.22").display().to_string()),
            "the newest tree of the superseded root was named"
        );
        let applied = parse_output(&run_locally(home.path(), true), true);
        assert_eq!(
            applied.stages.len(),
            named.stages.len(),
            "the two modes walked different stages"
        );
        assert!(
            !legacy.join("0.5.20").exists(),
            "a stale legacy tree survived"
        );
        assert!(
            !legacy.join("0.5.21").exists(),
            "a stale legacy tree survived"
        );
        assert!(
            legacy.join("0.5.22").exists(),
            "the newest legacy tree was removed"
        );
    }

    /// A dry run names the stale build tree and the one delivered version that
    /// is neither `current` nor the newest — and leaves every one of them on
    /// disk. This is the claim `--dry-run` makes, checked against the
    /// filesystem rather than against the flag.
    #[test]
    fn a_dry_run_names_the_right_paths_and_removes_none_of_them() {
        let (home, product) = scratch_host();
        let reclamation = parse_output(&run_locally(home.path(), false), false);
        let stage = |name: &str| {
            reclamation
                .stages
                .iter()
                .find(|stage| stage.stage == name)
                .unwrap_or_else(|| panic!("{name} ran"))
        };
        // No stado on the scratch host, so the janitor stage says so.
        assert_eq!(
            stage("registry_cleanup_unavailable").detail.as_deref(),
            Some("no stado binary on this host")
        );
        assert_eq!(
            stage(BUILD_SCRATCH_STAGE).paths,
            vec![home
                .path()
                .join(BUILD_WORK_ROOT)
                .join("stado")
                .display()
                .to_string()]
        );
        assert_eq!(
            stage(DELIVERED_TREES_STAGE).paths,
            vec![product.join("0.4.9").display().to_string()]
        );
        // Two stale clones in the root, and the newest of them is kept, so the
        // older one is the whole of what this stage names.
        assert_eq!(
            stage(CHROMIUM_CLONES_STAGE).paths,
            vec![clone_root(home.path())
                .join("code_sign_clone.older")
                .display()
                .to_string()]
        );
        for kept in [
            "0.4.9",
            "0.5.0",
            "0.5.1",
            "current",
            ".macos-capability-backup-20260803",
        ] {
            assert!(
                product.join(kept).exists(),
                "{kept} was removed by a preview"
            );
        }
        assert!(home.path().join(BUILD_WORK_ROOT).join("stado").exists());
    }

    /// An apply removes exactly what the preview named, and nothing else: the
    /// fresh build tree, the tree `current` points at, and the newest tree all
    /// survive.
    #[test]
    fn an_apply_removes_exactly_what_the_preview_named() {
        let (home, product) = scratch_host();
        let named = parse_output(&run_locally(home.path(), false), false);
        let applied = parse_output(&run_locally(home.path(), true), true);
        let paths = |reclamation: &Reclamation| -> Vec<String> {
            let mut paths: Vec<String> = reclamation
                .stages
                .iter()
                .flat_map(|stage| stage.paths.clone())
                .collect();
            paths.sort();
            paths
        };
        assert_eq!(paths(&named), paths(&applied));
        assert!(!product.join("0.4.9").exists(), "the stale tree survived");
        assert!(!home.path().join(BUILD_WORK_ROOT).join("stado").exists());
        for kept in ["0.5.0", "0.5.1", "current"] {
            assert!(product.join(kept).exists(), "{kept} was removed");
        }
        assert!(
            product.join(".macos-capability-backup-20260803").exists(),
            "host-local state nobody delivered was removed"
        );
        assert!(
            home.path().join(BUILD_WORK_ROOT).join("fresh").exists(),
            "a build tree younger than the age gate was removed"
        );
        let clones = clone_root(home.path());
        assert!(
            !clones.join("code_sign_clone.older").exists(),
            "the stale clone survived"
        );
        assert!(
            clones.join("code_sign_clone.newest").exists(),
            "the newest clone was removed"
        );
        assert!(
            clones.join("someone-elses-directory").exists(),
            "an entry macOS never named was removed"
        );
    }

    /// A path a live process names is never removed, even when every other
    /// rule would have let it go: the `ps` snapshot is the gate.
    #[test]
    fn a_path_a_live_process_holds_is_left_alone() {
        let (home, product) = scratch_host();
        let doomed = product.join("0.4.9");
        // A process whose ARGV contains the path, for as long as the sweep
        // takes. It never opens the path, which is the point: executing out of
        // a tree is how these directories are really in use, and an open file
        // handle is not what that looks like. The trailing `; :` keeps the
        // shell from exec'ing `sleep` over itself, which would drop the
        // argument that makes this test mean anything.
        let mut holder = Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 30; :")
            .arg("stado-reclaim-test-holder")
            .arg(&doomed)
            .spawn()
            .expect("holder");
        let reclamation = parse_output(&run_locally(home.path(), true), true);
        holder.kill().expect("holder stopped");
        holder.wait().expect("holder reaped");
        let held = reclamation
            .stages
            .iter()
            .find(|stage| stage.stage == DELIVERED_TREES_STAGE)
            .expect("the trees stage ran");
        assert!(
            held.paths.is_empty() && held.items == 0,
            "a held path was reported as reclaimed: {:?}",
            held.paths
        );
        assert!(doomed.exists(), "a held path was removed");
        // The gate is per candidate, not per stage: the unheld build tree in
        // the same run still went.
        assert!(!home.path().join(BUILD_WORK_ROOT).join("stado").exists());
    }

    /// The clones stage keeps a clone a live process names and a clone younger
    /// than the age gate, and takes the stale unheld ones in the same run —
    /// which is what makes the two survivals mean something.
    ///
    /// A running browser is the case both gates exist for: macOS makes the
    /// clone at launch, so a live session's clone is young, and a session that
    /// has outlived the gate is the one a live argv still names.
    #[test]
    fn the_clones_stage_keeps_the_young_and_the_held() {
        let (home, _product) = scratch_host();
        let clones = clone_root(home.path());
        // No stamp: as young as a browser that just started.
        let young = make_clone(&clones, "code_sign_clone.young", None);
        let held_clone = make_clone(&clones, "code_sign_clone.held", Some("202601010000"));
        let mut holder = Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 30; :")
            .arg("stado-reclaim-test-holder")
            .arg(&held_clone)
            .spawn()
            .expect("holder");
        let reclamation = parse_output(&run_locally(home.path(), true), true);
        holder.kill().expect("holder stopped");
        holder.wait().expect("holder reaped");
        let stage = reclamation
            .stages
            .iter()
            .find(|stage| stage.stage == CHROMIUM_CLONES_STAGE)
            .expect("the clones stage ran");
        assert!(young.exists(), "a clone younger than the gate was removed");
        assert!(held_clone.exists(), "a held clone was removed");
        assert!(
            !stage.paths.iter().any(|path| {
                path == &young.display().to_string() || path == &held_clone.display().to_string()
            }),
            "a kept clone was reported as reclaimed: {:?}",
            stage.paths
        );
        // The armed run took the stale, unheld clones beside them. `young` is
        // now the newest, so both backdated clones of the fixture were
        // eligible: age and the process table are the only reasons anything
        // survived here.
        assert!(!clones.join("code_sign_clone.older").exists());
        assert!(!clones.join("code_sign_clone.newest").exists());
        assert_eq!(stage.items, 2, "{:?}", stage.paths);
    }

    fn target() -> ComputeTarget {
        serde_json::from_value(json!({
            "name": "control-host",
            "kind": "local",
            "ssh": "charles@control-host.local",
            "release_platform": "darwin-arm64",
        }))
        .expect("registry target")
    }

    /// A janitor report in the shape `disk-cleanup` prints it: one line of
    /// canonical JSON with per-cleaner counts.
    fn plan(eligible: i64, deleted: i64) -> String {
        json!({
            "outcome": "cleaned",
            "cleaners": {
                "huggingface_cache": {
                    "scanned_items": 40,
                    "eligible_items": eligible,
                    "deleted_items": deleted,
                    "expected_bytes": 1_000,
                },
            },
        })
        .to_string()
    }

    /// The whole marker stream of a dry run, folded into stages in execution
    /// order with the janitor's own count and every filesystem stage's paths.
    #[test]
    fn a_dry_run_reports_every_stage_it_walked() {
        let stdout = format!(
            "STADO_RECLAIM_FREE\tbefore\t2097152\n\
             STADO_RECLAIM_CLEANUP\t2097152\t2097152\t{}\n\
             STADO_RECLAIM_ITEM\tbuild_scratch\t/Users/charles/.stado/build-work/stado\n\
             STADO_RECLAIM_STAGE\tbuild_scratch\t2097152\t2097152\n\
             STADO_RECLAIM_ITEM\tdelivered_trees\t/Users/charles/.stado/services/weles/0.4.9\n\
             STADO_RECLAIM_ITEM\tdelivered_trees\t/Users/charles/.stado/services/weles/0.5.0\n\
             STADO_RECLAIM_STAGE\tdelivered_trees\t2097152\t2097152\n\
             STADO_RECLAIM_ITEM\tchromium_clones\t/var/folders/zy/x/X/org.chromium.Chromium.code_sign_clone/code_sign_clone.aa\n\
             STADO_RECLAIM_STAGE\tchromium_clones\t2097152\t2097152\n\
             STADO_RECLAIM_FREE\tafter\t2097152\n",
            plan(7, 0)
        );
        let reclamation = parse_output(&stdout, false);
        assert_eq!(reclamation.mode, DRY_RUN_MODE);
        let named: Vec<(&str, usize)> = reclamation
            .stages
            .iter()
            .map(|stage| (stage.stage.as_str(), stage.items))
            .collect();
        assert_eq!(
            named,
            vec![
                (REGISTRY_CLEANUP_STAGE, 7),
                (BUILD_SCRATCH_STAGE, 1),
                (DELIVERED_TREES_STAGE, 2),
                (CHROMIUM_CLONES_STAGE, 1),
            ]
        );
        // Nothing was freed, which is the truth of a preview, and the report
        // says so with numbers rather than with a promise.
        let report = to_report(&target(), &reclamation);
        assert_eq!(report["free_gb_before"], json!(2.0));
        assert_eq!(report["free_gb_after"], json!(2.0));
        assert_eq!(report["mode"], json!(DRY_RUN_MODE));
    }

    /// An apply counts the janitor's DELETED items, not its eligible ones, and
    /// the measured free space is the reason the operator ran the command.
    #[test]
    fn an_apply_reports_what_came_back() {
        let stdout = format!(
            "STADO_RECLAIM_FREE\tbefore\t2097152\n\
             STADO_RECLAIM_CLEANUP\t2097152\t20971520\t{}\n\
             STADO_RECLAIM_STAGE\tbuild_scratch\t20971520\t62914560\n\
             STADO_RECLAIM_STAGE\tdelivered_trees\t62914560\t83886080\n\
             STADO_RECLAIM_FREE\tafter\t83886080\n",
            plan(9, 9)
        );
        let reclamation = parse_output(&stdout, true);
        assert_eq!(reclamation.mode, APPLY_MODE);
        assert_eq!(reclamation.stages[0].items, 9);
        let report = to_report(&target(), &reclamation);
        assert_eq!(report["free_gb_before"], json!(2.0));
        assert_eq!(report["free_gb_after"], json!(80.0));
        assert_eq!(report["stages"][0]["free_gb_after"], json!(20.0));
    }

    /// A host with no stado gets its janitor stage reported as unrun, under its
    /// own name, with null measurements — never as a stage that freed nothing.
    #[test]
    fn a_stage_that_could_not_run_is_named_not_omitted() {
        let stdout = "STADO_RECLAIM_FREE\tbefore\t2097152\n\
                      STADO_RECLAIM_UNAVAILABLE\tregistry_cleanup\tno stado binary on this host\n\
                      STADO_RECLAIM_STAGE\tbuild_scratch\t2097152\t2097152\n\
                      STADO_RECLAIM_STAGE\tdelivered_trees\t2097152\t2097152\n\
                      STADO_RECLAIM_FREE\tafter\t2097152\n";
        let reclamation = parse_output(stdout, true);
        let stage = &reclamation.stages[0];
        assert_eq!(stage.stage, "registry_cleanup_unavailable");
        assert_eq!(
            stage.detail.as_deref(),
            Some("no stado binary on this host")
        );
        let report = to_report(&target(), &reclamation);
        assert_eq!(report["stages"][0]["free_gb_before"], Value::Null);
        assert_eq!(report["stages"][0]["items"], json!(0));
    }

    /// The report carries the contract's keys and only those, and every stage
    /// row carries exactly four. Compared as sorted sets: the document is
    /// printed through `host recover`'s sorted-keys printer, so insertion
    /// order is not part of the contract.
    #[test]
    fn the_report_shape_is_the_contract() {
        let stdout = "STADO_RECLAIM_FREE\tbefore\t2097152\n\
                      STADO_RECLAIM_STAGE\tbuild_scratch\t2097152\t2097152\n";
        let reclamation = parse_output(stdout, false);
        let report = to_report(&target(), &reclamation);
        let mut keys: Vec<&str> = report.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["free_gb_after", "free_gb_before", "host", "mode", "stages"]
        );
        let mut stage: Vec<&str> = report["stages"][0]
            .as_object()
            .expect("a stage row")
            .keys()
            .map(String::as_str)
            .collect();
        stage.sort_unstable();
        assert_eq!(stage, ["free_gb_after", "free_gb_before", "items", "stage"]);
    }

    /// The two modes differ by the mode flag and by nothing else, so a preview
    /// walks the paths an apply removes.
    #[test]
    fn both_modes_run_the_same_program() {
        let preview = remote_script(false, &[], DEFAULT_WORK_ROOTS);
        let apply = remote_script(true, &[], DEFAULT_WORK_ROOTS);
        assert_eq!(preview.replace("apply=0", "apply=1"), apply);
        // Every root the program touches is a crate constant, spliced in.
        assert!(apply.contains(BUILD_WORK_ROOT));
        assert!(apply.contains(SERVICES_ROOT));
        assert!(apply.contains(chromium_clones::CLONE_ROOT_NAME));
        assert!(apply.contains(chromium_clones::CLONE_ENTRY_PREFIX));
        assert!(apply.contains(CONTAINER_PREFIX));
        assert!(!apply.contains(AGE_DAYS_MARK));
        assert!(!apply.contains(WC_WORDS_MARK));
        assert!(!apply.contains(CLONE_ROOT_MARK));
        assert!(!apply.contains(CLONE_CONTAINER_MARK));
        assert!(!apply.contains(CLONE_PREFIX_MARK));
        assert!(!apply.contains(CONTAINER_PREFIX_MARK));
        // The removal is the only place `rm` appears, and it is behind the
        // mode flag.
        assert_eq!(apply.matches("/bin/rm").count(), 1);
    }

    /// A dry run's program cannot remove anything: the one `rm` is inside the
    /// `apply` branch and the flag is off.
    #[test]
    fn a_dry_run_program_deletes_nothing() {
        let preview = remote_script(false, &[], DEFAULT_WORK_ROOTS);
        assert!(preview.contains("apply=0"));
        let removal = preview
            .find("/bin/rm")
            .expect("the removal is in the program");
        let guard = preview[..removal]
            .rfind("[ \"$apply\" = 1 ]")
            .expect("the removal is guarded by the mode flag");
        assert!(guard < removal);
    }
}
