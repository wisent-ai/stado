//! A janitor cleanup hold must not outlive the workload it stands for.
//!
//! # The invariant
//!
//! `disk_cleanup::acquire_workload_lock_in` takes a SHARED `fs2` hold on
//! `~/.cache/wisent-compute/disk-cleanup.lock` for one live workload, and the
//! janitor's own pass needs that same file EXCLUSIVELY. flock conflicts are
//! per open-file-description, not per process, so one shared hold that is
//! never released makes every exclusive acquire fail — including the ones made
//! by the very process holding it, which is the case that matters, because the
//! agent runs both.
//!
//! # What went wrong
//!
//! The hold used to live and die with the `ActiveSlot`, and a slot is
//! deliberately retained past its workload: `slots::advance_slot` returns
//! `SlotOutcome::Running` when the terminal artifact upload fails, so
//! finalization is retried on a later tick. That retry is unbounded. A store
//! that kept refusing one upload therefore converted a cross-process lock into
//! a permanent one inside a process that was otherwise healthy.
//!
//! Measured on `charless-mac-mini` on 2026-09-03: the agent (pid 79473, alive
//! 11.5 hours) held the lock, every pass reported `outcome: lock_busy,
//! duration_ms: 372`, the janitor's last success froze at 16:40:29Z, and
//! `host gates` closed the host to all work at 18.4 GiB free against a 15 GiB
//! watermark with eight jobs pinned to it. `lukasz-macbook` was closed the
//! same way at 118.7 GiB free against 100. Those two are the whole of
//! `darwin-arm64`, so the platform had no builder.
//!
//! # What is defended here
//!
//! The release path, not the incident: a hold that is settled because its
//! workload has exited must leave the lock TAKEABLE (not merely leave the
//! guard dropped from a struct), it must stay takeable across the unbounded
//! sequence of finalization ticks that follow, and it must be released when a
//! workload leaves by an unhappy path — a panic — rather than only by the
//! happy one. Nothing here spawns a workload or writes a job workdir; the hold
//! is taken through the same public function the agent calls.

use stado::providers::local::disk_cleanup::{
    acquire_workload_lock_in, ensure_state_dir, lock_relative_path, WorkloadLock,
};
use stado::providers::local::slots::release_hold_for_exited_workload;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

/// A fabricated `$HOME`. `secure_home` requires a real, non-symlink directory
/// owned by the effective uid, which a fresh temp dir is.
struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp HOME");
        // The janitor creates `~/.cache/wisent-compute` on its own first pass;
        // create it the same way so a probe taken before the first hold is
        // looking at the same file the janitor would.
        ensure_state_dir(dir.path()).expect("the janitor's state dir is ours to create");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The exact file the janitor locks, named by the crate constant rather
    /// than spelled here: a second copy of that path in a test is a test that
    /// keeps passing after a rename.
    fn lock_path(&self) -> PathBuf {
        self.dir.path().join(lock_relative_path())
    }

    /// One shared workload hold, taken the way `agent::run_agent` takes it.
    fn hold(&self) -> Option<WorkloadLock> {
        acquire_workload_lock_in(self.path()).expect("the lock file is ours to open")
    }

    /// Whether a cleanup pass could take the run lock right now.
    ///
    /// This is exactly what `disk_cleanup::acquire_lock` does and what decides
    /// `lock_busy`; it is spelled here because that function is private to the
    /// janitor and a test must not be able to change it.
    fn janitor_can_run(&self) -> bool {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            // Never truncated: `disk_cleanup::open_lock` does not truncate it
            // either, and a probe that emptied the lock file would be a probe
            // with a side effect on the thing it is measuring.
            .truncate(false)
            .open(self.lock_path())
            .expect("the lock file is openable");
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {
                let _ = fs2::FileExt::unlock(&file);
                true
            }
            Err(_) => false,
        }
    }
}

/// The fixture has to be able to block a pass, or every assertion below could
/// pass against a hold that was never taken.
#[test]
fn a_live_workload_hold_blocks_the_janitors_run_lock() {
    let home = Home::new();
    assert!(
        home.janitor_can_run(),
        "an unheld lock must be takeable, or this fixture proves nothing"
    );
    let hold = home.hold().expect("a free lock yields a shared hold");
    assert!(
        !home.janitor_can_run(),
        "a live workload hold must block the janitor's exclusive acquire"
    );
    drop(hold);
}

/// The defect. Settling the hold because the workload exited must free the
/// lock for the janitor — the property the wedge violated for 11.5 hours.
#[test]
fn a_hold_settled_at_workload_exit_frees_the_run_lock() {
    let home = Home::new();
    let mut hold = home.hold();
    assert!(hold.is_some(), "the fixture starts holding");
    assert!(!home.janitor_can_run(), "the fixture starts blocking");

    release_hold_for_exited_workload(&mut hold, &mut |_| {});

    assert!(
        hold.is_none(),
        "a settled hold must not be retained on the slot"
    );
    assert!(
        home.janitor_can_run(),
        "a workload that has exited must leave the janitor's run lock takeable"
    );
}

/// The failure mode, not the instance: after the workload exits, the slot is
/// retained and advanced again on every tick for as long as finalization keeps
/// failing. The lock must stay takeable across all of them, and settling an
/// already-settled hold must stay a no-op.
#[test]
fn the_lock_stays_takeable_across_an_unbounded_finalization_retry() {
    let home = Home::new();
    let mut hold = home.hold();
    release_hold_for_exited_workload(&mut hold, &mut |_| {});

    for tick in 0..8 {
        release_hold_for_exited_workload(&mut hold, &mut |_| {});
        assert!(
            hold.is_none(),
            "finalization tick {tick} must not resurrect a hold"
        );
        assert!(
            home.janitor_can_run(),
            "finalization tick {tick} must leave the run lock takeable"
        );
    }
}

/// A workload that leaves by an unhappy path. The guard's `Drop` is the
/// backstop for every exit this product does not spell — a panic, an error
/// returned through `?`, a dropped future — and it has to issue the unlock,
/// not merely close a descriptor.
#[test]
fn a_workload_that_leaves_by_panicking_does_not_leave_the_lock_held() {
    let home = Home::new();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _hold = home.hold().expect("a free lock yields a shared hold");
        assert!(!home.janitor_can_run(), "held inside the workload");
        panic!("the workload left by an unhappy path");
    }));
    assert!(outcome.is_err(), "the fixture must actually have panicked");
    assert!(
        home.janitor_can_run(),
        "a workload that panicked must not leave the janitor's run lock held"
    );
}
