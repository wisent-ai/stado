//! Where a submission's minutes go, written while it runs.
//!
//! `stado build submit` took six to eight minutes and said nothing until it
//! printed the queued build: the operator could not
//! tell staging from host selection from a registry write, and nobody could
//! say which of them to make faster. Every phase now names itself on stderr
//! when it starts and says how long it took when it ends, error or not, so a
//! slow submission shows the phase it is in and a finished one leaves the
//! breakdown. Stdout, and with it `--json`, is untouched. `stado release
//! changes submit|list` name their store requests the same way, so a handoff
//! that waits on a store which does not answer says which store and request.

use std::future::Future;
use std::time::Instant;

/// One named phase of one command; its end is written when it is dropped.
pub(crate) struct Phase {
    command: &'static str,
    name: String,
    started: Instant,
}

/// Start a phase of `stado build submit` and say so.
pub(crate) fn phase(name: impl Into<String>) -> Phase {
    phase_of("build submit", name)
}

/// Start a phase of `command` and say so.
pub(crate) fn phase_of(command: &'static str, name: impl Into<String>) -> Phase {
    let name = name.into();
    eprintln!("[{command}] {name}: started");
    Phase {
        command,
        name,
        started: Instant::now(),
    }
}

/// Run `work` as one named phase of `command`, saying so at both ends.
pub(crate) async fn timed<T>(
    command: &'static str,
    name: impl Into<String>,
    work: impl Future<Output = T>,
) -> T {
    let _phase = phase_of(command, name);
    work.await
}

impl Drop for Phase {
    fn drop(&mut self) {
        eprintln!(
            "[{}] {}: took {}s",
            self.command,
            self.name,
            self.started.elapsed().as_secs()
        );
    }
}
