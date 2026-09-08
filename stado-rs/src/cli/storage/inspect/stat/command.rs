//! The command itself, and the receipt its --json form writes.

use crate::cli::storage::*;

#[derive(Args, Debug)]
pub struct StorageStatArgs {
    /// Full object name, for example `queue/<job_id>.json` or
    /// `registry.json`.
    path: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageStatReceipt {
    schema: String,
    backend: String,
    bucket: String,
    path: String,
    state: String,
    size: Option<usize>,
    updated_at: Value,
    version: Option<String>,
    metadata: BTreeMap<String, String>,
    detail: Option<String>,
    metadata_error: Option<String>,
}

/// Exit code contract: zero means the question was ANSWERED (`present` or
/// `absent`), non-zero means it was not (`refused`, `unavailable`,
/// `unreachable`). Scripting an "is it gone?" check on the exit status
/// therefore never mistakes a store that could not answer for a drained one.
///
/// Branch on `state` for which of the five it was. The three non-zero states
/// used to be one word, so a caller that wanted to retry a transient outage,
/// or to stop and fix a credential, had to grep a prose detail line to tell
/// which it was looking at.
pub(in crate::cli::storage) async fn stat(args: &StorageStatArgs) -> Result<(), CmdError> {
    // Which store is even being asked. A `stado://releases/...` object lives in the
    // release channel, reached by its own route; the job store never held those bytes,
    // so asking it reports `absent` for every release ever published -- an answer
    // indistinguishable from a real absence. Only the witness differs here: one
    // rendering below reports whichever answered, so the two cannot drift.
    // ONLY a `stado://` argument names a namespace. `ObjectRef::parse` accepts
    // a bare `<namespace>/<key>` too, so a queue path was read as a coordinate
    // in a foreign namespace — `artifacts/models/...` became namespace
    // `artifacts` — and the probe went to the object API's list route, which
    // answered 401 for a namespace this token has no grant on. An object the
    // very same store serves cannot be reported unreachable because the path
    // was spelled without a scheme.
    let parsed = if args.path.starts_with("stado://") {
        crate::object_store::ObjectRef::parse(&args.path)
    } else {
        Err(crate::queue::StorageError::Other(
            "a bare path addresses the queue store, not a namespace".to_string(),
        ))
    };
    let release = match &parsed {
        Ok(object) if object.namespace() == "releases" => {
            RemoteObjectApi::configured_release_reader()?.map(|remote| (remote, object.to_string()))
        }
        _ => None,
    };

    // An object outside the queue namespace is not in the queue store and never
    // can be: `StadoObjectBackend` builds `ObjectRef::new(&self.namespace, path)`,
    // so every probe is re-prefixed with the queue namespace. Asking it about
    // `stado://sources/...` reported `absent` for objects that exist, and I
    // believed that answer twice tonight -- once far enough to publish a source
    // snapshot and repoint a recipe around a file that was never missing.
    let object_api = match (&parsed, &release) {
        (Ok(object), None)
            if RemoteObjectApi::release_authorized(object.namespace(), object.key())
                || object.namespace() != crate::config::wc_stado_storage_namespace() =>
        {
            RemoteObjectApi::configured_for_object(object)?.map(|remote| (remote, object.clone()))
        }
        _ => None,
    };

    let (presence, store_bucket, store_backend, metadata, updated, metadata_error) = match release {
        Some((remote, uri)) => {
            let presence = remote.stat_release(&uri).await?;
            // The channel answers presence, not bookkeeping: it serves bytes by
            // redirect, so there is no listing to carry metadata or a timestamp.
            // Reporting empty is honest; inventing them from the job store would
            // attach one store's bookkeeping to another store's answer.
            (
                presence,
                remote.base_url.to_string(),
                "release-channel".to_string(),
                BTreeMap::new(),
                None,
                None,
            )
        }
        None if object_api.is_some() => {
            let (remote, object) = object_api.expect("checked by the guard above");
            // The list route is the one surface proven to answer for these
            // namespaces, and it carries the bookkeeping the queue listing would
            // have supplied, so nothing is reported as empty that is known.
            let entries = remote.list(object.namespace(), object.key()).await?;
            let uri = object.to_string();
            let entry = entries
                .into_iter()
                .find(|value| value.get("uri").and_then(Value::as_str) == Some(uri.as_str()));
            let (presence, metadata, updated) = match entry {
                Some(value) => (
                    Presence::Present {
                        size: usize::try_from(
                            value
                                .get("size")
                                .or_else(|| value.get("bytes"))
                                .and_then(Value::as_u64)
                                .unwrap_or_default(),
                        )
                        .unwrap_or(usize::MAX),
                        version: None,
                        detail: Some(
                            "the list route reports size and metadata; it carries no CAS version"
                                .to_string(),
                        ),
                    },
                    value
                        .get("metadata")
                        .and_then(Value::as_object)
                        .map(|fields| {
                            fields
                                .iter()
                                .filter_map(|(name, item)| {
                                    item.as_str().map(|text| (name.clone(), text.to_string()))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    value
                        .get("updated_at")
                        .and_then(Value::as_str)
                        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                        .map(|stamp| stamp.with_timezone(&Utc)),
                ),
                None => (Presence::Absent, BTreeMap::new(), None),
            };
            (
                presence,
                remote.base_url.to_string(),
                "object-api".to_string(),
                metadata,
                updated,
                None,
            )
        }
        None => {
            let store = JobStorage::new().await?;
            let backend = store.backend();
            let probe_path = backend_key(backend, &args.path)?;
            let presence = probe(backend, &probe_path).await;

            // Metadata and the timestamp come from the listing, which is a separate
            // grant from object read: if listing is denied while the read worked,
            // say so rather than downgrading a known-present object to unreachable.
            let (metadata, updated, metadata_error) = match &presence {
                Presence::Present { .. } => match backend.list_blobs_with_meta(&probe_path).await {
                    Ok(blobs) => blobs
                        .into_iter()
                        .find(|blob| blob.name == probe_path)
                        .map_or_else(
                            || (BTreeMap::new(), None, None),
                            |blob| (blob.metadata, blob.updated, None),
                        ),
                    Err(err) => (BTreeMap::new(), None, Some(err.to_string())),
                },
                // Nothing else is known to be there, so there is no listing
                // worth asking for and no metadata to carry.
                _ => (BTreeMap::new(), None, None),
            };
            (
                presence,
                store.bucket_name().to_string(),
                store.backend_name().to_string(),
                metadata,
                updated,
                metadata_error,
            )
        }
    };

    let (state, size, version, detail) = (
        presence.state(),
        match &presence {
            Presence::Present { size, .. } => Some(*size),
            _ => None,
        },
        match &presence {
            Presence::Present { version, .. } => version.clone(),
            _ => None,
        },
        presence.detail(),
    );

    if args.json {
        echo_json(&serde_json::to_value(StorageStatReceipt {
            schema: "stado.storage-stat-receipt.v1".into(),
            backend: store_backend,
            bucket: store_bucket,
            path: args.path.clone(),
            state: state.into(),
            size,
            updated_at: render_optional_stamp(updated),
            version,
            metadata,
            detail,
            metadata_error,
        })?)?;
    } else {
        let mut rows = vec![
            vec!["path".to_string(), args.path.clone()],
            vec!["state".to_string(), state.to_string()],
            vec![
                "store".to_string(),
                format!("{store_bucket} ({store_backend})"),
            ],
        ];
        if let Some(size) = size {
            rows.push(vec!["size".to_string(), size.to_string()]);
        }
        rows.push(vec!["updated_at".to_string(), render_stamp(updated)]);
        rows.push(vec!["version".to_string(), version.unwrap_or_default()]);
        rows.push(vec!["metadata".to_string(), render_metadata(&metadata)]);
        if let Some(detail) = &detail {
            rows.push(vec!["detail".to_string(), detail.clone()]);
        }
        if let Some(error) = &metadata_error {
            rows.push(vec!["metadata_error".to_string(), error.clone()]);
        }
        print_table(&["FIELD", "VALUE"], &rows);
        if matches!(presence, Presence::Absent) {
            println!(
                "\nThe store ANSWERED: {:?} is not there. This is not the same as a store \
                 that refused the question, could not answer it now, or could not be \
                 reached at all.",
                args.path
            );
        }
    }

    // The exit-code contract: zero means the question was ANSWERED (`present`
    // or `absent`), non-zero means it was not (`refused`, `unavailable`,
    // `unreachable`). Scripting an "is it gone?" check on the exit status
    // therefore never mistakes a store that could not answer for a drained
    // one; branch on `state` for which answer, and for which kind of silence.
    if presence.answered() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{}{}",
        presence.unanswered_sentence(&args.path),
        inferred_namespace_hint(&args.path)
    )))
}
