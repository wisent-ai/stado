//! The accepted request shape: the field whitelist, the value patterns, and
//! the reservation lease a submission renews while it works.

use std::sync::LazyLock;

use serde_json::{Map, Value};

pub(in crate::machine) mod leases;
pub(in crate::machine) mod validate;

/// Where a submission's idempotency reservation stands: claimed by the
/// submission holding it, enqueuing its job, then accepted with the job's
/// result or released when the queue refused the job. Each is written and
/// compared through [`RequestState::word`], so no state is spelled twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::machine) enum RequestState {
    Claimed,
    Enqueuing,
    Accepted,
    Released,
}

impl RequestState {
    /// The state as the stored reservation names it.
    pub(in crate::machine) fn word(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Enqueuing => "enqueuing",
            Self::Accepted => "accepted",
            Self::Released => "released",
        }
    }

    /// Whether the stored reservation `record` stands in this state.
    pub(in crate::machine) fn holds(self, record: &Map<String, Value>) -> bool {
        record.get("state").and_then(Value::as_str) == Some(self.word())
    }

    /// Move the stored reservation `record` to this state.
    pub(in crate::machine) fn write(self, record: &mut Map<String, Value>) {
        record.insert("state".into(), Value::from(self.word()));
    }
}

/// Request fields accepted by `machine submit` (Python `REQUEST_FIELDS`).
const REQUEST_FIELDS: &[&str] = &[
    "client_request_id",
    "command",
    "provider",
    "gpu_type",
    "pinned_host",
    "vram_gb",
    "max_cost_per_hour_usd",
    "submitter_restarts",
    // Read only for requests stored before submitter_restarts existed: the
    // marketplace keeps its submission and sends it again verbatim, and the
    // one value any submitter sent meant that it starts new launches itself.
    "max_restarts",
    "pin_to_provider",
    "priority",
    "repo",
    "repo_ref",
    "repo_workdir",
    "repo_extras",
    "pre_command",
    "apt_packages",
    "output_uri",
    "verify_command",
    "exclusive",
    "terminates_agent",
    "boot_disk_gb",
    "runtime_seconds_estimate",
    "source_archive_path",
    "input_objects",
    "secret_env",
];

static REQUEST_ID_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$").expect("static regex compiles")
});
static HOSTNAME_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[a-z0-9][a-z0-9.-]{0,127}$").expect("static hostname regex compiles")
});
static APT_PACKAGE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9+._:-]*$").expect("static regex compiles")
});
static ENV_NAME_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("static regex compiles")
});
static SECRET_PART_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*$").expect("static regex compiles")
});
static REPO_REF_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[0-9a-f]{40}$").expect("static repo ref regex compiles"));
