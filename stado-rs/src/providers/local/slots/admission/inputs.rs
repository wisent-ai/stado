//! Declared Stado inputs, materialized into the job tree while storage
//! credentials are still confined to the trusted agent process: digest
//! checked, symlinks refused, and every path kept inside the work directory.

use super::*;

/// Materialize explicitly declared Stado objects while storage credentials
/// are still confined to the trusted agent process.
pub(crate) async fn materialize_stado_inputs(
    store: &JobStorage,
    inputs: &serde_json::Map<String, Value>,
    work_dir: &Path,
) -> Result<(), StorageError> {
    use sha2::Digest as _;
    fn reject_symlink(path: &Path) -> Result<(), StorageError> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(StorageError::PathEscape(
                format!("job input path contains a symlink: {}", path.display()),
            )),
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StorageError::Io(error)),
        }
    }

    for (name, value) in inputs {
        let Some(spec) = value.as_object() else {
            continue;
        };
        let Some(uri) = spec.get("stado_uri").and_then(Value::as_str) else {
            continue;
        };
        let relative = spec
            .get("relative_path")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                StorageError::Other(format!(
                    "input {name} with stado_uri requires relative_path"
                ))
            })?;
        let relative_path = Path::new(relative);
        if relative_path.as_os_str().is_empty()
            || relative_path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(StorageError::Other(format!(
                "input {name} relative_path must stay inside the job work directory"
            )));
        }
        let object = crate::remote::object_store::ObjectRef::parse(uri)?;
        // A software release lives in its own namespace and is served by the
        // public release channel; the plain blob read would silently ask the
        // job store's namespace for it and call the published artifact
        // absent. Everything else stays on the store the job runs against.
        let content = if object.namespace() == "releases" {
            store.download_release(&object.to_string()).await?
        } else {
            store.read_bytes(&store_name(&object)).await?
        }
        .ok_or_else(|| StorageError::Other(format!("input {name} is absent: {object}")))?;
        if let Some(expected) = spec
            .get("sha256")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let actual = hex::encode(sha2::Sha256::digest(&content));
            if actual != expected {
                return Err(StorageError::Other(format!(
                    "input {name} digest mismatch: expected {expected}, got {actual}"
                )));
            }
        }
        let destination = work_dir.join(relative_path);
        let mut checked = work_dir.to_path_buf();
        reject_symlink(&checked)?;
        for component in relative_path.components() {
            let std::path::Component::Normal(part) = component else {
                unreachable!("relative path was validated above");
            };
            checked.push(part);
            reject_symlink(&checked)?;
        }
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(destination, content).await?;
    }
    Ok(())
}
