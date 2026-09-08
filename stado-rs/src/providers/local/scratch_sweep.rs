//! The host destroys its own expired scratch leases, on the janitor's cadence.
//!
//! A lease with a lifetime nobody enforces is just an account with a comment.
//! `stado scratch create` reaps the host it leases on, which covers the run
//! that takes the next lease — and covers nothing on a box nobody leases again.
//! This is the other half: the agent already runs a janitor pass off its
//! critical path, and a pass that reaps the machine's own expired leases needs
//! no store, no caller and nobody's memory.
//!
//! The sweep runs against this machine's own registry entry, so the channel
//! executes locally and no ssh hop is involved. It never returns an error: a
//! sweep that cannot run must not take the janitor pass down with it, and a
//! host that cannot reap says so in its own log.

use crate::deploy::scratch;

/// One sweep of this machine's expired leases.
pub async fn sweep(log: &mut dyn FnMut(&str)) {
    let hostname = crate::providers::vast::system_hostname();
    let registry = match crate::deploy::host_channel::canonical_registry().await {
        Ok(registry) => registry,
        Err(exc) => {
            log(&format!(
                "scratch sweep: the registry did not answer, so no lease was read: {}",
                exc.0
            ));
            return;
        }
    };
    let target = match registry.lookup_self(&hostname) {
        Ok(Some(target)) => target.clone(),
        Ok(None) => {
            log("scratch sweep: this machine is not a registry target, so it holds no leases");
            return;
        }
        Err(exc) => {
            log(&format!("scratch sweep: {exc}"));
            return;
        }
    };
    let runner = crate::deploy::production_runner();
    match scratch::reap_target(&target, true, &runner).await {
        Ok(report) => log(&summary(&target.name, &report)),
        Err(exc) => log(&format!(
            "scratch sweep: {} holds leases that could not be swept: {}",
            target.name, exc.0
        )),
    }
}

/// One line an operator can read in the agent log: what went, what stayed, and
/// what refused to go.
fn summary(target: &str, report: &serde_json::Map<String, serde_json::Value>) -> String {
    let number = |key: &str| {
        report
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default()
    };
    let failures = report
        .get("failures")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    format!(
        "scratch sweep: {target} destroyed {} expired lease(s), kept {}, {failures} refused",
        number("destroyed"),
        number("kept")
    )
}
