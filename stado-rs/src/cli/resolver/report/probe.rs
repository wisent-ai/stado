use std::net::SocketAddr;

use tokio::net::TcpStream;

use crate::monitor::host_silence;
use crate::service_resolution;
use crate::targets;
use crate::wait;

use crate::cli::resolver::authority::paths::target_ssh_paths;
use crate::cli::resolver::directory::source::SnapshotSource;

/// Whether something is accepting connections at a declared bind.
pub(super) async fn bind_listening(bind: &str) -> bool {
    let Ok(address) = bind.trim().parse::<SocketAddr>() else {
        return false;
    };
    wait::until(
        wait::Kind::Network,
        "a TCP connection to a declared bind",
        address,
        TcpStream::connect(address),
    )
    .await
    .is_ok()
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
    /// How long the authority took to answer over SSH, measured around the
    /// fetch; `None` for the local authority, whose document is in hand. An
    /// authority that answers after hours is the explanation of a stale
    /// snapshot and of every relay held to that host, so the answer says it.
    pub(super) answered_after_seconds: Option<u64>,
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
            answered_after_seconds: None,
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
            answered_after_seconds: None,
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
            answered_after_seconds: None,
        };
    }
    let source = SnapshotSource::Authority {
        target: directory.authority.target.clone(),
        ssh,
        command: directory.authority.command.clone(),
    };
    let asked = std::time::Instant::now();
    let fetched = source.fetch(host_silence::READER_CLI).await;
    let answered_after_seconds = Some(asked.elapsed().as_secs());
    match fetched {
        Ok((_, _, generation)) => AuthorityAnswer {
            source: "ssh",
            reachable: true,
            generation: Some(generation),
            detail: None,
            answered_after_seconds,
        },
        Err(detail) => AuthorityAnswer {
            source: "ssh",
            reachable: false,
            generation: None,
            detail: Some(detail.to_string()),
            answered_after_seconds,
        },
    }
}
