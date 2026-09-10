//! Everything `stado space watermark` accepts, and the two questions argv
//! answers before the command touches the registry: is this a read, and does
//! it edit fields by hand or apply a declared policy whole.

use clap::Args;

/// Everything `space watermark` accepts.
#[derive(Args)]
pub struct WatermarkArgs {
    pub target: String,
    /// Arm this host with one policy `stado space policies` declares, whole.
    /// Mutually exclusive with the individual field flags below: a write
    /// either applies a reviewed declaration or edits fields by hand, and a
    /// call that did both would leave a document neither of them describes.
    #[arg(long = "policy")]
    pub policy: Option<String>,
    /// Authorize the `graphical_session` repair the named policy carries.
    /// Required by any policy that ends a logged-in session's processes, and
    /// refused on its own, because the authorization has no meaning apart
    /// from the declaration that names the processes.
    #[arg(long = "authorize-graphical-session")]
    pub authorize_graphical_session: bool,
    /// `off`, `report` or `enforce`; only `enforce` repairs anything.
    #[arg(long = "memory-mode")]
    pub memory_mode: Option<String>,
    /// Available memory below this many MiB is pressure.
    #[arg(long = "memory-low-free-mb")]
    pub memory_low_free_mb: Option<i64>,
    /// A pass stops as soon as this many MiB are available.
    #[arg(long = "memory-target-free-mb")]
    pub memory_target_free_mb: Option<i64>,
    /// Swap utilisation at or above this percentage is pressure on its own.
    #[arg(long = "memory-high-swap-used-pct")]
    pub memory_high_swap_used_pct: Option<i64>,
    /// How many repairs one pass may perform.
    #[arg(long = "memory-max-repairs-per-pass")]
    pub memory_max_repairs_per_pass: Option<i64>,
    /// Seconds one pass may spend.
    #[arg(long = "memory-max-pass-seconds")]
    pub memory_max_pass_seconds: Option<i64>,
    /// Permit one declared repair by name; repeat to permit several.
    #[arg(long = "memory-repair")]
    pub memory_repair: Vec<String>,
    /// A unit `restart_unit` may restart; repeat.
    #[arg(long = "memory-repair-unit")]
    pub memory_repair_unit: Vec<String>,
    /// A process `graphical_session` may end; repeat.
    #[arg(long = "memory-repair-process")]
    pub memory_repair_process: Vec<String>,
    /// The program `reap_recovery` runs.
    #[arg(long = "memory-repair-recovery")]
    pub memory_repair_recovery: Option<String>,
    /// Authorize `graphical_session` to end the processes it names.
    #[arg(long = "memory-allow-graphical-session", num_args = 1)]
    pub memory_allow_graphical_session: Option<bool>,
    /// Publish this host as not accepting jobs while it is over its watermark.
    #[arg(long = "memory-refuse-placement", num_args = 1)]
    pub memory_refuse_placement: Option<bool>,
    /// `off`, `report` or `enforce` for the disk janitor; only `enforce`
    /// deletes anything.
    #[arg(long = "disk-mode")]
    pub disk_mode: Option<String>,
    /// Free disk below this many GiB is pressure: the janitor starts a pass,
    /// the host refuses release builds, and a release build that would end
    /// below it is not placed here.
    #[arg(long = "disk-low-free-gb")]
    pub disk_low_free_gb: Option<i64>,
    /// A janitor pass stops as soon as this many GiB are free.
    #[arg(long = "disk-target-free-gb")]
    pub disk_target_free_gb: Option<i64>,
    /// Seconds between janitor passes.
    #[arg(long = "disk-check-interval-seconds")]
    pub disk_check_interval_seconds: Option<i64>,
    /// Bytes one janitor pass may delete.
    #[arg(long = "disk-max-bytes-per-pass")]
    pub disk_max_bytes_per_pass: Option<i64>,
    /// Directories one janitor pass may delete.
    #[arg(long = "disk-max-items-per-pass")]
    pub disk_max_items_per_pass: Option<i64>,
    /// Directories one janitor pass may cross.
    #[arg(long = "disk-max-scan-items")]
    pub disk_max_scan_items: Option<i64>,
    /// Seconds one janitor pass may spend.
    #[arg(long = "disk-max-pass-seconds")]
    pub disk_max_pass_seconds: Option<i64>,
    #[arg(long)]
    pub json: bool,
}

impl WatermarkArgs {
    /// Whether argv asked for a read rather than a write.
    pub(super) fn is_read_only(&self) -> bool {
        self.policy.is_none() && !self.authorize_graphical_session && !self.edits_fields_by_hand()
    }

    /// Whether argv carries any single-field write.
    pub(super) fn edits_fields_by_hand(&self) -> bool {
        self.edits_memory_fields() || self.edits_disk_fields()
    }

    pub(super) fn edits_memory_fields(&self) -> bool {
        self.memory_mode.is_some()
            || self.memory_low_free_mb.is_some()
            || self.memory_target_free_mb.is_some()
            || self.memory_high_swap_used_pct.is_some()
            || self.memory_max_repairs_per_pass.is_some()
            || self.memory_max_pass_seconds.is_some()
            || !self.memory_repair.is_empty()
            || !self.memory_repair_unit.is_empty()
            || !self.memory_repair_process.is_empty()
            || self.memory_repair_recovery.is_some()
            || self.memory_allow_graphical_session.is_some()
            || self.memory_refuse_placement.is_some()
    }

    /// The `--disk-*` flags, as `(disk_cleanup key, value)` pairs, in the
    /// order the registry validator names them.
    pub(super) fn disk_fields(&self) -> Vec<(&'static str, serde_json::Value)> {
        [
            ("mode", self.disk_mode.clone().map(serde_json::Value::from)),
            ("low_free_gb", self.disk_low_free_gb.map(Into::into)),
            ("target_free_gb", self.disk_target_free_gb.map(Into::into)),
            (
                "check_interval_seconds",
                self.disk_check_interval_seconds.map(Into::into),
            ),
            (
                "max_bytes_per_pass",
                self.disk_max_bytes_per_pass.map(Into::into),
            ),
            (
                "max_items_per_pass",
                self.disk_max_items_per_pass.map(Into::into),
            ),
            ("max_scan_items", self.disk_max_scan_items.map(Into::into)),
            (
                "max_pass_seconds",
                self.disk_max_pass_seconds.map(Into::into),
            ),
        ]
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .collect()
    }

    pub(super) fn edits_disk_fields(&self) -> bool {
        !self.disk_fields().is_empty()
    }
}
