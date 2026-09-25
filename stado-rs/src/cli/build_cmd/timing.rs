//! Where a submission's minutes go, written while it runs.
//!
//! `stado build submit` took six to eight minutes and said nothing until it
//! printed the queued build: the operator could not
//! tell staging from host selection from a registry write, and nobody could
//! say which of them to make faster. Every phase now names itself on stderr
//! when it starts and says how long it took when it ends, error or not, so a
//! slow submission shows the phase it is in and a finished one leaves the
//! breakdown. Stdout, and with it `--json`, is untouched.

use std::time::Instant;

/// One named phase; its end is written when it is dropped.
pub(crate) struct Phase {
    name: String,
    started: Instant,
}

/// Start a phase and say so.
pub(crate) fn phase(name: impl Into<String>) -> Phase {
    let name = name.into();
    eprintln!("[build submit] {name}: started");
    Phase {
        name,
        started: Instant::now(),
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        eprintln!(
            "[build submit] {}: took {}s",
            self.name,
            self.started.elapsed().as_secs()
        );
    }
}
