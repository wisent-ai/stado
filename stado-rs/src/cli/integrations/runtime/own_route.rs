//! A host process whose storage backend is the client route `stado` with a
//! loopback URL reads its registry and queue through an adapter its own
//! `--resolver` role binds. The worker, the release agent and the cleanup
//! watch read that store as soon as they start, and before the resolver had
//! bound they each answered "connection refused", ended with it, and took
//! the process down; launchd started it again and the race ran again. So
//! when the route is the process's own, those roles start after the
//! resolver publishes `serving` — on the resolver's own verdict, woken by the
//! kernel's notification of the publication, with no interval of ours.

use crate::cli::CmdError;

/// Whether this process's registry route is served by its own resolver.
fn reads_through_own_resolver(resolver: bool) -> bool {
    if !resolver {
        return false;
    }
    if !matches!(
        crate::config::wc_storage_backend(),
        "stado" | "stado-object"
    ) {
        return false;
    }
    url::Url::parse(crate::config::wc_stado_storage_url().trim())
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some_and(|host| host == "127.0.0.1" || host == "localhost" || host == "::1")
}

/// Wait for this process's resolver role to serve, when the store the other
/// roles read is behind it; a no-op otherwise.
pub(super) async fn await_own_resolver(resolver: bool) -> Result<(), CmdError> {
    if !reads_through_own_resolver(resolver) {
        return Ok(());
    }
    let pid = std::process::id();
    tokio::task::spawn_blocking(move || crate::cli::resolver::await_serving(pid))
        .await
        .map_err(|error| CmdError::click(format!("waiting on the resolver: {error}")))?
        .map_err(|cause| {
            CmdError::click(format!(
                "serve: the resolver this process reads its store through did not come to serve: {cause}"
            ))
        })
}
