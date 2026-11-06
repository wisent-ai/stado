//! `stado product cancel PRODUCT --surface S`: a person ends the installation
//! operation `stado product status` names on a surface, where nothing newer
//! supersedes it: the holder of the surface's lock, or an installation still
//! running its after-install steps without the lock. The holder and every
//! process under it are told to stop, the lock is taken once the holder has
//! let go of it (the kernel releases a flock when its holder exits, before
//! its parent reaps it), and the receipt records what was cancelled and
//! which process asked.
//! A holder placing files is refused, as a superseding installation waits for
//! it, because a half-replaced surface is worse than a late one.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::catalog::text;
use crate::common::{now, Runtime, PHASE_PLACING};
use crate::state::{self, ProductState};

/// The receipt field the lock holder a person cancelled is recorded under.
const CANCELLED_HOLDER: &str = "operation_cancelled";

/// The process that asked: this one, by its pid and command line, as a lock
/// record names its holder.
fn asking() -> Result<Value> {
    Ok(json!({
        "pid": i32::try_from(std::process::id())?,
        "command": std::env::args().collect::<Vec<_>>().join(" "),
    }))
}

/// Cancel the operation on `product`'s `surface`: the lock holder when one
/// holds it, else an after-install run the receipt records as running.
/// Refused, naming what was found, when the holder no longer runs with its
/// recorded birth time, when it is placing files, and when nothing runs.
pub(in crate::install) fn cancel(runtime: &Runtime, product: &Value, surface: &str) -> Result<Value> {
    let id = text(product, "id")?;
    let recipe = crate::install::recipe(product, surface)?;
    let lock = state::path(runtime, id, surface)?.with_extension("lock");
    let asked = asking()?;
    if let Some(held) = super::super::holder(&lock)? {
        let told = stop_holder(id, surface, &lock, &held, recipe)?;
        let writer = crate::common::files::lock_surface_waiting(&lock)?;
        let record = json!({
            "pid": held["pid"], "started_at": held["started_at"], "command": held["command"],
            "acquired_at": held["acquired_at"], "phase": held["phase"], "step": held["step"],
            "cancelled_by": asked, "told": told, "at": now(),
        });
        let receipt = match ProductState::load(runtime, id, surface)? {
            Some(mut receipt) => {
                receipt.extra.insert(CANCELLED_HOLDER.to_owned(), record.clone());
                receipt.save(runtime)?;
                format!("recorded in the surface's receipt under {CANCELLED_HOLDER}")
            }
            None => "no receipt exists for this surface, so the cancellation is kept only in this \
                     answer and in the lock record the next installation reclaims"
                .to_owned(),
        };
        drop(writer);
        return Ok(json!({
            "product": id, "surface": surface, "cancelled": "lock holder",
            "operation": record, "receipt": receipt,
        }));
    }
    let writer = crate::common::files::lock_surface_waiting(&lock)?;
    let ended = super::end_after_install(runtime, id, surface, super::Ending::Cancelled(asked))?;
    drop(writer);
    match ended {
        Some(ended) if ended["field"] == super::CANCELLED => Ok(json!({
            "product": id, "surface": surface, "cancelled": "after-install steps",
            "receipt_field": ended["field"], "operation": ended["record"],
        })),
        Some(ended) => bail!(
            "nothing to cancel on {id}/{surface}: its lock is free, and the after-install run its \
             receipt recorded was no longer running (recorded now as {}: {})",
            ended["field"],
            ended["record"]
        ),
        None => bail!(
            "nothing to cancel on {id}/{surface}: its lock is free and its receipt records no \
             after-install run; `stado product status {id} --surface {surface}` shows its state"
        ),
    }
}

/// Tell the holder `held` of `lock` and every process under it to stop, each
/// said on stderr; the answers. Refused when the recorded pid no longer runs
/// with its recorded birth time (the flock holder is then another process the
/// record does not name) or when it is placing files and runs none of
/// `recipe`'s after-install steps.
fn stop_holder(id: &str, surface: &str, lock: &std::path::Path, held: &Value, recipe: &Value) -> Result<Vec<Value>> {
    if held["running"] != true {
        bail!(
            "{id}/{surface}'s lock is held, but the pid its record names no longer runs with its \
             recorded birth time, so no recorded process is the operation to cancel: {}",
            super::super::clause(lock)
        );
    }
    let pid = i32::try_from(held["pid"].as_u64().context("the holder names no pid")?)?;
    let running = super::descendants(pid)?;
    if held["phase"].as_str() == Some(PHASE_PLACING) && super::after_install_step(&running, recipe).is_none() {
        bail!(
            "{id}/{surface} is being placed ({}); cancelling now would leave the surface half \
             replaced, so it is not stopped: cancel again once placement ends, or `stado product \
             rollback {id} --surface {surface}` after it",
            super::super::clause(lock)
        );
    }
    eprintln!("cancelling the installation {}", super::super::clause(lock));
    let told: Vec<Value> = running
        .iter()
        .map(|(child, _)| *child)
        .chain([pid])
        .map(super::stop)
        .collect();
    for each in &told {
        eprintln!("  {each}");
    }
    if told.iter().all(|each| each["told_to_stop"] != true) {
        bail!("no process of the installation {id}/{surface} could be told to stop: {}", json!(told));
    }
    Ok(told)
}
