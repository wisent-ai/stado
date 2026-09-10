//! What a service directory is made of: the authority that owns it, one
//! route per logical service, the endpoint each caller reaches it at, and the
//! resolver configuration a host runs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceDirectory {
    pub authority: ServiceAuthority,
    pub generation: u64,
    pub services: BTreeMap<String, ServiceRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceAuthority {
    pub target: String,
    /// Absolute Stado executable on the authority host.
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceRoute {
    #[serde(default)]
    pub placement_profile: Option<String>,
    /// Exact managed-service declaration for a fixed service. Placement-backed
    /// services derive this from their profile and must leave it absent.
    #[serde(default)]
    pub managed_service: Option<String>,
    pub active_host: String,
    pub endpoints: BTreeMap<String, ServiceEndpoint>,
    /// Addresses hosts would serve on if the service moved to them, never
    /// addresses to call ([`crate::targets::Service::standby`]).
    ///
    /// Nothing in this module resolves through it — a resolver hands out the
    /// active host's endpoint and a standby address is by construction not
    /// serving. It is modelled here for the asymmetry recorded on `verify`
    /// below: this reader denies unknown keys where `targets::Service`
    /// tolerates them, so a field added on the tolerant side alone takes
    /// every resolver on the fleet down the moment one directory entry is
    /// published carrying it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub standby: BTreeMap<String, ServiceEndpoint>,
    pub consumers: BTreeMap<String, ServiceConsumer>,
    /// How this route is checked against the world
    /// ([`crate::targets::Service::verification`] derives the default when it
    /// is absent).
    ///
    /// Modelled here because two readers parse this same entry with opposite
    /// strictness: `targets::Service` keeps unmodelled keys in a
    /// `serde(flatten)` `extra`, this one denies them outright. A field added
    /// to satisfy the tolerant reader alone would take the resolver down
    /// fleet-wide the moment it was published — every host refusing the whole
    /// directory over a key it merely did not know. Any future field on a
    /// service entry has to land in both places, in the same change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<crate::targets::VerifyDescriptor>,
    /// The deployable half of the declaration
    /// ([`crate::targets::Service::declaration`]). Kept in lockstep with the
    /// tolerant reader per the note on `verify` above: both readers must
    /// learn a new entry field in the same change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration: Option<crate::declaration::ServiceDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceEndpoint {
    /// Host-relative base URL. Loopback means loopback on `active_host`, not
    /// on the workload host.
    pub url: String,
    /// Which release is answering here, as `<product>@<version>`.
    ///
    /// Modelled in BOTH readers in this one change, exactly as the note on
    /// [`ServiceRoute::verify`] requires: `targets::ServiceEndpoint` keeps
    /// unmodelled keys in a flattened `extra` and this reader denies them, so
    /// publishing a field only the tolerant side knew would take every
    /// resolver in the fleet down over a key it merely did not recognize.
    /// `registry validate` refused precisely that on 2026-09-03 -- "unknown
    /// field `release_id`, expected `url`" -- which is the check doing its job.
    ///
    /// On the endpoint and not on the service because it is a fact about one
    /// address: a service with a standby endpoint would otherwise carry one
    /// release id for two different processes. Optional, because a directory
    /// entry written before a release identity was published is still a valid
    /// entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_id: Option<String>,
    /// The exact source revision that release was built from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
    /// The canonical API base UNDER that origin, e.g. `/api/v1`.
    ///
    /// Separate from `url` because the directory's endpoint is deliberately an
    /// origin: [`validate_endpoint`] requires host-relative loopback with a
    /// known port and no path, and the resolver's whole contract is built on
    /// that shape. A consumer that needs the versioned base -- Spis, asking
    /// the public task API for browser evidence -- needs it stated rather than
    /// guessed, and rewriting `url` to carry it would break every reader that
    /// composes its own paths onto the origin. So the base is published beside
    /// the origin, and the two compose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConsumer {
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolverConfig {
    pub api_bind: String,
    #[serde(default = "default_refresh_seconds")]
    pub refresh_seconds: u64,
    #[serde(default = "default_max_stale_seconds")]
    pub max_stale_seconds: u64,
    pub adapters: Vec<ResolverAdapter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolverAdapter {
    pub service: String,
    pub bind: String,
    pub consumer: String,
    #[serde(default = "default_adapter_idle_seconds")]
    pub idle_seconds: u64,
    #[serde(default = "default_adapter_connect_seconds")]
    pub connect_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedService {
    pub name: String,
    pub generation: u64,
    pub active_host: String,
    pub endpoint: ServiceEndpoint,
    pub ssh: Option<String>,
    pub ssh_fallbacks: Vec<crate::targets::SshConnectionPath>,
    pub capabilities: Vec<String>,
}

fn default_refresh_seconds() -> u64 {
    5
}

fn default_max_stale_seconds() -> u64 {
    60
}

/// Short enough that retained sockets stay bounded: two directory-freshness
/// windows.
///
/// A request/response connection sends nothing in either direction while the
/// service works, so this window is also a cap on how long a proxied service may
/// take to answer. Model dispatch legitimately exceeds two minutes, and raising
/// this default to cover it tripled retention for every adapter on the fleet --
/// which exhausted the resolver's file descriptors and took the whole local data
/// plane down with `Too many open files`. The long window belongs on the
/// adapters that need it, declared per adapter in the registry, not on
/// everything.
fn default_adapter_idle_seconds() -> u64 {
    default_max_stale_seconds().saturating_add(default_max_stale_seconds())
}

/// Budget for the first upstream byte on a freshly proxied connection.
///
/// Establishment is the one window where `idle_seconds` cannot help: nothing
/// has flowed yet, so a dead backend would otherwise hold the client until the
/// idle window lapses. Ten seconds is generous for a healthy TCP connect plus
/// SSH channel open, and adapters fronting a service that legitimately answers
/// slowly declare a larger budget per adapter, the same way they declare
/// `idle_seconds`.
fn default_adapter_connect_seconds() -> u64 {
    10
}

