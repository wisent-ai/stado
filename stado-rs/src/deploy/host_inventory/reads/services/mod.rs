//! What is running on this host: the artefacts its service units execute,
//! the forward markers they publish, the kernel socket table that says
//! whether anything still answers there, and the two axes one marker is
//! judged on.

use serde::{Deserialize, Serialize};

use super::super::*;
use crate::targets::{ComputeTarget, ServiceDirectory};

/// One service unit's live artefact: what `current` points at, and how old it
/// is beside the installed program of the same name.
///
/// The epochs are strings for the reason every other field here is one — the
/// host's sanitizer emits text, and an unreadable `stat` has to arrive as
/// absent rather than as zero, which would compare as 1970 and call every
/// artefact stale.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceArtifact {
    /// The service directory's name, which is the unit label.
    pub label: String,
    /// The `current` symlink's target, normally a `sha256-…` directory.
    pub current_target: String,
    /// The executable's file name inside the artefact.
    pub program: String,
    pub artefact_epoch: String,
    /// Mtime of `$HOME/.stado/bin/<program>`, when that program is installed.
    pub installed_epoch: String,
    /// What the artefact answers `--version` with, for the declared programs
    /// this fleet may execute. An mtime says when a file was written, never
    /// what is inside it.
    #[serde(default)]
    pub artefact_version: String,
    #[serde(default)]
    pub installed_version: String,
}

impl ServiceArtifact {
    fn epoch(value: &str) -> Option<i64> {
        value.trim().parse().ok()
    }

    /// Is the artefact this unit executes at least as new as the installed
    /// program of the same name? `None` when either mtime is missing — the
    /// absence of a fact, never folded into `true`.
    pub fn at_least_as_new_as_installed(&self) -> Option<bool> {
        let artefact = Self::epoch(&self.artefact_epoch)?;
        let installed = Self::epoch(&self.installed_epoch)?;
        Some(artefact >= installed)
    }

    /// How many seconds older than the installed program this artefact is.
    pub fn seconds_behind_installed(&self) -> Option<i64> {
        Some(Self::epoch(&self.installed_epoch)? - Self::epoch(&self.artefact_epoch)?)
    }
}

/// One `$HOME/.stado/forwards/*.url` marker, as read (or refused).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardMarker {
    pub name: String,
    /// [`MARKER_READ`], or `refused_symlink`, `refused_not_regular`,
    /// `refused_unreadable`, `read_empty`. Every one of those is a reason
    /// `url` is blank, stated where the blank is, so an empty `url` is never
    /// something this side has to interpret.
    pub state: String,
    pub url: String,
}

/// One listening TCP socket and the pid that owns it.
///
/// Every interface, not only loopback: an undeclared proxy on this host's
/// tailnet address served the fleet's external object traffic for five days
/// and could not appear here, because the collector dropped anything that was
/// not loopback and then kept only the first row per port. Readers that only
/// accept a loopback endpoint — [`verdict`], for one — test the address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listener {
    /// `127.0.0.1`, `::1`, or `*` for a socket bound to every interface —
    /// which answers on loopback too, and so can satisfy a marker.
    pub address: String,
    pub port: u32,
    pub pid: u32,
}

/// The loopback port a forward marker points at.
///
/// A marker is the one line `stado route open --remote` writes, for example
/// `http://127.0.0.1:8766`. A marker that is not that shape has no port to
/// reconcile, and saying so beats guessing one.
pub fn marker_port(url: &str) -> Option<u32> {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))?;
    let authority = rest.split('/').next()?;
    let (host, port) = authority.rsplit_once(':')?;
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return None;
    }
    let port: u32 = port.parse().ok()?;
    if port == u32::MIN || port > u32::from(u16::MAX) {
        return None;
    }
    Some(port)
}

/// One marker's verdict against the listener table: its port, and whether
/// anything is actually listening on it.
///
/// `listeners_state` is not decoration. A marker can only be called
/// [`STALE`] when the socket table it was checked against was actually read;
/// otherwise the verdict is [`UNKNOWN`], because "nothing is listening" and
/// "nothing could be asked" are opposite findings that look identical in an
/// empty `Vec<Listener>`.
pub fn verdict(
    marker: &ForwardMarker,
    listeners: &[Listener],
    listeners_state: &str,
) -> (Option<u32>, &'static str) {
    if marker.state != MARKER_READ {
        return (None, UNREADABLE);
    }
    let Some(port) = marker_port(&marker.url) else {
        return (None, UNREADABLE);
    };
    if listeners_state != LISTENERS_READ {
        return (Some(port), UNKNOWN);
    }
    // A forward marker names a LOOPBACK endpoint, so only a loopback-capable
    // socket can satisfy one. The collector no longer filters interfaces —
    // that filter is why three servers on one port could not be represented —
    // so the restriction lives here, where it is actually meant.
    let listening = listeners.iter().any(|listener| {
        listener.port == port
            && (listener.address == "*"
                || listener.address == "::1"
                || listener.address.starts_with("127."))
    });
    (Some(port), if listening { MATCHED } else { STALE })
}

/// The endpoint the registry declares for one service ON THIS host.
///
/// `endpoints[target]`, not the active host's endpoint: the question a
/// marker answers is "what should this box's forward point at", and a host
/// standing by for a service still carries a declared endpoint for it. A
/// service the directory does not name, or names without an entry for this
/// host, has nothing declared here and yields `None`.
pub fn declared_endpoint<'a>(
    directory: Option<&'a ServiceDirectory>,
    target: &ComputeTarget,
    service: &str,
) -> Option<&'a str> {
    directory?
        .services
        .get(service)?
        .endpoints
        .get(&target.name)
        .map(|endpoint| endpoint.url.as_str())
}

/// The address this target dials for a service the directory places
/// elsewhere: the bind of its own resolver adapter, as the target declares it.
///
/// The declared address of a marker has two sources, and reading only the
/// first is what made this axis wrong for every non-serving host. The
/// directory's `endpoints` map is the address a host SERVES on; a host that
/// does not serve the service reaches it through its own adapter, and that is
/// the address `service directory publish` writes into the marker. Comparing
/// such a marker with `endpoints` alone reported a correct file as
/// `undeclared` or `disagrees`.
///
/// One adapter is an address. Several are per consumer, and a marker names no
/// consumer, so nothing here elects one: the marker stays judged against the
/// directory alone, which is what `publish` also refuses to guess.
pub fn declared_adapter(target: &ComputeTarget, service: &str) -> Option<String> {
    let declared = target.extra.get("service_resolver")?;
    let config: crate::service_resolution::ResolverConfig =
        serde_json::from_value(declared.clone()).ok()?;
    let mut matches = config
        .adapters
        .iter()
        .filter(|adapter| adapter.service == service);
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(format!("http://{}", first.bind))
}

/// One marker's verdict against the REGISTRY: [`MATCHED`], [`DISAGREES`] or
/// [`UNDECLARED`].
///
/// This is a second axis, not a refinement of [`verdict`]. That one asks
/// whether anything is listening where the marker points; this one asks
/// whether the marker points where the fleet's own directory says it
/// should. They answer independently, and a marker that passes the first
/// and fails the second is the case worth catching: something answers, so
/// the host looks healthy, and consumers resolving through the directory
/// are sent somewhere else entirely.
///
/// When both sides are loopback endpoints the PORT is compared, because
/// `http://localhost:8895` and `http://127.0.0.1:8895` are one endpoint
/// written two ways and calling that a disagreement buries the real ones.
/// When either side is not, exact text is all that can be honestly
/// compared. A marker that could not be read cannot agree with anything, so
/// it lands on [`DISAGREES`]: the registry declares an endpoint this host
/// is not stating.
pub fn declaration_verdict(marker: &ForwardMarker, declared: Option<&str>) -> &'static str {
    let Some(declared) = declared else {
        return UNDECLARED;
    };
    let url = marker.url.trim();
    let agrees = match (marker_port(url), marker_port(declared)) {
        (Some(found), Some(wanted)) => found == wanted,
        _ => url == declared.trim(),
    };
    if agrees {
        MATCHED
    } else {
        DISAGREES
    }
}
