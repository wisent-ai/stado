//! Stop the current surface writer or its recorded post-placement step.

use super::{children, holder, stop};
use crate::common::{now, process::started_at, Runtime};
use crate::state::{self, ProductState};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde_json::{json, Value};
use stado_wait as wait;
use std::fs::{File, OpenOptions};
use std::path::Path;

/// Signal the surface owner and its children. A placement holder is waited
/// out through its lock; an after-install step has released that lock and is
/// identified by its process birth time, not by a reusable PID alone.
pub fn cancel(runtime: &Runtime, id: &str, surface: &str) -> Result<Value> {
    let lock = state::path(runtime, id, surface)?.with_extension("lock");
    if let Some(held) = holder(&lock)? {
        let pid = i32::try_from(held["pid"].as_u64().context("the holder names no pid")?)?;
        let signalled: Vec<Value> = children(pid)?
            .into_iter()
            .map(|(child, _)| child)
            .chain([pid])
            .map(stop)
            .collect();
        let file =
            File::open(&lock).with_context(|| format!("{} cannot be opened", lock.display()))?;
        wait::blocking_on_lock(
            format!("the cancelled installation operation pid {pid} to let go of its lock"),
            &lock,
            || file.lock_exclusive(),
        )
        .with_context(|| format!("waiting for {} was refused", lock.display()))?
        .with_context(|| {
            format!(
                "waiting for {} after cancelling pid {pid} failed",
                lock.display()
            )
        })?;
        let cancelled = json!({"at": now(), "holder": held, "signalled": signalled});
        if let Some(mut receipt) = ProductState::load(runtime, id, surface)? {
            receipt
                .extra
                .insert("cancelled".to_owned(), cancelled.clone());
            receipt.save(runtime)?;
        }
        file.unlock()
            .with_context(|| format!("{} cannot be released", lock.display()))?;
        return Ok(json!({"product": id, "surface": surface, "cancelled": cancelled}));
    }
    cancel_after_install(runtime, id, surface, &lock)
}

fn cancel_after_install(runtime: &Runtime, id: &str, surface: &str, lock: &Path) -> Result<Value> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(lock)
        .with_context(|| format!("{} cannot be opened", lock.display()))?;
    file.try_lock_exclusive().with_context(|| {
        format!(
            "a new installation acquired {} before cancellation",
            lock.display()
        )
    })?;
    let mut receipt = ProductState::load(runtime, id, surface)?
        .with_context(|| format!("no installation operation holds {id}/{surface}"))?;
    let marker = receipt
        .extra
        .get("after_install_running")
        .with_context(|| {
            format!("no installation operation holds {id}/{surface}; nothing was cancelled")
        })?;
    let pid = i32::try_from(
        marker["pid"]
            .as_i64()
            .context("after-install operation names no pid")?,
    )?;
    let expected = marker["started_at"]
        .as_str()
        .context("after-install operation records no process birth time")?;
    let observed = started_at(pid)?;
    if observed.as_deref() != Some(expected) {
        bail!(
            "{id}/{surface}: after-install pid {pid} is no longer the recorded process (recorded {expected}, observed {observed:?}); no process was signalled"
        );
    }
    let mut pending = vec![pid];
    let mut descendants = Vec::new();
    while let Some(parent) = pending.pop() {
        for (child, _) in children(parent)? {
            descendants.push(child);
            pending.push(child);
        }
    }
    let signalled: Vec<Value> = descendants
        .into_iter()
        .rev()
        .chain([pid])
        .map(stop)
        .collect();
    let observed_after_signal = started_at(pid)?;
    let cancelled = json!({
        "at": now(), "operation": marker, "signalled": signalled,
        "running_after_signal": observed_after_signal.as_deref() == Some(expected),
    });
    receipt
        .extra
        .insert("cancelled".to_owned(), cancelled.clone());
    receipt.save(runtime)?;
    file.unlock()
        .with_context(|| format!("{} cannot be released", lock.display()))?;
    Ok(json!({"product": id, "surface": surface, "cancelled": cancelled}))
}
