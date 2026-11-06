//! The durable files and trees a profile declares: copied from the source
//! and installed on the destination behind a backup the rollback can
//! restore (`copy`).
//!
//! A state path is relative to its root on each host separately: the home of
//! the account Stado runs as, or the work root the registry declares for that
//! host, so a store kept under a large volume on one host lands under the
//! large volume of the other.

mod age;
mod copy;
mod rehome;
mod settle;
mod space;

pub(in crate::cli::placement::transfer) use age::state_newest;
pub(in crate::cli::placement) use copy::discard_staging;
pub(in crate::cli::placement::transfer) use copy::{read_state, state_exists, write_state};
pub(in crate::cli::placement) use rehome::rehome_states;
pub(in crate::cli::placement) use settle::settle_state_backup;
pub(in crate::cli::placement::transfer) use space::{ensure_room, ensure_room_for, Ends};

use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::{marker_line, run_host_script};
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::placement::{PlacementState, StateRoot};
use crate::targets::ComputeTarget;

fn payload(text: &str) -> String {
    STANDARD.encode(text.as_bytes())
}

/// How the state is written in messages: `$HOME/<path>` or
/// `<work root>/<path>`.
pub(super) fn state_location(state: &PlacementState) -> String {
    match state.root {
        StateRoot::Home => format!("$HOME/{}", state.path),
        StateRoot::Work => format!("<work root>/{}", state.path),
    }
}

/// The directory the state's path is under on `target`, as a payload the
/// host script decodes: empty for the home, which the script reads from the
/// account's own `$HOME`; the declared work root otherwise. A host that
/// declares no work root cannot hold a work-rooted state, and is refused
/// before anything on it is touched.
pub(super) fn root_payload(
    target: &ComputeTarget,
    state: &PlacementState,
) -> Result<String, CmdError> {
    match state.root {
        StateRoot::Home => Ok(String::new()),
        StateRoot::Work => match target.work_root.as_deref() {
            Some(work_root) => Ok(payload(work_root)),
            None => Err(no_work_root(target, state)),
        },
    }
}

/// The refusal for a work-rooted state on a host whose registry entry
/// declares no work root.
pub(super) fn no_work_root(target: &ComputeTarget, state: &PlacementState) -> CmdError {
    CmdError::click(format!(
        "{}: state {} is kept under the work root, and the registry declares none for \
         {}; declare targets.{}.work_root before moving it there",
        target.name, state.path, target.name, target.name
    ))
    .stating(crate::primitives::failure::FailureCode::Config)
}

/// The script lines that set `full` to the state's file on the host.
fn full_path_lines(root: &str, path: &str) -> String {
    format!(
        r#"case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
relative=$(printf '%s' '{path}' | /usr/bin/base64 "$decode")
root=$(printf '%s' '{root}' | /usr/bin/base64 "$decode")
[ -n "$root" ] || root="$HOME"
full="$root/$relative""#,
        path = payload(path)
    )
}

pub(super) async fn restore_state(
    target: &ComputeTarget,
    state: &PlacementState,
    transaction_id: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let path = &state.path;
    let locate = full_path_lines(&root_payload(target, state)?, path);
    let transaction = STANDARD.encode(transaction_id.as_bytes());
    let script = format!(
        r#"set -eu
{locate}
txn=$(printf '%s' '{transaction}' | /usr/bin/base64 "$decode")
backup="$full.pre-stado-placement-$txn"
meta="$backup.meta"
/bin/rm -rf "$full.placement-$txn.stage"
if [ ! -f "$meta" ]; then
  printf 'STADO_PLACEMENT_RESTORE\tok\tuntouched\n'
else
  had=$(/bin/cat "$meta")
  if [ "$had" = yes ]; then
    [ -e "$backup" ] || {{ printf 'placement backup is missing: %s\n' "$backup" >/dev/stderr; false; }}
    /bin/rm -rf "$full"
    /bin/mv "$backup" "$full"
  else
    /bin/rm -rf "$full"
  fi
  /bin/rm -f "$meta"
  printf 'STADO_PLACEMENT_RESTORE\tok\n'
fi
"#
    );
    let output = run_host_script(target, &script, runner, "state rollback").await?;
    if marker_line(&output, "STADO_PLACEMENT_RESTORE\tok").is_none() {
        return Err(CmdError::click(format!(
            "{}: state rollback returned no marker for {path}",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(())
}
