//! `service ensure NAME --host HOST`: the idempotent half of `deploy`.
//!
//! [`persist_ensure_record`] is the registry half of one pass, kept whole:
//! either the document already says what the pass confirmed, or the
//! declaration is corrected, or it is added — and then the pass is recorded
//! beside the state it changed ([`record_ensure_audit`]).

use super::*;

mod predecessors;
pub(crate) mod program;
mod route;
pub(crate) mod run;

pub(crate) struct EnsureOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) from: Option<&'a str>,
    pub(crate) args: &'a [String],
    pub(crate) env: &'a [String],
    pub(crate) unset_env: &'a [String],
    pub(crate) reason: &'a str,
    pub(crate) as_daemon: bool,
    pub(crate) as_launch_agent: bool,
    pub(crate) as_json: bool,
}

/// What one `ensure` pass writes: the declaration the document ends up
/// carrying, and the audit record of the pass that put it there.
async fn persist_ensure_record(
    record: &ManagedService,
    already: &Option<ManagedService>,
    replaced: &[String],
    outcome: &service::EnsureOutcome,
    plan: &service::DeployPlan,
    reason: &str,
    host: &str,
) -> Result<Option<String>, CmdError> {
    let recovered_snapshot = if outcome.changed() {
        Some(registry_after_host_change().await?)
    } else {
        None
    };
    let generation = match &already {
        // Declared, at the same file and running the same program, by the
        // registry: the document already says what this pass just confirmed,
        // so nothing is written to it.
        Some(existing)
            if existing.source == SOURCE_REGISTRY
                && existing.path == record.path
                && existing.kind == record.kind
                && existing.program == record.program
                && existing.args == record.args
                && existing.env == record.env
                && existing.systemd_unit == record.systemd_unit =>
        {
            None
        }
        // Declared by the registry at a different file. The system-domain
        // daemon path is not the per-login agent path, and a declaration
        // naming a file the host does not have is one no later command can
        // act on, so the declaration is corrected in one document write.
        Some(existing) if existing.source == SOURCE_REGISTRY => {
            // Expected generation: this read. `existing` and `record` were
            // decided before it — the unit was already ensured on the host —
            // so a lost race is reported instead of retried: re-running the
            // correction would replace a declaration this pass never saw.
            let (mut document, expected_generation) = match recovered_snapshot {
                Some(snapshot) => snapshot,
                None => registry::fetch_versioned_document().await?,
            };
            service::remove_service(&mut document, host, existing.unit_id()).map_err(click)?;
            service::add_service(&mut document, record).map_err(click)?;
            if existing.unit_id() != record.unit_id() {
                service::move_unit_references(&mut document, host, existing.unit_id(), record);
            }
            Some(registry::push_document_if(&document, &expected_generation).await?)
        }
        // Undeclared, or carried by the fixed host-recovery list. Both become
        // an explicit registry declaration, which for the recovery case is
        // exactly what `service adopt` is for.
        _ => Some(record_declaration(record).await?),
    };
    let generation = absorb_replaced(record, host, replaced)
        .await?
        .or(generation);

    // Recorded only when something actually changed. An audit trail that also
    // records the passes which changed nothing is one nobody reads.
    let audited = if outcome.changed() || generation.is_some() {
        Some(record_ensure_audit(record, outcome, plan, reason, generation.as_deref()).await?)
    } else {
        None
    };
    Ok::<_, CmdError>(audited)
}

/// Where the record of one `ensure` pass lives, relative to the canonical
/// registry document.
const ENSURE_AUDIT_PREFIX: &str = "service_audit";

/// Append the record of one `ensure` pass beside the state it changed.
///
/// This command exists to be run from a script, and a change nobody typed is a
/// change nobody remembers making: the reason the operator gave, what the host
/// did about it, and the registry generation it produced are written as one
/// create-only object through [`targets::RegistryStore::write_beside`] — beside
/// the registry document, because the registry is the state that changed and on
/// a GCS deployment the queue store is a different bucket entirely.
async fn record_ensure_audit(
    record: &ManagedService,
    outcome: &service::EnsureOutcome,
    plan: &service::DeployPlan,
    reason: &str,
    generation: Option<&str>,
) -> Result<String, CmdError> {
    let store = targets::RegistryStore::open()
        .await
        .map_err(CmdError::from)?;
    let now = chrono::Utc::now();
    let body = serde_json::to_string_pretty(&json!({
        "action": outcome.action,
        "host": record.host,
        "service": record.name,
        "unit": record.unit_id(),
        "kind": record.kind,
        "path": record.path,
        "domain": outcome.domain_word(),
        "pid": outcome.pid.trim().parse::<u32>().ok(),
        "program": plan.program,
        "argv": plan.argv,
        "reason": reason,
        "registry_generation": generation,
        "recorded_at": now.to_rfc3339(),
        "actor": crate::cli::work::autonomy::actor(),
    }))?;
    // Timestamp first so one host's records sort by when they happened, and
    // compact rather than RFC-3339 because the key is also a file name on the
    // local-file backend.
    let key = format!(
        "{ENSURE_AUDIT_PREFIX}/{}/{}-{}.json",
        record.host,
        now.format("%Y%m%dT%H%M%S%.6fZ"),
        record.unit_id()
    );
    let (path, created) = store
        .write_beside(&key, &body)
        .await
        .map_err(CmdError::from)?;
    if !created {
        return Err(CmdError::refused(format!(
            "{path} already exists, so this pass was not recorded; an audit record is never \
             replaced"
        )));
    }
    Ok(path)
}
/// A changed unit may own the registry API itself. Read the registry
/// authoritatively after activation, not merely the new process's PID; a
/// registry that is not answering yet is that read's own error.
async fn registry_after_host_change() -> Result<(Value, String), CmdError> {
    registry::fetch_versioned_document().await
}

/// Withdraw the declarations on `host` of the units the product's one
/// process replaced (every declaration that runs its program under another
/// label), and point every placement profile and service-directory entry
/// that named one of them at `record`, in one write.
///
/// The declaration a product ran under before is not always found as the
/// same service: an older label may be declared under itself, not under the
/// product's name, so ensure added the product's unit beside it and left the
/// placement profile naming the old unit, which the release agent then read
/// as the unit to hand the port to. Returns the new generation, or `None`
/// when no replaced unit was declared there.
async fn absorb_replaced(
    record: &ManagedService,
    host: &str,
    replaced: &[String],
) -> Result<Option<String>, CmdError> {
    if replaced.is_empty() {
        return Ok(None);
    }
    let (mut document, expected_generation) = registry::fetch_versioned_document().await?;
    let mut absorbed = false;
    for unit in replaced {
        if unit == record.unit_id() {
            continue;
        }
        if service::remove_service(&mut document, host, unit).is_ok() {
            service::move_unit_references(&mut document, host, unit, record);
            absorbed = true;
        }
    }
    if !absorbed {
        return Ok(None);
    }
    Ok(Some(
        registry::push_document_if(&document, &expected_generation).await?,
    ))
}
