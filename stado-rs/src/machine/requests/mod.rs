//! The accepted request shape: the field whitelist, the value patterns, and
//! the reservation lease a submission renews while it works.

use std::sync::LazyLock;

pub(in crate::machine) mod leases;
pub(in crate::machine) mod validate;

/// Request fields accepted by `machine submit` (Python `REQUEST_FIELDS`).
const REQUEST_FIELDS: &[&str] = &[
    "client_request_id",
    "command",
    "provider",
    "gpu_type",
    "pinned_host",
    "vram_gb",
    "max_cost_per_hour_usd",
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
