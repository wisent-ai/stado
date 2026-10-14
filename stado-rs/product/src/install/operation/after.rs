//! Taking a surface's lock for an installation, and what the installation
//! does once its files are placed: it records the end of placement, runs its
//! recipe's after-install steps without the lock, and takes the lock back to
//! record their outcome.

use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::fs::File;
use std::path::Path;

use crate::common::Runtime;
use crate::state::ProductState;

/// The surface's lock for an installation of `recipe`: an older holder past
/// placement is stopped first (`stop_past_placement`), unless the caller
/// asked to wait for whatever holds it (`--wait`).
pub(in crate::install) fn lock_surface(
    runtime: &Runtime,
    lock: &Path,
    recipe: &Value,
) -> Result<File> {
    if !runtime.wait_for_writer {
        super::stop_past_placement(lock, recipe)?;
    }
    runtime.surface_lock(lock)
}

/// The installation recorded in `installed` has placed and recorded its
/// files: the lock says so, and `recipe`'s `after_install` steps run without
/// holding `writer`. The lock is taken back to record their outcome and is
/// returned held. A newer installation that recorded itself in between is
/// not overwritten: the outcome is refused, naming both.
pub(in crate::install) fn after_install(
    runtime: &Runtime,
    (id, surface, host): (&str, &str, Option<&str>),
    recipe: &Value,
    installed: &mut ProductState,
    lock: &Path,
    writer: File,
) -> Result<File> {
    crate::common::mark_placed(lock)?;
    let Some(steps) = recipe.get("after_install") else {
        return Ok(writer);
    };
    let recorded = serde_json::to_value(&*installed)?;
    drop(writer);
    let outcomes = super::super::after_install::run(id, surface, host, steps, installed);
    let writer = runtime.surface_lock(lock)?;
    let current = ProductState::load(runtime, id, surface)?;
    if serde_json::to_value(&current)? != recorded {
        bail!(
            "another installation of {id}/{surface} recorded itself while this one's after-install steps \
             ran ({}); their outcome is not written over it",
            match &outcomes {
                Ok(outcomes) => json!(outcomes).to_string(),
                Err(error) => format!("they failed: {error:#}"),
            }
        );
    }
    match outcomes {
        Ok(outcomes) => {
            installed
                .extra
                .insert("after_install".to_owned(), json!(outcomes));
            installed.save(runtime)?;
            Ok(writer)
        }
        Err(error) => {
            installed.extra.insert(
                "after_install_failure".to_owned(),
                json!(format!("{error:#}")),
            );
            installed.save(runtime)?;
            Err(error)
        }
    }
}
