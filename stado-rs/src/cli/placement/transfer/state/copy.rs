//! How a state travels: the source's file or directory is copied into a
//! staging copy on this machine, then into a stage beside the destination
//! path, and one rename puts it in place; the destination's previous file or
//! tree is renamed to the backup the rollback restores and the settle step
//! removes.
//!
//! Every copy goes through rsync as a file, never through a script's text: a
//! state carried inside a script is held whole in memory on both ends and is
//! bounded by the shell's largest string.
//!
//! A move reads each state twice into the same staging copy: once while the
//! source still serves and once after it is fenced. The second read compares
//! by content, so only what changed in between is sent while the service is
//! down.

use std::path::PathBuf;

use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::{full_path_lines, root_payload};
use crate::cli::placement::transfer::{marker_line, run_host_script, StateSnapshot};
use crate::cli::CmdError;
use crate::deploy::host_delivery::{sync_path, Compare, Copied, Direction};
use crate::deploy::{host_channel, Runner};
use crate::placement::{PlacementState, StateRoot};
use crate::targets::ComputeTarget;

const PRESENT: &str = "STADO_PLACEMENT_STATE\tpresent";
const MISSING: &str = "STADO_PLACEMENT_STATE\tmissing";
const STAGED: &str = "STADO_PLACEMENT_STATE\tstaged";
const WRITTEN: &str = "STADO_PLACEMENT_WRITE\tok";

/// What the state is on disk, and the shell test that tells it.
fn kind(state: &PlacementState) -> (Copied, &'static str, &'static str) {
    if state.tree {
        (Copied::Tree, "-d", "a directory")
    } else {
        (Copied::File, "-f", "a regular file")
    }
}

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
        // A host that declares no work root cannot hold a work-rooted state.
        StateRoot::Work => match &target.work_root {
            Some(work_root) => work_root.clone(),
            None => return Err(super::no_work_root(target, state)),
        },
    };
    Ok(format!("{}/{}", root.trim_end_matches('/'), state.path))
}

/// The directory this machine keeps state copies in between the source and
/// the destination: under its own declared work root, or `~/.stado/placement`
/// under the home every Stado file is kept under. A home neither HOME nor
/// the user database names is the refusal, where this process used to stop.
pub(super) fn staging_base() -> Result<PathBuf, CmdError> {
    Ok(match crate::providers::local::work_base::declared() {
        Some(root) => root.join("placement"),
        None => crate::config_file::home()
            .map_err(CmdError::declaration)?
            .join(".stado")
            .join("placement"),
    })
}

/// The directory of one transaction's copies under [`staging_base`].
fn transaction_staging(transaction_id: &str) -> Result<PathBuf, CmdError> {
    Ok(staging_base()?.join(transaction_id))
}

/// Where this machine keeps one state's copy: one path per transaction and
/// state.
fn staging(transaction_id: &str, state: &PlacementState) -> Result<PathBuf, CmdError> {
    Ok(transaction_staging(transaction_id)?.join(state.path.replace('/', "%2F")))
}

/// Remove every copy `transaction_id` staged on this machine, as a move that
/// ended does: a rolled-back move leaves nothing to resume from, since a
/// later move is a new transaction.
pub(in crate::cli::placement) fn discard_staging(
    transaction_id: &str,
) -> Result<(), CmdError> {
    let directory = transaction_staging(transaction_id)?;
    match std::fs::remove_dir_all(&directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CmdError::click(format!(
            "the copies placement transaction {transaction_id} staged at {} could not be \
             removed: {error}",
            directory.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))),
    }
}

/// Whether `target` holds the state, refusing a path that is not the kind
/// the state declares.
pub(in crate::cli::placement::transfer) async fn state_exists(
    target: &ComputeTarget,
    state: &PlacementState,
    runner: &Runner,
) -> Result<bool, CmdError> {
    let (_, test, described) = kind(state);
    let locate = full_path_lines(&root_payload(target, state)?, &state.path);
    let script = format!(
        r#"set -eu
{locate}
if [ {test} "$full" ]; then printf '%s\n' '{PRESENT}';
elif [ -e "$full" ]; then printf '%s is not {described}\n' "$full" >&2; exit 65;
else printf '%s\n' '{MISSING}'; fi
"#
    );
    let output = run_host_script(target, &script, runner, "state preflight").await?;
    Ok(marker_line(&output, PRESENT).is_some())
}

/// Copy the source's state into its staging copy on this machine. A copy
/// this transaction already took (before the fence) is compared by content,
/// so a file rewritten within the second of that copy is carried.
pub(in crate::cli::placement::transfer) async fn read_state(
    target: &ComputeTarget,
    state: &PlacementState,
    transaction_id: &str,
    runner: &Runner,
) -> Result<StateSnapshot, CmdError> {
    if !state_exists(target, state, runner).await? {
        if state.required {
            return Err(CmdError::click(format!(
                "{}: required state {} is absent on the source",
                target.name, state.path
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound));
        }
        return Ok(StateSnapshot {
            spec: state.clone(),
            copy: None,
        });
    }
    let local = staging(transaction_id, state)?;
    let staged = local.try_exists().map_err(|error| {
        CmdError::click(format!(
            "cannot tell whether {} is staged at {}: {error}",
            state.path,
            local.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let compare = if staged {
        Compare::Content
    } else {
        Compare::Quick
    };
    let (copied, ..) = kind(state);
    let remote = absolute(target, state, runner).await?;
    sync_path(
        target,
        &remote,
        &local,
        copied,
        Direction::Pull,
        compare,
        runner,
    )
    .await
    .map_err(CmdError::from)?;
    Ok(StateSnapshot {
        spec: state.clone(),
        copy: Some(local),
    })
}

/// Put the state in place on the destination: the previous file or tree
/// becomes the backup the rollback restores, the copy is staged beside the
/// path and renamed over it. A file state is installed owner-only.
pub(in crate::cli::placement::transfer) async fn write_state(
    target: &ComputeTarget,
    snapshot: &StateSnapshot,
    transaction_id: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let state = &snapshot.spec;
    let (copied, test, described) = kind(state);
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
if [ -e "$backup" ] || [ -e "$meta" ]; then printf 'placement backup already exists: %s\n' "$backup" >&2; exit 73; fi
had=no
if [ {test} "$full" ]; then had=yes;
elif [ -e "$full" ]; then printf '%s is not {described}\n' "$full" >&2; exit 65; fi
printf '%s\n' "$had" > "$meta"
/bin/rm -rf "$stage"
printf '%s\n' '{STAGED}'
"#
    );
    let output = run_host_script(target, &prepare, runner, "state stage").await?;
    if marker_line(&output, STAGED).is_none() {
        return Err(CmdError::click(format!(
            "{}: staging {} returned no marker",
            target.name, state.path
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let present = if let Some(local) = &snapshot.copy {
        let stage = format!(
            "{}.placement-{transaction_id}.stage",
            absolute(target, state, runner).await?
        );
        sync_path(
            target,
            &stage,
            local,
            copied,
            Direction::Push,
            Compare::Quick,
            runner,
        )
        .await
        .map_err(CmdError::from)?;
        "yes"
    } else {
        "no"
    };
    let owner_only = match copied {
        Copied::File => r#"/bin/chmod 600 "$stage""#,
        Copied::Tree => ":",
    };
    let commit = format!(
        r#"set -eu
{locate}
txn=$(printf '%s' '{transaction}' | /usr/bin/base64 "$decode")
backup="$full.pre-stado-placement-$txn"
stage="$full.placement-$txn.stage"
[ ! -e "$full" ] || /bin/mv "$full" "$backup"
if [ '{present}' = yes ]; then {owner_only}; /bin/mv "$stage" "$full"; else /bin/rm -rf "$stage"; fi
printf '%s\n' '{WRITTEN}'
"#
    );
    let output = run_host_script(target, &commit, runner, "state install").await?;
    if marker_line(&output, WRITTEN).is_none() {
        return Err(CmdError::click(format!(
            "{}: installing {} returned no marker",
            target.name, state.path
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    if let Some(local) = &snapshot.copy {
        let removed = match copied {
            Copied::Tree => std::fs::remove_dir_all(local),
            Copied::File => std::fs::remove_file(local),
        };
        removed.map_err(|error| {
            CmdError::click(format!(
                "{} is installed on {}, and its staging copy {} could not be removed: {error}",
                state.path,
                target.name,
                local.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    Ok(())
}
