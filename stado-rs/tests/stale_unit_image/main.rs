//! A managed unit whose live process is executing a file its unit no longer
//! names — measured on the machine running this test.
//!
//! The condition nothing in this fleet could see. `self_update::recycle_
//! replaced_units` cycles a unit only as a side effect of the invocation that
//! replaced the bytes: it joins the file names it just wrote onto the install
//! directory, reads each loaded unit's `argv`, and kickstarts one only when
//! `argv[0]` STRING-EQUALS a replaced path. There is no inode, no mtime and no
//! process-age comparison anywhere in it, a failed kickstart only logs that
//! the process keeps the old image, and nothing ever revisits a process left
//! behind. Staleness was detectable at replacement time and never again.
//!
//! WHAT THIS AREA USED TO DO, and no longer does. It declared a target called
//! `macbook-fake` carrying an SSH destination, wrote plists for labels ending
//! `-fake`, and started the probe process itself with `Command::spawn`. No
//! launchd unit was ever loaded, so on this build the scan — which takes each
//! label's live PID from the native manager — found no process for any of
//! them and the whole area was measuring nothing it claimed to measure.
//!
//! What runs here now is the real flow, end to end, on this machine:
//!
//! - the registry's one target IS this machine, named by its own host name
//!   lower-cased, with no SSH destination at all, so the product takes its
//!   current-host path;
//! - the unit is really loaded, by `/bin/launchctl bootstrap` into this
//!   login's own `gui/<uid>` domain, and really booted out afterwards with the
//!   removal read back off launchd;
//! - the image it executes is a real file this test placed in its own
//!   tempdir, replaced on disk exactly as an installer replaces one;
//! - and the product's verdict is checked against the digest, the inode, the
//!   link count, the size and the modification time this test measured itself.
//!
//! DELETED, with the reason. `a_unit_on_another_host_is_unread_rather_than_
//! clean` asserted the row `registry doctor` prints for a host it is not
//! running on. That row can only exist for a registry target that is NOT this
//! machine, which is a declared host name for a machine nothing here can
//! reach — the fake host constant this work exists to remove. There is no
//! second registered host on this workstation, so the case is gone rather
//! than dressed up. The remote half of that behaviour is unmeasured here and
//! is stated as unmeasured.

mod cases;
mod host;
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
fn stale_image_probe_child() {
    let Ok(seconds) = std::env::var(unit::HOLD) else {
        return;
    };
    let held = seconds.parse().unwrap_or(unit::HOLD_SECONDS);
    std::thread::sleep(Duration::from_secs(held));
}
