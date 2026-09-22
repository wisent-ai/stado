//! Waiting, and the guarantee that nothing a case started outlives it.

use super::*;

mod capacity;
mod submission;

pub(crate) use capacity::*;
pub(crate) use submission::*;

/// A child process a case owns: killed and reaped when the case ends, however
/// it ends, so a failed assertion never leaves an agent or a release running.
pub(crate) struct Running(pub(crate) Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
