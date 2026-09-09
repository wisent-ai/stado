//! A janitor cleanup hold must not outlive the workload it stands for.
//!
//! # The invariant
//!
//! The agent takes a SHARED hold on `~/.cache/wisent-compute/disk-cleanup.lock`
//! for every live workload, and a cleanup pass needs that same file
//! EXCLUSIVELY. flock conflicts are per open-file-description, not per
//! process, so one shared hold that is never released makes every exclusive
//! acquire fail — including the ones made by the very process holding it,
//! which is the case that matters, because the agent runs both.
//!
//! # What went wrong
//!
//! The hold used to live and die with the `ActiveSlot`, and a slot is
//! deliberately retained past its workload: `slots::advance_slot` keeps it
//! `Running` when the terminal record cannot be written, so finalization is
//! retried on a later tick, unboundedly. A store that kept refusing one upload
//! therefore converted a cross-process lock into a permanent one inside a
//! process that was otherwise healthy and kept publishing capacity.
//!
//! Measured on `charless-mac-mini` on 2026-09-03: the agent (pid 79473, alive
//! 11.5 hours) held the lock, every pass reported `outcome: lock_busy`, the
//! janitor's last success froze at 16:40:29Z, `host gates` read that age and
//! closed the host to all work at 18.4 GiB free against a 15 GiB watermark
//! with eight jobs pinned to it. `lukasz-macbook` was closed the same way at
//! 118.7 GiB free against 100. Those two are the whole of `darwin-arm64`, so
//! the platform had no builder.
//!
//! # What is defended here
//!
//! Three journeys, each one a real `stado submit`, a real `stado agent
//! --target` claiming on this machine, and a real `stado disk-cleanup --once
//! --to-target` asking for an enforcing pass:
//!
//! - while the workload runs the pass is refused, `outcome:
//!   lock_busy_unattributed`, and the backdated cache directory it would have
//!   deleted is still on disk;
//! - once the workload has a terminal record the same command reclaims that
//!   directory for real;
//! - and with the store refusing every terminal prefix — the exact shape of
//!   the wedge, the slot retained and finalization retried forever — the lock
//!   still becomes takeable once the workload's process is gone.
//!
//! # What this area used to do, and no longer does
//!
//! It called `acquire_workload_lock_in` on a fabricated `$HOME`, probed the
//! lock with its own `fs2::try_lock_exclusive`, and called
//! `release_hold_for_exited_workload` by hand. No agent, no workload, no job,
//! no store, no pass: the lock was never taken by the thing that takes it, and
//! never released by the thing that releases it.
//!
//! DELETED, with the reason.
//!
//! `a_live_workload_hold_blocks_the_janitors_run_lock` and
//! `a_hold_settled_at_workload_exit_frees_the_run_lock` are the two halves of
//! `cases::a_live_workload_refuses_the_pass_and_its_exit_gives_it_back`, which
//! asks the product's own command instead of re-implementing its lock probe.
//!
//! `the_lock_stays_takeable_across_an_unbounded_finalization_retry` called the
//! release function in a loop; the retry it names is now produced by making
//! the store refuse the terminal record, in
//! `cases::a_retained_slot_still_gives_the_run_lock_back`.
//!
//! `a_workload_that_leaves_by_panicking_does_not_leave_the_lock_held` panicked
//! inside the test process and caught its own unwind. No workload the product
//! runs is a panic in the agent's address space — a workload is a child
//! process — and the guard's `Drop` has no command behind it. The unhappy exit
//! that IS reachable is a workload that exits non-zero, which the product
//! records in `failed/`, and that is
//! `cases::a_workload_that_fails_gives_the_run_lock_back`.

mod cases;
mod fixture;
mod fleet;
