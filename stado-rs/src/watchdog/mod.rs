//! Box diagnostics watchdog, run as the `stado serve --watchdog` role.
//!
//! Collects fault-isolated box diagnostics (systemctl status, journalctl
//! tails, ps, nvidia-smi, df, free, each volume as the disk-full rule reads
//! it, and the fleet's capacity through `stado capacity list --json`), then
//! uploads the JSON payload to `box_diagnostics/<host>.json` (plus
//! `box_diagnostics/<host>/latest.json`) in the declared bucket every
//! `--watchdog-interval-seconds`, through [`JobStorage`](crate::queue::JobStorage)
//! so every storage backend works. A pass whose upload fails keeps its
//! payload as `~/.stado/box_diagnostics/<host>.json`.

mod cli;
mod collect;
mod runner;
mod upload;

pub use cli::ParsedArgs;
pub(crate) use cli::{configured_bucket, run};
pub use collect::collect;
pub use runner::{CommandRunner, RunOutcome, SystemRunner};
pub use upload::{once, once_with, upload_with, Pass};

pub(crate) use collect::hostname;

/// The storage prefix every host's diagnostics are uploaded under.
pub const OUT_PREFIX: &str = "box_diagnostics";
