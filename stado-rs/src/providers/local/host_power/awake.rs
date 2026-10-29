//! Keeping a Mac awake while the agent holds work, and only then.
//!
//! A shared Mac sleeps when its owner leaves it, and a sleeping host drops
//! every job it runs and every route through it. Changing its power settings
//! (`pmset`) would change the owner's machine for good; instead the agent
//! holds a power assertion while it has work and releases it when idle:
//! `caffeinate -s -w <agent pid>`, which prevents system sleep only while
//! the Mac is on mains power and ends by itself when the agent exits.

use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// The assertion this agent holds now, if any.
static HELD: Mutex<Option<Child>> = Mutex::new(None);

/// The assertion program macOS ships.
const CAFFEINATE: &str = "/usr/bin/caffeinate";

/// Hold the assertion while `working`, release it otherwise, saying each
/// change through `log_fn`. Does nothing off macOS.
pub(crate) fn hold_while(working: bool, log_fn: &mut dyn FnMut(&str)) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let mut held = match HELD.lock() {
        Ok(held) => held,
        Err(poisoned) => poisoned.into_inner(),
    };
    // An assertion process that ended by itself holds nothing.
    if let Some(child) = held.as_mut() {
        if let Ok(Some(status)) = child.try_wait() {
            log_fn(&format!("power assertion ended by itself ({status})"));
            *held = None;
        }
    }
    match (working, held.take()) {
        (true, Some(child)) => *held = Some(child),
        (true, None) => {
            let pid = std::process::id().to_string();
            match Command::new(CAFFEINATE)
                .args(["-s", "-w", &pid])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => {
                    log_fn(&format!(
                        "power assertion held while work runs (caffeinate -s, pid {}): the Mac \
                         does not sleep on mains power",
                        child.id()
                    ));
                    *held = Some(child);
                }
                Err(error) => log_fn(&format!(
                    "power assertion could not be taken, so the Mac may sleep under running \
                     work: {CAFFEINATE}: {error}"
                )),
            }
        }
        (false, Some(mut child)) => {
            let pid = child.id();
            let ended = child.kill().and_then(|()| {
                crate::wait::blocking(
                    crate::wait::Kind::Process,
                    format!("caffeinate pid {pid} ending"),
                    "this host",
                    || child.wait(),
                )
            });
            match ended {
                Ok(_) => log_fn("power assertion released: no work runs here"),
                Err(error) => log_fn(&format!(
                    "power assertion pid {pid} could not be ended: {error}"
                )),
            }
        }
        (false, None) => {}
    }
}
