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

/// The surface's lock for an installation of `recipe` on `(id, surface)`: an
/// older holder past placement is stopped first (`stop_past_placement`), and
/// once the lock is held an older installation still running its after-install
/// steps without it is stopped too (`stop_after_install`), unless the caller
/// asked to wait for whatever holds it (`--wait`). With `--wait` an older
/// installation running its after-install steps is waited for as well: it
/// holds no lock while they run, so a waiting installation took the lock at
/// once, recorded itself, and the older one's outcome was then refused
/// (`another installation ... recorded itself while this one's after-install
/// steps ran`), so of two waiting installations one always failed.
pub(in crate::install) fn lock_surface(
    runtime: &Runtime,
    (id, surface): (&str, &str),
    lock: &Path,
    recipe: &Value,
) -> Result<File> {
    if runtime.wait_for_writer {
        loop {
            let writer = runtime.surface_lock(lock)?;
            let Some(running) = super::running_after_install(runtime, id, surface)? else {
                return Ok(writer);
            };
            drop(writer);
            super::wait_for_after_install(id, surface, &running)?;
        }
    }
    super::stop_past_placement(lock, recipe)?;
    let writer = runtime.surface_lock(lock)?;
    super::stop_after_install(runtime, id, surface)?;
    Ok(writer)
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
    let pid = i32::try_from(std::process::id())?;
    let started_at = crate::common::process::birth::started_at(pid)?
        .ok_or_else(|| anyhow::anyhow!("the installing process pid {pid} has already exited"))?;
    installed.extra.insert(
        "after_install_running".to_owned(),
        json!({"pid": pid, "started_at": started_at, "steps": steps}),
    );
    installed.save(runtime)?;
    drop(writer);
    let outcomes = super::super::after_install::run(id, surface, host, steps, installed);
    let writer = runtime.surface_lock(lock)?;
    let current = ProductState::load(runtime, id, surface)?;
    if serde_json::to_value(&current)? != serde_json::to_value(Some(&*installed))? {
        bail!(
            "another installation of {id}/{surface} recorded itself while this one's after-install steps \
             ran ({}); their outcome is not written over it",
            match &outcomes {
                Ok(outcomes) => json!(outcomes).to_string(),
                Err(error) => format!("they failed: {error:#}"),
            }
        );
    }
    installed.extra.remove("after_install_running");
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
