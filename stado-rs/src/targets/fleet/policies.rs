use crate::targets::*;

// ---------------------------------------------------------------------------
// __init__.py — data models
// ---------------------------------------------------------------------------

pub(crate) fn default_runtime() -> String {
    "daemon".to_string()
}

pub(crate) fn default_interval_seconds() -> i64 {
    180
}

pub(crate) fn default_state_uri() -> String {
    "stado://system/registry".to_string()
}

/// Tolerate explicit JSON null where Python does `d.get(key) or <default>`.
pub(crate) fn de_null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Weles worker policy for a local target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WelesPolicy {
    pub enabled: bool,
    pub actions: Vec<String>,
    /// Where the Weles worker writes run recordings
    /// (WELES_RECORDINGS_ROOT). Optional; when set, the disk cleaner's
    /// weles_recordings.root should point at <recordings_dir> so policy and
    /// writer never drift apart.
    #[serde(default)]
    pub recordings_dir: Option<String>,
}

/// One identity a host is expected to hold, as opposed to one action it may run.
///
/// The distinction is the whole point. `WelesPolicy.actions` answers "may this host
/// do X" -- permission and capacity. It cannot answer "is this the machine where a
/// two-factor prompt for controlyourai@gmail.com will appear", because that is not a
/// permission at all: it is a property the machine either has or has not, granted by
/// a third party and revocable without telling us.
///
/// Routing such work by an action allowlist buries the discovery of a missing
/// identity at the deepest point of the flow -- a browser trajectory waiting for a
/// code no machine will ever display, until it times out. Declaring the binding here
/// lets the fleet refuse at dispatch and name the host that must be enrolled.
///
/// `verified_at` is deliberately not part of the declaration. A binding is a claim
/// until a host observes it, exactly like a release phase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityBinding {
    /// Identity family, e.g. "apple-account".
    pub kind: String,
    /// The identity itself, e.g. "controlyourai@gmail.com".
    pub identity: String,
    /// Operating-system user holding it, when the identity is per-user rather than
    /// per-machine. An Apple account signed into one macOS user does not make the
    /// other users on that Mac trusted.
    #[serde(default)]
    pub user: Option<String>,
    /// Observed, never declared: when a host last proved it still holds this.
    #[serde(default)]
    pub verified_at: Option<String>,
}

/// One disk cleaner's policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskCleanerPolicy {
    pub min_age_seconds: i64,
    /// Explicit opt-in to delete weles run directories WITHOUT durable
    /// upload proof (default false: age is reportable but never authorizes
    /// deletion).
    #[serde(default)]
    pub allow_missing_upload_proof: bool,
    /// Absolute path override for the cleaner's scan root (default: the
    /// cleaner's well-known location, e.g. ~/weles/recordings for weles).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// `release_store` only: how many newest versions of each product stay
    /// with no other reason to keep them — the rollback ladder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_newest: Option<i64>,
}

/// Disk-cleanup policy for a local target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskCleanupPolicy {
    pub mode: String,
    pub check_interval_seconds: i64,
    pub low_free_gb: i64,
    pub target_free_gb: i64,
    pub max_bytes_per_pass: i64,
    pub max_items_per_pass: i64,
    pub max_scan_items: i64,
    /// Seconds one pass may spend before it stops and hands its cursor on.
    ///
    /// Optional, and absent means the janitor's own `DEADLINE_SECONDS` — 30 —
    /// so nothing changes for a host that does not declare it. It exists
    /// because on 2026-09-02 this was the ONLY bound in this policy an
    /// operator could not declare, and it was the one that bound: the pass on
    /// `lukasz-macbook` reported `caps: {deadline: true, scan: false, items:
    /// false, bytes: false}` after crossing 59,588 of 879,559 directories,
    /// well under its declared `max_scan_items` of 100,000. Every other limit
    /// here was tunable and none of them was in the way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pass_seconds: Option<i64>,
    pub cleaners: BTreeMap<String, DiskCleanerPolicy>,
}

impl DiskCleanupPolicy {
    /// What a `local` target that declares no `disk_cleanup` is measured
    /// against.
    ///
    /// Before this existed, an undeclared host was not a host with a lenient
    /// policy — it was a host the janitor refused to look at, because
    /// `resolve_canonical_policy` treated a missing declaration as a lookup
    /// failure. `lukasz-macbook` builds and publishes everything this fleet
    /// ships and declared nothing, so nothing watched it: it reached 1.8 GiB
    /// free of 1.8 TiB carrying ~305 GB of cargo target trees, builds started
    /// dying with `No space left on device`, the CI runner could not write its
    /// own `_diag` pages, and the first anyone knew was four dead release
    /// trains later. The registry's silence was read as "nothing to do" rather
    /// than "nobody has said".
    ///
    /// `report`, deliberately, and this is the whole judgement in this
    /// function. A default that deleted would delete on hosts whose operator
    /// never asked for a janitor, which is a worse failure than the one it
    /// prevents. `report` performs the identical scan and counts every
    /// eligible item without unlinking one, so an undeclared host becomes
    /// VISIBLE — free space, pressure, and how much reclaimable cache it is
    /// sitting on — and arming it stays an explicit registry declaration.
    ///
    /// `build_caches` is the cleaner named here because it is the one that
    /// answers for this failure: it evicts only directories carrying a
    /// `CACHEDIR.TAG` written by the build tool itself, which is what cargo
    /// writes into every `target/`. Nothing else needs to be guessed at.
    pub fn reporting_default() -> Self {
        let mut cleaners = BTreeMap::new();
        cleaners.insert(
            "build_caches".to_string(),
            DiskCleanerPolicy {
                // A cache younger than a day may belong to a build in flight.
                min_age_seconds: 86_400,
                allow_missing_upload_proof: false,
                root: None,
                keep_newest: None,
            },
        );
        Self {
            mode: "report".to_string(),
            check_interval_seconds: 3_600,
            low_free_gb: 100,
            target_free_gb: 200,
            max_bytes_per_pass: 64 * 1024_i64.pow(3),
            max_items_per_pass: 512,
            max_scan_items: MAX_SCAN_ITEMS_CEILING,
            max_pass_seconds: None,
            cleaners,
        }
    }
}

/// One named alternative route for the target's SSH host-control channel.
///
/// The destination is still ordinary OpenSSH syntax. The name identifies the
/// network underneath it (`nebula`, `tailscale`, `wireguard`, `lan`, or any
/// fleet-specific path); list order is failover order. Keeping the network
/// label separate from the SSH transport lets Stado add routes without
/// duplicating remote-execution policy or host credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshConnectionPath {
    pub name: String,
    pub destination: String,
}

/// Stable name of the existing `ssh` destination when it is rendered beside
/// named fallback paths.
pub const PRIMARY_SSH_CONNECTION: &str = "primary";

/// The mobile automation runtime one host must carry.
///
/// This is the per-host statement of required software the fleet did not
/// have. `managed_versions` covers stado-managed release binaries under
/// `~/.stado/bin`; `placement_profiles` moves services between hosts and says
/// nothing about software; `capabilities` names capability families and their
/// providers, not host programs. So the requirement behind the iOS and
/// Android capture families lived nowhere, and the only trace of it in the
/// repository was the probe side: `deploy::host_exec` approved
/// `appium --version`, `appium driver list --installed`, `which adb` and
/// `adb devices -l` on 2026-09-03 so a placement could be asked whether it
/// can run, with nothing anywhere stating what the answer ought to be.
///
/// Declared here rather than hardcoded in the verifier for the reason
/// [`crate::deploy::weles_browser_runtime`] reads Playwright's revisions out
/// of the installed release instead of pinning them in Rust: a constant in
/// the checkout drifts from the fleet and then verifies the wrong thing.
// `Map<String, Value>` blocks `Eq`, for the reason spelled out at
// [`VerifyDescriptor`]: `serde_json` will not promise reflexivity for floats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MobileRuntime {
    /// Exact Appium server version, bare (`3.2.1`), never a range and never
    /// `latest`: a coordinate, for the reason
    /// [`crate::deploy::host_release`] refuses to resolve one.
    pub appium: String,
    /// Appium drivers this host must have installed, by their Appium driver
    /// name (`xcuitest`, `uiautomator2`). A version cannot answer for these:
    /// each is a separate install, and a placement fails at its first
    /// command without the one its platform needs.
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub drivers: Vec<String>,
    /// Whether this host must carry Android platform-tools, the package
    /// `adb` lives in. Declared as a requirement and not as a version: the
    /// vendor publishes one rolling `latest` archive per platform and stamps
    /// the build into `adb version`, so a pinned number here would be a
    /// promise the source cannot keep.
    #[serde(default)]
    pub platform_tools: bool,
    /// Keys this build does not model, kept verbatim, for the reason
    /// [`VerifyDescriptor::extra`] keeps them.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
