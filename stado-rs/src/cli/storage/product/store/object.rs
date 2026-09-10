//! `stado storage put`, and the one implementation of how an object reaches
//! the channel that the release publisher shares with it.

use crate::cli::storage::*;

#[derive(Args, Debug)]
pub struct StoragePutArgs {
    /// stado://<namespace>/<key>.
    uri: String,
    /// Local source file, or '-' for stdin.
    source: String,
    /// Refuse to replace an existing object. Implied for stado://releases/...
    /// because release objects are immutable.
    #[arg(long)]
    if_absent: bool,
    /// Media type retained as provider-neutral object metadata.
    #[arg(long, default_value = "application/octet-stream")]
    content_type: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoragePutReceipt {
    schema: String,
    state: String,
    created: bool,
    uri: String,
    sha256: String,
    bytes: usize,
    content_type: String,
}

fn read_object_source(source: &str) -> Result<Vec<u8>, CmdError> {
    if source == "-" {
        let mut bytes = Vec::new();
        std::io::stdin().lock().read_to_end(&mut bytes)?;
        Ok(bytes)
    } else {
        Ok(std::fs::read(source)?)
    }
}

#[derive(Debug, Clone)]
struct StoreObjectOutcome {
    uri: String,
    created: bool,
}

/// Store one object through whichever route its namespace requires, create-only
/// for `releases` whether or not the caller asks.
///
/// Shared with `stado release publish` so the publisher cannot drift from what
/// `storage put` does: one implementation of "how an object reaches the channel".
pub(crate) async fn store_object(
    uri: &str,
    source: &str,
    content_type: &str,
    if_absent: bool,
) -> Result<String, CmdError> {
    store_object_with_metadata(uri, source, content_type, if_absent, &BTreeMap::new()).await
}

pub(crate) async fn store_object_with_metadata(
    uri: &str,
    source: &str,
    content_type: &str,
    if_absent: bool,
    extra_metadata: &BTreeMap<String, String>,
) -> Result<String, CmdError> {
    Ok(
        store_object_with_metadata_outcome(uri, source, content_type, if_absent, extra_metadata)
            .await?
            .uri,
    )
}

async fn store_object_with_metadata_outcome(
    uri: &str,
    source: &str,
    content_type: &str,
    if_absent: bool,
    extra_metadata: &BTreeMap<String, String>,
) -> Result<StoreObjectOutcome, CmdError> {
    let object = crate::remote::object_store::ObjectRef::parse(uri)?;
    let uri = object.to_string();
    let create_only = if_absent || object.namespace() == "releases";
    let mut metadata = crate::remote::object_store::metadata(&object, content_type);
    for (name, value) in extra_metadata {
        if !name.starts_with("stado-")
            || metadata.contains_key(name)
            || value.is_empty()
            || name.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            return Err(CmdError::click(
                "custom object metadata must use unique non-empty stado-* fields",
            ));
        }
        metadata.insert(name.clone(), value.clone());
    }
    if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        let bytes = read_object_source(source)?;
        if create_only {
            match remote.get_optional(&uri).await? {
                Some(existing) if existing == bytes => {
                    return Ok(StoreObjectOutcome {
                        uri,
                        created: false,
                    })
                }
                Some(_) => {
                    return Err(CmdError::click(format!(
                        "immutable object already differs on the writer: {uri}"
                    )))
                }
                None => {}
            }
        }
        let expected_sha = Sha256::digest(&bytes);
        remote
            .put_with_metadata(&uri, content_type, create_only, bytes, extra_metadata)
            .await?;
        let stored = remote.get(&uri).await?;
        if Sha256::digest(&stored) != expected_sha {
            return Err(CmdError::click(format!(
                "object writer read-back differs immediately after PUT: {uri}"
            )));
        }
        return Ok(StoreObjectOutcome { uri, created: true });
    }
    let path = object.storage_path();
    let store = JobStorage::new().await?;
    let stdin_bytes = if create_only && source == "-" {
        Some(read_object_source(source)?)
    } else {
        None
    };
    let uploaded = if create_only {
        if let Some(bytes) = stdin_bytes.as_ref() {
            let mut staged = tempfile::NamedTempFile::new()?;
            staged.write_all(bytes)?;
            store.upload_file_if_absent(&path, staged.path()).await?
        } else {
            store
                .upload_file_if_absent(&path, std::path::Path::new(source))
                .await?
        }
    } else {
        let bytes = read_object_source(source)?;
        store.upload_bytes(&path, &bytes).await?;
        true
    };
    if !uploaded {
        let incoming = match stdin_bytes {
            Some(bytes) => bytes,
            None => read_object_source(source)?,
        };
        let existing = store.read_bytes(&path).await?.ok_or_else(|| {
            CmdError::click(format!(
                "{object} won create-only admission but is no longer readable"
            ))
        })?;
        if Sha256::digest(&existing) == Sha256::digest(&incoming) {
            return Ok(StoreObjectOutcome {
                uri,
                created: false,
            });
        }
        let policy = if object.namespace() == "releases" {
            "release objects are immutable"
        } else {
            "--if-absent refused to replace it"
        };
        return Err(CmdError::click(format!(
            "{object} already exists; {policy}"
        )));
    }
    store.backend().set_metadata(&path, &metadata).await?;
    Ok(StoreObjectOutcome { uri, created: true })
}

pub(in crate::cli::storage) async fn put(args: &StoragePutArgs) -> Result<(), CmdError> {
    let outcome = store_object_with_metadata_outcome(
        &args.uri,
        &args.source,
        &args.content_type,
        args.if_absent,
        &BTreeMap::new(),
    )
    .await?;
    if args.json {
        let stored = fetch_object_from_writer(&outcome.uri).await?;
        echo_json(&serde_json::to_value(StoragePutReceipt {
            schema: "stado.storage-put-receipt.v1".into(),
            state: if outcome.created {
                "stored".into()
            } else {
                "replayed".into()
            },
            created: outcome.created,
            uri: outcome.uri,
            sha256: hex::encode(Sha256::digest(&stored)),
            bytes: stored.len(),
            content_type: args.content_type.clone(),
        })?)?;
    } else if outcome.created {
        println!("stored {}", outcome.uri);
    } else {
        println!("replayed {}", outcome.uri);
    }
    Ok(())
}
