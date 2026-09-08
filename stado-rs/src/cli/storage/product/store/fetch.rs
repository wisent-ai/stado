//! Fetching one object, reading it with its version token, and swapping it
//! against that token.

use crate::cli::storage::*;

/// Fetch through the authenticated object writer route, including release
/// objects that are not yet visible through the public download facade.
pub(crate) async fn fetch_object_from_writer(uri: &str) -> Result<Vec<u8>, CmdError> {
    let object = crate::object_store::ObjectRef::parse(uri)?;
    let uri = object.to_string();
    if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        return remote.get(&uri).await;
    }
    let store = JobStorage::new().await?;
    let Some(bytes) = store.read_bytes(&object.storage_path()).await? else {
        return Err(CmdError::click(format!("{object}: absent")));
    };
    Ok(bytes)
}

/// Fetch one object's bytes through whichever route its namespace requires.
/// Shared with `stado release publish` for the same reason as [`store_object`].
pub(crate) async fn fetch_object(uri: &str) -> Result<Vec<u8>, CmdError> {
    let object = crate::object_store::ObjectRef::parse(uri)?;
    let uri = object.to_string();
    if object.namespace() == "releases" {
        if let Some(remote) = RemoteObjectApi::configured_release_reader()? {
            return remote.get_release(&uri).await;
        }
    } else if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        return remote.get(&uri).await;
    }
    let store = JobStorage::new().await?;
    let Some(bytes) = store.read_bytes(&object.storage_path()).await? else {
        return Err(CmdError::click(format!("{object}: absent")));
    };
    Ok(bytes)
}

pub(crate) async fn fetch_object_versioned(
    uri: &str,
) -> Result<Option<(Vec<u8>, String)>, CmdError> {
    let object = crate::object_store::ObjectRef::parse(uri)?;
    if object.namespace() == "releases" {
        return Err(CmdError::click(
            "release objects are immutable and have no catalog CAS path",
        ));
    }
    if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        return remote.get_versioned(&object.to_string()).await;
    }
    let store = JobStorage::new().await?;
    Ok(store
        .read_text_versioned(&object.storage_path())
        .await?
        .map(|value| (value.content.into_bytes(), value.version)))
}

pub(crate) async fn compare_and_swap_object(
    uri: &str,
    content: &[u8],
    content_type: &str,
    expected_version: &str,
) -> Result<(), CmdError> {
    let object = crate::object_store::ObjectRef::parse(uri)?;
    if object.namespace() == "releases" {
        return Err(CmdError::click("release objects cannot be replaced"));
    }
    if let Some(remote) = RemoteObjectApi::configured_for_object(&object)? {
        return remote
            .put_if_version(
                &object.to_string(),
                content_type,
                expected_version,
                content.to_vec(),
            )
            .await;
    }
    let text = std::str::from_utf8(content)
        .map_err(|_| CmdError::click("conditional object content must be UTF-8"))?;
    let store = JobStorage::new().await?;
    store
        .compare_and_swap_text(&object.storage_path(), expected_version, text)
        .await?;
    let metadata = crate::object_store::metadata(&object, content_type);
    store
        .backend()
        .set_metadata(&object.storage_path(), &metadata)
        .await?;
    Ok(())
}

pub(crate) async fn list_object_uris(
    namespace: &str,
    prefix: &str,
) -> Result<Vec<String>, CmdError> {
    let storage_prefix = crate::object_store::ObjectRef::namespace_prefix(namespace, prefix)?;
    if let Some(remote) = RemoteObjectApi::configured_for_list(namespace, prefix)? {
        return remote
            .list(namespace, prefix)
            .await?
            .into_iter()
            .map(|value| {
                value
                    .get("uri")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| CmdError::click("object list entry omitted uri"))
            })
            .collect();
    }
    let store = JobStorage::new().await?;
    let mut uris = Vec::new();
    for blob in store
        .backend()
        .list_blobs_with_meta(&storage_prefix)
        .await?
    {
        uris.push(crate::object_store::ObjectRef::from_storage_path(&blob.name)?.to_string());
    }
    uris.sort();
    Ok(uris)
}
