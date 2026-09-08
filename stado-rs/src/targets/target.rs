use super::*;

/// One routable box. Unknown registry keys land in
/// [`ComputeTarget::extra`] (Python's `extra` dict), via `#[serde(flatten)]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputeTarget {
    pub name: String,
    /// "local" | "gcp" | "vast"
    pub kind: String,
    /// Verified immutable-release coordinate for this host. Enrollment records
    /// it and every inventory compares it with the remote kernel/architecture.
    #[serde(default)]
    pub release_platform: String,
    #[serde(default)]
    pub gpu_type: Option<String>,
    #[serde(default)]
    pub ssh: Option<String>,
    /// Ordered alternative network routes for the same SSH host channel.
    ///
    /// `ssh` remains the preferred route and the compatibility surface for
    /// older fleet binaries. These paths are tried only after a side-effect-free
    /// SSH handshake says an earlier route is unavailable; the real remote
    /// operation is sent exactly once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh_fallbacks: Vec<SshConnectionPath>,
    /// The target whose channel key authenticates to this one, when that is
    /// not this target itself.
    ///
    /// The host channel materializes `stado-ssh-<target name>` from the
    /// credential store, so a target's name IS its key coordinate — which
    /// silently rules out two targets on one machine. A leased scratch target
    /// is exactly that case: a second login on a box whose key was minted
    /// once, under the box's name, and copied into the leased account's
    /// `authorized_keys`. Declaring the key's owner keeps one key per machine
    /// instead of minting a fresh identity for something that lives for an
    /// hour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key_target: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub spot: bool,
    #[serde(default)]
    pub team_id: Option<i64>,
    /// Stable placement class used by operators and service declarations.
    #[serde(default)]
    pub role: Option<String>,
    /// Declarative selector resolved to exactly one local target.
    #[serde(default)]
    pub host_heuristic: Option<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub hostnames: Vec<String>,
    #[serde(default)]
    pub weles: Option<WelesPolicy>,
    /// Identities this host is expected to hold. Empty for the ordinary compute
    /// target that holds none.
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub identities: Vec<IdentityBinding>,
    /// Skarbiec item holding this host's machine account, by item id
    /// (`host-account-<name>`). It is the only pointer from a host name to the
    /// credential that logs into that host, so it is modelled rather than left
    /// in [`ComputeTarget::extra`]: host repair has to follow it from Rust, and
    /// `registry doctor` reports an unmodelled, uncatalogued target key as a
    /// declaration with no reader — correctly, while it sits there.
    ///
    /// Read today by `scripts/read-host-account.py`, which resolves the pointer
    /// and fails when the vault holds no such item or the item names a different
    /// host, and by `scripts/put-host-account.py`, which refuses to write a
    /// credential the registry does not point at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<String>,
    #[serde(default)]
    pub disk_cleanup: Option<DiskCleanupPolicy>,
    /// The memory twin of [`ComputeTarget::disk_cleanup`]: what this host is
    /// allowed to reclaim when its memory, rather than its disk, is the scarce
    /// resource. A target that declares none is not exempt — it is measured
    /// against
    /// [`crate::providers::local::host_memory::schema::MemoryReclaimPolicy::reporting_default`],
    /// which reports and reclaims nothing, so an undeclared host is visible
    /// rather than invisible.
    ///
    /// The type lives beside the pass that reads it rather than here, for the
    /// reason [`ComputeTarget::display_stream`] carries
    /// [`crate::stream::schema::DisplayStream`]: the registry declares which
    /// hosts hold the capability, and the capability owns the shape of its own
    /// declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_reclaim: Option<crate::providers::local::host_memory::schema::MemoryReclaimPolicy>,
    /// Interactive display session this host renders and streams, when it has
    /// one. Read by `cli::stream` and `deploy::stream`; absent means the host is
    /// headless, which is what every host is until somebody declares otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_stream: Option<crate::stream::schema::DisplayStream>,
    /// env_overrides and agent_args propagate via the GCS registry to
    /// running agents — the agent compares them every poll and
    /// exits-for-restart when they change, so systemd brings it back up
    /// with the new env / CLI flags.
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub env_overrides: Map<String, Value>,
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub agent_args: Vec<String>,
    /// vram_gb is used by the agent to expand its capacity broadcast to
    /// every GCP gpu_type whose required VRAM ≤ this value
    /// (compatibility-list broadcast). Without it, the agent only
    /// advertises gpu_type as-is.
    #[serde(default)]
    pub vram_gb: Option<i64>,
    /// pinned_only=true: this host's agent claims ONLY jobs explicitly
    /// routed to it (Job.pinned_host or coordinator assigned_to). Keeps
    /// shared workstations from picking up stray queue backlog.
    #[serde(default)]
    pub pinned_only: bool,
    /// The mobile automation runtime this host must carry, when it is a host
    /// the mobile capture families are placed on. Absent means the host is
    /// not a mobile placement, which is what every host is until somebody
    /// declares otherwise — the same default as [`Self::display_stream`].
    ///
    /// Separate from [`Self::managed_versions`] on purpose, and the
    /// separation is the whole reason this field exists. `managed_versions`
    /// declares stado-managed binaries under `~/.stado/bin`, delivered by
    /// `host release` out of a release Stado published and verified by
    /// digest; every version diagnostic in the fleet
    /// (`deploy::service_converge`, `host reconcile`) enumerates it and looks
    /// for `$HOME/.stado/bin/<name>`. Appium is an npm package and `adb` is
    /// Google's, neither is a Stado release artefact, and neither lives
    /// there, so declaring them under `managed_versions` would have produced
    /// drift rows nothing could ever deliver against — a declaration with no
    /// reader, the exact shape `host_software` was written to remove.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mobile_runtime: Option<MobileRuntime>,
    /// Required version of each stado-managed binary under `~/.stado/bin`,
    /// keyed by binary name (`stado`, `skarbiec`) and holding the bare
    /// version number (`0.5.1`), never a prefixed banner like
    /// `stado 0.5.1`.
    ///
    /// This is the registry's DECLARATION of target state, and it is the
    /// half that was missing: `stado host inventory` could always read what
    /// a host actually runs, and had nothing to compare it against, so a
    /// host three releases behind looked exactly like a host at the tip.
    /// Optional on purpose — a target that declares nothing is reported as
    /// `undeclared` rather than as drift, so every registry written before
    /// this field existed stays valid.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub managed_versions: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ComputeTarget {
    /// Preferred SSH destination followed by its declared fallback routes.
    ///
    /// This iterator borrows registry storage: selecting a route does not copy
    /// destinations on the normal path.
    pub fn ssh_connections(&self) -> impl Iterator<Item = (&str, &str)> {
        self.ssh
            .as_deref()
            .filter(|destination| !destination.trim().is_empty())
            .map(|destination| (PRIMARY_SSH_CONNECTION, destination))
            .into_iter()
            .chain(
                self.ssh_fallbacks
                    .iter()
                    .map(|path| (path.name.as_str(), path.destination.as_str())),
            )
    }

    pub fn has_ssh_connection(&self) -> bool {
        self.ssh_connections().next().is_some()
    }

    /// The credential-store identity the host channel authenticates with:
    /// the declared key owner, or this target's own name.
    pub fn channel_key(&self) -> &str {
        self.ssh_key_target.as_deref().unwrap_or(&self.name)
    }
    pub fn provider(&self) -> Option<crate::capabilities::ProviderId> {
        crate::capabilities::variant(crate::capabilities::RuntimeFacet::HostTarget, &self.kind)
            .and_then(|variant| variant.provider)
    }

    pub fn is_provider(&self, provider: crate::capabilities::ProviderId) -> bool {
        self.provider() == Some(provider)
    }

    /// The version the registry requires of one stado-managed binary on
    /// this host, or `None` when it declares none.
    pub fn declared_version(&self, binary: &str) -> Option<&str> {
        self.managed_versions.get(binary).map(String::as_str)
    }

    /// Desired NVIDIA board power cap for this host. The field remains in
    /// `extra` so older Stado binaries preserve it during registry rewrites;
    /// validation above guarantees the accessor cannot observe zero, a
    /// negative value, or an integer wider than the driver accepts.
    pub fn gpu_power_limit_watts(&self) -> Option<u32> {
        self.extra
            .get("gpu_power_limit_watts")
            .and_then(Value::as_u64)
            .and_then(|watts| u32::try_from(watts).ok())
    }
}
