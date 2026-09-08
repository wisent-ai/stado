use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpStream;

use crate::monitor::host_silence;
use crate::service_resolution;
use crate::targets;

use crate::cli::resolver::authority::paths::target_ssh_paths;
use crate::cli::resolver::directory::source::SnapshotSource;

/// How long a bind probe waits for a loopback connect. A listener on loopback
/// answers in microseconds or is not there; this is slack for a loaded
/// machine, not a network budget.
const BIND_PROBE: Duration = Duration::from_millis(250);

/// Budget for the whole authority probe. [`ssh_command`] already carries
/// `ConnectTimeout=10`; this bounds everything after the connect, so a
/// diagnostic can never hang on the thing it is diagnosing.
const AUTHORITY_PROBE: Duration = Duration::from_secs(20);

/// Whether something is accepting connections at a declared bind.
pub(super) async fn bind_listening(bind: &str) -> bool {
    let Ok(address) = bind.trim().parse::<SocketAddr>() else {
        return false;
    };
    matches!(
        tokio::time::timeout(BIND_PROBE, TcpStream::connect(address)).await,
        Ok(Ok(_))
    )
}

/// Seconds since an ISO 8601 stamp, `None` when it does not parse.
pub(super) fn age_seconds(stamp: &str) -> Option<i64> {
    let then = chrono::DateTime::parse_from_rfc3339(stamp).ok()?;
    Some((chrono::Utc::now() - then.with_timezone(&chrono::Utc)).num_seconds())
}

/// Where the authority's document came from, and whether it answered.
pub(super) struct AuthorityAnswer {
    /// `"local"` when this host is the authority, `"ssh"` otherwise.
    pub(super) source: &'static str,
    pub(super) reachable: bool,
    /// The generation the authority publishes, when it answered.
    pub(super) generation: Option<u64>,
    /// Why it did not, verbatim.
    pub(super) detail: Option<String>,
}

/// Ask the registry authority for the generation it publishes.
///
/// When this host IS the authority there is nothing to ask: the document in
/// hand came from the authority read, and whether that read reached the store
/// or fell back to the last-known-good copy is already known — `notice` is
/// `Some` exactly when it fell back.
pub(super) async fn probe_authority(
    registry: &targets::Registry,
    directory: &service_resolution::ServiceDirectory,
    target: &str,
    notice: Option<&str>,
) -> AuthorityAnswer {
    if directory.authority.target == target {
        return AuthorityAnswer {
            source: "local",
            reachable: notice.is_none(),
            generation: Some(directory.generation),
            detail: notice.map(str::to_string),
        };
    }
    let Some(authority) = registry.lookup(&directory.authority.target) else {
        return AuthorityAnswer {
            source: "ssh",
            reachable: false,
            generation: None,
            detail: Some(format!(
                "registry target {:?} is missing",
                directory.authority.target
            )),
        };
    };
    let ssh = target_ssh_paths(authority);
    if ssh.is_empty() {
        return AuthorityAnswer {
            source: "ssh",
            reachable: false,
            generation: None,
            detail: Some(format!(
                "registry target {:?} has no SSH connection path",
                directory.authority.target
            )),
        };
    }
    let source = SnapshotSource::Authority {
        target: directory.authority.target.clone(),
        ssh,
        command: directory.authority.command.clone(),
    };
    match tokio::time::timeout(AUTHORITY_PROBE, source.fetch(host_silence::READER_CLI)).await {
        Ok(Ok((_, _, generation))) => AuthorityAnswer {
            source: "ssh",
            reachable: true,
            generation: Some(generation),
            detail: None,
        },
        Ok(Err(detail)) => AuthorityAnswer {
            source: "ssh",
            reachable: false,
            generation: None,
            detail: Some(detail),
        },
        Err(_) => AuthorityAnswer {
            source: "ssh",
            reachable: false,
            generation: None,
            detail: Some(format!(
                "no answer from the registry authority within {}s",
                AUTHORITY_PROBE.as_secs()
            )),
        },
    }
}
