use super::*;

/// One directory entry: where a service currently runs, and who may call it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Service {
    /// Placement profile that relocates this service, when it belongs to one.
    /// Profile members move as a group, in the profile's declared order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_profile: Option<String>,
    /// launchd/systemd unit that owns this service when no placement profile
    /// does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_service: Option<String>,
    /// Registry target currently serving it. Consumers resolve
    /// [`Service::endpoints`] with their OWN name, not with this one: the
    /// active host's entry is simply the address that host uses, which for a
    /// loopback service is also where it serves.
    pub active_host: String,
    /// The address a host USES to reach this service, keyed by the machine
    /// ASKING and never by the machine serving. These services bind loopback
    /// on their own box, so "where is Brama" has a different true answer per
    /// client and the directory states each one instead of leaving every
    /// caller to derive it. `service directory publish` writes
    /// `endpoints[<this host>]` into that host's
    /// `~/.stado/forwards/<service>.local`, which is the file consumers on it
    /// actually read — so an entry here is a promise to that host that the
    /// address works from where it stands.
    ///
    /// One meaning only, now. This comment used to say that a host carrying
    /// an endpoint is not thereby serving, which reads the map as "where each
    /// host would serve", while `publish` handed the same string out as a
    /// number to dial. Both readings survived the type, so on 2026-08-11
    /// `service verify` reported `brama` unreachable on a laptop that merely
    /// stands by for it and the entry had to be silenced by hand. The other
    /// meaning now has [`Service::standby`] and this one has nothing else to
    /// mean.
    #[serde(default)]
    pub endpoints: BTreeMap<String, ServiceEndpoint>,
    /// The address a host would serve on if the service moved there — which
    /// is not a promise that anything answers there now.
    ///
    /// A standby host is by definition not running the service, so silence on
    /// this address is the declared state and not a fault. Nothing may probe
    /// it and call the result a verdict: `service verify` lists these as
    /// `unverified` rows so the address is visible before the move rather
    /// than during it, and never counts one as a failure. Read it through
    /// [`Service::standby_for`], the dial address through
    /// [`Service::address_for`], and neither ever falls back to the other.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub standby: BTreeMap<String, ServiceEndpoint>,
    #[serde(default)]
    pub consumers: BTreeMap<String, ServiceConsumer>,
    /// How this declaration is observed, when its author said. Read it through
    /// [`Service::verification`], never directly: absent means the derived
    /// default, and a reader that treats absent as "do not check" reinstates
    /// the unverifiable declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifyDescriptor>,
    /// The deployable half of the declaration: where the bytes come from and
    /// what the unit runs. Absent on entries declared before the contract
    /// existed; older builds keep it verbatim in `extra`, so no writer drops
    /// it silently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration: Option<crate::declaration::ServiceDeclaration>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Service {
    /// How to check this declaration: what the author wrote, or the default
    /// derived for them.
    ///
    /// Deriving instead of requiring is deliberate, and the reason is custody
    /// rather than convenience. A required field is a gate, and the key to
    /// this one is out of reach: the canonical registry document cannot be
    /// rewritten without a signing key held elsewhere. "Every service declares
    /// how it is verified" would therefore land as an error against every
    /// entry that already exists, raised by a build that cannot fix a single
    /// one of them — a validator that fails and a verifier that refuses, while
    /// the declarations go on being unchecked. That is a worse position than
    /// the one this replaces, because it looks like progress.
    ///
    /// Derivation makes every declaration verifiable the day this lands and
    /// still leaves writing it down worth doing: the author whose service is
    /// not HTTP-from-every-endpoint-holder says so, and is the only one who
    /// has to.
    ///
    /// The default is exactly the probe `service verify` already ran, so no
    /// entry changes verdict because this field came into existence.
    pub fn verification(&self) -> VerifyDescriptor {
        self.verify.clone().unwrap_or_default()
    }

    /// The address `host` is told to dial, or `None` if the directory hands
    /// it none.
    ///
    /// [`Service::endpoints`] only. A standby address is never a fallback
    /// here: its one declared property is that nothing is listening on it
    /// yet, so returning it would answer "what do I call" with an address
    /// chosen for being dead.
    pub fn address_for(&self, host: &str) -> Option<&ServiceEndpoint> {
        self.endpoints.get(host)
    }

    /// The address `host` would serve on after a move, or `None` if it is not
    /// standing by for this service.
    ///
    /// [`Service::standby`] only, for the same reason in reverse. The pair
    /// exists so that no caller has to decide which map answers its question:
    /// one command reading `endpoints` as "would serve here" while another
    /// read it as "call this" is what cost `brama` a false `unreachable` on
    /// 2026-08-11.
    pub fn standby_for(&self, host: &str) -> Option<&ServiceEndpoint> {
        self.standby.get(host)
    }
}

/// The fleet's service directory: the single answer to "where does X run
/// right now, and may I call it".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceDirectory {
    pub authority: DirectoryAuthority,
    /// Bumped by the authority on every publication. A consumer that cached
    /// an older generation is holding endpoints that may already point at a
    /// host which has handed the service over — see
    /// [`ServiceDirectoryError::Stale`].
    pub generation: u64,
    #[serde(default)]
    pub services: BTreeMap<String, Service>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
