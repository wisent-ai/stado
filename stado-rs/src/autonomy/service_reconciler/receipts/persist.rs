//! Store the run's report, and say what refused the write when it is refused.

use crate::queue::{JobStorage, StorageError};

use super::super::{LATEST_REPORT, REPORT_PREFIX};
use super::report::ServiceReconcileReport;

/// A refused write must name the object and the boundary that refused it.
///
/// The object API authorizes a write by matching its key against the
/// configured namespace's prefix policies, and a key no policy covers comes
/// back `401 {"error":"unauthorized or non-immutable release write"}` — a
/// sentence naming neither the namespace, the prefix, nor the grant. The whole
/// autonomy layer writes under `autonomy/`, which no namespace policy declares,
/// so every run of this stage is refused on a deployment whose namespace does
/// not authorize that prefix. Diagnosing that from the bare 401 took twenty
/// minutes; the reconciliation verdict is worthless if the record of it cannot
/// be found, so the failure says exactly which object was refused.
pub(in crate::autonomy::service_reconciler) async fn persist_report(
    store: &JobStorage,
    report: &ServiceReconcileReport,
) -> Result<(), StorageError> {
    let object = report.created_at.replace(':', "-");
    let run_path = format!("{REPORT_PREFIX}/{object}.json");
    for path in [run_path.as_str(), LATEST_REPORT] {
        let immutable = path != LATEST_REPORT;
        if let Err(error) =
            crate::autonomy::storage::write_json(store, path, report, immutable).await
        {
            return Err(StorageError::Other(format!(
                "service reconciliation ran but could not record it at {path}: {error}. The \
                 configured Stado storage namespace must authorize `put` on this key's prefix; \
                 `stado config show` reports the namespace and its prefix policies"
            )));
        }
    }
    Ok(())
}
