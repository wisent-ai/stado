//! The two questions about the stores this control plane depends on: whether
//! the declared replica can hold what the primary writes, and whether a
//! service that calls itself healthy is actually serving.

use std::time::Duration;

use serde_json::Value;

use super::super::{Finding, Sweep, HEALTH_CHECK, REPLICA_CHECK};
use crate::queue::copy::Endpoint;

/// A replica that can never hold what the primary writes.
///
/// [`Endpoint::cannot_replicate`] is the same predicate the write path and the
/// coordinator's replication both consult; this reports the condition standing
/// rather than waiting for someone to notice 48 GiB of unresolvable objects.
///
/// **Reach: THIS control plane's configuration only.** The pairing that
/// actually produced 48 GiB of unaddressable objects on 2026-08-30 was
/// `charless-mac-mini`'s own — `wc_storage_backend: stado` with
/// `wc_backup_storage_backend: local`, read from that host's config, not from
/// here. This control plane declares `storage.backup: null` and so has nothing
/// to disagree about, which is why this arm reports a note rather than a
/// finding on the fleet it was written for. Extending it means reading each
/// host's resolved config the way `stado host config-show` does, one call per
/// host, and that is the next thing this check needs.
pub(in crate::fleet_shape) fn replica_addressing(result: &mut Sweep) {
    let primary = Endpoint::configured_primary();
    result.measured += 1;
    // Which of the three states this control plane is in is recorded either
    // way. A check whose quiet result and whose "there was nothing to check"
    // result look identical is the shape this module exists to refuse: on the
    // first live sweep this arm produced no finding and no note, and there was
    // no way to tell from the output whether the pairing was sound or simply
    // never read.
    // What the config FILE declares, beside what the resolver answers. These
    // disagreed on this control plane on 2026-08-31: the file declares
    // `storage.backup.backend = local` with a path, `stado config show`
    // resolves `wc_backup_storage_backend` to empty, `stado doctor`'s backup
    // row passes with "no mandatory S3 replica" — and a `storage ls` in the
    // same worktree printed the mirror refusal naming
    // `local://~/.stado/local-backup`, which requires that key to be set.
    // Two readers, two answers, one declaration: components that believe the
    // replica exists write 48 GiB into it while the diagnostics say there is
    // nothing there and pass.
    let declared_in_file = crate::config_file::load_config_file()
        .ok()
        .and_then(|file| file.get("storage").cloned())
        .and_then(|storage| storage.get("backup").cloned())
        .and_then(|backup| {
            backup
                .get("backend")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|backend| !backend.trim().is_empty());
    let Some(backup) = Endpoint::configured_backup() else {
        match declared_in_file {
            Some(backend) => result.record(Finding {
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
            None => result
                .notes
                .push(format!("{}: no replica declared", primary.describe())),
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
/// own boundaries are closed is not a healthy service: every authorized route
/// behind it returns 503, which is how a store outage read as a slow link for
/// most of a night.
pub async fn health_disagreement() -> Option<Finding> {
    let url = crate::config::wc_stado_storage_url();
    if url.is_empty() {
        return None;
    }
    let endpoint = format!("{}/healthz", url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(u8::BITS as u64))
        .build()
        .ok()?;
    let body: Value = client.get(&endpoint).send().await.ok()?.json().await.ok()?;
    let ok = body.get("ok").and_then(Value::as_bool).unwrap_or_default();
    let degraded = body
        .get("degraded")
        .and_then(Value::as_bool)
        .unwrap_or_default();
    let closed: Vec<String> = body
        .get("boundaries")
        .and_then(Value::as_object)
        .map(|boundaries| {
            boundaries
                .iter()
                .filter(|(_, open)| open.as_bool() == Some(false))
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default();
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
        command: "stado service logs com.wisent.always-on.stado-object-api --host <host> names \
                  why the boundary is closed; a credential answer is not fixed by restarting the \
                  process"
            .to_string(),
    })
}
