//! Fenced local slot lifecycle: atomic claim handoff, process-group execution,
//! heartbeat, Vast pause/resume, cooperative yield, cancellation, redacted
//! output persistence, and durable terminal transition.
//!
//! A running slot owns its workload process group, log handle, monotonic
//! timestamps, capacity accounting, and shared cleanup lock. Canonical output
//! is written through `JobStorage`; optional mirrors accept only `stado://`
//! destinations after the canonical write succeeds.
//!
//! One module, kept in parts small enough to read, grouped by the seams the
//! lifecycle already had: `model` (the slot, the job tree it executes in, and
//! the conversions its records are formatted with), `admission` (what this
//! host must be able to do before it claims), `reporting` (status, heartbeat,
//! output) and `lifecycle` (the claim, the cooperative yield, the tick).
//! Every part opens with `use super::*;`, so the imports below are the
//! module's single import list and a part sees the items of every other part
//! exactly as it did when this was one file. Each part is re-exported by
//! glob: `pub` items stay public, `pub(crate)` items stay crate-visible, and
//! `crate::providers::local::slots::<item>` still names every item it named
//! before.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use nix::sys::signal::Signal;
use nix::unistd::Pid;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::constants;
use crate::models::{
    activation_extraction_must_share_gpu, deprecated_activation_command_reason, isoformat_utc,
    job_state, Job,
};
use crate::queue::{JobStorage, StorageError};
use crate::sizing::Sizing;

use super::gpu_probe;
use super::helpers;
use super::{build_job_command, verify_command, Slot};

mod admission;
mod lifecycle;
mod model;
mod reporting;

pub use admission::*;
pub use lifecycle::*;
pub use model::*;
pub use reporting::*;
