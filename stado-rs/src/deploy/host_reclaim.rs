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
//! Three stages, in this order, and nothing else:
//!
//! 1. `registry_cleanup` — the host's OWN janitor
//!    ([`crate::providers::local::disk_cleanup`]), invoked exactly the way
//!    [`crate::deploy::host_cleanup`] invokes it, so the policy stays the one
//!    the registry declares and this module contains no cleanup policy of its
//!    own. `--dry-run` runs its planning phase; `--apply` runs the enforcing
//!    pass. The item count is the janitor's own.
//! 2. `build_scratch` — `$HOME/`[`BUILD_WORK_ROOT`], the release build scratch
//!    tree. `scripts/build-stado-linux-host.sh` and the reproduce helper both
//!    work there and neither removes what it wrote; a from-scratch release
//!    build leaves its checkout and its vendored sources behind every time.
//! 3. `delivered_trees` — the version directories under `$HOME/`[`SERVICES_ROOT`],
//!    where every `service deploy` and every artifact install stages one tree
//!    per version and keeps the previous one beside it as
//!    `current.before-<version>` so a rollback is a rename. Nothing has ever
//!    removed the ones a rollback will never reach.
//!
//! Five rules, encoded here rather than left to whoever is at the keyboard:
//!
//! - **nothing outside those roots.** Every candidate is produced by globbing
//!   one of the two declared roots; no path arrives from the registry, from the
//!   operator, or from the host's own output.
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
use super::{shlex_quote, DeployError, Runner};
use crate::deploy::artifact_install::SERVICES_ROOT;
use crate::targets::ComputeTarget;

/// The release build scratch tree, relative to the target account's home.
///
/// Its own root under `.stado`, and the one the fleet's checked-in build
/// helpers already use (`scripts/build-stado-linux-host.sh`,
/// `scripts/reproduce-release-build-host.sh`): a stage that reclaimed a
/// directory those helpers do not write would be reclaiming something else.
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
if [ -d "$services" ]; then
  for product in "$services"/*; do
    [ -d "$product" ] || continue
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
  done
fi
printf 'STADO_RECLAIM_STAGE\tdelivered_trees\t%s\t%s\n' "$before" "$(free_kb)"

printf 'STADO_RECLAIM_FREE\tafter\t%s\n' "$(free_kb)"
"#;

/// The remote program for one mode, with every substitution in place.
///
/// The stado candidates are quoted exactly the way
/// [`crate::deploy::host_recovery::remote_script`] quotes them, so `$HOME`
/// still expands on the remote side while the word stays one word.
pub fn remote_script(apply: bool) -> String {
    let wc_words = WC_CANDIDATES
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<String>>()
        .join(" ");
    REMOTE_SCRIPT_TEMPLATE
        .replace(APPLY_MARK, if apply { "1" } else { "0" })
        .replace(WC_WORDS_MARK, &wc_words)
        .replace(SERVICES_ROOT_MARK, SERVICES_ROOT)
        .replace(BUILD_WORK_MARK, BUILD_WORK_ROOT)
        .replace(AGE_DAYS_MARK, MIN_AGE_DAYS)
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
pub async fn record_audit(
    target: &ComputeTarget,
    reclamation: &Reclamation,
    reason: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    let record = json!({
        "at": crate::models::isoformat_utc(chrono::Utc::now()),
        "command": "stado host reclaim",
        "mode": reclamation.mode,
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
    let output = host_channel::run_script(&target, &remote_script(apply), runner).await?;
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
    fn run_locally(home: &Path, apply: bool) -> String {
        let mut child = Command::new("/bin/bash")
            .arg("-s")
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("bash");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(remote_script(apply).as_bytes())
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

    /// A scratch host carrying exactly what the two filesystem stages have to
    /// decide about: one stale build tree and one fresh one, three delivered
    /// versions of one product with `current` on the middle one, and — beside
    /// them — the dotted state backup this control plane's own host actually
    /// carries, which is older than every gate and must still survive.
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
        (home, product)
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

    fn target() -> ComputeTarget {
        serde_json::from_value(json!({
            "name": "charless-mac-mini",
            "kind": "local",
            "ssh": "charles@charless-mac-mini.local",
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
    /// order with the janitor's own count and both filesystem stages' paths.
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
        let preview = remote_script(false);
        let apply = remote_script(true);
        assert_eq!(preview.replace("apply=0", "apply=1"), apply);
        // Every root the program touches is a crate constant, spliced in.
        assert!(apply.contains(BUILD_WORK_ROOT));
        assert!(apply.contains(SERVICES_ROOT));
        assert!(!apply.contains(AGE_DAYS_MARK));
        assert!(!apply.contains(WC_WORDS_MARK));
        // The removal is the only place `rm` appears, and it is behind the
        // mode flag.
        assert_eq!(apply.matches("/bin/rm").count(), 1);
    }

    /// A dry run's program cannot remove anything: the one `rm` is inside the
    /// `apply` branch and the flag is off.
    #[test]
    fn a_dry_run_program_deletes_nothing() {
        let preview = remote_script(false);
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
