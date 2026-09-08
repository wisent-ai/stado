//! Box diagnostics watchdog — port of `stado/deploy/watchdog/cli.py`.
//!
//! Collects fault-isolated box diagnostics (systemctl status, journalctl
//! tails, ps, nvidia-smi, df, free, and a `gcloud storage ls capacity/`
//! probe), then uploads the JSON payload to
//! `gs://<bucket>/box_diagnostics/<host>.json` (plus
//! `box_diagnostics/<host>/latest.json`) every interval (default 60 s).
//!
//! Deviation: Python's `_upload` drives the google-cloud-storage SDK
//! directly; here the upload goes through [`JobStorage`](crate::queue::JobStorage) (per the port
//! plan), so the `WC_STORAGE_BACKEND=local` backend works too.
//!
//! The CLI uses argparse semantics (NOT click like the rest of the
//! package): usage/error text, exit code 2 on argument errors, and
//! `-h/--help` on stdout with exit 0. One argparse behavior is not
//! reproduced: Python's `int()` accepts arbitrarily large values and
//! unicode digits; here `--interval-s` parses as `i64`.

mod cli;
mod collect;
mod runner;
mod upload;

pub use cli::{cli_main, help_text, parse_args, usage_text, ParseOutcome, ParsedArgs};
pub use collect::collect;
pub use runner::{CommandRunner, RunOutcome, SystemRunner};
pub use upload::{once, once_with, upload_with};

pub(crate) use collect::hostname;

/// Python `DEFAULT_BUCKET` (the watchdog's own default, NOT config BUCKET).
pub const DEFAULT_BUCKET: &str = "wisent-compute";
/// Python `DEFAULT_INTERVAL_S`.
pub const DEFAULT_INTERVAL_S: i64 = 60;
/// Python `OUT_PREFIX`.
pub const OUT_PREFIX: &str = "box_diagnostics";
/// Local standby path when the upload fails (Python `_write_local`).
pub const LOCAL_STANDBY_PATH: &str = "/tmp/wisent_box_diagnostics_latest.json";
