//! `service ensure NAME --host HOST`: the idempotent half of `deploy`.
//!
//! [`persist_ensure_record`] is the registry half of one pass, kept whole:
//! either the document already says what the pass confirmed, or the
//! declaration is corrected, or it is added — and then the pass is recorded
//! beside the state it changed.

use super::*;

mod audit;
mod predecessors;
pub(crate) mod program;
mod route;
pub(crate) mod run;

use audit::record_ensure_audit;
use program::canonical_managed_unit;

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
            Some(registry::push_document_if(&document, &expected_generation).await?)
        }
        // Undeclared, or carried by the fixed host-recovery list. Both become
        // an explicit registry declaration, which for the recovery case is
        // exactly what `service adopt` is for.
        _ => Some(record_declaration(record).await?),
    };

    // Recorded only when something actually changed. An audit trail that also
    // records the passes which changed nothing is one nobody reads.
    let audited = if outcome.changed() || generation.is_some() {
        Some(record_ensure_audit(record, outcome, plan, reason, generation.as_deref()).await?)
    } else {
        None
    };
    Ok::<_, CmdError>(audited)
}

/// A changed unit may own the registry API itself. Read the registry
/// authoritatively after activation, not merely the new process's PID; a
/// registry that is not answering yet is that read's own error.
async fn registry_after_host_change() -> Result<(Value, String), CmdError> {
    registry::fetch_versioned_document().await
}
