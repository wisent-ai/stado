//! Box diagnostics watchdog, run as the `stado serve --watchdog` role.
//!
//! Collects fault-isolated box diagnostics (systemctl status, journalctl
//! tails, ps, nvidia-smi, df, free, and a `gcloud storage ls capacity/`
//! probe), then uploads the JSON payload to
//! `gs://<bucket>/box_diagnostics/<host>.json` (plus
//! `box_diagnostics/<host>/latest.json`) every declared
//! `--watchdog-interval-seconds`, through [`JobStorage`](crate::queue::JobStorage)
//! so the `WC_STORAGE_BACKEND=local` backend works too.

mod cli;
mod collect;
mod runner;
mod upload;

pub use cli::ParsedArgs;
pub(crate) use cli::{configured_bucket, run};
pub use collect::collect;
pub use runner::{CommandRunner, RunOutcome, SystemRunner};
pub use upload::{once, once_with, upload_with};

pub(crate) use collect::hostname;

/// Python `DEFAULT_BUCKET` (the watchdog's own default, NOT config BUCKET).
pub const DEFAULT_BUCKET: &str = "wisent-compute";
/// Python `OUT_PREFIX`.
pub const OUT_PREFIX: &str = "box_diagnostics";
/// Local standby path when the upload fails (Python `_write_local`).
pub const LOCAL_STANDBY_PATH: &str = "/tmp/wisent_box_diagnostics_latest.json";
