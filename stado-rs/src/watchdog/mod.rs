//! Box diagnostics watchdog, run as the `stado serve --watchdog` role.
//!
//! Collects fault-isolated box diagnostics (systemctl status, journalctl
//! tails, ps, nvidia-smi, df, free, and a `gcloud storage ls capacity/`
//! probe), then uploads the JSON payload to
//! `gs://<bucket>/box_diagnostics/<host>.json` (plus
//! `box_diagnostics/<host>/latest.json`) every declared
//! `--watchdog-interval-seconds`, through [`JobStorage`](crate::queue::JobStorage)
//! so the `WC_STORAGE_BACKEND=local` backend works too. A pass whose upload
//! fails keeps its payload as `~/.stado/box_diagnostics/<host>.json`.

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
