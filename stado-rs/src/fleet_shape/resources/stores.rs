//! The two questions about the stores this control plane depends on: whether
//! the declared replica can hold what the primary writes, and whether a
//! service that calls itself healthy is actually serving.

use serde_json::Value;

use super::super::{Finding, Sweep, HEALTH_CHECK, REPLICA_CHECK};
use crate::primitives::tally::Tally;
use crate::queue::copy::Endpoint;

/// A replica that can never hold what the primary writes.
///
/// [`Endpoint::cannot_replicate`] is the same predicate the write path and the
/// coordinator's replication both consult; this reports the condition standing
/// rather than waiting for someone to notice tens of GiB of unresolvable
/// objects.
///
/// **Reach: THIS control plane's configuration only.** The pairing that
/// produces unaddressable objects — `wc_storage_backend: stado` with
/// `wc_backup_storage_backend: local` — lives in each host's own config, not
/// here. This control plane declares `storage.backup: null` and so has nothing
/// to disagree about, which is why this arm reports a note rather than a
/// finding. Extending it means reading each host's resolved config the way
/// `stado host config show` does, one call per host.
pub(in crate::fleet_shape) fn replica_addressing(result: &mut Sweep) {
    let primary = Endpoint::configured_primary();
    result.measured.tally();
    // Which of the three states this control plane is in is recorded either
    // way. A check whose quiet result and whose "there was nothing to check"
    // result look identical is the shape this module exists to refuse: on the
    // first live sweep this arm produced no finding and no note, and there was
    // no way to tell from the output whether the pairing was sound or simply
    // never read.
    // What the config FILE declares, beside what the resolver answers. These
    // can disagree on a control plane: the file declares
    // `storage.backup.backend = local` with a path, `stado config show`
    // resolves `wc_backup_storage_backend` to empty, `stado doctor`'s backup
    // row passes with "no mandatory S3 replica" — and a `storage ls` in the
    // same worktree printed the mirror refusal naming
    // `local://~/.stado/local-backup`, which requires that key to be set.
    // Two readers, two answers, one declaration: components that believe the
    // replica exists write 48 GiB into it while the diagnostics say there is
    // nothing there and pass.
    let declared_in_file = match crate::config_file::load_config_file() {
        Ok(file) => Ok(file
            .get("storage")
            .and_then(|storage| storage.pointer("/backup/backend"))
            .and_then(Value::as_str)
            .filter(|backend| !backend.trim().is_empty())
            .map(str::to_string)),
        Err(error) => Err(error.to_string()),
    };
    let Some(backup) = Endpoint::configured_backup() else {
        match declared_in_file {
            Ok(Some(backend)) => result.record(Finding {
                check: REPLICA_CHECK,
                subject: format!("{}: storage.backup.backend", primary.describe()),
                declared: format!("the config file declares a {backend} replica"),
                observed: "the resolver answers that no replica is configured, so one half of \
                           this binary writes to a replica the other half says does not exist"
                    .to_string(),
                command: "compare `stado config show` against storage.backup in the config file; \
                          the resolver is the half to fix"
                    .to_string(),
            }),
            Ok(None) => result
                .notes
                .push(format!("{}: no replica declared", primary.describe())),
            // A config file this process cannot read leaves the comparison
            // unmade, which is a finding of its own, not a quiet pass.
            Err(unreadable) => result.record(Finding {
                check: REPLICA_CHECK,
                subject: format!("{}: storage.backup.backend", primary.describe()),
                declared: "the config file declares whether a replica exists".to_string(),
                observed: format!(
                    "the config file cannot be read, so the declaration cannot be compared \
                     with the resolver's answer: {unreadable}"
                ),
                command: "stado config validate names what the file refuses".to_string(),
            }),
        }
        return;
    };
    match primary.cannot_replicate(&backup) {
        Some(refusal) => result.record(Finding {
            check: REPLICA_CHECK,
            subject: format!("{} -> {}", primary.describe(), backup.describe()),
            declared: "storage.backup is a disaster-recovery replica of storage".to_string(),
            observed: refusal,
            command: "stado config set storage.backup.backend \"\" to stop declaring a replica, \
                      or point it at a store of the same kind as the primary"
                .to_string(),
        }),
        None => result.notes.push(format!(
            "{} can replicate to {}",
            primary.describe(),
            backup.describe()
        )),
    }
}

/// Whether a service that reports itself healthy is actually serving.
///
/// Read from the endpoint this control plane is configured to use, which is the
/// one whose answers the fleet depends on. `healthz` answering 200 while its
/// own boundaries are closed is not evidence of service: authorized routes
/// behind it can still return 503.
pub async fn health_disagreement() -> Option<Finding> {
    let url = match crate::config::wc_stado_storage_url() {
        Ok(url) => url,
        Err(cause) => {
            let configured = crate::config::wc_stado_storage_url_configured().to_string();
            return Some(unreadable_health(configured, cause));
        }
    };
    if url.is_empty() {
        return None;
    }
    let endpoint = format!("{}/healthz", url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    // A healthz this control plane cannot read is the worst disagreement of
    // all: the service the fleet depends on gives no answer. It used to end
    // the check silently, so the sweep reported nothing to disagree about.
    let answered = match crate::wait::request(client.get(&endpoint)).await {
        Ok(response) => response
            .json::<Value>()
            .await
            .map_err(|error| format!("its answer is not readable JSON: {}", error.without_url())),
        Err(error) => Err(format!("it could not be reached: {error}")),
    };
    let body = match answered {
        Ok(body) => body,
        Err(cause) => return Some(unreadable_health(endpoint, cause)),
    };
    // The flags are booleans in the health document; one absent or in
    // another shape is not the flag being set.
    let ok = matches!(body.get("ok"), Some(Value::Bool(true)));
    let degraded = matches!(body.get("degraded"), Some(Value::Bool(true)));
    let closed: Vec<String> = match body.get("boundaries").and_then(Value::as_object) {
        Some(boundaries) => boundaries
            .iter()
            .filter(|(_, open)| matches!(open, Value::Bool(false)))
            .map(|(name, _)| name.clone())
            .collect(),
        None => Vec::new(),
    };
    if !ok || !degraded || closed.is_empty() {
        return None;
    }
    Some(Finding {
        check: HEALTH_CHECK,
        subject: endpoint,
        declared: "healthz reports whether the service can do its work".to_string(),
        observed: format!(
            "healthz says ok while {} boundary/boundaries are closed ({}), so every route behind \
             them answers 503",
            closed.len(),
            closed.join(", ")
        ),
        command: format!(
            "stado service logs {} --host <host> names why the boundary is closed; a \
             credential answer is not fixed by restarting the process",
            match crate::deploy::service_catalog::host_unit() {
                Ok(unit) => unit,
                Err(error) => format!("<the host unit: {error}>"),
            }
        ),
    })
}

/// The finding for a healthz this control plane could not read.
fn unreadable_health(endpoint: String, cause: String) -> Finding {
    Finding {
        check: HEALTH_CHECK,
        subject: endpoint,
        declared: "healthz reports whether the service can do its work".to_string(),
        observed: format!("healthz gave no answer: {cause}"),
        command: "stado resolver status names which address this control plane is configured \
                  to reach and which process listens there"
            .to_string(),
    }
}
