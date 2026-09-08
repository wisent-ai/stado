//! What this host must be able to do before it claims a job: the environment
//! an untrusted workload inherits, the secrets the job declares, the system
//! packages and staging room it needs, and the declared inputs materialized
//! while storage credentials are still confined to the agent process.

use super::*;

mod environment;
mod inputs;
mod packages;
mod secrets;

// `packages` is the only part with a public surface; the environment, the
// declared inputs and the secret resolution are crate-visible, so their
// re-exports say so rather than claiming a public one.
pub(crate) use environment::*;
pub(crate) use inputs::*;
pub use packages::*;
pub(crate) use secrets::*;
