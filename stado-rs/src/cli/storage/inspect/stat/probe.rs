//! The existence probe the five states come from, and the observation that
//! picks which store to ask about one object. `stado storage stat` renders the
//! observation; a release hand-off refuses a manifest whose pinned input the
//! store answers is absent ([`require_present`]).

use crate::cli::storage::*;

/// Existence probe for one object.
///
/// Deliberately NOT `BlobBackend::exists`: the Azure implementation maps
/// every transport failure to `false` (Python parity), so a forbidden
/// container reads exactly like an empty one. `download_text_versioned` propagates
/// [`crate::queue::StorageError`] instead, and answers existence, size and
/// version token in one round trip.
pub(in crate::cli::storage) async fn probe(backend: &Arc<dyn BlobBackend>, path: &str) -> Presence {
    match backend.download_text_versioned(path).await {
        Ok(Some(found)) => Presence::Present {
            size: found.content.len(),
            version: Some(found.version),
            detail: None,
        },
        Ok(None) => Presence::Absent,
        // A non-UTF-8 body (a collected artifact) fails the versioned TEXT
        // read without the store being unreachable, so re-probe with the
        // binary read before calling it a transport failure. On a genuinely
        // unreachable store this second probe fails too and costs one
        // request.
        Err(err) => match backend.download_bytes(path).await {
            Ok(Some(bytes)) => Presence::Present {
                size: bytes.len(),
                version: None,
                detail: Some(format!("no version token: {err}")),
            },
            Ok(None) => Presence::Absent,
            Err(_) => unanswered_for_error(&err),
        },
    }
}

/// What the store said about one object, and which store said it.
pub(in crate::cli::storage) struct Observation {
    pub(in crate::cli::storage) presence: Presence,
    pub(in crate::cli::storage) bucket: String,
    pub(in crate::cli::storage) backend: String,
    pub(in crate::cli::storage) metadata: BTreeMap<String, String>,
    pub(in crate::cli::storage) updated: Option<DateTime<Utc>>,
    pub(in crate::cli::storage) metadata_error: Option<String>,
}

/// Ask the store that holds `path` whether it is there.
pub(in crate::cli::storage) async fn observe(path: &str) -> Result<Observation, CmdError> {
    // Which store is even being asked. A `stado://releases/...` object lives in the
    // release channel, reached by its own route; the job store never held those bytes,
    // so asking it reports `absent` for every release ever published -- an answer
    // indistinguishable from a real absence. Only the witness differs here: one
    // rendering reports whichever answered, so the two cannot drift.
    // ONLY a `stado://` argument names a namespace. `ObjectRef::parse` accepts
    // a bare `<namespace>/<key>` too, so a queue path was read as a coordinate
    // in a foreign namespace — `artifacts/models/...` became namespace
    // `artifacts` — and the probe went to the object API's list route, which
    // answered 401 for a namespace this token has no grant on. An object the
    // very same store serves cannot be reported unreachable because the path
    // was spelled without a scheme.
    let parsed = if path.starts_with("stado://") {
        crate::remote::object_store::ObjectRef::parse(path)
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
    // `stado://sources/...` reported `absent` for objects that exist.
    let object_api = match (&parsed, &release) {
        (Ok(object), None)
            if RemoteObjectApi::release_authorized(object.namespace(), object.key())
                || object.namespace() != crate::config::wc_stado_storage_namespace() =>
        {
            RemoteObjectApi::configured_for_object(object)?.map(|remote| (remote, object.clone()))
        }
        _ => None,
    };

    if let Some((remote, uri)) = release {
        let presence = remote.stat_release(&uri).await?;
        // The channel answers presence, not bookkeeping: it serves bytes by
        // redirect, so there is no listing to carry metadata or a timestamp.
        // Reporting empty is honest; inventing them from the job store would
        // attach one store's bookkeeping to another store's answer.
        return Ok(Observation {
            presence,
            bucket: remote.base_url.to_string(),
            backend: "release-channel".to_string(),
            metadata: BTreeMap::new(),
            updated: None,
            metadata_error: None,
        });
    }
    if let Some((remote, object)) = object_api {
        // The list route is the one surface proven to answer for these
        // namespaces, and it carries the bookkeeping the queue listing would
        // have supplied, so nothing is reported as empty that is known.
        let entries = remote.list(object.namespace(), object.key()).await?;
        let uri = object.to_string();
        let entry = entries
            .into_iter()
            .find(|value| value.get("uri").and_then(Value::as_str) == Some(uri.as_str()));
        let (presence, metadata, updated) = match entry {
            Some(value) => listed(&value, &uri)?,
            None => (Presence::Absent, BTreeMap::new(), None),
        };
        return Ok(Observation {
            presence,
            bucket: remote.base_url.to_string(),
            backend: "object-api".to_string(),
            metadata,
            updated,
            metadata_error: None,
        });
    }
    let store = super::super::store_for(path).await?;
    let backend = store.backend();
    let probe_path = backend_key(backend, path)?;
    let presence = probe(backend, &probe_path).await;

    // Metadata and the timestamp come from the listing, which is a separate
    // grant from object read: if listing is denied while the read worked,
    // say so rather than downgrading a known-present object to unreachable.
    let (metadata, updated, metadata_error) = match &presence {
        Presence::Present { .. } => match backend.list_blobs_with_meta(&probe_path).await {
            Ok(blobs) => match blobs.into_iter().find(|blob| blob.name == probe_path) {
                Some(blob) => (blob.metadata, blob.updated, None),
                None => (BTreeMap::new(), None, None),
            },
            Err(err) => (BTreeMap::new(), None, Some(err.to_string())),
        },
        // Nothing else is known to be there, so there is no listing
        // worth asking for and no metadata to carry.
        _ => (BTreeMap::new(), None, None),
    };
    Ok(Observation {
        presence,
        bucket: store.bucket_name().to_string(),
        backend: store.backend_name().to_string(),
        metadata,
        updated,
        metadata_error,
    })
}

type Listed = (Presence, BTreeMap<String, String>, Option<DateTime<Utc>>);

/// One object API list entry read as an answer. The size is what the route
/// promises for every object it lists, so an entry without one is the route
/// breaking that promise and is reported, not counted as an empty object.
/// Metadata and the update time are optional in the route's answer.
fn listed(value: &Value, uri: &str) -> Result<Listed, CmdError> {
    let Some(bytes) = value
        .get("size")
        .or_else(|| value.get("bytes"))
        .and_then(Value::as_u64)
    else {
        return Err(CmdError::click(format!(
            "the object API listed {uri} without a size or bytes field: {value}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    };
    let size = usize::try_from(bytes).map_err(|error| {
        CmdError::click(format!(
            "the object API listed {uri} with size {bytes}, beyond this host's address space: {error}"
        ))
    })?;
    let metadata = match value.get("metadata").and_then(Value::as_object) {
        Some(fields) => fields
            .iter()
            .filter_map(|(name, item)| item.as_str().map(|text| (name.clone(), text.to_string())))
            .collect(),
        None => BTreeMap::new(),
    };
    let updated = value
        .get("updated_at")
        .and_then(Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|stamp| stamp.with_timezone(&Utc));
    Ok((
        Presence::Present {
            size,
            version: None,
            detail: Some(
                "the list route reports size and metadata; it carries no CAS version".to_string(),
            ),
        },
        metadata,
        updated,
    ))
}

/// Refuse unless the object a release manifest pins is in its store.
///
/// A manifest whose pinned input was never stored, or was stored for a lock
/// the product has since moved past, used to be accepted at hand-off and
/// refused only when a build fetched it, after the batch carrying it had
/// queued. `what` names the declaration, so the refusal says which pin to
/// repair. A store that did not answer is an error of its own class, never
/// read as absent.
pub(crate) async fn require_present(uri: &str, what: &str) -> Result<(), CmdError> {
    let observed = observe(uri).await?;
    match &observed.presence {
        Presence::Present { .. } => Ok(()),
        Presence::Absent => Err(CmdError::refused(format!(
            "{what} pins {uri}, which {} ({}) answers is absent: store the object the pin names, \
             or pin one that is stored",
            observed.bucket, observed.backend
        ))),
        presence => {
            let mut unanswered =
                CmdError::click(format!("{what}: {}", presence.unanswered_sentence(uri)));
            unanswered.failure = presence.failure();
            Err(unanswered)
        }
    }
}
