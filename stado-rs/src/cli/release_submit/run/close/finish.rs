//! The pass that turns a queued submission into a published release without
//! anyone waiting in a terminal.
//!
//! `stado release submit` ends when the platform builds are queued. What used
//! to follow inside that same client process - waiting for every builder,
//! signing, publishing, delivering - is this pass, run by the control host's
//! release agent on every tick. It picks only runs whose builds have reached
//! a terminal queue state, so it never blocks the agent on a builder, and it
//! walks each one through the same `finish_run` that `stado release resume`
//! uses by hand.

use crate::cli::release_submit::builds::jobs::terminal::read_terminal_job;
use crate::cli::release_submit::run::reports::recent_runs;
use crate::cli::release_submit::run::resume::finish_run;
use crate::queue::storage::JobStorage;

/// How many recent runs one tick reads. A run is finished the tick after its
/// builds end, so anything older than the newest few is either terminal or
/// abandoned, and `stado release resume` still reaches it by hand.
const RECENT_RUNS_PER_TICK: usize = 12;

/// Runs whose builds are all terminal and that are not themselves terminal:
/// the ones one tick may finish. Only the control host finishes anything -
/// it owns the object store and the signing grant.
///
/// The control host is the one whose release API is local: no `api.url`,
/// or an `api.url` on this machine's own loopback. Until 2026-09-26 only the
/// first counted, and the control host's agent reaches its object API over
/// `http://127.0.0.1:18776` since that API became its own process, so every
/// agent returned here silently and no run was ever finished: Stado 0.22.4
/// sat `waiting` for a day with both builds completed.
pub async fn finish_ready_runs() -> Result<Vec<String>, String> {
    if !release_api_is_local(&crate::config::stado_api_url()) {
        return Ok(Vec::new());
    }
    let store = JobStorage::new().await.map_err(|error| error.to_string())?;
    let mut finished = Vec::new();
    for run in recent_runs(None, RECENT_RUNS_PER_TICK)
        .await
        .map_err(|error| error.to_string())?
    {
        let live = matches!(
            run["state"].as_str(),
            Some("waiting" | "publishing" | "delivering")
        );
        if !live {
            continue;
        }
        let Some(id) = run["run_id"].as_str() else {
            continue;
        };
        if !builds_terminal(&store, &run).await? {
            continue;
        }
        match finish_run(id, false).await {
            Ok(()) => finished.push(id.to_string()),
            // The failure is already persisted on the run object by
            // `finish_run`; the agent's log names it once and moves on.
            Err(error) => eprintln!("stado release agent run={id} finish failed: {error}"),
        }
    }
    Ok(finished)
}

/// Every platform this run submitted has a job the queue calls terminal.
/// A platform still failed from an earlier attempt has nothing to wait for.
async fn builds_terminal(store: &JobStorage, run: &serde_json::Value) -> Result<bool, String> {
    let Some(platforms) = run["platforms"].as_object() else {
        return Ok(false);
    };
    for platform in platforms.values() {
        if platform["state"].as_str() == Some("failed") {
            continue;
        }
        let Some(job_id) = platform["job_id"].as_str() else {
            return Ok(false);
        };
        if read_terminal_job(store, job_id)
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether the configured release API is this machine's own.
fn release_api_is_local(api_url: &str) -> bool {
    if api_url.is_empty() {
        return true;
    }
    reqwest::Url::parse(api_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "::1"))
}

#[cfg(test)]
mod tests {
    use super::release_api_is_local;

    #[test]
    fn only_an_absent_or_loopback_release_api_makes_this_the_control_host() {
        assert!(release_api_is_local(""));
        assert!(release_api_is_local("http://127.0.0.1:18776"));
        assert!(release_api_is_local("http://localhost:18776/"));
        assert!(release_api_is_local("http://[::1]:18776"));
        assert!(!release_api_is_local("https://stado.wisent.com"));
        assert!(!release_api_is_local(
            "http://charless-mac-mini.tail6443b3.ts.net:18776"
        ));
    }
}
