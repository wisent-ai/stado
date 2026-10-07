//! A tree state: a directory carried whole, for a store too large to read
//! into one snapshot. The fenced source's tree is copied into a staging tree
//! on this machine, then into a stage beside the destination path, and one
//! rename puts it in place; the destination's previous tree is renamed to the
//! same backup a file state uses, so the rollback and the cleanup in the
//! parent module restore or remove it the same way.

use std::path::PathBuf;

use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::{full_path_lines, root_payload};
use crate::cli::placement::transfer::{marker_line, run_host_script, StateSnapshot};
use crate::cli::CmdError;
use crate::deploy::host_delivery::{sync_directory, Direction};
use crate::deploy::{host_channel, Runner};
use crate::placement::{PlacementState, StateRoot};
use crate::targets::ComputeTarget;

const PRESENT: &str = "STADO_PLACEMENT_TREE\tpresent";
const MISSING: &str = "STADO_PLACEMENT_TREE\tmissing";
const STAGED: &str = "STADO_PLACEMENT_TREE\tstaged";
const WRITTEN: &str = "STADO_PLACEMENT_WRITE\tok";

/// The state's absolute path on `target`.
async fn absolute(
    target: &ComputeTarget,
    state: &PlacementState,
    runner: &Runner,
) -> Result<String, CmdError> {
    let root = match state.root {
        StateRoot::Home => host_channel::remote_home(target, runner)
            .await
            .map_err(CmdError::from)?,
        StateRoot::Work => {
            root_payload(target, state)?;
            target.work_root.clone().unwrap_or_default()
        }
    };
    Ok(format!("{}/{}", root.trim_end_matches('/'), state.path))
}

/// Where this machine keeps a tree between the source and the destination:
/// under its own declared work root, or `~/.stado/placement`, one directory
/// per transaction and state.
fn staging(transaction_id: &str, state: &PlacementState) -> PathBuf {
    let base = crate::providers::local::work_base::declared()
        .map(|root| root.join("placement"))
        .unwrap_or_else(|| crate::config_file::expand_tilde("~/.stado/placement"));
    base.join(transaction_id)
        .join(state.path.replace('/', "%2F"))
}

/// Whether the source holds the tree, refusing a path that is not a
/// directory.
pub(super) async fn tree_exists(
    target: &ComputeTarget,
    state: &PlacementState,
    runner: &Runner,
) -> Result<bool, CmdError> {
    let locate = full_path_lines(&root_payload(target, state)?, &state.path);
    let script = format!(
        r#"set -eu
{locate}
if [ -d "$full" ]; then printf '%s\n' '{PRESENT}';
elif [ -e "$full" ]; then printf '%s is not a directory\n' "$full" >&2; false;
else printf '%s\n' '{MISSING}'; fi
"#
    );
    let output = run_host_script(target, &script, runner, "tree preflight").await?;
    Ok(marker_line(&output, PRESENT).is_some())
}

/// Copy the fenced source's tree to this machine.
pub(super) async fn read_tree(
    target: &ComputeTarget,
    state: &PlacementState,
    transaction_id: &str,
    runner: &Runner,
) -> Result<StateSnapshot, CmdError> {
    if !tree_exists(target, state, runner).await? {
        if state.required {
            return Err(CmdError::click(format!(
                "{}: required state {} disappeared after fencing",
                target.name, state.path
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound));
        }
        return Ok(StateSnapshot {
            spec: state.clone(),
            bytes: None,
            tree: None,
        });
    }
    let local = staging(transaction_id, state);
    let remote = absolute(target, state, runner).await?;
    sync_directory(target, &remote, &local, Direction::Pull, runner)
        .await
        .map_err(CmdError::from)?;
    Ok(StateSnapshot {
        spec: state.clone(),
        bytes: None,
        tree: Some(local),
    })
}

/// Put the tree in place on the destination: the previous tree becomes the
/// backup the rollback restores, the copy is staged beside the path and
/// renamed over it.
pub(super) async fn write_tree(
    target: &ComputeTarget,
    snapshot: &StateSnapshot,
    transaction_id: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let state = &snapshot.spec;
    let locate = full_path_lines(&root_payload(target, state)?, &state.path);
    let transaction = STANDARD.encode(transaction_id.as_bytes());
    let prepare = format!(
        r#"set -eu
umask 077
{locate}
txn=$(printf '%s' '{transaction}' | /usr/bin/base64 "$decode")
backup="$full.pre-stado-placement-$txn"
meta="$backup.meta"
stage="$full.placement-$txn.stage"
/bin/mkdir -p "$(/usr/bin/dirname "$full")"
if [ -e "$backup" ] || [ -e "$meta" ]; then printf 'placement backup already exists: %s\n' "$backup" >&2; false; fi
had=no
if [ -d "$full" ]; then had=yes;
elif [ -e "$full" ]; then printf '%s is not a directory\n' "$full" >&2; false; fi
printf '%s\n' "$had" > "$meta"
/bin/rm -rf "$stage"
/bin/mkdir -p "$stage"
printf '%s\n' '{STAGED}'
"#
    );
    let output = run_host_script(target, &prepare, runner, "tree stage").await?;
    if marker_line(&output, STAGED).is_none() {
        return Err(CmdError::click(format!(
            "{}: staging the tree {} returned no marker",
            target.name, state.path
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let present = if let Some(local) = &snapshot.tree {
        let stage = format!(
            "{}.placement-{transaction_id}.stage",
            absolute(target, state, runner).await?
        );
        sync_directory(target, &stage, local, Direction::Push, runner)
            .await
            .map_err(CmdError::from)?;
        "yes"
    } else {
        "no"
    };
    let commit = format!(
        r#"set -eu
{locate}
txn=$(printf '%s' '{transaction}' | /usr/bin/base64 "$decode")
backup="$full.pre-stado-placement-$txn"
stage="$full.placement-$txn.stage"
[ ! -d "$full" ] || /bin/mv "$full" "$backup"
if [ '{present}' = yes ]; then /bin/mv "$stage" "$full"; else /bin/rm -rf "$stage"; fi
printf '%s\n' '{WRITTEN}'
"#
    );
    let output = run_host_script(target, &commit, runner, "tree install").await?;
    if marker_line(&output, WRITTEN).is_none() {
        return Err(CmdError::click(format!(
            "{}: installing the tree {} returned no marker",
            target.name, state.path
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    if let Some(local) = &snapshot.tree {
        std::fs::remove_dir_all(local).map_err(|error| {
            CmdError::click(format!(
                "the tree {} is installed on {}, and its staging copy {} could not be removed: \
                 {error}",
                state.path,
                target.name,
                local.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    Ok(())
}
