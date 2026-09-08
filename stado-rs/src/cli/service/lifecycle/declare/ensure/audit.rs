//! Where the record of one `ensure` pass lives.

use super::*;

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
pub(super) async fn record_ensure_audit(
    record: &ManagedService,
    outcome: &service::EnsureOutcome,
    plan: &service::DeployPlan,
    reason: &str,
    generation: Option<&str>,
) -> Result<String, CmdError> {
    let store = targets::RegistryStore::open()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
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
        "actor": crate::cli::autonomy_cmd::actor(),
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
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if !created {
        return Err(CmdError::click(format!(
            "{path} already exists, so this pass was not recorded; an audit record is never \
             replaced"
        )));
    }
    Ok(path)
}
