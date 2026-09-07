use super::*;

// ---------------------------------------------------------------------------
// fleet queue namespace — writer/reader alignment for anything submitted
// ---------------------------------------------------------------------------

/// Top-level registry key recording the queue namespace the fleet's
/// coordinator serves. Unmodelled by [`Registry`] itself, so it round-trips
/// through [`Registry::extra`] like every other key this build does not
/// own.
///
/// It exists because of what happened without it: an operator machine whose
/// `storage.stado.namespace` pointed at a different namespace than the
/// fleet's submitted build jobs that no fleet worker could ever see — the
/// queue namespace a job lands in is a property of the SUBMITTER's config,
/// not of the fleet, so writer and reader silently addressed two queues
/// through one object API. The coordinator records the namespace it serves
/// once per tick ([`record_fleet_queue_namespace`]); submitters compare
/// their ambient namespace against the record
/// ([`fleet_namespace_mismatch`]) and refuse rather than enqueue work into
/// a queue nobody claims.
pub const FLEET_QUEUE_NAMESPACE_KEY: &str = "fleet_queue_namespace";

/// The fleet queue namespace a registry document records, when one has been
/// recorded at all.
pub fn fleet_queue_namespace(document: &Value) -> Option<&str> {
    document
        .get(FLEET_QUEUE_NAMESPACE_KEY)
        .and_then(Value::as_str)
        .filter(|namespace| !namespace.trim().is_empty())
}

/// The operator-facing name of the queue jobs land in on THIS machine: the
/// Stado object namespace on that backend, the queue bucket everywhere
/// else, the backend id when neither is configured. For refusal and
/// diagnosis messages, never for routing.
pub fn queue_name() -> String {
    if crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
        == Some(crate::capabilities::StorageAdapter::StadoObject)
    {
        let namespace = crate::config::wc_stado_storage_namespace();
        if !namespace.trim().is_empty() {
            return namespace.to_string();
        }
    }
    let bucket = crate::config::bucket();
    if !bucket.is_empty() {
        return bucket.to_string();
    }
    crate::config::wc_storage_backend().to_string()
}

/// The one-sentence refusal when this machine's ambient Stado object
/// namespace is not the fleet queue namespace the registry records.
///
/// `None` — no refusal — in three cases: the storage backend has no
/// namespaces (on GCS/Azure/S3/local the bucket locator IS the accessor the
/// coordinator resolves, so both sides of the queue already address one
/// store), the registry records no fleet namespace yet (a fleet whose
/// coordinator predates the record cannot be checked, and refusing would
/// brick submissions against it), or the two agree.
pub fn fleet_namespace_mismatch(document: &Value) -> Option<String> {
    if crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
        != Some(crate::capabilities::StorageAdapter::StadoObject)
    {
        return None;
    }
    let fleet = fleet_queue_namespace(document)?;
    let ambient = crate::config::wc_stado_storage_namespace();
    if fleet == ambient {
        return None;
    }
    Some(format!(
        "build jobs go to the fleet queue namespace {fleet:?} recorded in the registry, \
         but this machine resolves storage.stado.namespace {ambient:?}; set \
         storage.stado.namespace (WC_STADO_STORAGE_NAMESPACE) to {fleet:?} so submitter \
         and fleet claim one queue"
    ))
}

/// Record `namespace` as the fleet queue namespace in the canonical
/// registry document — one fenced raw-document edit, the same pattern the
/// build poller's recipe writes use. Called once per coordinator tick with
/// the namespace the coordinator's own queue store resolves.
///
/// `Ok(true)` only when this call wrote. An up-to-date record, an empty
/// namespace (a non-Stado backend has no queue namespace to record) and a
/// lost compare-and-swap race are all `Ok(false)`: the first two are
/// terminal for the tick, and the race is retried next tick, so none of
/// them is a coordinator log line. Genuine store failures are `Err`.
pub async fn record_fleet_queue_namespace(namespace: &str) -> Result<bool, String> {
    let namespace = namespace.trim();
    if namespace.is_empty() {
        return Ok(false);
    }
    let store = RegistryStore::open()
        .await
        .map_err(|exc| format!("registry store open failed: {exc}"))?;
    let versioned = store
        .read_versioned()
        .await
        .map_err(|exc| format!("registry read failed: {exc}"))?
        .ok_or_else(|| format!("no registry document at {}", store.location()))?;
    let mut document: Value = serde_json::from_str(&versioned.content)
        .map_err(|exc| format!("registry parse failed: {exc}"))?;
    if fleet_queue_namespace(&document) == Some(namespace) {
        return Ok(false);
    }
    document
        .as_object_mut()
        .ok_or_else(|| "registry document is not an object".to_string())?
        .insert(
            FLEET_QUEUE_NAMESPACE_KEY.to_string(),
            Value::String(namespace.to_string()),
        );
    let payload = format!(
        "{}\n",
        serde_json::to_string_pretty(&document)
            .map_err(|exc| format!("registry serialize failed: {exc}"))?
    );
    match store.compare_and_swap(&versioned.version, &payload).await {
        Ok(_) => Ok(true),
        // A concurrent registry writer took the generation. The record is
        // one tick away, never worth a retry loop inside a tick.
        Err(StorageError::StorageConflict(_)) => Ok(false),
        Err(exc) => Err(format!("recording the fleet queue namespace failed: {exc}")),
    }
}
