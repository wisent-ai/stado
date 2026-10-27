//! Whether the destination has room for the states a move writes, asked
//! before the source is fenced.
//!
//! A destination whose volume cannot hold the states failed the move at its
//! first write, after the source was fenced, and the service stood down for
//! the whole rollback. Both figures are measured on the hosts (`du -sk` of
//! each state on the source, `df -Pk` of the nearest existing directory of
//! its path on the destination), so nothing here states a size of its own.

use super::{full_path_lines, root_payload, state_location};
use crate::cli::placement::transfer::{marker_line, run_host_script, MoveContext};
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::placement::{PlacementState, StateRoot};
use crate::targets::ComputeTarget;

const SIZE: &str = "STADO_PLACEMENT_SIZE\t";
const FREE: &str = "STADO_PLACEMENT_FREE\t";

/// The kibibytes the figure after `marker` on its line states.
fn figure(output: &crate::deploy::CommandOutput, marker: &str, host: &str) -> Result<u64, CmdError> {
    let line = marker_line(output, marker).ok_or_else(|| {
        CmdError::click(format!("{host}: the space reading returned no {marker:?} line"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let text = line[marker.len()..].trim();
    text.parse().map_err(|error| {
        CmdError::click(format!("{host}: the space reading {text:?} is not a number: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}

/// The kibibytes `state` takes on the source; a state that is absent takes
/// none, which `du` cannot say of a missing path.
async fn source_kib(source: &ComputeTarget, state: &PlacementState, runner: &Runner) -> Result<u64, CmdError> {
    let locate = full_path_lines(&root_payload(source, state)?, &state.path);
    let script = format!(
        r#"set -eu
set -o pipefail
{locate}
if [ -e "$full" ]; then printf '{SIZE}%s\n' "$(/usr/bin/du -sk "$full" | /usr/bin/cut -f1)";
else printf '{SIZE}0\n'; fi
"#
    );
    let output = run_host_script(source, &script, runner, "state size").await?;
    figure(&output, SIZE, &source.name)
}

/// The kibibytes free on the destination volume `state` would be written to:
/// the nearest directory of its path that exists, since a first move creates
/// the rest.
async fn destination_free_kib(
    destination: &ComputeTarget,
    state: &PlacementState,
    runner: &Runner,
) -> Result<u64, CmdError> {
    let locate = full_path_lines(&root_payload(destination, state)?, &state.path);
    let script = format!(
        r#"set -eu
set -o pipefail
{locate}
d="$full"
while [ ! -e "$d" ]; do d=$(/usr/bin/dirname "$d"); done
printf '{FREE}%s\n' "$(/bin/df -Pk "$d" | /usr/bin/awk 'NR==2 {{print $4}}')"
"#
    );
    let output = run_host_script(destination, &script, runner, "free space").await?;
    figure(&output, FREE, &destination.name)
}

/// Refuse the move when the states written under one root of the destination
/// take more than that root's volume has free, naming both figures and the
/// states.
pub(in crate::cli::placement::transfer) async fn ensure_room(
    context: &MoveContext,
    runner: &Runner,
) -> Result<(), CmdError> {
    let ends = Ends {
        source: &context.source,
        destination: &context.destination,
        registry: &context.registry,
    };
    ensure_room_for(&ends, &context.profile.state, runner).await
}

/// The hosts a copy runs between, and the registry that names this machine.
pub(in crate::cli::placement::transfer) struct Ends<'a> {
    pub source: &'a ComputeTarget,
    pub destination: &'a ComputeTarget,
    pub registry: &'a crate::targets::Registry,
}

/// [`ensure_room`] for `states` copied between `ends`, for a copy that is no
/// profile's move (a role move carries the store a profile cannot).
pub(in crate::cli::placement::transfer) async fn ensure_room_for(
    ends: &Ends<'_>,
    states: &[PlacementState],
    runner: &Runner,
) -> Result<(), CmdError> {
    let sizes = states.iter().map(|state| source_kib(ends.source, state, runner));
    let sizes = futures::future::join_all(sizes).await;
    let mut measured: Vec<(&PlacementState, u64)> = Vec::with_capacity(states.len());
    for (state, size) in states.iter().zip(sizes) {
        measured.push((state, size?));
    }
    let trees: Vec<&(&PlacementState, u64)> = measured.iter().filter(|(state, _)| state.tree).collect();
    let tree_need: u64 = trees.iter().map(|(_, size)| size).sum();
    let tree_names: Vec<String> = trees.iter().map(|(state, _)| state_location(state)).collect();
    let mut roots: Vec<(StateRoot, u64, Vec<String>, &PlacementState)> = Vec::new();
    for &(state, size) in &measured {
        match roots.iter_mut().find(|(root, ..)| *root == state.root) {
            Some((_, need, names, _)) => {
                *need = need.saturating_add(size);
                names.push(state_location(state));
            }
            None => roots.push((state.root, size, vec![state_location(state)], state)),
        }
    }
    let free = roots
        .iter()
        .map(|(_, _, _, first)| destination_free_kib(ends.destination, first, runner));
    let free = futures::future::join_all(free).await;
    for ((_, need, names, _), free) in roots.iter().zip(free) {
        let free = free?;
        if *need > free {
            return Err(CmdError::refused(format!(
                "{}: the volume the move writes {} to has {free} KiB free, and they take {need} \
                 KiB on {}; free space there, or declare a larger work root, before moving",
                ends.destination.name,
                names.join(", "),
                ends.source.name
            )));
        }
    }
    staging_room(ends, runner, tree_need, tree_names).await
}

/// A tree is copied through this machine (`sync_directory` pulls it into
/// [`super::tree::staging_base`] and pushes it on), so the trees need room
/// here too. This machine is asked like any host when the registry names
/// it; a machine the registry does not name cannot be asked, which is said.
async fn staging_room(
    ends: &Ends<'_>,
    runner: &Runner,
    need: u64,
    names: Vec<String>,
) -> Result<(), CmdError> {
    if names.is_empty() {
        return Ok(());
    }
    let Some(here) = ends
        .registry
        .targets
        .iter()
        .find(|target| crate::deploy::host_channel::target_is_this_host(target))
    else {
        eprintln!(
            "stado: this machine is no registry target, so whether it has room to stage {} \
             between the hosts is not measured",
            names.join(", ")
        );
        return Ok(());
    };
    let base = super::tree::staging_base()?;
    let encoded = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        base.to_string_lossy().as_bytes(),
    );
    let script = format!(
        r#"set -eu
set -o pipefail
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
d=$(printf '%s' '{encoded}' | /usr/bin/base64 "$decode")
while [ ! -e "$d" ]; do d=$(/usr/bin/dirname "$d"); done
printf '{FREE}%s\n' "$(/bin/df -Pk "$d" | /usr/bin/awk 'NR==2 {{print $4}}')"
"#
    );
    let output = run_host_script(here, &script, runner, "staging space").await?;
    let free = figure(&output, FREE, &here.name)?;
    if need > free {
        return Err(CmdError::refused(format!(
            "{}: {} is copied through {} on this machine, which has {free} KiB free, and it \
             takes {need} KiB on {}; free space there, or declare a larger work root for this \
             machine, before moving",
            here.name,
            names.join(", "),
            base.display(),
            ends.source.name
        )));
    }
    Ok(())
}
