use super::*;

// ---------------------------------------------------------------------------
// registry.json — native build recipes
// ---------------------------------------------------------------------------

/// Top-level registry key holding the fleet's build recipes. Unmodelled by
/// [`Registry`] itself, so the array round-trips through [`Registry::extra`]
/// and a writer of any vintage keeps it.
pub const BUILDS_KEY: &str = "builds";

/// Top-level registry key that, when `true`, halts all build polling
/// fleet-wide (the scheduler's kill switch).
pub const BUILDS_DISABLED_KEY: &str = "builds_disabled";

fn default_build_interval_seconds() -> u64 {
    300
}

/// One release platform's job-routing coordinates: every spelling of its
/// operating system and architecture the fleet writes down, canonical first.
///
/// Two spellings for one machine is not a hypothetical: enrollment reads
/// `uname` (`Darwin`/`arm64`), Rust reads `std::env::consts` (`macos`/
/// `aarch64`), the sandbox box declares `x86_64` and the cloud optimizer
/// accepts `amd64`. A router that knows one of them refuses jobs that name
/// the same host by another word, so every spelling this repository writes
/// belongs in one table rather than at each comparison.
struct PlatformRouting {
    platform: &'static str,
    /// [`Job::platform_os`] spellings; the first is what a build job declares.
    os: &'static [&'static str],
    /// [`Job::architecture`] spellings; the first is what a build job declares.
    arch: &'static [&'static str],
}

/// The platform words of [`crate::deploy::products::PLATFORMS`], each with
/// the job fields that route work to it.
const PLATFORM_ROUTING: [PlatformRouting; 2] = [
    PlatformRouting {
        platform: "darwin-arm64",
        os: &["darwin", "macos"],
        arch: &["arm64", "aarch64"],
    },
    PlatformRouting {
        platform: "linux-amd64",
        os: &["linux"],
        arch: &["amd64", "x86_64"],
    },
];

/// A platform the installer knows but nothing can route work to is a build
/// that submits jobs no host will ever claim. Fail the build of whoever adds
/// the platform word instead.
const _: () = assert!(PLATFORM_ROUTING.len() == crate::deploy::products::PLATFORMS.len());

fn platform_routing(platform: &str) -> Option<&'static PlatformRouting> {
    PLATFORM_ROUTING
        .iter()
        .find(|entry| entry.platform == platform)
}

/// The `(platform_os, architecture)` a job must declare to be routed to
/// `platform`, or `None` when the word is not a release platform.
///
/// The one place a platform word becomes job fields: the build poller and
/// `stado builds run` submit byte-identical routing, and the claiming agent
/// reads the same table back through [`platform_accepts_job`].
pub fn platform_job_os_arch(platform: &str) -> Option<(&'static str, &'static str)> {
    let routing = platform_routing(platform)?;
    Some((routing.os[0], routing.arch[0]))
}

/// Whether a host running `platform` may run a job declaring `platform_os`
/// and `architecture`.
///
/// An empty job field is no constraint — every job submitted before platform
/// routing existed carries two of them, and they stay claimable everywhere.
/// An unknown `platform` accepts nothing constrained: a host the release
/// pipeline does not publish for cannot be the intended target of a job that
/// names a platform.
pub fn platform_accepts_job(platform: &str, platform_os: &str, architecture: &str) -> bool {
    if platform_os.is_empty() && architecture.is_empty() {
        return true;
    }
    let Some(routing) = platform_routing(platform) else {
        return false;
    };
    let names = |spellings: &[&str], value: &str| {
        value.is_empty()
            || spellings
                .iter()
                .any(|spelling| spelling.eq_ignore_ascii_case(value))
    };
    names(routing.os, platform_os) && names(routing.arch, architecture)
}

/// The recorded outcome of one platform's most recent build job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRun {
    /// "succeeded" | "failed" | "running" | "unclaimable".
    pub status: String,
    /// RFC3339 timestamp of when the run was recorded.
    pub at: String,
    /// Queue job id of the build job. Empty for a run that was refused
    /// before submission (`unclaimable`): there is no job to point at, and
    /// an id that names no job must not be offered as one.
    pub job_id: String,
    /// Store-relative URIs of the uploaded artifacts, empty while running.
    #[serde(default)]
    pub artifact_uris: Vec<String>,
    /// Exact semantic version the built commit carries, when the sha is
    /// tagged; `None` for an untagged commit, which is most of them. A build
    /// never invents one: an untagged commit produces artifacts and no
    /// version, and nothing downstream may be declared from it.
    #[serde(default)]
    pub version: Option<String>,
    /// Whether this run's [`BuildRun::version`] was declared as the fleet's
    /// managed version for this platform (`auto_declare`).
    #[serde(default)]
    pub declared: bool,
    /// Why the run ended the way it did, when one sentence says it better
    /// than the status word alone: the supervision diagnosis on a `failed`
    /// run ("no worker claimed the job within 10m") or the refusal on an
    /// `unclaimable` one. Absent on ordinary outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One native build recipe: a repository the control plane polls, the
/// platforms it builds for, and the command it runs in a fresh shallow
/// checkout on each of them when the branch head moves.
///
/// Boundary: a build produces artifacts under each job's canonical results
/// URI and, with `auto_declare`, a managed-version declaration for the hosts
/// on the run's platform. It NEVER writes `release_control.products` — a
/// signed release is promoted by `stado release promote`, which verifies the
/// manifest and its signature against a release key. Building a
/// commit is evidence that it compiles; promoting it is a deliberate act on
/// verified artifacts, and collapsing the two would let a poller publish an
/// unsigned release to the fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRecipe {
    /// Unique kebab-case recipe name.
    pub name: String,
    /// HTTPS clone URL of the repository to build.
    pub repo: String,
    /// Branch the poller watches.
    #[serde(rename = "ref")]
    pub branch: String,
    /// Single POSIX sh build command run in the checkout.
    pub command: String,
    /// Paths in the checkout to upload as build artifacts.
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// Release platforms this recipe builds for, one job each. Words from
    /// [`crate::deploy::products::PLATFORMS`]; a recipe with none builds
    /// nothing, and `stado builds add` requires at least one.
    ///
    /// Null-tolerant like every other registry collection: a recipe entry
    /// that does not parse is dropped from [`read_build_recipes`] silently,
    /// so one hand-written `null` would take a recipe out of the poller AND
    /// out of every listing with nothing said.
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub platforms: Vec<String>,
    /// Explicit opt-in: a freshly added recipe never builds until enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Declare a succeeded run's version as the managed version for every
    /// host on that run's platform. Off by default: a repository that builds
    /// on every commit must not move the fleet on every commit.
    #[serde(default)]
    pub auto_declare: bool,
    /// Poll cadence in seconds.
    #[serde(default = "default_build_interval_seconds")]
    pub interval_seconds: u64,
    /// Branch head the poller last enqueued builds for.
    #[serde(default)]
    pub last_seen_ref: Option<String>,
    /// Most recent run per platform, keyed by the platform word. Absent for
    /// a recipe that has never built. See [`BuildRecipe::platforms`] for why
    /// an explicit null reads as empty.
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub runs: BTreeMap<String, BuildRun>,
}

/// The registry's build recipes. An absent `builds` key and entries that do
/// not parse both yield nothing: a recipe this build cannot model must not
/// stop the ones it can.
///
/// Absent keys are absent values, never a refusal — a recipe written before
/// `platforms`, `auto_declare` or `runs` existed parses with them empty (and
/// so builds nothing until an operator names its platforms), and a key this
/// build no longer models is dropped on the next write rather than blocking
/// the read.
pub fn read_build_recipes(registry: &Registry) -> Vec<BuildRecipe> {
    registry
        .extra
        .get(BUILDS_KEY)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Replace the registry's build recipes. The key lives in
/// [`Registry::extra`], so [`Registry::to_document`] carries it into the
/// canonical document like any other unmodelled block.
pub fn write_build_recipes(registry: &mut Registry, recipes: &[BuildRecipe]) {
    registry.extra.insert(
        BUILDS_KEY.to_string(),
        serde_json::to_value(recipes).expect("build recipes serialize infallibly"),
    );
}
