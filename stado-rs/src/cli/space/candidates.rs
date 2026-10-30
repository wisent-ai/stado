//! `stado space candidates`: what holds this machine's disk, ranked as the
//! next cleanup candidates, and what the janitor last removed.
//!
//! It reads; it never deletes. The janitor's pass at the disk-full rule
//! takes the `automatic` rows; every `proposal` is for the person at the
//! machine (or an operator) to decide.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::coverage::gib;
use super::report::print_json;
use crate::cli::CmdError;
use crate::providers::local::space_reclaim::candidates::{self, ledger, Class, Inputs};
use crate::providers::local::space_reclaim::rule::{self, read_fleet_volume};
use crate::providers::local::space_reclaim::state_relative_path;

/// How many of the janitor's newest removals the answer carries.
const REMOVALS_SHOWN: usize = 20;

/// This machine's registry entry, read without asking the registry
/// authority: what the janitor deletes in, and which consent-gated folders
/// this program may open. A machine whose entry cannot be read is treated as
/// the janitor treats it: shared, opening no gated folder; the answer says
/// why.
async fn this_host(home: &Path) -> (Option<PathBuf>, BTreeSet<String>, Option<String>) {
    let stado = home.join(".stado");
    let registry = match crate::targets::registry_without_authority(
        "ranking this machine's cleanup candidates",
    )
    .await
    {
        Ok((registry, _notice)) => registry,
        Err(error) => {
            return (
                Some(stado),
                BTreeSet::new(),
                Some(format!(
                    "this machine's registry entry could not be read ({error}), so, as the \
                     janitor does, only Stado's own tree is counted as the janitor's to take"
                )),
            )
        }
    };
    let hostname = crate::providers::vast::system_hostname();
    let target = match registry.lookup_self(&hostname) {
        Ok(Some(target)) => target,
        Ok(None) => {
            return (
                Some(stado),
                BTreeSet::new(),
                Some(format!(
                    "no registry target names this machine ({hostname}), so, as the janitor \
                     does, only Stado's own tree is counted as the janitor's to take"
                )),
            )
        }
        Err(error) => {
            return (
                Some(stado),
                BTreeSet::new(),
                Some(format!(
                    "this machine's registry entry is ambiguous ({error}), so, as the janitor \
                     does, only Stado's own tree is counted as the janitor's to take"
                )),
            )
        }
    };
    let admitted = match target.privacy_grants() {
        Ok(grants) => crate::targets::folders_for_this_program(&grants, home),
        Err(_) => BTreeSet::new(),
    };
    match target.ownership() {
        Ok(crate::targets::HostOwnership::Dedicated) => (None, admitted, None),
        Ok(crate::targets::HostOwnership::Shared) => (Some(stado), admitted, None),
        Err(refused) => (
            Some(stado),
            admitted,
            Some(format!(
                "{refused}; as the janitor does, only Stado's own tree is counted as the \
                 janitor's to take"
            )),
        ),
    }
}

pub(super) async fn candidates(min_bytes: u64, json_output: bool) -> Result<(), CmdError> {
    let home = crate::config_file::home().map_err(|cause| {
        CmdError::declaration(format!(
            "the account's home cannot be named, so nothing can be ranked: {cause}; set HOME to \
             this account's home directory"
        ))
        .machine_readable(json_output)
    })?;
    let min_bytes = i64::try_from(min_bytes).map_err(|_| {
        CmdError::usage(format!(
            "--min-bytes {min_bytes} is larger than any disk; give a size in bytes up to {}",
            i64::MAX
        ))
        .machine_readable(json_output)
    })?;
    // The home is checked before anything under it is read, so a home that
    // is not one is refused as that, not as the first file it lacks.
    candidates::check_home(&home)
        .map_err(|error| CmdError::declaration(error).machine_readable(json_output))?;
    // The janitor's state directory, beside its state file.
    let state_dir = match home.join(state_relative_path()).parent() {
        Some(directory) => directory.to_path_buf(),
        None => home.clone(),
    };
    // The record is read before the walk, so a damaged one is refused at once.
    let removed = ledger::newest(&state_dir, REMOVALS_SHOWN)
        .map_err(|error| CmdError::declaration(error).machine_readable(json_output))?;
    let (deletable, admitted, ownership_note) = this_host(&home).await;
    let survey = candidates::survey(&Inputs {
        home: &home,
        admitted: &admitted,
        deletable: deletable.as_deref(),
        min_bytes,
    })
    .map_err(|error| CmdError::declaration(error).machine_readable(json_output))?;
    let volume = read_fleet_volume(&home).map_err(|error| {
        CmdError::click(format!(
            "the volume holding {} cannot be measured, so the rule's verdict cannot be given: \
             {}; the home must be on a mounted volume this account can read",
            home.display(),
            error.message
        ))
        .machine_readable(json_output)
    })?;
    // Bytes to free before the volume is under the threshold again.
    let need_bytes = (-volume.headroom_bytes()).max(0);
    let rows = candidates::rows_json(&survey.candidates);
    let document = json!({
        "home": home,
        "rule": rule::rule_json(Some(volume)),
        "total_bytes": volume.total_bytes,
        "free_bytes": volume.free_bytes,
        "headroom_bytes": volume.headroom_bytes(),
        "need_bytes": need_bytes,
        "min_bytes": min_bytes,
        "classes": survey.totals.iter().map(|(class, listed, bytes)| json!({
            "class": class.word(),
            "listed": listed,
            "bytes": bytes,
        })).collect::<Vec<_>>(),
        "candidates": rows,
        "unmeasured": survey.unmeasured.iter().map(|row| json!({
            "path": row.path,
            "reason": row.reason,
        })).collect::<Vec<_>>(),
        "ownership_note": ownership_note,
        "removed": removed,
        "removal_record": ledger::ledger_path(&state_dir),
    });
    if json_output {
        return print_json(&document);
    }
    print_text(&document, &survey.candidates, volume, need_bytes);
    Ok(())
}

fn print_text(
    document: &Value,
    rows: &[candidates::Candidate],
    volume: rule::VolumeReading,
    need_bytes: i64,
) {
    println!(
        "volume {:.1}% used, rule at {}%: {}",
        volume.used_percent(),
        rule::DISK_FULL_PERCENT,
        if volume.full() {
            format!("{} to free before it is under the rule", gib(need_bytes))
        } else {
            format!("{} may still be written", gib(volume.headroom_bytes()))
        }
    );
    if let Some(note) = document["ownership_note"].as_str() {
        println!("note: {note}");
    }
    let mut cumulative = 0i64;
    let mut class = None;
    for row in rows {
        if class != Some(row.class) {
            class = Some(row.class);
            println!();
            println!(
                "{}",
                match row.class {
                    Class::AgentOutput => "agent output (rebuildable; the janitor takes the automatic rows at the rule)",
                    Class::Cache => "caches (fetched or compiled again)",
                    Class::UserData => "user data (proposals only; nothing in Stado deletes them)",
                }
            );
        }
        cumulative = cumulative.saturating_add(row.bytes);
        let reached =
            if need_bytes > 0 && cumulative - row.bytes < need_bytes && cumulative >= need_bytes {
                "  <- enough to be under the rule"
            } else {
                ""
            };
        println!(
            "  {:>10}  {:<9}  {:<11}  {}  ({}){reached}",
            gib(row.bytes),
            if row.cleaner.is_some() {
                "automatic"
            } else {
                "proposal"
            },
            row.safety.word(),
            row.path.display(),
            row.producer,
        );
    }
    if let Some(unmeasured) = document["unmeasured"]
        .as_array()
        .filter(|rows| !rows.is_empty())
    {
        println!();
        println!("not measured:");
        for row in unmeasured {
            println!(
                "  {}: {}",
                row["path"].as_str().unwrap_or_default(),
                row["reason"].as_str().unwrap_or_default()
            );
        }
    }
    let removed = document["removed"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    println!();
    if removed.is_empty() {
        println!("the janitor's removal record holds no removal yet");
    }
    for line in removed {
        let what = match line["path"].as_str() {
            Some(path) => path.to_string(),
            None => format!("{} item(s)", line["items"]),
        };
        println!(
            "removed {}  {}  {}  {what}",
            line["removed_at"].as_str().unwrap_or_default(),
            line["cleaner"].as_str().unwrap_or_default(),
            gib(line["bytes"].as_i64().unwrap_or_default()),
        );
    }
}
