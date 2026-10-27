//! The delivery routine fired by the coordinator's own tick. A host that
//! declares a delivery plan (`stado ci plan set`) runs one pass of
//! `stado ci run` on every tick, at the cadence the coordinator's registry
//! entry already declares (`interval_seconds`), so no schedule with a cadence
//! nobody stated is needed for repairs to be checked and installed. The pass
//! runs as a process of its own beside the tick, so a pass that builds for an
//! hour holds no tick back; the routine's own pass lock keeps two passes from
//! overlapping, and a pass that finds the lock held ends at once. A host with
//! no plan fires nothing; a plan that cannot be read is said in the tick's
//! log with its cause and fires nothing.

use super::log;

/// What the tick's log names the pass by while it waits for it.
const PASS: &str = "the delivery pass `stado ci run`";

/// Fire one pass of the delivery routine beside the tick when this host
/// declares a plan, and log how the pass ended once it ends.
pub(super) fn fire() {
    match crate::cli::ci::delivery::plan::read() {
        Ok(Some(_)) => {}
        Ok(None) => return,
        Err(error) => {
            log(&format!(
                "delivery plan unreadable, so no delivery pass is fired: {error}"
            ));
            return;
        }
    }
    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(error) => {
            log(&format!(
                "this Stado's executable cannot be named, so no delivery pass is fired: {error}"
            ));
            return;
        }
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(["ci", "run", "--json"])
        .stdin(std::process::Stdio::null());
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            log(&format!("{PASS} did not start: {error}"));
            return;
        }
    };
    tokio::spawn(async move {
        let ended = crate::wait::until(
            crate::wait::Kind::Process,
            PASS,
            "this host",
            child.wait_with_output(),
        )
        .await;
        match ended {
            Ok(output) if output.status.success() => log(&format!(
                "{PASS} ended: {}",
                String::from_utf8_lossy(&output.stdout).trim()
            )),
            Ok(output) => log(&format!(
                "{PASS} failed ({}): {} {}",
                output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => log(&format!("{PASS} could not be awaited: {error}")),
        }
    });
}
