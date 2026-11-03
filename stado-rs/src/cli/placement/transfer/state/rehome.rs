//! `stado placement rehome`: the home rewrite a move makes in each home-rooted
//! file state it carries, made on states already in place on the host a
//! profile runs on. A move made by a Stado that did not rewrite them left every
//! such file naming the source account's home; this is how that move is
//! completed, through the same read, rewrite and staged install a move uses,
//! with the file it replaces kept beside it.

use serde_json::{json, Value};

use super::copy::{read_state, rehome, write_state};
use super::settle::settle_state_backup;
use super::state_location;
use crate::cli::CmdError;
use crate::deploy::{host_channel, Runner};
use crate::placement::{PlacementProfile, StateRoot};
use crate::targets::ComputeTarget;

/// What happened to one state.
pub(in crate::cli::placement) struct Rehomed {
    pub location: String,
    pub outcome: &'static str,
    pub mentions: usize,
    pub kept: Option<String>,
}

impl Rehomed {
    pub(in crate::cli::placement) fn report(&self) -> Value {
        json!({"state": self.location, "outcome": self.outcome, "mentions": self.mentions,
            "kept": self.kept})
    }
}

/// Rewrite `from_home` to `target`'s own home in every home-rooted file state
/// of `profile` on `target`. A state the host does not hold, a directory, a
/// work-rooted state and a file naming no `from_home` are left as they are and
/// said so; `from_home` equal to the host's home is refused, since nothing
/// could be rewritten.
pub(in crate::cli::placement) async fn rehome_states(
    target: &ComputeTarget,
    profile: &PlacementProfile,
    from_home: &str,
    transaction_id: &str,
    runner: &Runner,
) -> Result<Vec<Rehomed>, CmdError> {
    let home = host_channel::remote_home(target, runner)
        .await
        .map_err(CmdError::from)?;
    if home.trim_end_matches('/') == from_home.trim_end_matches('/') {
        return Err(CmdError::refused(format!(
            "{} already runs as {from_home}; there is no other home to rewrite in {}'s states",
            target.name, profile.name
        )));
    }
    let mut done = Vec::new();
    for state in &profile.state {
        let location = state_location(state);
        if state.tree || state.root != StateRoot::Home {
            done.push(Rehomed { location, outcome: "carried_as_is", mentions: 0, kept: None });
            continue;
        }
        let mut snapshot = read_state(target, state, transaction_id, runner).await?;
        let Some(local) = snapshot.copy.clone() else {
            done.push(Rehomed { location, outcome: "absent", mentions: 0, kept: None });
            continue;
        };
        let mentions = rehome(&local, from_home, &home)?;
        if mentions == 0 {
            std::fs::remove_file(&local).map_err(|error| {
                CmdError::click(format!(
                    "{location} names no {from_home}, and its copy {} could not be removed: {error}",
                    local.display()
                ))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
            done.push(Rehomed { location, outcome: "unchanged", mentions: 0, kept: None });
            continue;
        }
        // Already rewritten: the install below must not rewrite it again.
        snapshot.source_home = None;
        write_state(target, &snapshot, transaction_id, runner).await?;
        let kept = settle_state_backup(target, state, transaction_id, runner).await?;
        eprintln!(
            "  {}: {mentions} mention(s) of {from_home} in {location} now name {home}; the previous file is kept at {}",
            target.name,
            kept.as_deref().unwrap_or("(none)")
        );
        done.push(Rehomed { location, outcome: "rewritten", mentions, kept });
    }
    Ok(done)
}
