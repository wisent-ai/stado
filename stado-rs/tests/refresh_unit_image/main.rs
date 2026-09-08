//! `stado service refresh-image` — the verb behind the `stale-unit-image`
//! row, exercised against a launchd unit this test really loads.
//!
//! `registry doctor` reports a unit whose live process is executing a file the
//! unit no longer names, and the row ends "Restarting the unit is what puts it
//! on the installed file, and nothing does that on its own". Until this
//! command that sentence instructed an operator to perform an action the
//! product did not offer as a checked operation.
//!
//! What these tests defend is mostly the refusals, because the refusals are
//! where the damage is. A remediation that restarts whatever it is pointed at
//! is a restart button, and on this fleet a restart button has already turned
//! a degraded host into a down one. So: a unit that is not stale is refused
//! and the refusal names the identity that was read; a replacement still
//! inside the settle window is refused as an installer mid-flight; a unit
//! whose identity could not be read is refused, because an unread state is no
//! more a reason to act than it is a reason to pass; and a machine no registry
//! target names is refused before anything is looked at.
//!
//! And the post-restart verdict, which is the other half of the discipline. On
//! 2026-09-03 pid 49727 — `com.wisent.compute.agent.lukasz-macbook` —
//! respawned under `KeepAlive` straight back onto the same unlinked inode
//! 182274754 it had just left. launchd re-execs the declared path and the path
//! was never the problem, so "a restart was issued" is not evidence that
//! anything changed.
//!
//! WHAT THIS AREA USED TO DO, and no longer does. It declared a target called
//! `macbook-fake` with an SSH destination, pointed at a label ending `-fake`,
//! started the probe itself with `Command::spawn` and loaded no launchd unit
//! at all — so on this build, which takes each label's live PID from the
//! native manager, no case here reached a running unit. It also carried a
//! stated gap: neither post-restart branch was exercised against a real
//! restart.
//!
//! That gap is closed. `outcome::a_stale_unit_is_restarted_onto_the_file_it_
//! declares` loads an agent into this login's own `gui/<uid>` domain, replaces
//! the file underneath it, and runs the command for real: launchd kickstarts
//! the unit, the second read lands on the declared file, and the bytes the new
//! process executes are checked against a sha256 this test computed. No
//! privilege beyond this login is needed for that, because the unit is a
//! LaunchAgent this test owns and boots out again.
//!
//! DELETED, with the reason. `a_machine_no_target_names_is_refused_before_it_
//! looks` used to declare a target for a machine that does not exist, which is
//! the fake host constant this work removes. The same refusal is reached
//! honestly by a registry that declares no target at all — see
//! `refusals::a_registry_that_names_no_machine_is_refused_before_it_looks` —
//! so the sentence is still defended and the invented host is gone. The
//! privileged half of the verb is not covered here either: a unit under
//! `/Library/LaunchDaemons` restarts through `sudo -n /bin/launchctl
//! kickstart`, and this test has no such credential, so no case pretends to.

mod host;
mod outcome;
mod refusals;
mod unit;

use std::time::Duration;

/// Not a test: the body of the process launchd holds for these cases.
///
/// It is a `#[test]` because the executable this area needs is one it can
/// copy, and a locally built test binary is the only executable available to
/// it — macOS SIGKILLs a copy of a signed platform binary such as `/bin/ls`
/// (exit 137, measured on this host), so no system binary can be the image.
///
/// The parent `cargo test` run reaches this function too, with
/// [`unit::HOLD`] unset, and it must return immediately there.
#[test]
fn refresh_image_probe_child() {
    let Ok(seconds) = std::env::var(unit::HOLD) else {
        return;
    };
    let held = seconds.parse().unwrap_or(unit::HOLD_SECONDS);
    std::thread::sleep(Duration::from_secs(held));
}
